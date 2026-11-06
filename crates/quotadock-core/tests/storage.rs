//! Database: migrations, persistence, privacy, restart/crash recovery, re-analysis.
//! Uses the full pipeline with synthetic RawReads (no backend, no app-server).

use quotadock_core::analytics::{QuotaEventType, WindowType};
use quotadock_core::codex::provider::RawRead;
use quotadock_core::codex::types::TransportKind;
use quotadock_core::commands;
use quotadock_core::pipeline::{CoreConfig, PollTrigger, QuotaDockCore};
use quotadock_core::storage::{derived, open_database, snapshots};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;

const T0: i64 = 1_791_374_400_000;
const MIN: i64 = 60_000;
const HOUR: i64 = 60 * MIN;
const SECRET_ACCOUNT: &str = "acct-0123456789-secret";

fn raw(five_used: f64, five_reset_s: i64, weekly_used: f64, weekly_reset_s: i64, credits: i64, allowed: Option<bool>) -> Value {
    let bucket = json!({
        "limitId": "codex",
        "primary": { "usedPercent": five_used, "windowDurationMins": 300, "resetsAt": five_reset_s },
        "secondary": { "usedPercent": weekly_used, "windowDurationMins": 10080, "resetsAt": weekly_reset_s },
        "planType": "plus"
    });
    json!({
        "ordinaryUsageAllowed": allowed,
        "rateLimits": bucket.clone(),
        "rateLimitsByLimitId": { "codex": bucket },
        "rateLimitResetCredits": { "availableCount": credits, "credits": null },
        "accountId": SECRET_ACCOUNT,
        "rateLimitUpsell": null
    })
}

fn read_at(at: i64, result: Value) -> RawRead {
    RawRead {
        request_started_at_ms: at - 400,
        response_received_at_ms: at,
        result,
        cli_version: "0.162.0-alpha.2".into(),
        transport: TransportKind::StdioAppServer,
        active_account_id: Some(SECRET_ACCOUNT.into()),
    }
}

fn open(path: &Path) -> QuotaDockCore {
    QuotaDockCore::open(CoreConfig::with_database(path.to_path_buf()), Arc::new(|_| {})).expect("core opens")
}

const R1: i64 = (T0 + 3 * HOUR) / 1000;
const W1: i64 = (T0 + 5 * 24 * HOUR) / 1000;

#[test]
fn migrations_pragmas_and_idempotent_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("q.sqlite3");
    let latest = quotadock_core::storage::migrations::MIGRATIONS.last().unwrap().version;
    let (_c, info) = open_database(&path).unwrap();
    assert_eq!(info.schema_version, latest);
    assert_eq!(info.journal_mode.to_lowercase(), "wal");
    assert!(info.foreign_keys);
    assert_eq!(info.busy_timeout_ms, 5000);
    drop(_c);
    let (c2, info2) = open_database(&path).unwrap();
    assert_eq!(info2.schema_version, latest);
    let applied: i64 = c2.query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0)).unwrap();
    assert_eq!(applied as usize, quotadock_core::storage::migrations::MIGRATIONS.len(), "each migration applied exactly once");
    let retention: String = c2.query_row("SELECT value_json FROM settings WHERE key = 'retention'", [], |r| r.get(0)).unwrap();
    assert!(retention.contains("\"cleanup_enabled\":false") && retention.contains("90"));
}

#[test]
fn only_one_engine_per_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("q.sqlite3");
    let first = open(&path);
    let second = QuotaDockCore::open(CoreConfig::with_database(path.clone()), Arc::new(|_| {}));
    assert!(second.is_err(), "a second engine on the same database is refused");
    drop(first);
    assert!(QuotaDockCore::open(CoreConfig::with_database(path), Arc::new(|_| {})).is_ok(), "lock released on drop");
}

#[test]
fn snapshot_persistence_and_privacy() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("q.sqlite3");
    let core = open(&path);
    core.ingest(&read_at(T0, raw(12.0, R1, 2.0, W1, 2, Some(true))), PollTrigger::Manual).unwrap();
    let s = core.with_db(snapshots::latest_snapshot).unwrap().unwrap();
    assert_eq!(s.five_hour.used_percent, Some(12.0));
    assert_eq!(s.five_hour.remaining_percent, Some(88.0));
    assert_eq!(s.five_hour.reset_at_ms, Some(R1 * 1000), "server seconds stored as UTC ms");
    assert_eq!(s.weekly.duration_mins, Some(10080));
    assert_eq!(s.reset_credit_count, Some(2));
    assert_eq!(s.account_check, "match");
    assert_eq!(s.analysis_status, "done");
    let fp = s.account_fingerprint.clone().unwrap();
    assert_eq!(fp.len(), 32);
    assert!(!fp.contains("secret"));
    drop(core);
    // The full account id must not appear anywhere in the database file (WAL included).
    let mut haystack = Vec::new();
    for entry in std::fs::read_dir(dir.path()).unwrap() {
        haystack.extend(std::fs::read(entry.unwrap().path()).unwrap());
    }
    let text = String::from_utf8_lossy(&haystack);
    assert!(!text.contains(SECRET_ACCOUNT), "account id leaked into the database files");
    assert!(text.contains("[REDACTED]"), "raw JSON is stored sanitized");
}

#[test]
fn failed_poll_is_recorded_without_a_fake_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let core = open(&dir.path().join("q.sqlite3"));
    let bad = read_at(T0, json!("not an object"));
    assert!(core.ingest(&bad, PollTrigger::Scheduled).is_err());
    let s = core.with_db(snapshots::poll_summary).unwrap();
    assert_eq!(s.snapshots, 0, "no 0% snapshot is invented");
}

#[tokio::test]
async fn provider_failure_is_a_poll_run_not_a_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let fake = env!("CARGO_BIN_EXE_fake-codex-app-server");
    let mut cfg = CoreConfig::with_database(dir.path().join("q.sqlite3"));
    cfg.provider.override_command = Some((fake.into(), vec!["--unauth".into()]));
    let core = QuotaDockCore::open(cfg, Arc::new(|_| {})).unwrap();
    let out = core.poll_once(PollTrigger::Scheduled).await;
    assert_eq!(out.status.as_str(), "authentication_error");
    let s = core.with_db(snapshots::poll_summary).unwrap();
    assert_eq!((s.snapshots, s.poll_runs, s.failed_polls), (0, 1, 1));
    assert_eq!(s.last_status.as_deref(), Some("authentication_error"));
    core.shutdown().await;
}

#[test]
fn expired_but_blocked_through_the_pipeline() {
    let dir = tempfile::tempdir().unwrap();
    let core = open(&dir.path().join("q.sqlite3"));
    core.ingest(&read_at(T0, raw(100.0, R1, 60.0, W1, 2, Some(true))), PollTrigger::Manual).unwrap();
    let after_reset = R1 * 1000 + 10 * MIN;
    core.ingest(&read_at(after_reset, raw(100.0, R1, 60.0, W1, 2, Some(false))), PollTrigger::Manual).unwrap();
    let usage = commands::get_current_codex_usage(&core).unwrap();
    assert_eq!(usage.availability.as_deref(), Some("blocked"));
    assert_eq!(usage.ordinary_usage_allowed, Some(false));
    let fh = usage.windows.iter().find(|w| w.window_type == WindowType::FiveHour).unwrap();
    assert_eq!(fh.observation_state.as_deref(), Some("expired_unconfirmed"));
    assert_eq!(fh.reset_schedule, "passed");
    assert_eq!(fh.confirmed_reset_at_ms, None, "a passed reset time is not offered as a countdown");

    // Case 9: null availability is unknown.
    core.ingest(&read_at(after_reset + 5 * MIN, raw(100.0, R1, 60.0, W1, 2, None)), PollTrigger::Manual).unwrap();
    assert_eq!(commands::get_current_codex_usage(&core).unwrap().availability.as_deref(), Some("unknown"));
}

#[test]
fn n_restart_with_a_pending_candidate_completes_confirmation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("q.sqlite3");
    {
        let core = open(&path);
        core.ingest(&read_at(T0, raw(12.0, R1, 2.0, W1, 2, Some(true))), PollTrigger::Manual).unwrap();
        core.ingest(&read_at(T0 + 5 * MIN, raw(12.0, R1 + 1200, 2.0, W1, 2, Some(true))), PollTrigger::Manual).unwrap();
        let pending = core.with_db(|c| derived::candidates(c, Some("pending"))).unwrap();
        assert_eq!(pending.len(), 1, "candidate persisted in SQLite");
    } // shutdown
    let core = open(&path);
    let st = core.engine_state();
    assert!(st.context.as_ref().unwrap().windows[&WindowType::FiveHour].pending.is_some(), "candidate restored");
    core.ingest(&read_at(T0 + 10 * MIN, raw(12.0, R1 + 1200, 2.0, W1, 2, Some(true))), PollTrigger::Manual).unwrap();
    let events = core.with_db(|c| derived::events(c, true, 10)).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, QuotaEventType::WindowScheduleChanged.as_str());
    let pending = core.with_db(|c| derived::candidates(c, Some("pending"))).unwrap();
    assert!(pending.is_empty());
    let confirmed = core.with_db(|c| derived::candidates(c, Some("confirmed"))).unwrap();
    assert_eq!(confirmed.len(), 1);
}

#[test]
fn crash_between_raw_commit_and_analysis_is_recovered_on_startup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("q.sqlite3");
    {
        let core = open(&path);
        core.ingest(&read_at(T0, raw(12.0, R1, 2.0, W1, 2, Some(true))), PollTrigger::Manual).unwrap();
    }
    // Simulate a crash after tx 1: a raw snapshot exists but was never analysed.
    {
        let (conn, _) = open_database(&path).unwrap();
        let n = quotadock_core::codex::normalize::normalize_rate_limits(&raw(13.0, R1, 2.0, W1, 2, Some(true))).unwrap();
        snapshots::insert_snapshot(
            &conn,
            &snapshots::SnapshotInsert {
                request_started_at_ms: T0 + 5 * MIN - 400,
                response_received_at_ms: T0 + 5 * MIN,
                account_fingerprint: Some(quotadock_core::codex::sanitize::account_fingerprint(SECRET_ACCOUNT)),
                account_check: "match",
                cli_version: Some("0.162.0-alpha.2"),
                transport: "stdio-app-server",
                normalized: &n,
                raw: &raw(13.0, R1, 2.0, W1, 2, Some(true)),
            },
        )
        .unwrap();
    }
    let core = open(&path);
    let pending = core.with_db(|c| snapshots::snapshot_ids_with_status(c, "pending")).unwrap();
    assert!(pending.is_empty(), "startup analysed the leftover snapshot");
    assert_eq!(core.engine_state().last_snapshot_id, Some(2));
}

#[test]
fn reanalysis_reproduces_the_same_derived_history() {
    let dir = tempfile::tempdir().unwrap();
    let core = open(&dir.path().join("q.sqlite3"));
    let seq = [
        (T0, raw(98.0, R1, 40.0, W1, 2, Some(true))),
        (T0 + 5 * MIN, raw(98.0, R1 + 600, 40.0, W1, 2, Some(true))),
        (T0 + 10 * MIN, raw(98.0, R1, 40.0, W1, 2, Some(true))),
        (R1 * 1000 + MIN, raw(1.0, R1 + 5 * 3600, 41.0, W1, 2, Some(true))),
        (R1 * 1000 + 6 * MIN, raw(2.0, R1 + 5 * 3600, 41.0, W1, 2, Some(true))),
    ];
    for (at, r) in &seq {
        core.ingest(&read_at(*at, r.clone()), PollTrigger::Scheduled).unwrap();
    }
    let strip =
        |v: Vec<derived::EventView>| v.into_iter().map(|e| (e.event_key, e.event_type, e.confidence, e.evidence)).collect::<Vec<_>>();
    let before = strip(core.with_db(|c| derived::events(c, true, 100)).unwrap());
    let state_before = core.engine_state();
    assert!(before.iter().any(|e| e.1 == "natural_rollover_observed"));
    assert!(before.iter().any(|e| e.1 == "transient_backend_state"));
    core.reanalyze_all().unwrap();
    let after = strip(core.with_db(|c| derived::events(c, true, 100)).unwrap());
    assert_eq!(before, after, "re-analysis from raw snapshots is deterministic");
    assert_eq!(state_before, core.engine_state());
    let s2 = core.with_db(|c| snapshots::snapshot_by_id(c, 2)).unwrap().unwrap();
    assert_eq!(s2.five_hour.quality, "suspect_transient");
}

#[test]
fn burn_rate_excludes_suspect_snapshots_and_never_crosses_a_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let core = open(&dir.path().join("q.sqlite3"));
    // Old cycle with heavy usage, then a confirmed rollover, then slow usage in the new cycle.
    core.ingest(&read_at(T0, raw(90.0, R1, 10.0, W1, 2, Some(true))), PollTrigger::Scheduled).unwrap();
    let base = R1 * 1000 + MIN;
    let r2 = R1 + 5 * 3600;
    let mut used = 1.0;
    for i in 0..8 {
        let at = base + i * 10 * MIN;
        // one transient glitch in the middle (A->B->A on reset time)
        let reset = if i == 4 { r2 + 3600 } else { r2 };
        core.ingest(&read_at(at, raw(used, reset, 10.0, W1, 2, Some(true))), PollTrigger::Scheduled).unwrap();
        if i != 4 {
            used += 1.0;
        }
    }
    let f = commands::get_usage_forecast(&core, WindowType::FiveHour).unwrap();
    assert!(f.burn.points >= 6, "{:?}", f.burn);
    let rate = f.burn.current_cycle_rate.unwrap();
    // Points (min, pp): (0,1) (10,2) (20,3) (30,4) (50,5) (60,6) (70,7) -> OLS slope 340/4171.4 pp/min.
    assert!((rate - 4.89).abs() < 0.05, "OLS over the new cycle only, got {rate}");
    assert_eq!(f.burn.points, 7, "the transient point is excluded");
}

#[test]
fn foreign_keys_set_null_on_snapshot_retention_delete() {
    let dir = tempfile::tempdir().unwrap();
    let core = open(&dir.path().join("q.sqlite3"));
    core.ingest(&read_at(T0, raw(12.0, R1, 2.0, W1, 2, Some(true))), PollTrigger::Manual).unwrap();
    core.ingest(&read_at(T0 + 5 * MIN, raw(12.0, R1 + 600, 2.0, W1, 2, Some(true))), PollTrigger::Manual).unwrap();
    core.ingest(&read_at(T0 + 10 * MIN, raw(12.0, R1, 2.0, W1, 2, Some(true))), PollTrigger::Manual).unwrap();
    // A future retention job may delete raw snapshots; events must survive with evidence intact.
    core.with_db(|c| c.execute("DELETE FROM snapshots WHERE id = 2", [])).unwrap();
    let events = core.with_db(|c| derived::events(c, true, 10)).unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0].evidence["transient"].is_array());
}

fn count(c: &rusqlite::Connection, sql: &str) -> i64 {
    c.query_row(sql, [], |r| r.get(0)).unwrap()
}

#[test]
fn a_new_classifier_reanalyses_history_and_keeps_earlier_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("q.sqlite3");
    let cur = quotadock_core::analytics::CLASSIFIER_VERSION;
    {
        let core = open(&path);
        // Idle (sliding) 5h window, then usage resumes: v2 re-anchors the analysis segment.
        for i in 0..4 {
            let at = T0 + i * 5 * MIN;
            core.ingest(&read_at(at, raw(0.0, (at + 5 * HOUR) / 1000, 2.0, W1, 2, Some(true))), PollTrigger::Scheduled).unwrap();
        }
        let started = T0 + 17 * MIN;
        for (i, u) in [4.0, 6.0, 9.0].into_iter().enumerate() {
            let at = T0 + (20 + 5 * i as i64) * MIN;
            core.ingest(&read_at(at, raw(u, (started + 5 * HOUR) / 1000, 2.0, W1, 2, Some(true))), PollTrigger::Scheduled).unwrap();
        }
    }
    // Make it look like an earlier classifier wrote everything (keys embed the version too), and
    // leave a dedup row with a version-prefixed scope for migration v4 to clean up.
    let (snapshots_before, events_before, cycles_before) = {
        let (c, _) = open_database(&path).unwrap();
        let old = "reset-detector-v0";
        for (table, cols) in [
            ("observed_cycles", vec!["cycle_key", "start_event_key", "end_event_key"]),
            ("quota_events", vec!["event_key", "superseded_by_event_key"]),
            ("quota_candidates", vec!["candidate_key", "resolution_event_key"]),
            ("window_tracks", vec!["current_cycle_key"]),
            ("engine_state", vec![]),
        ] {
            let mut set = vec![format!("classifier_version = '{old}'")];
            set.extend(cols.iter().map(|k| format!("{k} = replace({k}, '{cur}|', '{old}|')")));
            c.execute(&format!("UPDATE {table} SET {}", set.join(", ")), []).unwrap();
        }
        c.execute(
            "INSERT INTO notification_state (rule_id, provider, window_type, scope_key, armed, sent_at_ms, updated_at_ms)
             VALUES ('quota_below_30', 'codex', 'weekly', 'reset-detector-v1|abc|weekly|cycle|5', 0, 1, 1)",
            [],
        )
        .unwrap();
        // Run migration v4's statement again over the leftover row (it is idempotent).
        let v4 = quotadock_core::storage::migrations::MIGRATIONS.iter().find(|m| m.version == 4).unwrap();
        c.execute_batch(v4.sql).unwrap();
        (
            count(&c, "SELECT COUNT(*) FROM snapshots"),
            count(&c, "SELECT COUNT(*) FROM quota_events"),
            count(&c, "SELECT COUNT(*) FROM observed_cycles"),
        )
    };

    let core = open(&path);
    core.with_db(|c| {
        assert_eq!(count(c, "SELECT COUNT(*) FROM snapshots"), snapshots_before, "raw history untouched");
        let q = |sql: &str| c.query_row(sql, [cur], |r| r.get::<_, i64>(0)).unwrap();
        assert_eq!(
            count(c, "SELECT COUNT(*) FROM quota_events WHERE classifier_version = 'reset-detector-v0'"),
            events_before,
            "earlier events kept"
        );
        assert_eq!(
            count(c, "SELECT COUNT(*) FROM observed_cycles WHERE classifier_version = 'reset-detector-v0'"),
            cycles_before,
            "earlier cycles kept"
        );
        assert_eq!(q("SELECT COUNT(*) FROM engine_state WHERE classifier_version = ?1"), 1, "state rebuilt for the new classifier");
        assert_eq!(q("SELECT COUNT(*) FROM observed_cycles WHERE classifier_version = ?1"), cycles_before, "same cycles re-derived");
        assert_eq!(q("SELECT COUNT(*) FROM quota_events WHERE classifier_version = ?1"), events_before, "same events re-derived");
        let scope: String =
            c.query_row("SELECT scope_key FROM notification_state WHERE rule_id = 'quota_below_30'", [], |r| r.get(0)).unwrap();
        assert_eq!(scope, "abc|weekly|cycle|5", "dedup scope no longer carries the classifier version");
        Ok(())
    })
    .unwrap();
    let track = core
        .engine()
        .views(core.engine_state().context.as_ref().unwrap())
        .into_iter()
        .find(|v| v.window_type == WindowType::FiveHour)
        .unwrap();
    assert_eq!(track.active_origin_ms, Some(T0 + 17 * MIN), "re-analysis applied the v2 re-anchor");
    assert_eq!(track.analysis_segment_start_snapshot_id, Some(5));
}

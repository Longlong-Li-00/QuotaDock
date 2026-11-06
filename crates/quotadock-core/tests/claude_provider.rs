//! Phase 3.5 Claude provider: sources (fake CLI + private bridge folder), storage, window
//! instances, reminder dedup, cross-provider aggregation, tray, history, events. Never touches the
//! real Claude Code, ~/.claude, ~/.quotadock or the user's LOCALAPPDATA.

use quotadock_core::analytics::forecast::Risk;
use quotadock_core::analytics::model::WindowType;
use quotadock_core::claude::model::*;
use quotadock_core::claude::monitor::{ClaudeMonitor, ClaudeSourceState, ClaudeStatus};
use quotadock_core::claude::statusline::{self, BridgeRecord, BridgeWindow};
use quotadock_core::claude::store::{self, StoredObservation};
use quotadock_core::commands::WindowStatus;
use quotadock_core::pipeline::{CoreConfig, QuotaDockCore};
use quotadock_core::product::claude_view;
use quotadock_core::product::facts::MonitorFacts;
use quotadock_core::product::history::{self, HistoryData, HistoryRange};
use quotadock_core::product::notifications::{self, CandidateKind, NotifyInput, SqliteStore, WindowRuleInput};
use quotadock_core::product::runtime::ProductRuntime;
use quotadock_core::product::settings::{ClaudeNotificationSettings, Lang, LanguagePref, NotificationSettings};
use quotadock_core::product::tray::{DisplayTone, Freshness, ProviderTrayLine, Severity, TrayIconKind, TrayStatusModel, with_provider};
use quotadock_core::product::widget::WidgetReset;
use quotadock_core::time::now_ms;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const FAKE: &str = env!("CARGO_BIN_EXE_fake-claude");
const H: i64 = 3_600_000;

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
    core: Arc<QuotaDockCore>,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let core = Arc::new(QuotaDockCore::open(CoreConfig::with_database(root.join("q.sqlite3")), Arc::new(|_| {})).unwrap());
        Self { _dir: dir, root, core }
    }

    fn monitor(&self, mode: &str) -> ClaudeMonitor {
        let cwd = self.root.join("claude").join("cwd");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::write(cwd.join("mode.txt"), mode).unwrap();
        ClaudeMonitor::new(&self.root, &self.root.join("local")).with_cli_override(PathBuf::from(FAKE))
    }

    fn calls(&self) -> String {
        std::fs::read_to_string(self.root.join("claude").join("cwd").join("calls.log")).unwrap_or_default()
    }

    fn reopen(self) -> Self {
        let Env { _dir, root, core } = self;
        drop(core);
        let core = Arc::new(QuotaDockCore::open(CoreConfig::with_database(root.join("q.sqlite3")), Arc::new(|_| {})).unwrap());
        Env { _dir, root, core }
    }
}

fn obs(at: i64, windows: &[(&str, f64, Option<i64>)]) -> ClaudeObservation {
    ClaudeObservation {
        source: ClaudeSource::StatusLine,
        captured_at_ms: at,
        claude_version: Some("2.1.278".into()),
        billing: Billing::Subscription,
        windows: windows
            .iter()
            .map(|(k, used, reset)| ClaudeWindowObs {
                key: (*k).into(),
                kind: match *k {
                    FIVE_HOUR_KEY => WindowKind::FiveHour,
                    WEEKLY_KEY => WindowKind::Weekly,
                    _ => WindowKind::Extra,
                },
                label: None,
                duration_mins: None,
                used_percent: *used,
                resets_at_ms: *reset,
                reset_precision_ms: 1000,
            })
            .collect(),
        raw_sanitized: serde_json::json!({}),
    }
}

// --------------------------------------------------------------------------- sources

#[tokio::test]
async fn active_usage_is_a_guarded_local_command_and_is_stored() {
    let e = Env::new();
    let m = e.monitor("ok");
    let env = m.refresh_environment(now_ms()).await;
    assert_eq!(env.version.as_deref(), Some("2.1.278"));
    assert_eq!(env.billing(), Billing::Subscription);
    assert_eq!(env.auth.as_ref().and_then(|a| a.subscription_type.as_deref()), Some("max"));
    assert!(!format!("{env:?}").contains("someone@example.com"), "no email kept, only a fingerprint");
    assert!(m.active_due(&e.core, true, false, false, now_ms()).unwrap(), "no data yet: the fallback may run");
    assert!(!m.active_due(&e.core, false, false, false, now_ms()).unwrap(), "off by default means never");
    let ingested = m.run_active(&e.core, now_ms()).await.unwrap().expect("stored");
    assert_eq!(ingested.source, ClaudeSource::ActiveCli);
    let latest = e.core.with_db(|c| store::latest(c, PROVIDER_CLAUDE)).unwrap().unwrap();
    let keys: Vec<&str> = latest.windows.iter().map(|w| w.window_key.as_str()).collect();
    assert_eq!(keys, vec!["five_hour", "weekly", "weekly_model:fable"]);
    assert_eq!(latest.windows[1].remaining_percent, 29.0);
    assert!(latest.windows.iter().all(|w| w.resets_at_ms.is_some_and(|r| r > now_ms()) && w.instance_key.is_some()));
    let calls = e.calls();
    assert!(calls.contains("-p /usage --output-format json --model quotadock-usage-guard-no-model --no-session-persistence"), "{calls}");
    assert!(!m.active_due(&e.core, true, false, false, now_ms()).unwrap(), "fresh data: no second query");
    assert!(!m.active_due(&e.core, true, true, false, now_ms()).unwrap(), "fresh data: not even in the foreground");
}

#[tokio::test]
async fn rate_limits_back_off_and_suspend_the_active_source_after_three() {
    let e = Env::new();
    let m = e.monitor("ratelimit");
    m.refresh_environment(now_ms()).await;
    let t0 = now_ms();
    assert!(m.run_active(&e.core, t0).await.unwrap().is_none());
    let a = m.active_state();
    assert_eq!(a.consecutive_rate_limits, 1);
    assert!(a.next_allowed_ms.unwrap() >= t0 + 15 * 60_000, "Retry-After 120 s is below the 15 min floor");
    assert!(!m.active_due(&e.core, true, true, true, now_ms()).unwrap(), "backing off, even for a manual refresh");
    m.run_active(&e.core, now_ms()).await.unwrap();
    m.run_active(&e.core, now_ms()).await.unwrap();
    let a = m.active_state();
    assert!(a.suspended, "three 429 in a row suspend the active source for the session");
    let runs = e.core.with_db(|c| store::recent_runs(c, PROVIDER_CLAUDE, 10)).unwrap();
    assert_eq!(runs.len(), 3);
    assert!(runs.iter().all(|r| r.status == "rate_limited" && r.retry_after_secs == Some(120)));
    let status = m.status(&e.core, true, true, now_ms()).unwrap();
    assert_eq!(status.refresh_capability, "unavailable", "suspended active source, no passive integration");
}

#[tokio::test]
async fn model_usage_in_the_envelope_disables_the_active_source() {
    let e = Env::new();
    let m = e.monitor("inference");
    m.refresh_environment(now_ms()).await;
    assert!(m.run_active(&e.core, now_ms()).await.unwrap().is_none());
    assert!(m.active_state().needs_verification);
    assert!(!m.active_due(&e.core, true, true, true, now_ms()).unwrap());
    assert_eq!(e.core.with_db(|c| store::recent_runs(c, PROVIDER_CLAUDE, 1)).unwrap()[0].status, "inference_detected");
    assert!(e.core.with_db(|c| store::latest(c, PROVIDER_CLAUDE)).unwrap().is_none());
    assert_eq!(m.status(&e.core, true, true, now_ms()).unwrap().state, ClaudeSourceState::NeedsVerification);
}

#[tokio::test]
async fn usage_lines_in_an_unknown_shape_need_verification() {
    let e = Env::new();
    let m = e.monitor("badformat");
    m.refresh_environment(now_ms()).await;
    assert!(m.run_active(&e.core, now_ms()).await.unwrap().is_none());
    assert!(m.active_state().needs_verification, "a format change is not guessed and stops the active source");
    assert!(!m.active_due(&e.core, true, false, true, now_ms() + 2 * H).unwrap());
    assert_eq!(e.core.with_db(|c| store::recent_runs(c, PROVIDER_CLAUDE, 1)).unwrap()[0].status, "unrecognized");
    assert!(e.core.with_db(|c| store::latest(c, PROVIDER_CLAUDE)).unwrap().is_none());
}

/// Regression (soak 2026-10-13): during a network outage Claude Code's local /usage printed no usage
/// line; QuotaDock disabled the active source until restart and Claude went unmonitored for a day.
/// Now it backs off like any temporary error and recovers by itself once /usage answers again.
#[tokio::test]
async fn usage_text_without_usage_lines_backs_off_and_recovers() {
    let e = Env::new();
    let m = e.monitor("unrecognized"); // "Usage information is not available right now."
    m.refresh_environment(now_ms()).await;
    let t0 = now_ms();
    assert!(m.run_active(&e.core, t0).await.unwrap().is_none());
    let a = m.active_state();
    assert!(!a.needs_verification && !a.suspended, "not a reason to disable the source");
    assert_eq!(a.consecutive_errors, 1);
    assert!(a.next_allowed_ms.unwrap() >= t0 + 15 * 60_000, "first back-off step: 15 min");
    let run = &e.core.with_db(|c| store::recent_runs(c, PROVIDER_CLAUDE, 1)).unwrap()[0];
    assert_eq!(run.status, "no_usage_lines");
    let error = run.error.clone().unwrap_or_default();
    assert!(!error.contains("not available"), "only a description of the text is stored: {error}");
    assert!(e.core.with_db(|c| store::latest(c, PROVIDER_CLAUDE)).unwrap().is_none(), "nothing guessed");
    assert_ne!(m.status(&e.core, true, true, now_ms()).unwrap().state, ClaudeSourceState::NeedsVerification);
    assert!(!m.active_due(&e.core, true, false, false, now_ms() + 10 * 60_000).unwrap(), "backing off");

    // Still failing after the back-off: the next step is 30 min, still no latch.
    let t1 = t0 + 16 * 60_000;
    assert!(m.active_due(&e.core, true, false, false, t1).unwrap());
    m.run_active(&e.core, t1).await.unwrap();
    let a = m.active_state();
    assert_eq!((a.consecutive_errors, a.needs_verification), (2, false));
    assert!(a.next_allowed_ms.unwrap() >= now_ms() + 29 * 60_000, "second back-off step: 30 min after the run finished");

    // /usage answers again: the next due query succeeds and clears the error state.
    std::fs::write(e.root.join("claude").join("cwd").join("mode.txt"), "ok").unwrap();
    let t2 = t1 + 31 * 60_000;
    assert!(m.active_due(&e.core, true, false, false, t2).unwrap());
    assert!(m.run_active(&e.core, t2).await.unwrap().is_some());
    let a = m.active_state();
    assert_eq!((a.consecutive_errors, a.next_allowed_ms, a.last_error.clone()), (0, None, None));
    assert!(e.core.with_db(|c| store::latest(c, PROVIDER_CLAUDE)).unwrap().is_some());
}

#[tokio::test]
async fn api_key_billing_is_never_shown_as_subscription_quota() {
    let e = Env::new();
    let m = e.monitor("apikey");
    let env = m.refresh_environment(now_ms()).await;
    assert_eq!(env.billing(), Billing::ApiKey);
    assert_eq!(m.status(&e.core, true, true, now_ms()).unwrap().state, ClaudeSourceState::ApiBilling);
    assert!(!m.active_due(&e.core, true, true, true, now_ms()).unwrap(), "no subscription quota to ask for");
    // Even if run, an API-billing answer is never stored as quota.
    assert!(m.run_active(&e.core, now_ms()).await.unwrap().is_none());
    assert!(e.core.with_db(|c| store::latest(c, PROVIDER_CLAUDE)).unwrap().is_none());
}

#[tokio::test]
async fn logged_out_and_not_configured_states() {
    let e = Env::new();
    let m = e.monitor("loggedout");
    m.refresh_environment(now_ms()).await;
    assert_eq!(m.status(&e.core, true, false, now_ms()).unwrap().state, ClaudeSourceState::NotAuthenticated);
    let e2 = Env::new();
    let m2 = e2.monitor("ok");
    m2.refresh_environment(now_ms()).await;
    let st = m2.status(&e2.core, true, false, now_ms()).unwrap();
    assert_eq!(st.state, ClaudeSourceState::NotConfigured, "no bridge, active refresh off");
    assert_eq!(m2.status(&e2.core, false, false, now_ms()).unwrap().state, ClaudeSourceState::Disabled);
}

fn set_mode(e: &Env, mode: &str) {
    std::fs::write(e.root.join("claude").join("cwd").join("mode.txt"), mode).unwrap();
}

/// A runtime whose Claude ticks only run the environment check (no /usage reads, so no quota
/// notices) unless `active` is set.
fn auth_runtime(e: &Env, mode: &str, active: bool) -> Arc<ProductRuntime> {
    let rt = ProductRuntime::with_claude(e.core.clone(), e.monitor(mode)).unwrap();
    let mut s = rt.settings();
    s.language = LanguagePref::En;
    s.claude.active_refresh = Some(active);
    rt.update_settings(s).unwrap();
    rt
}

/// A manual Claude tick (always re-checks the environment); the notification title, if any.
async fn manual_tick(rt: &ProductRuntime) -> Option<String> {
    rt.claude_tick(false, true).await.unwrap().and_then(|a| a.notification).map(|(n, _)| n.title)
}

#[tokio::test]
async fn signing_out_after_a_confirmed_sign_in_notifies_once_per_episode() {
    let e = Env::new();
    let rt = auth_runtime(&e, "ok", false);
    assert_eq!(manual_tick(&rt).await, None, "signed in");
    set_mode(&e, "loggedout");
    assert_eq!(manual_tick(&rt).await.as_deref(), Some("Claude monitoring stopped"));
    assert_eq!(manual_tick(&rt).await, None, "same episode");
    let log: Vec<(String, String, String)> = e
        .core
        .with_db(|c| {
            let mut s = c.prepare("SELECT provider, title, body FROM notification_log ORDER BY id")?;
            s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap();
    let expected = ("claude", "Claude monitoring stopped", "Claude Code is signed out. Sign in again to resume monitoring.");
    assert_eq!(log, vec![(expected.0.to_string(), expected.1.to_string(), expected.2.to_string())]);
    // Restart while still signed out: the episode is persisted, nothing is sent again.
    drop(rt);
    let e = e.reopen();
    let rt = auth_runtime(&e, "loggedout", false);
    assert_eq!(manual_tick(&rt).await, None, "no repeat after a restart");
    // Signing in re-arms it; the next sign-out is a new episode.
    set_mode(&e, "ok");
    assert_eq!(manual_tick(&rt).await, None);
    set_mode(&e, "loggedout");
    assert_eq!(manual_tick(&rt).await.as_deref(), Some("Claude monitoring stopped"));
    let calls = e.calls();
    assert!(calls.lines().all(|l| l == "--version" || l == "auth status --json"), "only the version and auth checks ran: {calls}");
}

#[tokio::test]
async fn a_sign_out_notice_that_could_not_be_recorded_is_sent_at_the_next_check() {
    let e = Env::new();
    let rt = auth_runtime(&e, "ok", false);
    assert_eq!(manual_tick(&rt).await, None, "signed in");
    // The notification log rejects the write once (disk full, database busy...).
    e.core
        .with_db(|c| c.execute_batch("CREATE TRIGGER t_fail BEFORE INSERT ON notification_log BEGIN SELECT RAISE(ABORT, 'test'); END;"))
        .unwrap();
    set_mode(&e, "loggedout");
    assert!(rt.claude_tick(false, true).await.is_err());
    e.core.with_db(|c| c.execute_batch("DROP TRIGGER t_fail;")).unwrap();
    assert_eq!(manual_tick(&rt).await.as_deref(), Some("Claude monitoring stopped"), "the episode was not lost");
    assert_eq!(manual_tick(&rt).await, None, "and is sent once");
}

#[tokio::test]
async fn no_sign_out_notice_when_monitoring_stops_during_the_check() {
    for stop in ["pause", "disable"] {
        let e = Env::new();
        let rt = auth_runtime(&e, "ok", false);
        assert_eq!(manual_tick(&rt).await, None, "signed in");
        set_mode(&e, "loggedout");
        let rt2 = rt.clone();
        // The second future runs while the tick waits for `claude auth status`.
        let (tick, _) = tokio::join!(rt.claude_tick(false, true), async {
            if stop == "pause" {
                rt2.pause().await;
            } else {
                let mut s = rt2.settings();
                s.claude.enabled = false;
                rt2.update_settings(s).unwrap();
            }
        });
        let title = tick.unwrap().and_then(|a| a.notification).map(|(n, _)| n.title);
        assert_eq!(title, None, "{stop}: monitoring stopped before the result arrived");
    }
}

#[test]
fn the_sign_out_notice_is_never_folded_away() {
    use quotadock_core::product::claude_auth;
    use quotadock_core::product::notifications::{Candidate, RuleKey};
    let other = |priority: u8, kind: CandidateKind, title: &str| Candidate {
        priority,
        kind,
        provider: "Codex",
        window: Some(WindowType::Weekly),
        remaining: Some(5.0),
        keys: vec![RuleKey { rule_id: title.into(), provider: "codex".into(), window_type: "weekly".into(), scope_key: "c".into() }],
        title: title.into(),
        body: title.into(),
        short: title.into(),
    };
    let candidates = vec![
        other(1, CandidateKind::AvailabilityRestored, "available again"),
        other(2, CandidateKind::QuotaRefresh, "refreshed early"),
        other(3, CandidateKind::QuotaCritical, "below 10%"),
        claude_auth::candidate(Lang::En, 7),
    ];
    let n = notifications::aggregate(candidates, Lang::En).unwrap();
    assert_eq!(n.title, "Claude monitoring stopped", "shown even when three other reminders fire together");
    assert!(n.body.contains("available again"), "{}", n.body);
}

#[tokio::test]
async fn no_sign_out_notice_without_an_explicit_sign_out_after_a_confirmed_sign_in() {
    // Never seen signed in: not "just signed out".
    let e = Env::new();
    let rt = auth_runtime(&e, "loggedout", false);
    assert_eq!(manual_tick(&rt).await, None);
    // A failed check or a missing `loggedIn` is no answer at all.
    set_mode(&e, "ok");
    assert_eq!(manual_tick(&rt).await, None);
    for mode in ["authmissing", "autherror"] {
        set_mode(&e, mode);
        assert_eq!(manual_tick(&rt).await, None, "{mode}");
    }
    // Monitoring disabled: no check at all, so no notice.
    let mut s = rt.settings();
    s.claude.enabled = false;
    rt.update_settings(s).unwrap();
    set_mode(&e, "loggedout");
    assert!(rt.claude_tick(false, true).await.unwrap().is_none());
    s = rt.settings();
    s.claude.enabled = true;
    rt.update_settings(s).unwrap();
    // /usage problems (rate limit, text without usage lines) are not authentication results.
    for mode in ["ratelimit", "unrecognized"] {
        let e = Env::new();
        let rt = auth_runtime(&e, "ok", false);
        assert_eq!(manual_tick(&rt).await, None);
        let mut s = rt.settings();
        s.claude.active_refresh = Some(true);
        rt.update_settings(s).unwrap();
        set_mode(&e, mode);
        assert_eq!(manual_tick(&rt).await, None, "{mode}");
        assert!(e.calls().contains("-p /usage"), "{mode}: the active read ran and failed");
    }
}

#[tokio::test]
async fn data_on_the_active_schedule_stays_live_between_background_refreshes() {
    let e = Env::new();
    let m = e.monitor("ok");
    m.refresh_environment(now_ms()).await;
    m.run_active(&e.core, now_ms()).await.unwrap().expect("stored");
    // 12 min later: the background refresh (every 15 min) is not due yet.
    let later = now_ms() + 12 * 60_000;
    let with_active = m.status(&e.core, true, true, later).unwrap();
    assert_eq!(with_active.state, ClaudeSourceState::Live, "refreshed on schedule");
    assert!(with_active.live);
    let passive_only = m.status(&e.core, true, false, later).unwrap();
    assert!(!passive_only.live, "without the active source, 12 min old data is not live");
    let overdue = m.status(&e.core, true, true, now_ms() + 21 * 60_000).unwrap();
    assert_eq!(overdue.state, ClaudeSourceState::Stale, "a missed background refresh shows as stale");
}

#[tokio::test]
async fn a_hung_cli_times_out_and_leaves_nothing_behind() {
    let e = Env::new();
    let m = e.monitor("ok").with_cli_timeout(Duration::from_secs(3));
    m.refresh_environment(now_ms()).await;
    std::fs::write(e.root.join("claude").join("cwd").join("mode.txt"), "slow").unwrap();
    let started = std::time::Instant::now();
    assert!(m.run_active(&e.core, now_ms()).await.unwrap().is_none());
    assert!(started.elapsed() < Duration::from_secs(20));
    assert_eq!(e.core.with_db(|c| store::recent_runs(c, PROVIDER_CLAUDE, 1)).unwrap()[0].status, "error");
    assert!(m.active_state().last_error.unwrap().contains("did not finish"));
}

fn bridge(at: i64, five: Option<f64>, week: Option<f64>) -> BridgeRecord {
    let mut rl = BTreeMap::new();
    let r = (now_ms() + 3 * H) / 1000;
    if let Some(u) = five {
        rl.insert("five_hour".into(), BridgeWindow { used_percentage: u, resets_at: Some(r) });
    }
    if let Some(u) = week {
        rl.insert("seven_day".into(), BridgeWindow { used_percentage: u, resets_at: Some(r + 70 * 3600) });
    }
    BridgeRecord {
        schema: statusline::BRIDGE_SCHEMA,
        received_at_ms: at,
        claude_version: Some("2.1.278".into()),
        model: None,
        session: None,
        rate_limits: rl,
    }
}

#[test]
fn passive_bridge_records_are_stored_once_and_ignored_while_paused() {
    let e = Env::new();
    let m = e.monitor("ok");
    let dir = m.bridge_dir().to_path_buf();
    let t = now_ms();
    statusline::write_record(&dir, &bridge(t, Some(42.7), Some(61.0))).unwrap();
    assert!(m.poll_passive(&e.core, t).unwrap().is_some());
    assert!(m.poll_passive(&e.core, t + 1).unwrap().is_none(), "same record twice is stored once");
    let latest = e.core.with_db(|c| store::latest(c, PROVIDER_CLAUDE)).unwrap().unwrap();
    assert_eq!(latest.source, "statusline");
    assert!((latest.windows[0].used_percent - 42.7).abs() < 1e-9, "raw precision kept");
    // Paused: updates are not stored, and not stored retroactively after resuming.
    m.set_paused(true, t + 10);
    statusline::write_record(&dir, &bridge(t + 20, Some(50.0), Some(62.0))).unwrap();
    assert!(m.poll_passive(&e.core, t + 30).unwrap().is_none());
    m.set_paused(false, t + 40);
    assert!(m.poll_passive(&e.core, t + 50).unwrap().is_none(), "arrived during the pause");
    statusline::write_record(&dir, &bridge(t + 60, Some(51.0), Some(62.0))).unwrap();
    assert!(m.poll_passive(&e.core, t + 70).unwrap().is_some());
    // A first reply in a new session has no rate_limits yet: nothing stored.
    statusline::write_record(&dir, &bridge(t + 80, None, None)).unwrap();
    assert!(m.poll_passive(&e.core, t + 90).unwrap().is_none());
    assert_eq!(e.core.with_db(|c| c.query_row("SELECT COUNT(*) FROM provider_snapshots", [], |r| r.get::<_, i64>(0))).unwrap(), 2);
}

#[tokio::test]
async fn the_newest_observation_wins_whatever_its_source() {
    let e = Env::new();
    let m = e.monitor("ok");
    m.refresh_environment(now_ms()).await;
    m.run_active(&e.core, now_ms()).await.unwrap().expect("active stored");
    let active_at = e.core.with_db(|c| store::latest(c, PROVIDER_CLAUDE)).unwrap().unwrap().captured_at_ms;
    let dir = m.bridge_dir().to_path_buf();
    // A status-line record written before that active read is older information: not ingested.
    statusline::write_record(&dir, &bridge(active_at - 60_000, Some(40.0), Some(50.0))).unwrap();
    assert!(m.poll_passive(&e.core, now_ms()).unwrap().is_none());
    assert_eq!(e.core.with_db(|c| store::latest(c, PROVIDER_CLAUDE)).unwrap().unwrap().source, "active_cli");
    // A newer status-line record replaces the active read as the current observation.
    statusline::write_record(&dir, &bridge(active_at + 1_000, Some(41.0), Some(50.0))).unwrap();
    assert!(m.poll_passive(&e.core, now_ms()).unwrap().is_some());
    assert_eq!(e.core.with_db(|c| store::latest(c, PROVIDER_CLAUDE)).unwrap().unwrap().source, "statusline");
}

#[test]
fn window_shapes_are_mapped_by_source_keys() {
    let only_weekly = statusline::to_observation(&bridge(1, None, Some(10.0))).unwrap();
    assert_eq!(only_weekly.windows.len(), 1);
    assert_eq!((only_weekly.windows[0].key.as_str(), only_weekly.windows[0].kind), (WEEKLY_KEY, WindowKind::Weekly));
    let only_five = statusline::to_observation(&bridge(1, Some(10.0), None)).unwrap();
    assert_eq!(only_five.windows[0].kind, WindowKind::FiveHour);
    let mut extra = bridge(1, Some(1.0), Some(2.0));
    extra.rate_limits.insert("seven_day_sonnet".into(), BridgeWindow { used_percentage: 3.0, resets_at: None });
    let o = statusline::to_observation(&extra).unwrap();
    let x = o.windows.iter().find(|w| w.kind == WindowKind::Extra).unwrap();
    assert_eq!((x.key.as_str(), x.label.as_deref(), x.resets_at_ms), ("statusline:seven_day_sonnet", Some("seven_day_sonnet"), None));
}

// --------------------------------------------------------------------------- reminders

fn claude_round(e: &Env, used: f64, reset: i64, at: i64) -> Option<String> {
    e.core.with_db_mut(|c| store::ingest(c, &obs(at, &[(WEEKLY_KEY, used, Some(reset))]), Some("acct-fp"), at)).unwrap();
    let latest = e.core.with_db(|c| store::latest(c, PROVIDER_CLAUDE)).unwrap();
    let inputs = claude_view::window_inputs(latest.as_ref(), &[], at);
    e.core
        .with_db(|c| {
            let cands = claude_view::evaluate(&inputs, &[], &ClaudeNotificationSettings::default(), Lang::En, &SqliteStore(c));
            Ok(match notifications::aggregate(cands, Lang::En) {
                Some(n) => {
                    notifications::record(c, PROVIDER_CLAUDE, &n, Lang::En, at)?;
                    Some(n.title)
                }
                None => None,
            })
        })
        .unwrap()
}

#[test]
fn claude_weekly_threshold_dedup_survives_restart_and_rearms_after_refresh() {
    let e = Env::new();
    let t = now_ms();
    let reset = t + 70 * H;
    assert_eq!(claude_round(&e, 69.0, reset, t), None, "31 % left");
    assert_eq!(claude_round(&e, 71.0, reset, t + 1).as_deref(), Some("Claude weekly quota below 30%"));
    assert_eq!(claude_round(&e, 80.0, reset, t + 2), None, "20 % left: same 30 % rule");
    assert_eq!(claude_round(&e, 91.0, reset, t + 3).as_deref(), Some("Claude weekly quota below 10%"));
    let e = e.reopen();
    assert_eq!(claude_round(&e, 91.0, reset, t + 4), None, "restart: still 9 %, no duplicate");
    // The reset passes and Claude Code reports a new period: reminders re-arm.
    let next = reset + 168 * H;
    assert_eq!(claude_round(&e, 75.0, next, reset + 60_000).as_deref(), Some("Claude weekly quota below 30%"));
    // 5-hour reminders are off by default.
    e.core.with_db_mut(|c| store::ingest(c, &obs(t + 5, &[(FIVE_HOUR_KEY, 95.0, Some(t + 2 * H))]), Some("acct-fp"), t + 5)).unwrap();
    let latest = e.core.with_db(|c| store::latest(c, PROVIDER_CLAUDE)).unwrap();
    let inputs = claude_view::window_inputs(latest.as_ref(), &[], t + 5);
    let none = e
        .core
        .with_db(|c| Ok(claude_view::evaluate(&inputs, &[], &ClaudeNotificationSettings::default(), Lang::En, &SqliteStore(c))))
        .unwrap();
    assert!(none.is_empty());
}

#[test]
fn a_passed_reset_never_drives_reminders() {
    let e = Env::new();
    let t = now_ms();
    e.core.with_db_mut(|c| store::ingest(c, &obs(t - 2 * H, &[(WEEKLY_KEY, 95.0, Some(t - H))]), None, t)).unwrap();
    let latest = e.core.with_db(|c| store::latest(c, PROVIDER_CLAUDE)).unwrap();
    let inputs = claude_view::window_inputs(latest.as_ref(), &[], t);
    assert_eq!(inputs[0].remaining, None, "the previous period's 5 % is not the current quota");
}

#[test]
fn codex_and_claude_low_in_one_evaluation_become_one_notification() {
    let e = Env::new();
    let t = now_ms();
    e.core.with_db_mut(|c| store::ingest(c, &obs(t, &[(WEEKLY_KEY, 91.0, Some(t + 50 * H))]), None, t)).unwrap();
    let latest = e.core.with_db(|c| store::latest(c, PROVIDER_CLAUDE)).unwrap();
    let facts = MonitorFacts::default();
    let codex_windows =
        vec![WindowRuleInput { window: WindowType::Weekly, remaining: Some(8.0), cycle_key: Some("c-1".into()), risk: None }];
    let n = e
        .core
        .with_db(|c| {
            let mut cands = notifications::evaluate_windows(
                &NotifyInput {
                    provider: "codex",
                    facts: &facts,
                    poll_success: true,
                    windows: &codex_windows,
                    new_events: &[],
                    settings: &NotificationSettings::default(),
                    lang: Lang::En,
                    now_ms: t,
                },
                &SqliteStore(c),
            );
            let inputs = claude_view::window_inputs(latest.as_ref(), &[], t);
            cands.extend(claude_view::evaluate(&inputs, &[], &ClaudeNotificationSettings::default(), Lang::En, &SqliteStore(c)));
            Ok(notifications::aggregate(cands, Lang::En).unwrap())
        })
        .unwrap();
    assert_eq!(n.title, "AI quotas are running low");
    assert!(n.body.contains("Codex weekly: 8% left") && n.body.contains("Claude weekly: 9% left"), "{}", n.body);
    assert_eq!(n.kinds[0], CandidateKind::QuotaCritical);
    let providers: std::collections::BTreeSet<&str> = n.keys.iter().map(|k| k.provider.as_str()).collect();
    assert_eq!(providers.into_iter().collect::<Vec<_>>(), vec!["claude", "codex"], "each provider deduplicates its own rule");
}

#[test]
fn claude_refresh_event_is_user_visible_but_its_notice_is_off_by_default() {
    let e = Env::new();
    let t = now_ms();
    let reset = t + H;
    e.core.with_db_mut(|c| store::ingest(c, &obs(t, &[(FIVE_HOUR_KEY, 80.0, Some(reset))]), None, t)).unwrap();
    let r = e
        .core
        .with_db_mut(|c| store::ingest(c, &obs(reset + 5 * 60_000, &[(FIVE_HOUR_KEY, 2.0, Some(reset + 5 * H))]), None, reset + 5 * 60_000))
        .unwrap();
    let events = e.core.with_db(|c| store::events_by_ids(c, &r.event_ids)).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!((events[0].event_type.as_str(), events[0].visibility.as_str()), ("quota_refreshed_observed", "user"));
    let latest = e.core.with_db(|c| store::latest(c, PROVIDER_CLAUDE)).unwrap();
    let inputs = claude_view::window_inputs(latest.as_ref(), &[], reset + 5 * 60_000);
    let off = e
        .core
        .with_db(|c| Ok(claude_view::evaluate(&inputs, &events, &ClaudeNotificationSettings::default(), Lang::En, &SqliteStore(c))))
        .unwrap();
    assert!(off.is_empty());
    let on = ClaudeNotificationSettings { allowance_refresh: true, ..Default::default() };
    let cands = e.core.with_db(|c| Ok(claude_view::evaluate(&inputs, &events, &on, Lang::En, &SqliteStore(c)))).unwrap();
    assert_eq!(cands[0].title, "Claude 5-hour quota reset");
}

// --------------------------------------------------------------------------- tray, history, events

fn base_tray(severity: Severity) -> TrayStatusModel {
    TrayStatusModel {
        severity,
        icon: TrayIconKind::Normal,
        paused: false,
        availability: Some(true),
        availability_unknown_persistent: false,
        five_hour_remaining: Some(73.0),
        weekly_remaining: Some(94.0),
        updated_at_ms: Some(1),
        freshness: Freshness::Fresh,
        minutes_since_update: Some(0),
        reasons: vec![],
        tooltip_lines: vec!["QuotaDock".into(), "5h: 73% · Weekly: 94%".into(), "Updated just now".into()],
        menu_five_hour: "5h 73%".into(),
        menu_weekly: "Weekly 94%".into(),
        providers: vec![],
    }
}

fn claude_line(severity: Severity, note: Option<&str>) -> ProviderTrayLine {
    ProviderTrayLine {
        provider: "claude",
        display_name: "Claude",
        severity,
        reasons: vec![],
        five_hour_remaining: Some(61.0),
        weekly_remaining: Some(72.0),
        five_hour_tone: DisplayTone::Normal,
        weekly_tone: DisplayTone::Normal,
        live: note.is_none(),
        has_data: true,
        polls_failing: false,
        not_found: false,
        unverified_version: false,
        show_five_hour: true,
        note: note.map(str::to_string),
        note_short: None,
        waiting_note: None,
        menu_header: "Claude".into(),
        menu_five_hour: "5h 61%".into(),
        menu_weekly: "Weekly 72%".into(),
    }
}

#[test]
fn tray_takes_the_highest_severity_and_keeps_every_provider() {
    let healthy_stale = with_provider(base_tray(Severity::Normal), claude_line(Severity::Normal, Some("Not live")), Lang::En);
    assert_eq!(healthy_stale.severity, Severity::Normal, "stale Claude data never turns the tray red");
    assert_eq!(healthy_stale.tooltip_lines, vec!["QuotaDock", "Claude 5h: 61% · Weekly: 72% · Not live", "Updated just now"]);
    let codex_error = with_provider(base_tray(Severity::Error), claude_line(Severity::Normal, None), Lang::En);
    assert_eq!((codex_error.severity, codex_error.icon), (Severity::Error, TrayIconKind::Error));
    let both_low = with_provider(base_tray(Severity::Warning), claude_line(Severity::Critical, None), Lang::En);
    assert_eq!((both_low.severity, both_low.icon), (Severity::Critical, TrayIconKind::Critical));
    assert!(quotadock_core::product::tray::tooltip_text(&both_low).chars().count() <= 127);
}

#[tokio::test]
async fn claude_tray_line_counts_only_current_enabled_windows() {
    let e = Env::new();
    let m = e.monitor("ok");
    m.refresh_environment(now_ms()).await;
    let t = now_ms();
    e.core
        .with_db_mut(|c| store::ingest(c, &obs(t, &[(WEEKLY_KEY, 91.0, Some(t + 50 * H)), (FIVE_HOUR_KEY, 96.0, Some(t + H))]), None, t))
        .unwrap();
    let status = m.status(&e.core, true, false, t).unwrap();
    assert_eq!(status.state, ClaudeSourceState::Live);
    let latest = e.core.with_db(|c| store::latest(c, PROVIDER_CLAUDE)).unwrap();
    let line = claude_view::tray_line(&status, latest.as_ref(), None, &ClaudeNotificationSettings::default(), Lang::En, t);
    assert_eq!(line.severity, Severity::Critical);
    assert_eq!(line.reasons, vec!["weekly_remaining_10"], "5-hour reminders off: 4 % left does not count");
    let later = t + 2 * H; // the 5-hour reset passed; weekly still current
    let line = claude_view::tray_line(&status, latest.as_ref(), None, &ClaudeNotificationSettings::default(), Lang::En, later);
    assert_eq!(line.five_hour_remaining, None);
    assert_eq!(line.weekly_remaining, Some(9.0));
    assert_eq!((line.five_hour_tone, line.weekly_tone), (DisplayTone::Unknown, DisplayTone::Critical), "a passed reset has no tone");
}

#[tokio::test]
async fn claude_display_tone_follows_remaining_percent_not_reminders() {
    let e = Env::new();
    let m = e.monitor("ok");
    m.refresh_environment(now_ms()).await;
    let t = now_ms();
    e.core
        .with_db_mut(|c| store::ingest(c, &obs(t, &[(WEEKLY_KEY, 40.0, Some(t + 50 * H)), (FIVE_HOUR_KEY, 96.0, Some(t + 4 * H))]), None, t))
        .unwrap();
    let latest = e.core.with_db(|c| store::latest(c, PROVIDER_CLAUDE)).unwrap();
    let line = |status: &ClaudeStatus, now: i64| claude_view::tray_line(status, latest.as_ref(), None, &ClaudeNotificationSettings::default(), Lang::En, now);

    let live = line(&m.status(&e.core, true, false, t).unwrap(), t);
    assert_eq!((live.five_hour_tone, live.weekly_tone), (DisplayTone::Critical, DisplayTone::Normal));
    assert_eq!(live.severity, Severity::Normal, "5-hour reminders are off");
    assert!(live.reasons.is_empty(), "{:?}", live.reasons);

    let later = t + 3 * H;
    let stale = m.status(&e.core, true, false, later).unwrap();
    assert!(!stale.live);
    let not_live = line(&stale, later);
    assert_eq!((not_live.five_hour_tone, not_live.weekly_tone), (DisplayTone::Critical, DisplayTone::Muted), "a stale low stays low, a stale high goes muted");

    m.set_paused(true, t);
    let paused = m.status(&e.core, true, false, t).unwrap();
    assert!(paused.live, "the data is recent, but monitoring is paused");
    assert_eq!(line(&paused, t).weekly_tone, DisplayTone::Muted);
}

#[tokio::test]
async fn a_tray_line_with_no_data_says_why_unless_it_is_really_waiting() {
    let e = Env::new();
    let m = e.monitor("ok");
    m.refresh_environment(now_ms()).await;
    let t = now_ms();
    let mut status = m.status(&e.core, true, false, t).unwrap();
    let mut line = |state, latest: Option<&StoredObservation>, lang| {
        status.state = state;
        claude_view::tray_line(&status, latest, None, &ClaudeNotificationSettings::default(), lang, t)
    };
    use ClaudeSourceState::*;
    for (state, en, zh) in [
        (NotConfigured, "Not set up", "未设置"),
        (ApiBilling, "API billing", "API 计费"),
        (NotFound, "Not installed", "未安装"),
        (NotAuthenticated, "Not signed in", "未登录"),
        (NeedsVerification, "Needs verification", "需要验证"),
        (SourceError, "Cannot read data", "无法读取数据"),
    ] {
        let (l_en, l_zh) = (line(state, None, Lang::En), line(state, None, Lang::ZhCn));
        assert_eq!((l_en.note.as_deref(), l_en.waiting_note.as_deref()), (Some(en), Some(en)), "{state:?}");
        assert_eq!((l_zh.note.as_deref(), l_zh.waiting_note.as_deref()), (Some(zh), Some(zh)), "{state:?}");
    }
    // The true wait, and states where a wait is not the question.
    for state in [Waiting, Paused, Live] {
        assert_eq!(line(state, None, Lang::En).waiting_note, None, "{state:?}");
    }

    // With data the row has its values and its age: nothing here to tell apart from a wait.
    let o = obs(t, &[(WEEKLY_KEY, 40.0, Some(t + 50 * H)), (FIVE_HOUR_KEY, 20.0, Some(t + H))]);
    e.core.with_db_mut(|c| store::ingest(c, &o, None, t)).unwrap();
    let latest = e.core.with_db(|c| store::latest(c, PROVIDER_CLAUDE)).unwrap();
    for state in [NotConfigured, ApiBilling, NotFound, NotAuthenticated, NeedsVerification, SourceError, Waiting, Stale] {
        assert_eq!(line(state, latest.as_ref(), Lang::En).waiting_note, None, "{state:?}");
    }
    assert_eq!(line(Waiting, latest.as_ref(), Lang::En).note.as_deref(), Some("Not live"));
}

#[tokio::test]
async fn only_the_words_that_do_not_fit_have_a_short_form() {
    let e = Env::new();
    let m = e.monitor("ok");
    m.refresh_environment(now_ms()).await;
    let t = now_ms();
    let mut status = m.status(&e.core, true, false, t).unwrap();
    let mut short = |state, lang| {
        status.state = state;
        claude_view::tray_line(&status, None, None, &ClaudeNotificationSettings::default(), lang, t).note_short
    };
    use ClaudeSourceState::*;
    // Measured beside both value columns: the full English words need 141 and 137 px of 118.
    assert_eq!(short(NeedsVerification, Lang::En).as_deref(), Some("Needs check"));
    assert_eq!(short(SourceError, Lang::En).as_deref(), Some("Can't read"));
    for state in [NotConfigured, ApiBilling, NotFound, NotAuthenticated, Waiting, Stale, Live, Paused] {
        assert_eq!(short(state, Lang::En), None, "{state:?}");
    }
    for state in [NeedsVerification, SourceError, NotConfigured, ApiBilling, NotFound, NotAuthenticated, Waiting, Stale, Live, Paused] {
        assert_eq!(short(state, Lang::ZhCn), None, "{state:?}");
    }
}

#[test]
fn claude_window_status_carries_the_reset_precision() {
    let e = Env::new();
    let t = now_ms();
    let mut o = obs(t, &[(WEEKLY_KEY, 40.0, Some(t + 50 * H)), (FIVE_HOUR_KEY, 20.0, Some(t + H))]);
    o.windows[0].reset_precision_ms = 60_000;
    e.core.with_db_mut(|c| store::ingest(c, &o, None, t)).unwrap();
    let latest = e.core.with_db(|c| store::latest(c, PROVIDER_CLAUDE)).unwrap();
    let windows = claude_view::window_statuses(latest.as_ref(), t);
    let reset = |list: &[WindowStatus], w: WindowType| WidgetReset::of(list.iter().find(|s| s.window_type == w));
    assert_eq!(reset(&windows, WindowType::FiveHour), WidgetReset { schedule: "confirmed", at_ms: Some(t + H), precision_ms: Some(1000) });
    assert_eq!(reset(&windows, WindowType::Weekly), WidgetReset { schedule: "confirmed", at_ms: Some(t + 50 * H), precision_ms: Some(60_000) });
    // The 5-hour reset passed: never a countdown.
    let later = claude_view::window_statuses(latest.as_ref(), t + 2 * H);
    assert_eq!(reset(&later, WindowType::FiveHour), WidgetReset { schedule: "passed", at_ms: None, precision_ms: Some(1000) });
    // And no value from the previous period, as on the widget and the tray; its reported time stays.
    let five = later.iter().find(|s| s.window_type == WindowType::FiveHour).unwrap();
    assert_eq!((five.used_percent, five.remaining_percent), (None, None), "a passed reset has no current value");
    assert!(five.present);
    assert_eq!(five.reported_reset_at_ms, Some(t + H));
    let weekly = later.iter().find(|s| s.window_type == WindowType::Weekly).unwrap();
    assert_eq!((weekly.used_percent, weekly.remaining_percent), (Some(40.0), Some(60.0)), "the current window keeps its value");
    // No such window in the data, and no data at all.
    let weekly_only = obs(t + 1, &[(WEEKLY_KEY, 40.0, Some(t + 50 * H))]);
    e.core.with_db_mut(|c| store::ingest(c, &weekly_only, None, t + 1)).unwrap();
    let latest = e.core.with_db(|c| store::latest(c, PROVIDER_CLAUDE)).unwrap();
    assert_eq!(reset(&claude_view::window_statuses(latest.as_ref(), t), WindowType::FiveHour), WidgetReset::MISSING);
    assert_eq!(reset(&claude_view::window_statuses(None, t), WindowType::Weekly), WidgetReset::MISSING);
}

#[tokio::test]
async fn claude_extra_limit_carries_the_reset_precision() {
    let e = Env::new();
    let m = e.monitor("ok");
    m.refresh_environment(now_ms()).await;
    let t = now_ms();
    let mut o = obs(t, &[("seven_day_sonnet", 3.0, Some(t + 50 * H)), ("seven_day_fable", 4.0, Some(t + 50 * H))]);
    o.windows[0].reset_precision_ms = 60_000;
    e.core.with_db_mut(|c| store::ingest(c, &o, None, t)).unwrap();
    let latest = e.core.with_db(|c| store::latest(c, PROVIDER_CLAUDE)).unwrap();
    let status = m.status(&e.core, true, false, t).unwrap();
    let d = claude_view::dashboard(&status, latest.as_ref(), vec![], t);
    let precision = |key: &str| d.extra_limits.iter().find(|x| x.key == key).unwrap().reset_precision_ms;
    assert_eq!((precision("seven_day_sonnet"), precision("seven_day_fable")), (Some(60_000), Some(1000)));
}

/// A Claude forecast folded into a Codex low: the "Also" line still says it is an estimate.
#[test]
fn a_claude_forecast_folded_into_a_codex_toast_still_says_estimated() {
    let e = Env::new();
    let facts = MonitorFacts::default();
    let codex = vec![WindowRuleInput { window: WindowType::Weekly, remaining: Some(8.0), cycle_key: Some("c-1".into()), risk: None }];
    let claude = vec![claude_view::ClaudeWindowInput { window: WindowType::Weekly, remaining: Some(60.0), instance_key: Some("i-1".into()), risk: Some(Risk::AtRisk) }];
    let rules = ClaudeNotificationSettings { forecast_weekly: true, ..Default::default() };
    for (lang, title, also) in [
        (Lang::ZhCn, "Claude 额度可能提前用完", "另外: Claude 额度可能提前用完（估算）"),
        (Lang::En, "Claude quota may run out early", "Also: Claude quota may run out early (estimated)"),
    ] {
        let (n, forecast_title) = e
            .core
            .with_db(|c| {
                let mut cands = notifications::evaluate_windows(
                    &NotifyInput {
                        provider: "codex",
                        facts: &facts,
                        poll_success: true,
                        windows: &codex,
                        new_events: &[],
                        settings: &NotificationSettings::default(),
                        lang,
                        now_ms: now_ms(),
                    },
                    &SqliteStore(c),
                );
                cands.extend(claude_view::evaluate(&claude, &[], &rules, lang, &SqliteStore(c)));
                let forecast_title = cands.iter().find(|c| c.kind == CandidateKind::ForecastAtRisk).unwrap().title.clone();
                Ok((notifications::aggregate(cands, lang).unwrap(), forecast_title))
            })
            .unwrap();
        assert_eq!(forecast_title, title, "the title stays as it was");
        assert_eq!(n.kinds, vec![CandidateKind::QuotaCritical, CandidateKind::ForecastAtRisk]);
        assert!(n.body.contains(also), "{}", n.body);
    }
}

#[test]
fn history_breaks_claude_series_between_window_instances() {
    let e = Env::new();
    let t = now_ms() - 10 * H;
    let reset = t + 2 * H;
    for (i, used) in [10.0, 20.0, 30.0].into_iter().enumerate() {
        let at = t + i as i64 * 30 * 60_000;
        e.core.with_db_mut(|c| store::ingest(c, &obs(at, &[(FIVE_HOUR_KEY, used, Some(reset))]), None, at)).unwrap();
    }
    let after = reset + 10 * 60_000;
    e.core.with_db_mut(|c| store::ingest(c, &obs(after, &[(FIVE_HOUR_KEY, 3.0, Some(after + 5 * H))]), None, after)).unwrap();
    let (from_ms, resolution_ms) = history::range_bounds(HistoryRange::Day, now_ms(), None);
    let mut data = HistoryData {
        range: HistoryRange::Day,
        from_ms,
        to_ms: now_ms(),
        resolution_ms,
        series: vec![],
        markers: vec![],
        source_snapshots: 0,
    };
    e.core.with_db(|c| history::append_claude(c, &mut data)).unwrap();
    let five = data.series.iter().find(|s| s.provider == "claude" && s.window_type == WindowType::FiveHour).unwrap();
    assert_eq!(five.points.iter().map(|p| p.segment).collect::<Vec<_>>(), vec![0, 0, 0, 1], "a new instance is a new segment");
    assert_eq!(data.markers.len(), 1, "the refresh event is a marker");
    assert_eq!(data.markers[0].provider, "claude");
}

#[test]
fn events_of_both_providers_share_one_timeline() {
    let e = Env::new();
    let t = now_ms();
    e.core.with_db_mut(|c| store::ingest(c, &obs(t - 3 * H, &[(FIVE_HOUR_KEY, 80.0, Some(t - 2 * H))]), None, t)).unwrap();
    e.core.with_db_mut(|c| store::ingest(c, &obs(t - H, &[(FIVE_HOUR_KEY, 1.0, Some(t + 4 * H))]), None, t)).unwrap();
    let local = e.root.join("local");
    let rt = ProductRuntime::with_claude(e.core.clone(), ClaudeMonitor::new(&e.root, Path::new(&local))).unwrap();
    let events = rt.events(false, None, 50).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!((events[0].provider.as_str(), events[0].event_type.as_str()), ("claude", "quota_refreshed_observed"));
    assert!(events[0].id < 0, "provider_events ids are negated to stay unique");
    assert!(rt.events(false, Some(events[0].observed_at_ms), 50).unwrap().is_empty(), "keyset by time");
}

#[tokio::test]
async fn diagnostics_export_includes_claude_without_account_values() {
    let e = Env::new();
    let m = e.monitor("ok");
    m.refresh_environment(now_ms()).await;
    m.run_active(&e.core, now_ms()).await.unwrap().expect("stored");
    let rt = ProductRuntime::with_claude(e.core.clone(), m).unwrap();
    let path = rt.export_diagnostics().await.expect("export written");
    let text = std::fs::read_to_string(&path).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["claude"]["billing"], "subscription");
    assert_eq!(v["claude"]["recentRuns"].as_array().map(Vec::len), Some(1));
    assert!(!text.contains("someone@example.com") && !text.contains("org-"), "no account values");
}

/// Real run 2026-10-08: weekly 31 → 34 % over a 2-hour evening session was projected over the rest
/// of the week as "may run out early" (43 pp/day; the week's own average was ~11 pp/day).
#[test]
fn a_weekly_claude_forecast_needs_a_day_of_history() {
    use quotadock_core::analytics::config::AnalyticsConfig;
    use quotadock_core::analytics::forecast::Risk;
    let e = Env::new();
    let t = now_ms();
    let weekly_reset = t + 75 * H;
    for (i, used) in [31.0, 32.0, 33.0, 34.0].into_iter().enumerate() {
        let at = t - 2 * H + i as i64 * 40 * 60_000;
        e.core.with_db_mut(|c| store::ingest(c, &obs(at, &[(WEEKLY_KEY, used, Some(weekly_reset)), (FIVE_HOUR_KEY, used - 15.0, Some(t + 2 * H))]), None, at)).unwrap();
    }
    let latest = e.core.with_db(|c| store::latest(c, PROVIDER_CLAUDE)).unwrap();
    let f = e.core.with_db(|c| claude_view::forecasts(c, latest.as_ref(), &AnalyticsConfig::default(), t)).unwrap();
    let weekly = f.iter().find(|f| f.window_type == WindowType::Weekly).unwrap();
    assert_eq!(weekly.risk, Risk::InsufficientData, "two hours say nothing about the rest of the week");
    assert_eq!(weekly.required_span_ms, claude_view::WEEKLY_MIN_SPAN_MS);
    let five = f.iter().find(|f| f.window_type == WindowType::FiveHour).unwrap();
    assert!(five.burn_rate_per_hour.is_some(), "the 5-hour forecast keeps the 30-minute minimum");
}

/// A nested with_db used to block the thread forever (and with it every later poll).
#[test]
#[should_panic(expected = "nested with_db")]
fn nested_database_access_fails_instead_of_deadlocking() {
    let e = Env::new();
    let _ = e.core.with_db(|_| Ok(e.core.with_db(|_| Ok(())).is_err()));
}

fn run_bridge(local: &Path, args: &[&str], stdin: &str) -> String {
    use std::io::Write;
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_quotadock-claude-bridge"))
        .args(args)
        .env("USERPROFILE", local)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

const STATUS_JSON: &str = r#"{"session_id":"s-1","transcript_path":"C:/x/t.jsonl","cwd":"D:/private","version":"2.1.278","model":{"display_name":"Opus"},"rate_limits":{"five_hour":{"used_percentage":12.5,"resets_at":1791500000},"seven_day":{"used_percentage":30,"resets_at":1791900000}}}"#;

#[test]
fn bridge_binary_writes_the_record_and_prints_one_line() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_bridge(dir.path(), &[], STATUS_JSON);
    assert_eq!(out, "Opus · 5h 13% used · week 30% used");
    let rec = statusline::read_record(&statusline::bridge_dir(dir.path())).expect("record written");
    assert_eq!(rec.rate_limits["five_hour"].used_percentage, 12.5);
    let raw = std::fs::read_to_string(statusline::bridge_dir(dir.path()).join(statusline::BRIDGE_FILE)).unwrap();
    assert!(!raw.contains("private") && !raw.contains("t.jsonl") && !raw.contains("s-1"));
    // Garbage input never breaks Claude Code: a line is still printed.
    assert_eq!(run_bridge(dir.path(), &[], "not json"), "usage after first reply");
}

#[test]
fn bridge_binary_chains_an_existing_status_line() {
    use quotadock_core::claude::integration::b64_encode;
    let dir = tempfile::tempdir().unwrap();
    // Works under Git Bash and PowerShell alike (whichever Claude Code would use here).
    let cmd = "echo chained-line";
    let out = run_bridge(dir.path(), &["--chain-b64", &b64_encode(cmd.as_bytes())], STATUS_JSON);
    assert_eq!(out.trim(), "chained-line", "the user's own status line is shown");
    assert!(statusline::read_record(&statusline::bridge_dir(dir.path())).is_some(), "and the quota is still recorded");
}

#[tokio::test]
async fn the_bridge_folder_learns_whether_5_hour_is_shown_but_is_never_created_for_it() {
    use quotadock_core::claude::statusline::{BridgeDisplay, read_display};
    let e = Env::new();
    let rt = ProductRuntime::with_claude(e.core.clone(), e.monitor("ok")).unwrap();
    let dir = rt.claude.bridge_dir().to_path_buf();
    let _ = std::fs::remove_dir_all(&dir);
    rt.modify_settings(|s| s.claude.show_five_hour = false).unwrap();
    assert!(!dir.exists(), "no bridge folder: nothing is created");
    std::fs::create_dir_all(&dir).unwrap();
    assert_eq!(read_display(&dir), BridgeDisplay::default(), "no file yet: 5-hour shown");
    rt.modify_settings(|s| s.claude.show_five_hour = false).unwrap();
    assert_eq!(read_display(&dir), BridgeDisplay { show_five_hour: false });
    rt.modify_settings(|s| s.claude.show_five_hour = true).unwrap();
    assert_eq!(read_display(&dir), BridgeDisplay { show_five_hour: true });
}

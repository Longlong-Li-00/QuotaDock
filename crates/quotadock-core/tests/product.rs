//! Phase 3 product layer: notification rules/dedup/aggregation, tray severity, pause/resume,
//! settings, history series. Uses the real pipeline + SQLite with synthetic reads (no backend).

use quotadock_core::analytics::forecast::Risk;
use quotadock_core::analytics::model::{EvidenceConfidence, NewEvent, QuotaEventType, WindowType};
use quotadock_core::codex::provider::{PollStatus, RawRead};
use quotadock_core::codex::types::TransportKind;
use quotadock_core::pipeline::{CoreConfig, PollOutcome, PollTrigger, QuotaDockCore};
use quotadock_core::product::facts::{AuthoritativeAvailability, MonitorFacts};
use quotadock_core::product::history::HistoryRange;
use quotadock_core::product::i18n;
use quotadock_core::product::notifications::{self, CandidateKind, NotificationStore, NotifyInput, RuleKey, WindowRuleInput};
use quotadock_core::product::runtime::ProductRuntime;
use quotadock_core::product::settings::{AppSettings, Lang, LanguagePref, NotificationSettings, load_settings};
use quotadock_core::product::tray::{
    DisplayTone, MenuRow, ProviderTrayLine, Severity, TrayIconKind, TrayInput, derive_tray_status, display_tone, menu_line, value_from_last_period, with_provider,
    without_codex,
};
use quotadock_core::storage::snapshots::{SnapshotView, WindowView};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

const T0: i64 = 1_791_374_400_000;
const MIN: i64 = 60_000;
const HOUR: i64 = 60 * MIN;
const ACCOUNT: &str = "acct-product-test";

fn raw(five_used: f64, five_reset_s: i64, weekly_used: Option<f64>, weekly_reset_s: i64, allowed: Option<bool>) -> Value {
    let secondary = match weekly_used {
        Some(u) => json!({ "usedPercent": u, "windowDurationMins": 10080, "resetsAt": weekly_reset_s }),
        None => Value::Null,
    };
    let bucket = json!({
        "limitId": "codex",
        "primary": { "usedPercent": five_used, "windowDurationMins": 300, "resetsAt": five_reset_s },
        "secondary": secondary,
    });
    json!({
        "ordinaryUsageAllowed": allowed,
        "rateLimits": bucket.clone(),
        "rateLimitsByLimitId": { "codex": bucket },
        "rateLimitResetCredits": { "availableCount": 2, "credits": null },
        "accountId": ACCOUNT,
    })
}

/// A runtime whose Claude side lives in the test's own folder: `ProductRuntime::new` would use the
/// real profile, where the status-line bridge keeps its files.
fn isolated_runtime(core: &Arc<QuotaDockCore>) -> Arc<ProductRuntime> {
    let dir = core.database_info().path.parent().unwrap().to_path_buf();
    let monitor = quotadock_core::claude::monitor::ClaudeMonitor::new(&dir, &dir.join("home"));
    ProductRuntime::with_claude(core.clone(), monitor).unwrap()
}

struct Harness {
    core: Arc<QuotaDockCore>,
    rt: Arc<ProductRuntime>,
}

impl Harness {
    fn open(path: &Path) -> Self {
        let core = Arc::new(QuotaDockCore::open(CoreConfig::with_database(path.to_path_buf()), Arc::new(|_| {})).unwrap());
        let rt = isolated_runtime(&core);
        Self { core, rt }
    }

    /// One successful poll through the full pipeline + product evaluation.
    fn poll(&self, at: i64, result: Value) -> Option<String> {
        let read = RawRead {
            request_started_at_ms: at - 300,
            response_received_at_ms: at,
            result,
            cli_version: "0.162.0-alpha.2".into(),
            transport: TransportKind::StdioAppServer,
            active_account_id: Some(ACCOUNT.into()),
        };
        let (id, out) = self.core.ingest(&read, PollTrigger::Scheduled).unwrap();
        let outcome =
            PollOutcome { status: PollStatus::Success, snapshot_id: Some(id), error: None, events: out.events, log_lines: vec![] };
        self.rt.after_poll(&outcome).unwrap().notification.map(|(n, _)| n.title)
    }

    fn failed_poll(&self) -> Option<String> {
        self.failed_poll_at(quotadock_core::time::now_ms())
    }

    fn failed_poll_at(&self, at: i64) -> Option<String> {
        self.core
            .with_db(|c| {
                quotadock_core::storage::snapshots::insert_poll_run(
                    c,
                    at,
                    at,
                    "scheduled",
                    "network_error",
                    Some("timeout"),
                    None,
                    None,
                    None,
                )
            })
            .unwrap();
        let outcome = PollOutcome {
            status: PollStatus::NetworkError,
            snapshot_id: None,
            error: Some("timeout".into()),
            events: vec![],
            log_lines: vec![],
        };
        self.rt.after_poll(&outcome).unwrap().notification.map(|(n, _)| n.title)
    }

    fn english(&self) {
        let mut s = self.rt.settings();
        s.language = LanguagePref::En;
        self.rt.update_settings(s).unwrap();
    }
}

const R5: i64 = (T0 + 4 * HOUR) / 1000;
const RW: i64 = (T0 + 5 * 24 * HOUR) / 1000;

// --------------------------------------------------------------------------- availability

#[test]
fn availability_recovery_false_to_true_notifies_once() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::open(&dir.path().join("q.db"));
    h.english();
    assert_eq!(h.poll(T0, raw(100.0, R5, Some(40.0), RW, Some(false))), None, "true -> false: no notification");
    assert_eq!(h.poll(T0 + 5 * MIN, raw(100.0, R5, Some(40.0), RW, Some(false))), None);
    let f = h.core.with_db(quotadock_core::product::facts::load_facts).unwrap();
    assert_eq!(f.latest.as_ref().map(|l| (l.id, l.ordinary_usage_allowed)), Some((2, Some(false))), "facts expose the latest snapshot");
    assert_eq!(f.previous_authoritative.as_ref().map(|p| (p.snapshot_id, p.allowed)), Some((1, false)));
    assert_eq!(h.poll(T0 + 10 * MIN, raw(100.0, R5, Some(40.0), RW, Some(true))).as_deref(), Some("Codex is available again"));
    assert_eq!(h.poll(T0 + 15 * MIN, raw(100.0, R5, Some(40.0), RW, Some(true))), None, "only once per blocked period");
}

#[test]
fn null_to_true_and_startup_true_do_not_notify() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::open(&dir.path().join("q.db"));
    assert_eq!(h.poll(T0, raw(10.0, R5, Some(40.0), RW, Some(true))), None, "first snapshot ever: not a transition");
    let dir2 = tempfile::tempdir().unwrap();
    let h2 = Harness::open(&dir2.path().join("q.db"));
    assert_eq!(h2.poll(T0, raw(10.0, R5, Some(40.0), RW, None)), None);
    assert_eq!(h2.poll(T0 + 5 * MIN, raw(10.0, R5, Some(40.0), RW, Some(true))), None, "null -> true never counts");
}

#[test]
fn recovery_after_a_long_gap_is_not_treated_as_a_transition() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::open(&dir.path().join("q.db"));
    h.poll(T0, raw(100.0, R5, Some(40.0), RW, Some(false)));
    // QuotaDock was closed for hours; the next snapshot says allowed.
    assert_eq!(h.poll(T0 + 3 * HOUR, raw(5.0, R5 + 4 * 3600, Some(40.0), RW, Some(true))), None);
}

// --------------------------------------------------------------------------- thresholds

#[test]
fn weekly_threshold_dedup_survives_restart_and_rearms_on_a_new_cycle() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("q.db");
    {
        let h = Harness::open(&path);
        h.english();
        assert_eq!(h.poll(T0, raw(10.0, R5, Some(69.0), RW, Some(true))), None, "31% remaining");
        assert_eq!(h.poll(T0 + 5 * MIN, raw(10.0, R5, Some(71.0), RW, Some(true))).as_deref(), Some("Codex weekly quota below 30%"));
        assert_eq!(h.poll(T0 + 10 * MIN, raw(10.0, R5, Some(80.0), RW, Some(true))), None, "29 -> 20: no second 30% alert");
        assert_eq!(h.poll(T0 + 15 * MIN, raw(10.0, R5, Some(91.0), RW, Some(true))).as_deref(), Some("Codex weekly quota below 10%"));
    }
    // Restart: still 9% remaining, nothing new.
    let h = Harness::open(&path);
    assert_eq!(h.poll(T0 + 20 * MIN, raw(10.0, R5, Some(91.0), RW, Some(true))), None, "no duplicate after restart");
    // Weekly rollover (previous reset passed, usage back to ~5%): a new observed cycle re-arms.
    let rw2 = RW + 7 * 24 * 3600;
    let after = RW * 1000 + 5 * MIN;
    assert_eq!(h.poll(after, raw(10.0, (after / 1000) + 3 * 3600, Some(5.0), rw2, Some(true))), None);
    h.poll(after + 5 * MIN, raw(10.0, (after / 1000) + 3 * 3600, Some(5.0), rw2, Some(true)));
    h.poll(after + 10 * MIN, raw(10.0, (after / 1000) + 3 * 3600, Some(40.0), rw2, Some(true)));
    assert_eq!(
        h.poll(after + 15 * MIN, raw(10.0, (after / 1000) + 3 * 3600, Some(71.0), rw2, Some(true))).as_deref(),
        Some("Codex weekly quota below 30%"),
        "re-armed for the new cycle"
    );
}

#[test]
fn startup_below_ten_percent_sends_the_ten_percent_alert_once() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::open(&dir.path().join("q.db"));
    h.english();
    assert_eq!(h.poll(T0, raw(10.0, R5, Some(92.0), RW, Some(true))).as_deref(), Some("Codex weekly quota below 10%"));
    // The 30% rule was settled by the 10% alert for this cycle.
    assert_eq!(h.poll(T0 + 5 * MIN, raw(10.0, R5, Some(92.0), RW, Some(true))), None);
}

#[test]
fn five_hour_reminders_are_off_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::open(&dir.path().join("q.db"));
    // Tray freshness uses the wall clock, so this snapshot is "now".
    let now = quotadock_core::time::now_ms();
    assert_eq!(h.poll(now, raw(95.0, now / 1000 + 3 * 3600, Some(10.0), now / 1000 + 5 * 86400, Some(true))), None);
    let tray = h.rt.tray_status().unwrap();
    assert_eq!(tray.severity, Severity::Normal, "5h at 5% does not raise the tray while its reminders are off");
}

#[test]
fn missing_window_never_alerts() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::open(&dir.path().join("q.db"));
    h.poll(T0, raw(10.0, R5, Some(60.0), RW, Some(true)));
    assert_eq!(h.poll(T0 + 5 * MIN, raw(10.0, R5, None, RW, Some(true))), None, "weekly null: no threshold alert");
}

// --------------------------------------------------------------------------- provider errors

#[test]
fn provider_error_needs_three_failures_and_fires_once_per_streak() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::open(&dir.path().join("q.db"));
    h.english();
    // The 15-minute rule uses the wall clock: the last valid snapshot is "just now".
    let now = quotadock_core::time::now_ms();
    let (r5, rw) = (now / 1000 + 3 * 3600, now / 1000 + 5 * 86400);
    h.poll(now, raw(10.0, r5, Some(10.0), rw, Some(true)));
    assert_eq!(h.failed_poll(), None);
    assert_eq!(h.failed_poll(), None);
    assert_eq!(h.failed_poll().as_deref(), Some("QuotaDock cannot refresh Codex usage"));
    assert_eq!(h.failed_poll(), None, "same failure streak");
    h.poll(now + MIN, raw(10.0, r5, Some(10.0), rw, Some(true)));
    for _ in 0..2 {
        assert_eq!(h.failed_poll(), None);
    }
    assert!(h.failed_poll().is_some(), "a new streak after a success can alert again");
}

#[test]
fn provider_error_time_rule_counts_the_failure_streak_not_time_the_app_was_closed() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::open(&dir.path().join("q.db"));
    h.english();
    let now = quotadock_core::time::now_ms();
    let (r5, rw) = (now / 1000 + 3 * 3600, now / 1000 + 5 * 86400);
    // Last valid data is 3 h old (QuotaDock was closed); one failed startup read is not an outage.
    h.poll(now - 3 * HOUR, raw(10.0, r5, Some(10.0), rw, Some(true)));
    assert_eq!(h.failed_poll_at(now), None);

    // A streak whose first failure is 16 min old alerts with only two failures.
    let h2 = Harness::open(&dir.path().join("q2.db"));
    h2.english();
    h2.poll(now - 20 * MIN, raw(10.0, r5, Some(10.0), rw, Some(true)));
    h2.core
        .with_db(|c| {
            quotadock_core::storage::snapshots::insert_poll_run(
                c,
                now - 16 * MIN,
                now - 16 * MIN,
                "scheduled",
                "network_error",
                Some("timeout"),
                None,
                None,
                None,
            )
        })
        .unwrap();
    assert_eq!(h2.failed_poll_at(now).as_deref(), Some("QuotaDock cannot refresh Codex usage"));
}

// --------------------------------------------------------------------------- pure rules

#[derive(Default)]
struct MemStore(RefCell<HashSet<String>>);

impl MemStore {
    fn mark(&self, keys: &[RuleKey]) {
        for k in keys {
            self.0.borrow_mut().insert(format!("{}|{}|{}", k.rule_id, k.window_type, k.scope_key));
        }
    }
}

impl NotificationStore for MemStore {
    fn is_sent(&self, k: &RuleKey) -> bool {
        self.0.borrow().contains(&format!("{}|{}|{}", k.rule_id, k.window_type, k.scope_key))
    }
}

fn facts_with(latest_allowed: Option<bool>) -> MonitorFacts {
    let w = |used: f64| WindowView {
        present: true,
        used_percent: Some(used),
        remaining_percent: Some(100.0 - used),
        reset_at_ms: Some(T0 + HOUR),
        duration_mins: Some(300),
        quality: "normal".into(),
    };
    MonitorFacts {
        latest: Some(SnapshotView {
            id: 10,
            captured_at_ms: T0,
            account_fingerprint: Some("fp".into()),
            account_check: "match".into(),
            cli_version: None,
            transport: "stdio-app-server".into(),
            ordinary_usage_allowed: latest_allowed,
            availability: "allowed".into(),
            five_hour: w(10.0),
            weekly: w(91.0),
            reset_credit_count: Some(2),
            bucket_ids: vec!["codex".into()],
            quality: "normal".into(),
            analysis_status: "done".into(),
        }),
        last_valid_snapshot_at_ms: Some(T0),
        ..Default::default()
    }
}

fn weekly(remaining: Option<f64>, risk: Option<Risk>, cycle: &str) -> Vec<WindowRuleInput> {
    vec![WindowRuleInput { window: WindowType::Weekly, remaining, cycle_key: Some(cycle.into()), risk }]
}

fn eval(facts: &MonitorFacts, windows: &[WindowRuleInput], events: &[NewEvent], store: &MemStore) -> Vec<notifications::Candidate> {
    notifications::evaluate(
        &NotifyInput {
            provider: "codex",
            facts,
            poll_success: true,
            windows,
            new_events: events,
            settings: &NotificationSettings::default(),
            lang: Lang::En,
            now_ms: T0,
        },
        store,
    )
}

#[test]
fn forecast_at_risk_once_per_cycle() {
    let store = MemStore::default();
    let facts = facts_with(Some(true));
    let low = eval(&facts, &weekly(Some(60.0), Some(Risk::Low), "c1"), &[], &store);
    assert!(low.is_empty());
    let at = eval(&facts, &weekly(Some(60.0), Some(Risk::AtRisk), "c1"), &[], &store);
    assert_eq!(at.iter().map(|c| c.kind).collect::<Vec<_>>(), vec![CandidateKind::ForecastAtRisk]);
    store.mark(&notifications::aggregate(at, Lang::En).unwrap().keys);
    assert!(eval(&facts, &weekly(Some(60.0), Some(Risk::Watch), "c1"), &[], &store).is_empty(), "Watch never notifies");
    assert!(
        eval(&facts, &weekly(Some(60.0), Some(Risk::AtRisk), "c1"), &[], &store).is_empty(),
        "AtRisk -> Watch -> AtRisk: same cycle, no second"
    );
    assert_eq!(eval(&facts, &weekly(Some(60.0), Some(Risk::AtRisk), "c2"), &[], &store).len(), 1, "new cycle re-arms");
}

#[test]
fn low_quota_and_forecast_in_one_poll_become_one_message() {
    let store = MemStore::default();
    let facts = facts_with(Some(true));
    let cands = eval(&facts, &weekly(Some(9.0), Some(Risk::AtRisk), "c1"), &[], &store);
    assert_eq!(cands.len(), 2);
    let n = notifications::aggregate(cands, Lang::En).unwrap();
    assert_eq!(n.title, "Codex quota is running low");
    assert!(n.body.contains("9% weekly quota remaining."));
    assert!(n.body.contains("may run out before the next confirmed reset"));
    assert!(!n.body.contains("Also:"), "merged, not appended");
    let rules: Vec<&str> = n.keys.iter().map(|k| k.rule_id.as_str()).collect();
    assert!(rules.contains(&"quota_below_10") && rules.contains(&"quota_below_30") && rules.contains(&"forecast_at_risk"));
}

/// A forecast that is not the toast's own title must still read as an estimate in the "Also" line.
#[test]
fn a_forecast_folded_into_another_window_toast_still_says_estimated() {
    let store = MemStore::default();
    let facts = facts_with(Some(true));
    let settings = NotificationSettings { five_hour_quota: true, ..Default::default() };
    let windows = [
        WindowRuleInput { window: WindowType::FiveHour, remaining: Some(8.0), cycle_key: Some("f1".into()), risk: None },
        WindowRuleInput { window: WindowType::Weekly, remaining: Some(60.0), cycle_key: Some("w1".into()), risk: Some(Risk::AtRisk) },
    ];
    let cases = [
        (Lang::ZhCn, "Codex 5 小时额度低于 10%", "另外: Codex 额度可能提前用完 (每周)（估算）", "估算"),
        (Lang::En, "Codex 5-hour quota below 10%", "Also: Codex quota may run out early (Weekly) (estimated)", "estimated"),
    ];
    for (lang, title, also, word) in cases {
        let input = NotifyInput { provider: "codex", facts: &facts, poll_success: true, windows: &windows, new_events: &[], settings: &settings, lang, now_ms: T0 };
        let cands = notifications::evaluate_windows(&input, &store);
        let f = cands.iter().find(|c| c.kind == CandidateKind::ForecastAtRisk).expect("forecast candidate");
        assert!(!f.title.contains(word), "the title stays as it was: {}", f.title);
        assert_eq!(f.keys, vec![RuleKey { rule_id: "forecast_at_risk".into(), provider: "codex".into(), window_type: "weekly".into(), scope_key: "w1".into() }]);
        let n = notifications::aggregate(cands, lang).unwrap();
        assert_eq!(n.title, title);
        assert!(n.body.contains(also), "{}", n.body);
        assert_eq!(n.kinds, vec![CandidateKind::QuotaCritical, CandidateKind::ForecastAtRisk]);
    }
}

#[test]
fn several_triggers_produce_one_notification_ordered_by_priority() {
    let store = MemStore::default();
    let mut facts = facts_with(Some(true));
    facts.previous_authoritative = Some(AuthoritativeAvailability { allowed: false, captured_at_ms: T0 - 5 * MIN, snapshot_id: 9 });
    let refresh = NewEvent {
        event_key: "ev-refresh".into(),
        account_fingerprint: Some("fp".into()),
        window_type: Some(WindowType::Weekly),
        event_type: QuotaEventType::UnscheduledQuotaRefresh,
        observed_at_ms: T0,
        previous_snapshot_id: None,
        current_snapshot_id: 10,
        confidence: EvidenceConfidence::Medium,
        confirmed: true,
        evidence: json!({}),
        supersedes_event_key: None,
    };
    let cands = eval(&facts, &weekly(Some(9.0), Some(Risk::AtRisk), "c1"), &[refresh], &store);
    let n = notifications::aggregate(cands, Lang::En).unwrap();
    assert_eq!(n.title, "Codex is available again", "availability recovery has the highest priority");
    assert!(n.body.contains("Also:"), "{}", n.body);
    assert_eq!(n.kinds.len(), 3, "availability + refresh + merged low-quota/forecast");
}

#[test]
fn diagnostic_events_never_notify_and_credit_needs_high_confidence() {
    let store = MemStore::default();
    let facts = facts_with(Some(true));
    let ev = |t: QuotaEventType, c: EvidenceConfidence| NewEvent {
        event_key: format!("{t:?}"),
        account_fingerprint: Some("fp".into()),
        window_type: Some(WindowType::FiveHour),
        event_type: t,
        observed_at_ms: T0,
        previous_snapshot_id: None,
        current_snapshot_id: 10,
        confidence: c,
        confirmed: true,
        evidence: json!({ "creditBefore": 2, "creditAfter": 1 }),
        supersedes_event_key: None,
    };
    let diags = [
        ev(QuotaEventType::SlidingAnchorDetected, EvidenceConfidence::Medium),
        ev(QuotaEventType::WindowScheduleChanged, EvidenceConfidence::Medium),
        ev(QuotaEventType::TransientBackendState, EvidenceConfidence::Low),
        ev(QuotaEventType::BackendSnapshotReconciliation, EvidenceConfidence::Medium),
        ev(QuotaEventType::BucketTopologyChanged, EvidenceConfidence::Medium),
        ev(QuotaEventType::UnknownQuotaTransition, EvidenceConfidence::Low),
        ev(QuotaEventType::NaturalRolloverObserved, EvidenceConfidence::High), // allowance refresh: off by default
        ev(QuotaEventType::ResetCreditRedeemed, EvidenceConfidence::Medium),
    ];
    assert!(eval(&facts, &weekly(Some(60.0), None, "c1"), &diags, &store).is_empty());
    let credit =
        eval(&facts, &weekly(Some(60.0), None, "c1"), &[ev(QuotaEventType::ResetCreditRedeemed, EvidenceConfidence::High)], &store);
    let n = notifications::aggregate(credit, Lang::En).unwrap();
    assert_eq!(n.title, "Codex reset credit used");
    assert!(n.body.contains("2 → 1 credits remaining."));
}

#[test]
fn early_refresh_notifies_once_live_and_a_classifier_replay_never_resends_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("q.db");
    // Weekly 3 % with a fixed reset, then idle (0 %, resetsAt = now + 7 d) days before that reset.
    let idle = |at: i64| raw(10.0, R5, Some(0.0), at / 1000 + 7 * 24 * 3600 + 1, Some(true));
    let mut sent = vec![];
    let mut bodies = vec![];
    {
        let h = Harness::open(&path);
        h.english();
        for i in 0..5 {
            sent.extend(h.poll(T0 + i * 5 * MIN, raw(10.0, R5, Some(3.0), RW, Some(true))));
        }
        for i in 5..12 {
            let at = T0 + i * 5 * MIN;
            let read = RawRead {
                request_started_at_ms: at - 300,
                response_received_at_ms: at,
                result: idle(at),
                cli_version: "0.162.0-alpha.2".into(),
                transport: TransportKind::StdioAppServer,
                active_account_id: Some(ACCOUNT.into()),
            };
            let (id, out) = h.core.ingest(&read, PollTrigger::Scheduled).unwrap();
            let outcome =
                PollOutcome { status: PollStatus::Success, snapshot_id: Some(id), error: None, events: out.events, log_lines: vec![] };
            if let Some((n, _)) = h.rt.after_poll(&outcome).unwrap().notification {
                sent.push(n.title);
                bodies.push(n.body);
            }
        }
    }
    assert_eq!(sent, vec!["Codex quota reset early (Weekly)".to_string()], "one live notification, when confirmed");
    assert_eq!(bodies, vec!["QuotaDock observed an early reset of the Codex weekly quota. The cause is unknown.".to_string()]);
    // A classifier upgrade re-derives everything from the raw snapshots. The replayed refresh is
    // history: even without its dedup record it must not be sent as if it had just happened.
    {
        let (mut c, _) = quotadock_core::storage::open_database(&path).unwrap();
        let tx = c.transaction().unwrap();
        quotadock_core::storage::derived::reset_derived(&tx).unwrap();
        tx.execute("DELETE FROM notification_state", []).unwrap();
        tx.commit().unwrap();
    }
    let h = Harness::open(&path);
    let replayed = h.core.with_db(|c| quotadock_core::storage::derived::events(c, false, 50)).unwrap();
    assert_eq!(replayed.iter().filter(|e| e.event_type == "unscheduled_quota_refresh").count(), 1, "re-derived on open");
    assert_eq!(h.poll(T0 + 60 * MIN, idle(T0 + 60 * MIN)), None, "replayed events are never notified");
}

// --------------------------------------------------------------------------- tray

fn tray(
    facts: &MonitorFacts,
    risk: Option<Risk>,
    settings: &NotificationSettings,
    paused: bool,
    lang: Lang,
) -> quotadock_core::product::tray::TrayStatusModel {
    derive_tray_status(&TrayInput { facts, paused, weekly_risk: risk, show_five_hour: true, settings, lang, now_ms: T0 + MIN })
}

#[test]
fn tray_severity_precedence() {
    let s = NotificationSettings::default();
    let mut f = facts_with(Some(true));
    // weekly 9% remaining -> critical
    assert_eq!(tray(&f, None, &s, false, Lang::En).severity, Severity::Critical);
    // provider unavailable outranks everything
    f.trailing_failures = 1;
    f.last_poll_status = Some("provider_unavailable".into());
    let m = tray(&f, None, &s, false, Lang::En);
    assert_eq!(m.severity, Severity::Error);
    assert_eq!(m.icon, TrayIconKind::Error);
    // warning only: weekly 25% remaining
    let mut g = facts_with(Some(true));
    if let Some(l) = g.latest.as_mut() {
        l.weekly.remaining_percent = Some(25.0);
    }
    assert_eq!(tray(&g, None, &s, false, Lang::En).severity, Severity::Warning);
    // forecast AtRisk alone -> warning
    let mut h = facts_with(Some(true));
    if let Some(l) = h.latest.as_mut() {
        l.weekly.remaining_percent = Some(60.0);
    }
    assert_eq!(tray(&h, Some(Risk::AtRisk), &s, false, Lang::En).severity, Severity::Warning);
    assert_eq!(tray(&h, Some(Risk::Low), &s, false, Lang::En).severity, Severity::Normal);
    // blocked -> critical
    assert_eq!(tray(&facts_with(Some(false)), None, &s, false, Lang::En).severity, Severity::Critical);
}

#[test]
fn tray_unknown_availability_becomes_warning_after_three_snapshots() {
    let s = NotificationSettings::default();
    let mut f = facts_with(None);
    if let Some(l) = f.latest.as_mut() {
        l.weekly.remaining_percent = Some(60.0);
    }
    f.consecutive_null_availability = 1;
    let m = tray(&f, None, &s, false, Lang::En);
    assert_eq!(m.severity, Severity::Normal);
    assert!(m.tooltip_lines.iter().any(|l| l == "Availability unknown"));
    f.consecutive_null_availability = 3;
    let m = tray(&f, None, &s, false, Lang::En);
    assert_eq!(m.severity, Severity::Warning);
    assert!(m.tooltip_lines.iter().any(|l| l == "Availability has remained unknown"));
}

#[test]
fn tray_five_hour_only_counts_when_enabled() {
    let mut f = facts_with(Some(true));
    if let Some(l) = f.latest.as_mut() {
        l.weekly.remaining_percent = Some(60.0);
        l.five_hour.remaining_percent = Some(5.0);
    }
    let off = NotificationSettings::default();
    assert_eq!(tray(&f, None, &off, false, Lang::En).severity, Severity::Normal);
    let on = NotificationSettings { five_hour_quota: true, ..Default::default() };
    assert_eq!(tray(&f, None, &on, false, Lang::En).severity, Severity::Critical);
}

#[test]
fn tray_paused_and_stale_presentation() {
    let s = NotificationSettings::default();
    let mut f = facts_with(Some(true));
    if let Some(l) = f.latest.as_mut() {
        l.weekly.remaining_percent = Some(60.0);
    }
    f.last_valid_snapshot_at_ms = Some(T0 - 40 * MIN);
    let stale = tray(&f, None, &s, false, Lang::En);
    assert_eq!(stale.severity, Severity::Error, "no valid snapshot for > 30 min");
    assert!(stale.tooltip_lines.iter().any(|l| l.starts_with("Data stale")));
    let paused = tray(&f, None, &s, true, Lang::ZhCn);
    assert_eq!(paused.icon, TrayIconKind::Paused);
    assert_eq!(paused.tooltip_lines[0], "QuotaDock — 已暂停");
    assert!(paused.tooltip_lines.iter().any(|l| l.starts_with("上次更新")));
    assert_ne!(paused.severity, Severity::Error, "staleness while paused is not an error");
    assert_eq!(tray(&facts_with(Some(true)), None, &s, false, Lang::ZhCn).tooltip_lines[1], "5小时：90% · 每周：9%");
}

// --------------------------------------------------------------------------- display tone

fn with_remaining(five: f64, week: f64) -> MonitorFacts {
    let mut f = facts_with(Some(true));
    if let Some(l) = f.latest.as_mut() {
        l.five_hour.remaining_percent = Some(five);
        l.weekly.remaining_percent = Some(week);
    }
    f
}

#[test]
fn display_tone_boundaries() {
    use DisplayTone::*;
    assert_eq!(display_tone(Some(30.1), true), Normal);
    assert_eq!(display_tone(Some(30.0), true), Low);
    assert_eq!(display_tone(Some(10.1), true), Low);
    assert_eq!(display_tone(Some(10.0), true), Critical);
    assert_eq!((display_tone(None, true), display_tone(None, false)), (Unknown, Unknown));
    // Stale high goes muted; stale low stays: a value can only fall inside one window.
    assert_eq!((display_tone(Some(80.0), false), display_tone(Some(30.1), false)), (Muted, Muted));
    assert_eq!((display_tone(Some(30.0), false), display_tone(Some(10.1), false)), (Low, Low));
    assert_eq!((display_tone(Some(10.0), false), display_tone(Some(0.0), false)), (Critical, Critical));
    let names: Vec<Value> = [Normal, Low, Critical, Muted, Unknown].iter().map(|t| serde_json::to_value(t).unwrap()).collect();
    assert_eq!(names, vec![json!("normal"), json!("low"), json!("critical"), json!("muted"), json!("unknown")]);
}

/// The separation itself: colour follows remaining percent, severity and reasons follow reminders.
#[test]
fn four_percent_five_hour_with_reminder_off_is_critical_tone_and_normal_severity() {
    let f = with_remaining(4.0, 60.0);
    let off = NotificationSettings::default();
    assert!(!off.five_hour_quota);
    let m = tray(&f, None, &off, false, Lang::En);
    assert_eq!(m.providers[0].five_hour_tone, DisplayTone::Critical);
    assert_eq!(m.severity, Severity::Normal);
    assert_eq!(m.providers[0].severity, Severity::Normal);
    assert!(m.reasons.is_empty(), "{:?}", m.reasons);
    assert!(m.providers[0].reasons.is_empty(), "{:?}", m.providers[0].reasons);
    assert_eq!(m.icon, TrayIconKind::Normal);

    // The reminder on: the rules react, the tone does not move.
    let on = NotificationSettings { five_hour_quota: true, ..Default::default() };
    let m = tray(&f, None, &on, false, Lang::En);
    assert_eq!(m.providers[0].five_hour_tone, DisplayTone::Critical);
    assert_eq!(m.severity, Severity::Critical);
    assert_eq!(m.reasons, vec!["five_hour_remaining_10"]);

    // The other way round: the weekly reminder off leaves a weekly low coloured but silent.
    let quiet = NotificationSettings { weekly_quota: false, ..Default::default() };
    let m = tray(&with_remaining(60.0, 25.0), None, &quiet, false, Lang::En);
    assert_eq!(m.providers[0].weekly_tone, DisplayTone::Low);
    assert_eq!(m.severity, Severity::Normal);
    assert!(m.reasons.is_empty(), "{:?}", m.reasons);
}

#[test]
fn codex_tone_follows_freshness_and_pause() {
    let s = NotificationSettings::default();
    let tones = |f: &MonitorFacts, paused: bool| {
        let m = tray(f, None, &s, paused, Lang::En);
        (m.providers[0].five_hour_tone, m.providers[0].weekly_tone)
    };
    let mut f = with_remaining(90.0, 9.0);
    assert_eq!(tones(&f, false), (DisplayTone::Normal, DisplayTone::Critical), "fresh");
    f.last_valid_snapshot_at_ms = Some(T0 - 15 * MIN);
    assert_eq!(tones(&f, false), (DisplayTone::Normal, DisplayTone::Critical), "maybe stale is still current");
    assert_eq!(tones(&f, true), (DisplayTone::Muted, DisplayTone::Critical), "paused: never healthy, a low stays low");
    f.last_valid_snapshot_at_ms = Some(T0 - 40 * MIN);
    assert_eq!(tones(&f, false), (DisplayTone::Muted, DisplayTone::Critical), "stale high muted, stale low kept");
}

#[test]
fn codex_tone_is_unknown_without_a_usable_current_value() {
    let s = NotificationSettings::default();
    let five_tone = |change: &dyn Fn(&mut WindowView)| {
        let mut f = with_remaining(40.0, 60.0);
        change(&mut f.latest.as_mut().unwrap().five_hour);
        tray(&f, None, &s, false, Lang::En).providers[0].five_hour_tone
    };
    assert_eq!(five_tone(&|_| {}), DisplayTone::Normal);
    assert_eq!(five_tone(&|w| w.present = false), DisplayTone::Unknown, "missing window");
    assert_eq!(five_tone(&|w| w.quality = "transient".into()), DisplayTone::Unknown, "not normal quality");
    assert_eq!(five_tone(&|w| w.remaining_percent = None), DisplayTone::Unknown);
    assert_eq!(five_tone(&|w| w.reset_at_ms = Some(T0)), DisplayTone::Unknown, "the reset time has passed");
    assert_eq!(five_tone(&|w| w.reset_at_ms = None), DisplayTone::Normal, "no reset time is not a passed one");
    let none = tray(&MonitorFacts::default(), None, &s, false, Lang::En);
    assert_eq!((none.providers[0].five_hour_tone, none.providers[0].weekly_tone), (DisplayTone::Unknown, DisplayTone::Unknown));
}

// --------------------------------------------------------------------------- tray menu rows

fn claude_row(five: Option<f64>, weekly: Option<f64>, has_data: bool, note: Option<&str>) -> ProviderTrayLine {
    ProviderTrayLine {
        provider: "claude",
        display_name: "Claude",
        severity: Severity::Normal,
        reasons: vec![],
        five_hour_remaining: five,
        weekly_remaining: weekly,
        five_hour_tone: display_tone(five, true),
        weekly_tone: display_tone(weekly, true),
        live: note.is_none(),
        has_data,
        polls_failing: false,
        not_found: false,
        unverified_version: false,
        show_five_hour: true,
        note: note.map(str::to_string),
        note_short: None,
        waiting_note: None,
        menu_header: String::new(),
        menu_five_hour: String::new(),
        menu_weekly: String::new(),
    }
}

/// The Codex row in both languages, for the same facts.
fn codex_rows(f: &MonitorFacts, paused: bool, show_five_hour: bool) -> (String, String) {
    let s = NotificationSettings::default();
    let row = |lang| {
        let m = derive_tray_status(&TrayInput { facts: f, paused, weekly_risk: None, show_five_hour, settings: &s, lang, now_ms: T0 + MIN });
        let r = menu_line(&m, "codex", lang);
        assert!(r.enabled, "a provider that is on opens the main window");
        r.text
    };
    (row(Lang::ZhCn), row(Lang::En))
}

fn claude_rows(line: ProviderTrayLine, note_en: Option<&str>) -> (String, String) {
    let base = |lang| without_codex(false, lang);
    let zh = menu_line(&with_provider(base(Lang::ZhCn), line.clone(), Lang::ZhCn), "claude", Lang::ZhCn);
    let en_line = ProviderTrayLine { note: note_en.map(str::to_string), ..line };
    let en = menu_line(&with_provider(base(Lang::En), en_line, Lang::En), "claude", Lang::En);
    assert!(zh.enabled && en.enabled);
    (zh.text, en.text)
}

fn pair(zh: &str, en: &str) -> (String, String) {
    (zh.to_string(), en.to_string())
}

#[test]
fn menu_line_shows_values_rounded_and_drops_a_hidden_five_hour() {
    let f = with_remaining(35.6, 9.0);
    assert_eq!(codex_rows(&f, false, true), pair("Codex  5小时 36% · 每周 9%", "Codex  5h 36% · Weekly 9%"));
    assert_eq!(codex_rows(&f, false, false), pair("Codex  每周 9%", "Codex  Weekly 9%"));
    assert_eq!(claude_rows(claude_row(Some(61.0), Some(72.0), true, None), None), pair("Claude  5小时 61% · 每周 72%", "Claude  5h 61% · Weekly 72%"));
    let hidden = ProviderTrayLine { show_five_hour: false, ..claude_row(Some(61.0), Some(72.0), true, None) };
    assert_eq!(claude_rows(hidden, None), pair("Claude  每周 72%", "Claude  Weekly 72%"));
}

#[test]
fn menu_line_keeps_the_codex_status_visible() {
    // Limit reached comes from ordinaryUsageAllowed and keeps the values.
    let mut blocked = with_remaining(0.0, 40.0);
    blocked.latest.as_mut().unwrap().ordinary_usage_allowed = Some(false);
    assert_eq!(codex_rows(&blocked, false, true), pair("Codex · 已达到限制  5小时 0% · 每周 40%", "Codex · Limit reached  5h 0% · Weekly 40%"));
    // A limit from old data says so: paused, or the data's age.
    assert_eq!(codex_rows(&blocked, true, true), pair("Codex · 已达到限制 · 已暂停  5小时 0% · 每周 40%", "Codex · Limit reached · Paused  5h 0% · Weekly 40%"));
    let mut blocked_stale = blocked.clone();
    blocked_stale.last_valid_snapshot_at_ms = Some(T0 - 179 * MIN);
    assert_eq!(codex_rows(&blocked_stale, false, true), pair("Codex · 已达到限制 · 数据已过时 · 3 小时  5小时 0% · 每周 40%", "Codex · Limit reached · Data stale · 3 h  5h 0% · Weekly 40%"));
    // 0 % alone is not a limit.
    assert_eq!(codex_rows(&with_remaining(0.0, 40.0), false, true), pair("Codex  5小时 0% · 每周 40%", "Codex  5h 0% · Weekly 40%"));

    // Signed out: what to do, nothing else.
    let mut signed_out = with_remaining(36.0, 60.0);
    signed_out.trailing_failures = 1;
    signed_out.last_poll_status = Some("authentication_error".into());
    assert_eq!(codex_rows(&signed_out, false, true), pair("Codex · 请先登录 Codex", "Codex · Sign in to Codex first"));

    let mut unavailable = with_remaining(36.0, 60.0);
    unavailable.trailing_failures = 1;
    unavailable.last_poll_status = Some("provider_unavailable".into());
    assert_eq!(codex_rows(&unavailable, false, true), pair("Codex · 无法刷新 Codex 用量  5小时 36% · 每周 60%", "Codex · Cannot refresh Codex usage  5h 36% · Weekly 60%"));

    // A limit while reads fail: a failed read stores nothing, so the flag stands, and it comes first.
    let mut blocked_unavailable = blocked.clone();
    blocked_unavailable.trailing_failures = 1;
    blocked_unavailable.last_poll_status = Some("provider_unavailable".into());
    assert_eq!(
        codex_rows(&blocked_unavailable, false, true),
        pair("Codex · 已达到限制 · 无法刷新 Codex 用量  5小时 0% · 每周 40%", "Codex · Limit reached · Cannot refresh Codex usage  5h 0% · Weekly 40%")
    );
    assert_eq!(
        codex_rows(&blocked_unavailable, false, false),
        pair("Codex · 已达到限制 · 无法刷新 Codex 用量  每周 40%", "Codex · Limit reached · Cannot refresh Codex usage  Weekly 40%")
    );
    // Signed out stays what to do, nothing else, also with a limit.
    let mut blocked_signed_out = blocked.clone();
    blocked_signed_out.trailing_failures = 1;
    blocked_signed_out.last_poll_status = Some("authentication_error".into());
    assert_eq!(codex_rows(&blocked_signed_out, false, true), pair("Codex · 请先登录 Codex", "Codex · Sign in to Codex first"));

    // The data's age.
    let mut maybe = with_remaining(36.0, 60.0);
    maybe.last_valid_snapshot_at_ms = Some(T0 - 14 * MIN);
    assert_eq!(codex_rows(&maybe, false, true), pair("Codex · 数据可能已过时 · 15 分钟  5小时 36% · 每周 60%", "Codex · Data may be stale · 15 min  5h 36% · Weekly 60%"));
    let mut stale = with_remaining(36.0, 60.0);
    stale.last_valid_snapshot_at_ms = Some(T0 - 40 * MIN);
    assert_eq!(codex_rows(&stale, false, true), pair("Codex · 数据已过时 · 41 分钟  5小时 36% · 每周 60%", "Codex · Data stale · 41 min  5h 36% · Weekly 60%"));
    assert_eq!(codex_rows(&stale, false, false), pair("Codex · 数据已过时 · 41 分钟  每周 60%", "Codex · Data stale · 41 min  Weekly 60%"));

    // Paused is not an age alarm.
    assert_eq!(codex_rows(&stale, true, true), pair("Codex · 已暂停  5小时 36% · 每周 60%", "Codex · Paused  5h 36% · Weekly 60%"));

    // A missing window is a dash, never 0 %.
    let mut missing = with_remaining(36.0, 60.0);
    let five = &mut missing.latest.as_mut().unwrap().five_hour;
    five.present = false;
    five.remaining_percent = None;
    assert_eq!(codex_rows(&missing, false, true), pair("Codex  5小时 — · 每周 60%", "Codex  5h — · Weekly 60%"));

    // First launch: no values yet.
    assert_eq!(codex_rows(&MonitorFacts::default(), false, true), pair("Codex · 正在连接 Codex…", "Codex · Connecting to Codex…"));
}

#[test]
fn menu_line_does_not_say_connecting_while_every_poll_fails() {
    let failing = MonitorFacts { trailing_failures: 3, last_poll_status: Some("network_error".into()), ..Default::default() };
    assert_eq!(codex_rows(&failing, false, true), pair("Codex · 无法刷新 Codex 用量", "Codex · Cannot refresh Codex usage"));
    assert_eq!(codex_rows(&failing, true, true), pair("Codex · 已暂停", "Codex · Paused"), "paused keeps its word");
    // Display only: nothing raised, no reason, no note, the tooltip as before.
    let m = tray(&failing, None, &NotificationSettings::default(), false, Lang::En);
    let line = &m.providers[0];
    assert!(line.polls_failing && line.note.is_none() && line.reasons.is_empty(), "{line:?}");
    assert_eq!((m.severity, line.severity), (Severity::Normal, Severity::Normal));
    assert_eq!(m.tooltip_lines, vec!["QuotaDock", "Cannot refresh Codex usage"], "the tooltip says why, as the menu row does");
}

#[test]
fn menu_line_names_a_missing_installation_and_only_that() {
    let failing = |error: &str| MonitorFacts {
        trailing_failures: 3,
        last_poll_status: Some("provider_unavailable".into()),
        last_error: Some(error.into()),
        ..Default::default()
    };
    let not_found = failing("no codex executable found");
    assert_eq!(codex_rows(&not_found, false, true), pair("Codex · 未找到 Codex", "Codex · Codex installation not found"));
    // Display only: the same reason and level, and the tooltip's words as before.
    let s = NotificationSettings::default();
    let m = tray(&not_found, None, &s, false, Lang::En);
    assert!(m.providers[0].not_found);
    assert_eq!((m.reasons.clone(), m.severity), (vec!["provider_unavailable"], Severity::Error));
    assert_eq!(m.tooltip_lines, vec!["QuotaDock", "Codex installation not found"]);

    // An older version says so, as the Overview does; any other failure keeps the general words.
    let older = failing("only unverified older codex versions found; no executable with a verified or newer version");
    assert_eq!(codex_rows(&older, false, true), pair("Codex · Codex 版本尚未验证", "Codex · Codex version not yet verified"));
    let m = tray(&older, None, &s, false, Lang::En);
    assert!(!m.providers[0].not_found && m.providers[0].unverified_version);
    assert_eq!(m.tooltip_lines, vec!["QuotaDock", "Codex version not yet verified"]);
    let other = failing("connection closed");
    assert_eq!(codex_rows(&other, false, true), pair("Codex · 无法刷新 Codex 用量", "Codex · Cannot refresh Codex usage"));
    let m = tray(&other, None, &s, false, Lang::En);
    assert!(!m.providers[0].not_found && !m.providers[0].unverified_version);
    assert_eq!(m.tooltip_lines, vec!["QuotaDock", "Cannot refresh Codex usage"]);
    // Those words alone are not enough: this failure is not a provider_unavailable one.
    let network = MonitorFacts { last_poll_status: Some("network_error".into()), ..not_found.clone() };
    assert!(!tray(&network, None, &s, false, Lang::En).providers[0].not_found);
    assert!(!tray(&facts_with(Some(true)), None, &s, false, Lang::En).providers[0].not_found);

    // With data and a limit: the limit first, the values stay.
    let mut limit = with_remaining(0.0, 40.0);
    limit.latest.as_mut().unwrap().ordinary_usage_allowed = Some(false);
    limit.trailing_failures = 1;
    limit.last_poll_status = not_found.last_poll_status.clone();
    limit.last_error = not_found.last_error.clone();
    assert_eq!(
        codex_rows(&limit, false, true),
        pair("Codex · 已达到限制 · 未找到 Codex  5小时 0% · 每周 40%", "Codex · Limit reached · Codex installation not found  5h 0% · Weekly 40%")
    );
}

/// `with_remaining` is a read at T0 and `tray` looks at T0 + MIN. Both languages.
fn aged(minutes: i64) -> MonitorFacts {
    let mut f = with_remaining(36.0, 60.0);
    f.last_valid_snapshot_at_ms = Some(T0 + MIN - minutes * MIN);
    f
}

fn tooltip_age(minutes: i64, paused: bool) -> (String, String) {
    let line = |lang| tray(&aged(minutes), None, &NotificationSettings::default(), paused, lang).tooltip_lines[2].clone();
    (line(Lang::ZhCn), line(Lang::En))
}

#[test]
fn a_stale_age_goes_to_hours_at_two_hours() {
    assert_eq!(codex_rows(&aged(119), false, true), pair("Codex · 数据已过时 · 119 分钟  5小时 36% · 每周 60%", "Codex · Data stale · 119 min  5h 36% · Weekly 60%"));
    assert_eq!(codex_rows(&aged(120), false, true), pair("Codex · 数据已过时 · 2 小时  5小时 36% · 每周 60%", "Codex · Data stale · 2 h  5h 36% · Weekly 60%"));
    assert_eq!(codex_rows(&aged(179), false, true), pair("Codex · 数据已过时 · 2 小时  5小时 36% · 每周 60%", "Codex · Data stale · 2 h  5h 36% · Weekly 60%"));
    assert_eq!(codex_rows(&aged(1800), false, true), pair("Codex · 数据已过时 · 30 小时  5小时 36% · 每周 60%", "Codex · Data stale · 30 h  5h 36% · Weekly 60%"));
    assert_eq!(tooltip_age(119, false), pair("数据已过时 · 119 分钟", "Data stale · 119 min"));
    assert_eq!(tooltip_age(120, false), pair("数据已过时 · 2 小时", "Data stale · 2 h"));
}

#[test]
fn a_paused_tooltip_says_just_now_then_the_same_age() {
    assert_eq!(tooltip_age(0, true), pair("上次更新：刚刚", "Last update: just now"));
    assert_eq!(tooltip_age(1, true), pair("上次更新：1 分钟前", "Last update: 1 min ago"));
    assert_eq!(tooltip_age(119, true), pair("上次更新：119 分钟前", "Last update: 119 min ago"));
    assert_eq!(tooltip_age(120, true), pair("上次更新：2 小时前", "Last update: 2 h ago"));
    assert_eq!(tooltip_age(1800, true), pair("上次更新：30 小时前", "Last update: 30 h ago"));
    // Only ever 10 to 30 minutes: left as it is.
    assert_eq!(i18n::maybe_stale(Lang::En, 15), "Data may be stale · 15 min");
}

#[test]
fn state_words_are_in_sentence_case() {
    assert_eq!(i18n::not_live(Lang::En), "Not live");
    for (state, word) in [
        ("not_configured", "Not set up"),
        ("waiting", "Waiting for Claude Code"),
        ("api_billing", "API billing"),
        ("not_found", "Not installed"),
        ("not_authenticated", "Not signed in"),
        ("paused", "Paused"),
        ("disabled", "Off"),
        ("needs_verification", "Needs verification"),
        ("source_error", "Cannot read data"),
        ("anything else", "Unavailable"),
    ] {
        assert_eq!(i18n::claude_state_short(Lang::En, state), word);
    }
}

// --------------------------------------------------------------------------- a passed Codex window

#[test]
fn only_a_used_window_read_before_its_reset_is_from_the_last_period() {
    let now = T0 + MIN;
    let last = |captured: i64, reset: Option<i64>, remaining: Option<f64>| value_from_last_period(captured, reset, remaining, now);
    assert!(last(T0 - HOUR, Some(T0), Some(1.0)));
    assert!(last(T0 - HOUR, Some(T0), Some(99.9)));
    assert!(last(T0 - HOUR, Some(now), Some(1.0)), "due this very moment has passed, as the tone has it");
    assert!(!last(T0 - HOUR, Some(now + 1), Some(1.0)), "not passed yet");
    assert!(!last(T0, Some(T0), Some(1.0)), "read at the reset time");
    assert!(!last(T0 + 1, Some(T0), Some(1.0)), "read after it: the service's current answer");
    assert!(!last(T0 - HOUR, Some(T0), Some(100.0)), "idle: nothing left to reset");
    assert!(!last(T0 - HOUR, None, Some(1.0)), "no reset time is not a passed one");
    assert!(!last(T0 - HOUR, Some(T0), None), "no value to take away");
}

/// A read at T0, 1 % of 5-hour left, a reset 30 s later, looked at from T0 + MIN.
fn passed_five_hour() -> MonitorFacts {
    let mut f = with_remaining(1.0, 60.0);
    f.latest.as_mut().unwrap().five_hour.reset_at_ms = Some(T0 + 30_000);
    f
}

#[test]
fn a_codex_value_from_the_last_period_is_a_dash_on_the_tray() {
    let s = NotificationSettings::default();
    let f = passed_five_hour();
    let m = tray(&f, None, &s, false, Lang::En);
    assert_eq!((m.five_hour_remaining, m.providers[0].five_hour_remaining), (None, None));
    assert_eq!((m.weekly_remaining, m.providers[0].weekly_remaining), (Some(60.0), Some(60.0)), "only that window");
    assert_eq!(m.providers[0].five_hour_tone, DisplayTone::Unknown);
    assert_eq!(m.tooltip_lines[1], "5h: — · Weekly: 60%");
    assert_eq!((m.menu_five_hour.as_str(), m.providers[0].menu_five_hour.as_str()), ("5h —", "5h —"));
    assert_eq!(codex_rows(&f, false, true), pair("Codex  5小时 — · 每周 60%", "Codex  5h — · Weekly 60%"), "no word on this row, and no old number either");
    assert_eq!(codex_rows(&f, true, true), pair("Codex · 已暂停  5小时 — · 每周 60%", "Codex · Paused  5h — · Weekly 60%"));

    // The weekly window the same way.
    let mut weekly = with_remaining(36.0, 5.0);
    weekly.latest.as_mut().unwrap().weekly.reset_at_ms = Some(T0 + 30_000);
    assert_eq!(codex_rows(&weekly, false, true), pair("Codex  5小时 36% · 每周 —", "Codex  5h 36% · Weekly —"));

    // A limit keeps its word; it comes from the flag and says nothing about which window.
    let mut blocked = passed_five_hour();
    blocked.latest.as_mut().unwrap().ordinary_usage_allowed = Some(false);
    assert_eq!(codex_rows(&blocked, false, true), pair("Codex · 已达到限制  5小时 — · 每周 60%", "Codex · Limit reached  5h — · Weekly 60%"));
}

#[test]
fn a_read_after_the_reset_an_idle_window_and_no_reset_time_keep_their_numbers() {
    let s = NotificationSettings::default();
    let kept = |change: &dyn Fn(&mut SnapshotView), value: f64| {
        let mut f = passed_five_hour();
        change(f.latest.as_mut().unwrap());
        let m = tray(&f, None, &s, false, Lang::En);
        assert_eq!((m.five_hour_remaining, m.providers[0].five_hour_remaining), (Some(value), Some(value)));
        m.tooltip_lines[1].clone()
    };
    assert_eq!(kept(&|l| l.captured_at_ms = T0 + 40_000, 1.0), "5h: 1% · Weekly: 60%", "the read came after the reset time");
    assert_eq!(kept(&|l| l.five_hour.remaining_percent = Some(100.0), 100.0), "5h: 100% · Weekly: 60%", "idle");
    assert_eq!(kept(&|l| l.five_hour.reset_at_ms = None, 1.0), "5h: 1% · Weekly: 60%", "no reset time");
    assert_eq!(kept(&|l| l.five_hour.reset_at_ms = Some(T0 + HOUR), 1.0), "5h: 1% · Weekly: 60%", "not passed yet");
}

/// The rules read the window itself: the same reason, level and icon as before the value left the views.
#[test]
fn a_codex_value_from_the_last_period_still_counts_for_the_rules() {
    let on = NotificationSettings { five_hour_quota: true, ..Default::default() };
    let m = tray(&passed_five_hour(), None, &on, false, Lang::En);
    assert_eq!(m.five_hour_remaining, None);
    assert_eq!(m.reasons, vec!["five_hour_remaining_10"]);
    assert_eq!((m.severity, m.icon), (Severity::Critical, TrayIconKind::Critical));
    let off = tray(&passed_five_hour(), None, &NotificationSettings::default(), false, Lang::En);
    assert_eq!(off.severity, Severity::Normal);
    assert!(off.reasons.is_empty(), "{:?}", off.reasons);
}

#[test]
fn menu_line_shows_the_claude_note_and_never_an_availability_claim() {
    let not_live = claude_row(None, Some(45.0), true, Some("非实时"));
    assert_eq!(claude_rows(not_live, Some("Not live")), pair("Claude · 非实时  5小时 — · 每周 45%", "Claude · Not live  5h — · Weekly 45%"));
    let not_set_up = claude_row(None, None, false, Some("未设置"));
    assert_eq!(claude_rows(not_set_up, Some("Not set up")), pair("Claude · 未设置", "Claude · Not set up"));
    // 100 % used, and a limit reason that could only be a mistake: still only numbers.
    let empty = ProviderTrayLine { reasons: vec!["limit_reached"], severity: Severity::Critical, ..claude_row(Some(0.0), Some(0.0), true, None) };
    assert_eq!(claude_rows(empty, None), pair("Claude  5小时 0% · 每周 0%", "Claude  5h 0% · Weekly 0%"));
}

#[test]
fn menu_line_disables_a_provider_that_is_turned_off() {
    let row = |provider, lang| menu_line(&without_codex(false, lang), provider, lang);
    assert_eq!(row("codex", Lang::ZhCn), MenuRow { text: "Codex · 已关闭".into(), enabled: false });
    assert_eq!(row("claude", Lang::ZhCn), MenuRow { text: "Claude · 已关闭".into(), enabled: false });
    assert_eq!(row("codex", Lang::En), MenuRow { text: "Codex · Off".into(), enabled: false });
    assert_eq!(row("claude", Lang::En), MenuRow { text: "Claude · Off".into(), enabled: false });
}

// --------------------------------------------------------------------------- pause / settings / history

#[tokio::test]
async fn pause_stops_the_app_server_and_is_not_persisted() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("q.db");
    let fake = env!("CARGO_BIN_EXE_fake-codex-app-server");
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/codex-rate-limits-real-redacted.json");
    let mut cfg = CoreConfig::with_database(path.clone());
    cfg.provider.override_command = Some((fake.into(), vec!["--fixture".into(), fixture.to_string_lossy().into_owned()]));
    let core = Arc::new(QuotaDockCore::open(cfg, Arc::new(|_| {})).unwrap());
    let rt = isolated_runtime(&core);
    assert_eq!(core.poll_once(PollTrigger::Manual).await.status, PollStatus::Success);
    let pid = core.provider_status().await.app_server_pid.expect("app-server running");
    rt.pause().await;
    assert!(rt.is_paused());
    assert_eq!(core.provider_status().await.app_server_pid, None, "QuotaDock's app-server child is stopped");
    let out = std::process::Command::new("tasklist").args(["/FI", &format!("PID eq {pid}"), "/NH"]).output().unwrap();
    assert!(!String::from_utf8_lossy(&out.stdout).contains(&pid.to_string()));
    assert_eq!(rt.refresh().await, quotadock_core::polling::RefreshDecision::Paused, "no backend request while paused");
    assert!(rt.tray_status().unwrap().paused);
    rt.resume();
    assert!(!rt.is_paused());
    assert_eq!(core.poll_once(PollTrigger::Manual).await.status, PollStatus::Success, "resume starts the provider again");
    let starts: Vec<String> = core
        .with_db(|c| {
            let mut s = c.prepare("SELECT reason FROM provider_sessions ORDER BY id")?;
            s.query_map([], |r| r.get(0))?.collect()
        })
        .unwrap();
    assert_eq!(starts, ["initial", "after_stop"], "app-server starts are recorded for soak diagnostics");
    core.shutdown().await;
    drop(rt);
    drop(core);
    // Restart: pause is not persisted.
    let core = Arc::new(QuotaDockCore::open(CoreConfig::with_database(path), Arc::new(|_| {})).unwrap());
    assert!(!isolated_runtime(&core).is_paused());
}

#[test]
fn settings_round_trip_with_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::open(&dir.path().join("q.db"));
    let d = h.rt.settings();
    assert_eq!(d, AppSettings::default());
    assert!(d.close_to_tray && !d.launch_at_startup && !d.notifications.five_hour_quota && !d.notifications.allowance_refresh);
    assert!(d.notifications.weekly_quota && d.notifications.availability_restored && d.notifications.reset_credit);
    let mut s = d.clone();
    s.language = LanguagePref::ZhCn;
    s.notifications.five_hour_quota = true;
    h.rt.update_settings(s.clone()).unwrap();
    assert_eq!(h.core.with_db(load_settings).unwrap(), s);
    assert_eq!(h.rt.lang(), Lang::ZhCn);
    // Unknown/missing fields fall back to defaults.
    let partial: AppSettings = serde_json::from_str(r#"{"language":"en"}"#).unwrap();
    assert_eq!(partial.notifications, NotificationSettings::default());
}

#[test]
fn history_series_break_at_cycle_boundaries_and_carry_markers() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::open(&dir.path().join("q.db"));
    h.poll(T0, raw(80.0, R5, Some(10.0), RW, Some(true)));
    h.poll(T0 + 5 * MIN, raw(85.0, R5, Some(10.0), RW, Some(true)));
    // 5h rollover after its reset time.
    let after = R5 * 1000 + 2 * MIN;
    h.poll(after, raw(1.0, after / 1000 + 4 * 3600, Some(10.0), RW, Some(true)));
    h.poll(after + 5 * MIN, raw(2.0, after / 1000 + 4 * 3600, Some(10.0), RW, Some(true)));
    let hist = h
        .core
        .with_db(|c| {
            quotadock_core::product::history::load_history(
                c,
                &quotadock_core::codex::sanitize::account_fingerprint(ACCOUNT),
                HistoryRange::Day,
                after + 10 * MIN,
                None,
            )
        })
        .unwrap();
    let five = hist.series.iter().find(|s| s.window_type == WindowType::FiveHour).unwrap();
    let segments: HashSet<i64> = five.points.iter().map(|p| p.segment).collect();
    assert_eq!(segments.len(), 2, "two observed cycles -> two segments");
    assert!(five.points[2].segment != five.points[1].segment);
    assert!(hist.markers.iter().any(|m| m.event_type == "natural_rollover_observed"));
    let weekly = hist.series.iter().find(|s| s.window_type == WindowType::Weekly).unwrap();
    assert_eq!(weekly.points.iter().map(|p| p.segment).collect::<HashSet<_>>().len(), 1);
    // The runtime path (current account) works too.
    assert!(h.rt.history(HistoryRange::CurrentCycle).unwrap().is_some());
}

#[test]
fn history_downsamples_long_ranges() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::open(&dir.path().join("q.db"));
    for i in 0..48 {
        h.poll(T0 + i * 5 * MIN, raw(10.0 + (i as f64) * 0.1, R5, Some(10.0), RW, Some(true)));
    }
    let fp = quotadock_core::codex::sanitize::account_fingerprint(ACCOUNT);
    let now = T0 + 48 * 5 * MIN;
    let day = h.core.with_db(|c| quotadock_core::product::history::load_history(c, &fp, HistoryRange::Day, now, None)).unwrap();
    let month = h.core.with_db(|c| quotadock_core::product::history::load_history(c, &fp, HistoryRange::Month, now, None)).unwrap();
    let n_day = day.series[1].points.len();
    let n_month = month.series[1].points.len();
    assert_eq!(n_day, 48, "24 h: every poll");
    assert!(n_month <= 5, "30 d: hourly buckets ({n_month})");
    assert_eq!(month.resolution_ms, Some(HOUR));
}

#[test]
fn history_short_ranges_show_every_poll_of_the_last_hours() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::open(&dir.path().join("q.db"));
    for i in 0..96 {
        h.poll(T0 + i * 5 * MIN, raw(10.0 + (i as f64) * 0.1, R5, Some(10.0), RW, Some(true))); // 8 hours
    }
    let fp = quotadock_core::codex::sanitize::account_fingerprint(ACCOUNT);
    let now = T0 + 96 * 5 * MIN;
    let load = |r| h.core.with_db(|c| quotadock_core::product::history::load_history(c, &fp, r, now, None)).unwrap();
    let six = load(HistoryRange::SixHours);
    let twelve = load(HistoryRange::TwelveHours);
    assert_eq!((six.from_ms, six.resolution_ms), (now - 6 * HOUR, None), "6 h: raw points");
    assert_eq!((twelve.from_ms, twelve.resolution_ms), (now - 12 * HOUR, None), "12 h: raw points");
    let weekly = |d: &quotadock_core::product::history::HistoryData| d.series.iter().find(|s| s.window_type == WindowType::Weekly).unwrap().points.clone();
    assert_eq!(weekly(&six).len(), 72, "only the polls of the last 6 hours");
    assert!(weekly(&six).iter().all(|p| p.t >= now - 6 * HOUR));
    assert_eq!(weekly(&twelve).len(), 96, "all 8 hours fit in 12 h");
    assert_eq!(serde_json::to_value(HistoryRange::SixHours).unwrap(), "six_hours");
    assert_eq!(serde_json::from_value::<HistoryRange>(serde_json::json!("twelve_hours")).unwrap(), HistoryRange::TwelveHours);
}

#[test]
fn the_autostart_command_quotes_a_path_with_spaces() {
    use quotadock_core::product::autostart::run_command;
    let exe = std::path::Path::new(r"C:\Users\Jane Doe\AppData\Local\Programs\QuotaDock\quotadock-app.exe");
    assert_eq!(run_command(exe), r#""C:\Users\Jane Doe\AppData\Local\Programs\QuotaDock\quotadock-app.exe" --autostart"#);
}

// ------------------------------------------------------------------------------ tray icon bars

mod tray_bars {
    use super::*;
    use quotadock_core::product::tray::TrayStatusModel;
    use quotadock_core::product::tray_icon::{Bar, SIZE, bar_of, bars_for, render};

    fn pixel(img: &[u8], x: usize, y: usize) -> [u8; 4] {
        let i = (y * SIZE + x) * 4;
        [img[i], img[i + 1], img[i + 2], img[i + 3]]
    }

    fn bar(line: ProviderTrayLine) -> Option<Bar> {
        bar_of(Some(&line))
    }

    #[test]
    fn a_bar_is_as_tall_as_the_weekly_remaining_and_takes_the_worse_tone() {
        let base = claude_row(Some(80.0), Some(73.0), true, None);
        assert_eq!(bar(base.clone()), Some(Bar { remaining: 73.0, tone: DisplayTone::Normal }));
        let low_five = ProviderTrayLine { five_hour_tone: DisplayTone::Low, ..base.clone() };
        assert_eq!(bar(low_five.clone()).unwrap().tone, DisplayTone::Low, "a low 5-hour turns its bar amber");
        let hidden = ProviderTrayLine { show_five_hour: false, ..low_five };
        assert_eq!(bar(hidden).unwrap().tone, DisplayTone::Normal, "not when 5-hour is hidden");
        let limit = ProviderTrayLine { reasons: vec!["limit_reached"], ..base.clone() };
        assert_eq!(bar(limit).unwrap().tone, DisplayTone::Critical);
        assert_eq!(bar(ProviderTrayLine { weekly_remaining: None, ..base }), None, "no value: an empty track");
        assert_eq!(bar_of(None), None, "a provider turned off: an empty track");
    }

    #[test]
    fn paused_and_error_keep_their_own_icons() {
        for (icon, bars) in [(TrayIconKind::Normal, true), (TrayIconKind::Critical, true), (TrayIconKind::Paused, false), (TrayIconKind::Error, false)] {
            let m = TrayStatusModel { icon, ..tray(&facts_with(Some(true)), None, &NotificationSettings::default(), false, Lang::En) };
            assert_eq!(bars_for(&m).is_some(), bars, "{icon:?}");
        }
    }

    #[test]
    fn the_image_fills_from_the_bottom_in_the_tone_colour() {
        let img = render([Some(Bar { remaining: 50.0, tone: DisplayTone::Low }), None]);
        assert_eq!(img.len(), SIZE * SIZE * 4);
        let fill = pixel(&img, 10, 26);
        assert_eq!(&fill[..3], &[0xC7, 0x7C, 0x00], "amber, opaque");
        assert_eq!(fill[3], 255);
        let track = pixel(&img, 10, 8);
        assert_eq!(&track[..3], &[0xC7, 0x7C, 0x00], "a low bar's track is tinted too");
        let neutral = render([Some(Bar { remaining: 50.0, tone: DisplayTone::Normal }), None]);
        assert_eq!(&pixel(&neutral, 10, 8)[..3], &[0x76, 0x76, 0x76]);
        let sliver = render([Some(Bar { remaining: 1.0, tone: DisplayTone::Critical }), None]);
        assert_eq!(pixel(&sliver, 10, 26)[3], 255, "1 %: still two full pixel rows");
        assert!(track[3] > 50 && track[3] < 120, "the empty part is a faint track: {}", track[3]);
        let empty_bar = pixel(&img, 22, 26);
        assert!(empty_bar[3] < 120, "no value on the right: track only");
        assert_eq!(pixel(&img, 2, 16)[3], 0, "outside the bars: transparent");
        assert_eq!(pixel(&img, 6, 4)[3], 0, "rounded corner");
        let full = render([Some(Bar { remaining: 100.0, tone: DisplayTone::Normal }), Some(Bar { remaining: 0.0, tone: DisplayTone::Critical })]);
        assert_eq!(pixel(&full, 10, 5)[3], 255, "100 %: full height");
        assert!(pixel(&full, 22, 26)[3] < 120, "0 %: nothing filled");
    }
}

/// Not a check: writes the bar icons as PPM files over a light and a dark taskbar colour for a look
/// (QUOTADOCK_ICON_PREVIEW=<folder>).
#[test]
fn tray_bar_icon_preview() {
    use quotadock_core::product::tray_icon::{Bar, SIZE, render};
    let Some(dir) = std::env::var_os("QUOTADOCK_ICON_PREVIEW") else { return };
    let b = |remaining, tone| Some(Bar { remaining, tone });
    let sets = [
        [b(73.0, DisplayTone::Normal), b(97.0, DisplayTone::Normal)],
        [b(73.0, DisplayTone::Normal), b(22.0, DisplayTone::Low)],
        [b(6.0, DisplayTone::Critical), b(60.0, DisplayTone::Normal)],
        [b(82.0, DisplayTone::Muted), None],
    ];
    let scale = 4;
    let (w, h) = (sets.len() * SIZE * scale, 2 * SIZE * scale);
    let mut out = format!("P6 {w} {h} 255\n").into_bytes();
    for (row, bg) in [[0xf3u8, 0xf3, 0xf3], [0x20, 0x20, 0x20]].iter().enumerate() {
        let imgs: Vec<Vec<u8>> = sets.iter().map(|s| render(*s)).collect();
        for y in 0..SIZE * scale {
            for img in &imgs {
                for x in 0..SIZE * scale {
                    let i = ((y / scale) * SIZE + x / scale) * 4;
                    let a = f64::from(img[i + 3]) / 255.0;
                    for c in 0..3 {
                        out.push((f64::from(img[i + c]) * a + f64::from(bg[c]) * (1.0 - a)).round() as u8);
                    }
                }
            }
        }
        let _ = row;
    }
    std::fs::write(std::path::Path::new(&dir).join("tray-bars.ppm"), out).unwrap();
}

//! reset-detector-v3: a window with a fixed anchor that turns into an idle sliding anchor before its
//! reset time (a small early refresh; real 2026-10-23 evidence) is confirmed exactly once, the old
//! reset time is no longer presented as confirmed, and the detector never stays on a stale
//! reference. Plus the cases that must not change: noise, jitter, A->B->A, natural rollovers,
//! account isolation, restarts and deterministic replay.

mod common;
use common::*;
use quotadock_core::analytics::*;
use serde_json::Value;

const DAY: i64 = 24 * HOUR;
const WEEK_MINS: i64 = 10080;

fn fixture() -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/real-weekly-idle-refresh-2026-10-23.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("fixture present")).expect("fixture is JSON")
}

fn win(w: &Value) -> WindowObs {
    if w.get("present") == Some(&Value::Bool(false)) {
        return WindowObs::missing();
    }
    WindowObs::new(w["usedPercent"].as_f64().unwrap(), w["resetAtMs"].as_i64().unwrap(), w["durationMins"].as_i64().unwrap())
}

fn feed(tl: &mut Timeline, s: &Value) {
    assert_eq!(s["id"].as_i64().unwrap(), tl.next_id, "fixture snapshot ids are consecutive");
    tl.allowed = s["ordinaryUsageAllowed"].as_bool();
    tl.credits = s["resetCreditCount"].as_i64();
    tl.buckets = s["bucketIds"].as_array().unwrap().iter().map(|b| b.as_str().unwrap().to_string()).collect();
    tl.observe(s["capturedAtMs"].as_i64().unwrap(), win(&s["fiveHour"]), win(&s["weekly"]));
}

fn replay(fx: &Value) -> Timeline {
    let snaps = fx["snapshots"].as_array().unwrap();
    let mut tl = Timeline::new();
    tl.next_id = snaps[0]["id"].as_i64().unwrap();
    for s in snaps {
        feed(&mut tl, s);
    }
    tl
}

fn of(tl: &Timeline, t: WindowType, ty: QuotaEventType) -> Vec<NewEvent> {
    tl.events().into_iter().filter(|e| e.window_type == Some(t) && e.event_type == ty).collect()
}

fn opens(tl: &Timeline, t: WindowType) -> usize {
    tl.cycle_ops().iter().filter(|op| matches!(op, CycleOp::Open { window_type, .. } if *window_type == t)).count()
}

fn accepted_last(tl: &Timeline, t: WindowType) -> i64 {
    tl.state.context.as_ref().unwrap().windows[&t].history.back().unwrap().snapshot_id
}

/// The idle weekly window of the fixture: first idle observation and the reset time it replaced.
const FIRST_IDLE: i64 = 9653;
const OLD_RESET: i64 = 1_793_200_901_000; // 2026-10-29 00:21:41 KST

// ------------------------------------------------------------------------------ real evidence

#[test]
fn real_weekly_idle_refresh_is_confirmed_once() {
    let fx = fixture();
    let snaps = fx["snapshots"].as_array().unwrap();
    let mut tl = Timeline::new();
    tl.next_id = snaps[0]["id"].as_i64().unwrap();
    for s in snaps {
        feed(&mut tl, s);
        if s["id"].as_i64().unwrap() == FIRST_IDLE {
            // A conflicting idle observation is pending: the old reset time is in doubt.
            assert_ne!(tl.track(WindowType::Weekly).confirmed_reset_at_ms, Some(OLD_RESET), "stale anchor not presented as confirmed");
        }
    }

    let refresh = of(&tl, WindowType::Weekly, QuotaEventType::UnscheduledQuotaRefresh);
    assert_eq!(refresh.len(), 1, "exactly one refresh: {:?}", tl.types());
    let e = &refresh[0];
    assert_eq!(e.confidence, EvidenceConfidence::Medium);
    assert_eq!(e.evidence["cause"], "unknown", "the cause is never inferred");
    assert_eq!(e.previous_snapshot_id, Some(FIRST_IDLE - 1), "baseline: the last fixed-anchor observation");
    assert_eq!(e.observed_at_ms, snaps.iter().find(|s| s["id"] == FIRST_IDLE).unwrap()["capturedAtMs"].as_i64().unwrap());
    assert_eq!(e.current_snapshot_id, FIRST_IDLE + 2, "confirmed only after three consecutive idle observations");
    for bad in [QuotaEventType::NaturalRolloverObserved, QuotaEventType::ResetCreditRedeemed] {
        assert!(of(&tl, WindowType::Weekly, bad).is_empty(), "no {bad:?}");
    }
    assert_eq!(tl.user_visible().len(), 1, "the refresh is the only user-visible event: {:?}", tl.user_visible());

    // reset-detector-v2 wrote one transient event per poll for 13 hours (118 in this fixture).
    let transients = of(&tl, WindowType::Weekly, QuotaEventType::TransientBackendState).len();
    assert!(transients <= 1, "diagnostics bounded, got {transients}");

    // The reference moves on, the idle anchor is recognised, the old reset is gone.
    let last = snaps.last().unwrap()["id"].as_i64().unwrap();
    assert_eq!(accepted_last(&tl, WindowType::Weekly), last, "accepted reference advanced to the latest observation");
    let w = tl.track(WindowType::Weekly);
    assert_eq!(w.confirmed_reset_at_ms, None, "no confirmed reset for an idle window");
    assert!(w.sliding_anchor, "idle sliding anchor recognised");
    assert_eq!(w.observation_state, WindowObservationState::IdleOrUninitialized);
    assert_ne!(w.candidate_state, ResetCandidateState::Pending);

    // History is no longer cut: every idle weekly point is usable again.
    for s in snaps.iter().filter(|s| s["id"].as_i64().unwrap() >= FIRST_IDLE) {
        let q = tl.final_quality(s["id"].as_i64().unwrap(), WindowType::Weekly);
        assert!(q.is_none() || q == Some(SnapshotQuality::Normal), "snapshot {} weekly quality {q:?}", s["id"]);
    }

    // One cycle boundary for the one transition.
    assert_eq!(tl.closes(WindowType::Weekly), 1);
    assert_eq!(opens(&tl, WindowType::Weekly), 2, "first observation + the refresh");
}

#[test]
fn real_replay_writes_four_events_in_total() {
    let tl = replay(&fixture());
    let got: Vec<(Option<WindowType>, QuotaEventType)> = tl.events().iter().map(|e| (e.window_type, e.event_type)).collect();
    assert_eq!(
        got,
        vec![
            (Some(WindowType::FiveHour), QuotaEventType::SlidingAnchorDetected),
            // #9616: the fixed weekly anchor reported resetsAt + 1 s for one poll (A->B->A).
            (Some(WindowType::Weekly), QuotaEventType::TransientBackendState),
            (Some(WindowType::Weekly), QuotaEventType::UnscheduledQuotaRefresh),
            // #9671: credits 3 -> 2, 18 polls after the refresh: not correlated with it.
            (None, QuotaEventType::ResetCreditCountChanged),
        ],
        "reset-detector-v2 wrote {} events for the same snapshots",
        fixture()["_meta"]["v2Events"].as_array().unwrap().iter().map(|e| e["n"].as_i64().unwrap()).sum::<i64>()
    );
    let credit = tl.events().into_iter().find(|e| e.event_type == QuotaEventType::ResetCreditCountChanged).unwrap();
    assert_eq!(credit.evidence["correlatedQuotaRefresh"], false, "the refresh is never attributed to a reset credit");
}

#[test]
fn five_hour_window_is_unaffected_on_the_real_sequence() {
    let tl = replay(&fixture());
    assert!(tl.user_visible().iter().all(|e| e.window_type == Some(WindowType::Weekly)), "no 5h user-visible event");
    assert_eq!(tl.closes(WindowType::FiveHour), 0, "no 5h cycle boundary");
    let f = tl.track(WindowType::FiveHour);
    assert!(f.sliding_anchor, "the idle 5h window stays a sliding anchor");
    assert_eq!(f.confirmed_reset_at_ms, None);
}

#[test]
fn replay_is_deterministic() {
    let fx = fixture();
    let a = replay(&fx);
    let b = replay(&fx);
    let dump = |tl: &Timeline| serde_json::to_string(&tl.outputs).unwrap();
    assert_eq!(dump(&a), dump(&b));
    assert_eq!(a.state, b.state);
}

#[test]
fn restart_mid_candidate_keeps_the_outcome() {
    let fx = fixture();
    let snaps = fx["snapshots"].as_array().unwrap();
    let full = replay(&fx);
    // Stop right after the first idle observation (candidate pending), persist, start again.
    let mut first = Timeline::new();
    first.next_id = snaps[0]["id"].as_i64().unwrap();
    for s in snaps.iter().filter(|s| s["id"].as_i64().unwrap() <= FIRST_IDLE) {
        feed(&mut first, s);
    }
    let persisted = serde_json::to_string(&first.state).unwrap();
    let mut second = Timeline::new();
    second.state = serde_json::from_str(&persisted).unwrap();
    second.next_id = FIRST_IDLE + 1;
    second.outputs = first.outputs.clone();
    for s in snaps.iter().filter(|s| s["id"].as_i64().unwrap() > FIRST_IDLE) {
        feed(&mut second, s);
    }
    assert_eq!(serde_json::to_string(&second.events()).unwrap(), serde_json::to_string(&full.events()).unwrap());
    assert_eq!(second.state, full.state);
}

// ------------------------------------------------------------------------------ synthetic

/// Idle weekly point at `at`: 0 %, resetsAt = at + 7 d (+ a little latency).
fn idle(at: i64) -> WindowObs {
    wk(0.0, at + 7 * DAY + 1_000)
}

fn idle5(at: i64) -> WindowObs {
    fh(0.0, at + 5 * HOUR + 1_000)
}

/// Five polls of a fixed weekly anchor `reset` at `used`, starting at T0, 5 min apart. Returns the next time.
fn fixed_run(tl: &mut Timeline, used: f64, reset: i64) -> i64 {
    let mut at = T0;
    for _ in 0..5 {
        tl.observe(at, idle5(at), wk(used, reset));
        at += 5 * MIN;
    }
    at
}

#[test]
fn idle_needs_consecutive_observations_and_a_return_is_transient() {
    let mut tl = Timeline::new();
    let reset = T0 + 5 * DAY;
    let mut at = fixed_run(&mut tl, 3.0, reset);
    for _ in 0..2 {
        tl.observe(at, idle5(at), idle(at));
        at += 5 * MIN;
    }
    // Back to the fixed anchor before the third idle observation: A->B->A.
    tl.observe(at, idle5(at), wk(3.0, reset));
    assert!(tl.user_visible().is_empty(), "two idle observations are not enough: {:?}", tl.user_visible());
    assert!(of(&tl, WindowType::Weekly, QuotaEventType::UnscheduledQuotaRefresh).is_empty());
    let w = tl.track(WindowType::Weekly);
    assert_eq!(w.confirmed_reset_at_ms, Some(reset), "the fixed anchor stands");
}

#[test]
fn small_drop_within_noise_into_idle_is_not_a_refresh() {
    let mut tl = Timeline::new();
    let mut at = fixed_run(&mut tl, 1.0, T0 + 5 * DAY);
    for _ in 0..8 {
        tl.observe(at, idle5(at), idle(at));
        at += 5 * MIN;
    }
    assert!(tl.user_visible().is_empty(), "1 pp is noise: no user-visible refresh: {:?}", tl.user_visible());
    let w = tl.track(WindowType::Weekly);
    assert!(w.sliding_anchor, "the idle anchor is still recognised");
    assert_eq!(w.confirmed_reset_at_ms, None, "the old reset is not presented");
    assert!(of(&tl, WindowType::Weekly, QuotaEventType::TransientBackendState).len() <= 1, "diagnostics bounded");
    assert_eq!(accepted_last(&tl, WindowType::Weekly), tl.next_id - 1, "reference advanced");
}

#[test]
fn usage_noise_and_reset_jitter_are_not_refreshes() {
    let mut tl = Timeline::new();
    let reset = T0 + 5 * DAY;
    let mut at = fixed_run(&mut tl, 20.0, reset);
    for (used, r) in [(19.0, reset), (20.0, reset), (20.0, reset + 1_000), (20.0, reset), (21.0, reset)] {
        tl.observe(at, idle5(at), wk(used, r));
        at += 5 * MIN;
    }
    assert!(tl.user_visible().is_empty(), "{:?}", tl.user_visible());
    assert_eq!(tl.track(WindowType::Weekly).confirmed_reset_at_ms, Some(reset));
}

#[test]
fn natural_weekly_rollover_is_unaffected() {
    let mut tl = Timeline::new();
    let reset = T0 + 30 * MIN;
    let mut at = fixed_run(&mut tl, 50.0, reset);
    assert!(at < reset);
    at = reset + MIN;
    for _ in 0..5 {
        tl.observe(at, idle5(at), idle(at));
        at += 5 * MIN;
    }
    assert_eq!(of(&tl, WindowType::Weekly, QuotaEventType::NaturalRolloverObserved).len(), 1);
    assert!(of(&tl, WindowType::Weekly, QuotaEventType::UnscheduledQuotaRefresh).is_empty(), "a passed reset is a rollover, not a refresh");
}

#[test]
fn substantial_drop_into_idle_is_a_medium_confidence_refresh() {
    let mut tl = Timeline::new();
    let mut at = fixed_run(&mut tl, 43.0, T0 + 4 * DAY);
    for _ in 0..5 {
        tl.observe(at, idle5(at), idle(at));
        at += 5 * MIN;
    }
    let r = of(&tl, WindowType::Weekly, QuotaEventType::UnscheduledQuotaRefresh);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].confidence, EvidenceConfidence::Medium, "a consistent idle anchor is stable evidence");
    assert_eq!(r[0].evidence["cause"], "unknown");
}

#[test]
fn account_change_during_an_idle_candidate_is_isolated() {
    let mut tl = Timeline::new();
    let mut at = fixed_run(&mut tl, 3.0, T0 + 5 * DAY);
    tl.observe(at, idle5(at), idle(at));
    at += 5 * MIN;
    tl.account = "b".repeat(32);
    for _ in 0..5 {
        tl.observe(at, idle5(at), idle(at));
        at += 5 * MIN;
    }
    assert!(of(&tl, WindowType::Weekly, QuotaEventType::UnscheduledQuotaRefresh).is_empty(), "never compared across accounts");
    assert!(tl.types().contains(&QuotaEventType::AccountChanged));
}

#[test]
fn missing_or_malformed_weekly_does_not_create_a_false_refresh() {
    let mut tl = Timeline::new();
    let mut at = fixed_run(&mut tl, 3.0, T0 + 5 * DAY);
    tl.observe(at, idle5(at), idle(at));
    at += 5 * MIN;
    tl.observe(at, idle5(at), WindowObs::missing());
    at += 5 * MIN;
    tl.observe(
        at,
        idle5(at),
        WindowObs { present: true, used_percent: None, reset_at_ms: Some(at + 7 * DAY), duration_mins: Some(WEEK_MINS) },
    );
    assert!(tl.user_visible().is_empty(), "no refresh from missing or malformed points: {:?}", tl.user_visible());
    at += 5 * MIN;
    for _ in 0..4 {
        tl.observe(at, idle5(at), idle(at));
        at += 5 * MIN;
    }
    assert_eq!(of(&tl, WindowType::Weekly, QuotaEventType::UnscheduledQuotaRefresh).len(), 1, "confirmed once the idle anchor holds");
}

#[test]
fn a_drifting_backend_value_does_not_freeze_the_reference() {
    // Usage stays, but resetsAt jumps to a new value on every poll: every candidate is superseded.
    let mut tl = Timeline::new();
    let reset = T0 + 5 * DAY;
    let mut at = fixed_run(&mut tl, 40.0, reset);
    for k in 1..=40 {
        tl.observe(at, idle5(at), wk(40.0, reset + k * 10 * MIN));
        at += 5 * MIN;
    }
    let behind = tl.next_id - 1 - accepted_last(&tl, WindowType::Weekly);
    assert!(behind <= 12, "the reference is at most 12 polls behind, got {behind}");
    assert!(tl.user_visible().is_empty(), "no user-visible event from unstable values: {:?}", tl.user_visible());
    let diagnostics = tl.events().iter().filter(|e| e.window_type == Some(WindowType::Weekly)).count();
    assert!(diagnostics <= 8, "one diagnostic per run of superseded candidates and one per re-base, got {diagnostics} for 40 polls");
}

#[test]
fn natural_five_hour_rollover_is_unaffected() {
    for used in [4.0, 35.0] {
        let mut tl = Timeline::new();
        let reset = T0 + 20 * MIN;
        let w1 = T0 + 5 * DAY;
        let mut at = T0;
        while at < reset {
            tl.observe(at, fh(used, reset), wk(10.0, w1));
            at += 5 * MIN;
        }
        // The window expired at its reset time: idle, resetsAt = now + 5 h.
        for _ in 0..4 {
            tl.observe(at, idle5(at), wk(10.0, w1));
            at += 5 * MIN;
        }
        assert_eq!(of(&tl, WindowType::FiveHour, QuotaEventType::NaturalRolloverObserved).len(), 1, "used {used}: {:?}", tl.types());
        assert!(of(&tl, WindowType::FiveHour, QuotaEventType::UnscheduledQuotaRefresh).is_empty(), "used {used}");
        assert_eq!(tl.closes(WindowType::FiveHour), 1);
        assert_eq!(tl.user_visible().len(), 1, "used {used}: {:?}", tl.user_visible());
    }
}

#[test]
fn a_window_idle_seconds_before_its_reset_is_a_natural_rollover() {
    // The backend rolled the 5h window over 30 s before the local clock reached resetsAt.
    let mut tl = Timeline::new();
    let reset = T0 + 20 * MIN + 30_000;
    let w1 = T0 + 5 * DAY;
    let mut at = T0;
    for _ in 0..5 {
        tl.observe(at, fh(4.0, reset), wk(10.0, w1));
        at += 5 * MIN;
    }
    assert!(at - 5 * MIN < reset && at > reset);
    let mut at = reset - 30_000;
    for _ in 0..3 {
        tl.observe(at, idle5(at), wk(10.0, w1));
        at += 5 * MIN;
    }
    assert_eq!(of(&tl, WindowType::FiveHour, QuotaEventType::NaturalRolloverObserved).len(), 1, "{:?}", tl.types());
    assert!(of(&tl, WindowType::FiveHour, QuotaEventType::UnscheduledQuotaRefresh).is_empty(), "not an early refresh");
}

#[test]
fn usage_after_an_idle_refresh_reanchors_the_analysis_segment() {
    let mut tl = Timeline::new();
    let mut at = fixed_run(&mut tl, 3.0, T0 + 5 * DAY);
    for _ in 0..4 {
        tl.observe(at, idle5(at), idle(at));
        at += 5 * MIN;
    }
    let cycle = tl.track(WindowType::Weekly).current_cycle_key;
    // Usage started 2 min before this poll: the new weekly window is anchored at its first use.
    let started = at - 2 * MIN;
    tl.observe(at, idle5(at), wk(1.0, started + 7 * DAY));
    let first_active = tl.next_id - 1;
    at += 5 * MIN;
    tl.observe(at, idle5(at), wk(2.0, started + 7 * DAY));
    let w = tl.track(WindowType::Weekly);
    assert_eq!(w.analysis_segment_start_snapshot_id, Some(first_active), "segment restarts at the first active observation");
    assert_eq!(w.active_origin_ms, Some(started), "derived origin = resetsAt - 7 d (not a server timestamp)");
    assert_eq!(w.confirmed_reset_at_ms, Some(started + 7 * DAY));
    assert!(!w.sliding_anchor);
    assert_eq!(w.current_cycle_key, cycle, "same observed cycle: the refresh was the boundary");
    assert_eq!(tl.user_visible().len(), 1, "only the refresh itself: {:?}", tl.user_visible());
    assert_eq!(tl.closes(WindowType::Weekly), 1);
}

#[test]
fn five_hour_idle_to_active_reanchor_is_unaffected() {
    let mut tl = Timeline::new();
    let w1 = T0 + 5 * DAY;
    for i in 0..3 {
        let at = T0 + i * 5 * MIN;
        tl.observe(at, fh(0.0, at + 5 * HOUR), wk(2.0, w1));
    }
    let started = T0 + 12 * MIN;
    tl.observe(T0 + 15 * MIN, fh(4.0, started + 5 * HOUR), wk(2.0, w1));
    tl.observe(T0 + 20 * MIN, fh(6.0, started + 5 * HOUR), wk(2.0, w1));
    let t = tl.track(WindowType::FiveHour);
    assert_eq!(t.analysis_segment_start_snapshot_id, Some(4));
    assert_eq!(t.active_origin_ms, Some(started));
    assert_eq!(t.confirmed_reset_at_ms, Some(started + 5 * HOUR));
    assert_eq!(tl.types(), vec![QuotaEventType::SlidingAnchorDetected]);
    assert_eq!(tl.closes(WindowType::FiveHour), 0);
}

#[test]
fn idle_refresh_with_a_correlated_credit_decrease_is_a_redemption() {
    let mut tl = Timeline::new();
    let mut at = fixed_run(&mut tl, 5.0, T0 + 5 * DAY);
    tl.credits = Some(1); // 2 -> 1 in the same poll as the first idle observation
    for _ in 0..4 {
        tl.observe(at, idle5(at), idle(at));
        at += 5 * MIN;
    }
    let types = tl.types();
    assert!(types.contains(&QuotaEventType::ResetCreditRedeemed), "{types:?}");
    assert!(!types.contains(&QuotaEventType::UnscheduledQuotaRefresh), "{types:?}");
    assert!(!types.contains(&QuotaEventType::ResetCreditCountChanged), "the decrease is explained: {types:?}");
    assert_eq!(tl.closes(WindowType::Weekly), 1);
}

#[test]
fn a_quick_return_to_the_old_anchor_after_an_idle_refresh_is_reconciled() {
    let mut tl = Timeline::new();
    let reset = T0 + 5 * DAY;
    let mut at = fixed_run(&mut tl, 3.0, reset);
    for _ in 0..3 {
        tl.observe(at, idle5(at), idle(at));
        at += 5 * MIN;
    }
    let refresh = of(&tl, WindowType::Weekly, QuotaEventType::UnscheduledQuotaRefresh);
    assert_eq!(refresh.len(), 1);
    tl.observe(at, idle5(at), wk(3.0, reset));
    let rec = of(&tl, WindowType::Weekly, QuotaEventType::BackendSnapshotReconciliation);
    assert_eq!(rec.len(), 1, "{:?}", tl.types());
    assert_eq!(rec[0].supersedes_event_key.as_ref(), Some(&refresh[0].event_key));
    let w = tl.track(WindowType::Weekly);
    assert!(!w.sliding_anchor, "the idle declaration is reverted too");
    assert_eq!(w.confirmed_reset_at_ms, Some(reset), "the earlier anchor is restored");
    assert!(w.current_cycle_key.unwrap().ends_with("|weekly|cycle|1"), "the earlier cycle is reopened");
}

// ------------------------------------------------------------------ independent review, 2026-10-26

/// Idle as the backend reports it when its clock runs `ahead_ms` ahead of the local one.
fn idle5_ahead(at: i64, ahead_ms: i64) -> WindowObs {
    fh(0.0, at + ahead_ms + 5 * HOUR)
}

#[test]
fn a_rollover_seen_early_with_fast_polls_is_still_a_rollover() {
    // The backend rolled the 5h window over 30 s before the local reset time, and three reads (one
    // scheduled, two manual refreshes 10 s apart) landed in those 30 s.
    for used in [4.0, 35.0] {
        let mut tl = Timeline::new();
        let reset = T0 + 20 * MIN + 30_000;
        let w1 = T0 + 5 * DAY;
        let mut at = T0;
        for _ in 0..5 {
            tl.observe(at, fh(used, reset), wk(10.0, w1));
            at += 5 * MIN;
        }
        for at in [reset - 30_000, reset - 20_000, reset - 10_000] {
            tl.observe(at, idle5_ahead(at, 30_000), wk(10.0, w1));
        }
        let mut at = reset + 4 * MIN;
        for _ in 0..3 {
            tl.observe(at, idle5_ahead(at, 30_000), wk(10.0, w1));
            at += 5 * MIN;
        }
        assert_eq!(of(&tl, WindowType::FiveHour, QuotaEventType::NaturalRolloverObserved).len(), 1, "used {used}: {:?}", tl.types());
        assert!(of(&tl, WindowType::FiveHour, QuotaEventType::UnscheduledQuotaRefresh).is_empty(), "used {used}: not an early refresh");
        assert_eq!(tl.closes(WindowType::FiveHour), 1, "used {used}");
    }
}

#[test]
fn a_weekly_rollover_seen_early_within_noise_closes_the_cycle() {
    let mut tl = Timeline::new();
    let reset = T0 + 20 * MIN + 30_000;
    let mut at = T0;
    for _ in 0..5 {
        tl.observe(at, idle5(at), wk(1.0, reset));
        at += 5 * MIN;
    }
    for at in [reset - 30_000, reset - 20_000, reset - 10_000] {
        tl.observe(at, idle5(at), wk(0.0, at + 30_000 + 7 * DAY));
    }
    let mut at = reset + 4 * MIN;
    for _ in 0..3 {
        tl.observe(at, idle5(at), wk(0.0, at + 30_000 + 7 * DAY));
        at += 5 * MIN;
    }
    assert_eq!(of(&tl, WindowType::Weekly, QuotaEventType::NaturalRolloverObserved).len(), 1, "{:?}", tl.types());
    assert_eq!(tl.closes(WindowType::Weekly), 1, "the old weekly cycle is closed");
}

/// Fixed weekly anchor at `used`, then `drift` polls whose resetsAt moves 1 s further each time
/// (each candidate superseded by the next), then the window turns idle at `idle_at`.
fn superseded_run_then_idle(used: f64, reset: i64, drift: i64, idle_at: Option<i64>) -> Timeline {
    let mut tl = Timeline::new();
    let mut at = T0;
    for _ in 0..3 {
        tl.observe(at, idle5(at), wk(used, reset));
        at += 5 * MIN;
    }
    for k in 1..=drift {
        tl.observe(at, idle5(at), wk(used, reset + k * 1_000));
        at += 5 * MIN;
    }
    if let Some(t) = idle_at {
        at = t;
    }
    for _ in 0..6 {
        tl.observe(at, idle5(at), idle(at));
        at += 5 * MIN;
    }
    tl
}

#[test]
fn a_refresh_on_the_poll_that_ends_a_superseded_run_is_kept() {
    // 12 superseded candidates would re-base the reference on the next poll; that poll is the refresh.
    for used in [30.0, 8.0] {
        let tl = superseded_run_then_idle(used, T0 + 5 * DAY, 12, None);
        assert_eq!(of(&tl, WindowType::Weekly, QuotaEventType::UnscheduledQuotaRefresh).len(), 1, "used {used}: {:?}", tl.types());
        assert_eq!(tl.closes(WindowType::Weekly), 1, "used {used}: one cycle boundary");
    }
}

#[test]
fn a_rollover_on_the_poll_that_ends_a_superseded_run_is_kept() {
    let reset = T0 + 90 * MIN;
    let tl = superseded_run_then_idle(30.0, reset, 12, Some(reset + MIN));
    assert_eq!(of(&tl, WindowType::Weekly, QuotaEventType::NaturalRolloverObserved).len(), 1, "{:?}", tl.types());
    assert_eq!(tl.closes(WindowType::Weekly), 1);
}

#[test]
fn notification_scopes_ignore_the_classifier_version() {
    use quotadock_core::product::notifications::version_free_scope;
    assert_eq!(version_free_scope("reset-detector-v3|abc|weekly|cycle|13"), "abc|weekly|cycle|13");
    assert_eq!(version_free_scope("reset-detector-v2|abc|weekly|cycle|13"), version_free_scope("reset-detector-v3|abc|weekly|cycle|13"));
}

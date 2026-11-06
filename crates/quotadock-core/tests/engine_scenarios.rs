//! Synthetic reset-engine scenarios (spec tests A–N plus the earlier Case 1–13 timelines).

mod common;
use common::*;
use quotadock_core::analytics::*;
use quotadock_core::codex::types::UsageAvailability;

const R1: i64 = T0 + 3 * HOUR; // a 5h reset in the future
const W1: i64 = T0 + 5 * 24 * HOUR; // a weekly reset far in the future

fn reset_events(tl: &Timeline) -> Vec<QuotaEventType> {
    tl.types().into_iter().filter(|t| t.is_cycle_boundary()).collect()
}

#[test]
fn a_transient_timestamp_is_rejected() {
    // Mirrors the Phase 1B observation: A -> B -> A with unchanged usage.
    let mut tl = Timeline::new();
    tl.observe(T0, fh(12.0, R1), wk(2.0, W1));
    tl.observe(T0 + 5 * MIN, fh(12.0, R1 + 600_000), wk(2.0, W1));
    assert_eq!(tl.track(WindowType::FiveHour).observation_state, WindowObservationState::ScheduleChangeCandidate);
    assert_eq!(tl.track(WindowType::FiveHour).candidate_state, ResetCandidateState::Pending);
    assert_eq!(tl.track(WindowType::FiveHour).confirmed_reset_at_ms, None, "one observation never confirms an anchor");
    tl.observe(T0 + 10 * MIN, fh(12.0, R1), wk(2.0, W1));

    assert!(reset_events(&tl).is_empty(), "no reset");
    assert!(!tl.types().contains(&QuotaEventType::WindowScheduleChanged), "no confirmed schedule change");
    assert_eq!(tl.types(), vec![QuotaEventType::TransientBackendState]);
    assert!(tl.user_visible().is_empty(), "diagnostic only");
    assert_eq!(tl.final_quality(2, WindowType::FiveHour), Some(SnapshotQuality::SuspectTransient));
    let t = tl.track(WindowType::FiveHour);
    assert_eq!(t.confirmed_reset_at_ms, Some(R1));
    assert_eq!(t.observation_state, WindowObservationState::Active);
    assert_eq!(tl.closes(WindowType::FiveHour), 0, "no cycle boundary");
    let log = tl.outputs.iter().flat_map(|o| o.log_lines.iter()).cloned().collect::<Vec<_>>().join("\n");
    assert!(
        log.contains("old=") && log.contains("deltaSeconds=600") && log.contains("classification=candidate_opened:schedule_change"),
        "{log}"
    );
    assert!(log.contains("classification=transient_rejected"), "{log}");
}

#[test]
fn b_stable_new_timestamp_is_a_schedule_change_not_a_reset() {
    let mut tl = Timeline::new();
    tl.observe(T0, fh(12.0, R1), wk(2.0, W1));
    tl.observe(T0 + 5 * MIN, fh(12.0, R1 + 20 * MIN), wk(2.0, W1));
    tl.observe(T0 + 10 * MIN, fh(12.0, R1 + 20 * MIN), wk(2.0, W1));
    assert_eq!(tl.types(), vec![QuotaEventType::WindowScheduleChanged]);
    let e = &tl.events()[0];
    assert_eq!(e.confidence, EvidenceConfidence::Medium);
    assert_eq!(e.event_type.visibility(), EventVisibility::Diagnostic);
    assert!(reset_events(&tl).is_empty());
    assert_eq!(tl.closes(WindowType::FiveHour), 0);
    assert_eq!(tl.track(WindowType::FiveHour).confirmed_reset_at_ms, Some(R1 + 20 * MIN));
    assert_eq!(e.evidence["stableNewResetAtPolls"], 2);
}

#[test]
fn c_sliding_idle_anchor_is_detected_and_never_confirmed() {
    let mut tl = Timeline::new();
    let base = T0;
    for i in 0..3 {
        let at = base + i * 5 * MIN;
        tl.observe(at, fh(0.0, at + 5 * HOUR), wk(2.0, W1));
    }
    let t = tl.track(WindowType::FiveHour);
    assert_eq!(t.observation_state, WindowObservationState::IdleOrUninitialized);
    assert!(t.sliding_anchor);
    assert_eq!(t.confirmed_reset_at_ms, None, "a sliding anchor is never a confirmed reset time");
    assert_eq!(tl.types(), vec![QuotaEventType::SlidingAnchorDetected]);
    let ev = &tl.events()[0].evidence;
    assert_eq!(ev["samples"], 3);
    assert_eq!(ev["expectedDurationSeconds"], 18000);
    assert_eq!(ev["detected"], true);
    // Continued sliding produces no further events.
    tl.observe(base + 15 * MIN, fh(0.0, base + 15 * MIN + 5 * HOUR), wk(2.0, W1));
    assert_eq!(tl.types().len(), 1);
}

#[test]
fn d_normal_consumption_stays_in_one_active_cycle() {
    let mut tl = Timeline::new();
    for (i, u) in [12.0, 13.0, 15.0].into_iter().enumerate() {
        tl.observe(T0 + i as i64 * 5 * MIN, fh(u, R1), wk(2.0, W1));
    }
    assert!(tl.types().is_empty());
    let t = tl.track(WindowType::FiveHour);
    assert_eq!(t.observation_state, WindowObservationState::Active);
    assert_eq!(t.confirmed_reset_at_ms, Some(R1));
    assert_eq!(t.anchor_polls, 3);
    let opens = tl.cycle_ops().iter().filter(|o| matches!(o, CycleOp::Open { .. })).count();
    assert_eq!(opens, 2, "one cycle per window, opened at first observation");
}

#[test]
fn e_natural_rollover_needs_confirmation_and_uses_observed_time() {
    let mut tl = Timeline::new();
    tl.observe(T0, fh(98.0, R1), wk(40.0, W1));
    let t1 = R1 + 2 * MIN; // previous reset has passed
    let r2 = t1 + 4 * HOUR;
    tl.observe(t1, fh(1.0, r2), wk(40.0, W1));
    assert!(reset_events(&tl).is_empty(), "first sighting is only a candidate");
    tl.observe(t1 + 5 * MIN, fh(2.0, r2), wk(40.0, W1));
    let ev: Vec<_> = tl.events().into_iter().filter(|e| e.event_type == QuotaEventType::NaturalRolloverObserved).collect();
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].confidence, EvidenceConfidence::High);
    assert_eq!(ev[0].observed_at_ms, t1, "observed time, not a guessed exact rollover time");
    assert_eq!(ev[0].evidence["crossedPreviousResetTime"], true);
    assert_eq!(tl.closes(WindowType::FiveHour), 1);
    assert_eq!(tl.closes(WindowType::Weekly), 0);
}

#[test]
fn f_expired_but_blocked_stays_blocked() {
    let mut tl = Timeline::new();
    tl.observe(T0, fh(100.0, R1), wk(60.0, W1));
    tl.allowed = Some(false);
    tl.observe(R1 + 10 * MIN, fh(100.0, R1), wk(60.0, W1));
    assert_eq!(tl.track(WindowType::FiveHour).observation_state, WindowObservationState::ExpiredUnconfirmed);
    assert!(tl.types().is_empty());
    // Availability is never derived by the engine; it is the backend flag.
    assert_eq!(UsageAvailability::from_flag(Some(false)), UsageAvailability::Blocked);
    let ctx = tl.state.context.as_ref().unwrap();
    assert_eq!(ctx.last_availability, quotadock_core::analytics::transitions::Availability::Blocked);
}

#[test]
fn g_reset_credit_redemption_is_high_confidence_and_single() {
    let mut tl = Timeline::new();
    tl.observe(T0, fh(80.0, R1), wk(50.0, W1));
    tl.credits = Some(1);
    let t1 = T0 + 5 * MIN;
    tl.observe(t1, fh(0.0, t1 + 4 * HOUR), wk(0.0, t1 + 6 * 24 * HOUR));
    tl.observe(t1 + 5 * MIN, fh(0.0, t1 + 4 * HOUR), wk(0.0, t1 + 6 * 24 * HOUR));
    let ev = tl.events();
    let credit: Vec<_> = ev.iter().filter(|e| e.event_type == QuotaEventType::ResetCreditRedeemed).collect();
    assert_eq!(credit.len(), 1, "one redemption event for both windows");
    assert_eq!(credit[0].confidence, EvidenceConfidence::High);
    assert_eq!(credit[0].window_type, None);
    assert!(credit[0].evidence["windows"]["five_hour"].is_object());
    assert!(credit[0].evidence["windows"]["weekly"].is_object());
    assert_eq!(credit[0].evidence["creditBefore"], 2);
    assert_eq!(credit[0].evidence["creditAfter"], 1);
    assert!(!tl.types().contains(&QuotaEventType::UnscheduledQuotaRefresh));
    assert_eq!(tl.closes(WindowType::FiveHour), 1);
    assert_eq!(tl.closes(WindowType::Weekly), 1);
}

#[test]
fn h_delayed_credit_update_correlates_into_one_event() {
    let mut tl = Timeline::new();
    tl.observe(T0, fh(80.0, R1), wk(50.0, W1));
    let t1 = T0 + 5 * MIN;
    tl.observe(t1, fh(0.0, t1 + 4 * HOUR), wk(0.0, t1 + 6 * 24 * HOUR)); // refresh, credits still 2
    assert!(reset_events(&tl).is_empty());
    tl.credits = Some(1);
    tl.observe(t1 + 5 * MIN, fh(0.0, t1 + 4 * HOUR), wk(0.0, t1 + 6 * 24 * HOUR)); // credit drop arrives
    tl.observe(t1 + 10 * MIN, fh(0.0, t1 + 4 * HOUR), wk(0.0, t1 + 6 * 24 * HOUR));
    tl.observe(t1 + 15 * MIN, fh(1.0, t1 + 4 * HOUR), wk(0.0, t1 + 6 * 24 * HOUR));
    assert_eq!(reset_events(&tl), vec![QuotaEventType::ResetCreditRedeemed], "exactly one reset event");
    let e = tl.events().into_iter().find(|e| e.event_type == QuotaEventType::ResetCreditRedeemed).unwrap();
    assert_eq!(e.observed_at_ms, t1, "first observation of the refresh");
    assert!(!tl.types().contains(&QuotaEventType::ResetCreditCountChanged));
}

#[test]
fn i_unscheduled_refresh_does_not_guess_a_cause() {
    let mut tl = Timeline::new();
    let w2 = W1 + 2 * 24 * HOUR;
    tl.observe(T0, fh(10.0, R1), wk(70.0, W1));
    for i in 1..=3 {
        tl.observe(T0 + i * 5 * MIN, fh(10.0, R1), wk(1.0, w2));
    }
    let ev: Vec<_> = tl.events().into_iter().filter(|e| e.event_type == QuotaEventType::UnscheduledQuotaRefresh).collect();
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].window_type, Some(WindowType::Weekly));
    assert_eq!(ev[0].confidence, EvidenceConfidence::Medium);
    assert_eq!(ev[0].evidence["cause"], "unknown");
    assert_eq!(ev[0].evidence["creditsDelta"], 0);
    assert_eq!(ev[0].event_type.as_str(), "unscheduled_quota_refresh");
}

#[test]
fn j_refresh_with_unchanged_reset_time() {
    let mut tl = Timeline::new();
    tl.observe(T0, fh(10.0, R1), wk(70.0, W1));
    for i in 1..=3 {
        tl.observe(T0 + i * 5 * MIN, fh(10.0, R1), wk(0.0, W1));
    }
    let ev: Vec<_> = tl.events().into_iter().filter(|e| e.event_type == QuotaEventType::UnscheduledQuotaRefresh).collect();
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].evidence["resetAtDeltaSeconds"], 0);
    assert_eq!(ev[0].evidence["cause"], "unknown");
}

#[test]
fn k_full_state_a_b_a_creates_no_cycle_boundary() {
    let mut tl = Timeline::new();
    tl.observe(T0, fh(34.0, R1), wk(5.0, W1));
    tl.observe(T0 + 5 * MIN, fh(25.0, R1 + 30 * MIN), wk(5.0, W1));
    tl.observe(T0 + 10 * MIN, fh(34.0, R1), wk(5.0, W1));
    assert_eq!(tl.types(), vec![QuotaEventType::TransientBackendState]);
    assert_eq!(tl.closes(WindowType::FiveHour), 0);
    assert_eq!(tl.final_quality(2, WindowType::FiveHour), Some(SnapshotQuality::SuspectTransient));
}

#[test]
fn l_window_null_in_between_is_temporarily_missing() {
    let mut tl = Timeline::new();
    tl.observe(T0, fh(10.0, R1), wk(40.0, W1));
    tl.observe(T0 + 5 * MIN, fh(10.0, R1), WindowObs::missing());
    assert_eq!(tl.track(WindowType::Weekly).observation_state, WindowObservationState::TemporarilyMissing);
    tl.observe(T0 + 10 * MIN, fh(10.0, R1), wk(40.0, W1));
    assert!(tl.types().is_empty(), "no reset, no topology change for a one-poll gap");
    assert_eq!(tl.track(WindowType::Weekly).observation_state, WindowObservationState::Active);
}

#[test]
fn l2_window_missing_for_long_is_a_topology_change_not_exhaustion() {
    let mut tl = Timeline::new();
    tl.observe(T0, fh(10.0, R1), wk(40.0, W1));
    for i in 1..=3 {
        tl.observe(T0 + i * 5 * MIN, fh(10.0, R1), WindowObs::missing());
    }
    assert_eq!(tl.types(), vec![QuotaEventType::BucketTopologyChanged]);
    assert!(reset_events(&tl).is_empty());
}

#[test]
fn m_account_change_closes_the_comparison_context() {
    let mut tl = Timeline::new();
    tl.observe(T0, fh(80.0, R1), wk(80.0, W1));
    tl.account = "b".repeat(32);
    tl.observe(T0 + 5 * MIN, fh(2.0, R1 + HOUR), wk(5.0, W1 + HOUR));
    assert_eq!(tl.types(), vec![QuotaEventType::AccountChanged]);
    assert!(reset_events(&tl).is_empty(), "80% -> 2% across accounts is not a reset");
    assert_eq!(tl.closes(WindowType::FiveHour), 1);
    assert_eq!(tl.closes(WindowType::Weekly), 1);
    let ctx = tl.state.context.as_ref().unwrap();
    assert_eq!(ctx.account_fingerprint, "b".repeat(32));
    // Evidence never carries full fingerprints of either account.
    assert_eq!(tl.events()[0].evidence["previousAccount"], "aaaaaaaa");
}

#[test]
fn n_candidate_survives_a_restart_via_serialized_state() {
    let mut tl = Timeline::new();
    tl.observe(T0, fh(12.0, R1), wk(2.0, W1));
    tl.observe(T0 + 5 * MIN, fh(12.0, R1 + 20 * MIN), wk(2.0, W1));
    // "Shutdown": keep only the persisted JSON, build a brand-new engine.
    let persisted = serde_json::to_string(&tl.state).unwrap();
    let mut restarted = Timeline::new();
    restarted.state = serde_json::from_str(&persisted).unwrap();
    restarted.next_id = tl.next_id;
    restarted.observe(T0 + 10 * MIN, fh(12.0, R1 + 20 * MIN), wk(2.0, W1));
    assert_eq!(restarted.types(), vec![QuotaEventType::WindowScheduleChanged]);
}

#[test]
fn case1_normal_consumption_same_cycle() {
    let mut tl = Timeline::new();
    for (i, u) in [10.0, 11.0, 13.0, 16.0].into_iter().enumerate() {
        tl.observe(T0 + i as i64 * 20 * MIN, fh(u, R1), wk(2.0, W1));
    }
    assert!(tl.types().is_empty());
    assert_eq!(tl.closes(WindowType::FiveHour), 0);
}

#[test]
fn case11_new_bucket_is_topology_only() {
    let mut tl = Timeline::new();
    tl.observe(T0, fh(10.0, R1), wk(2.0, W1));
    tl.buckets = vec!["another_limit".into(), "codex".into()];
    tl.observe(T0 + 5 * MIN, fh(11.0, R1), wk(2.0, W1));
    tl.observe(T0 + 10 * MIN, fh(12.0, R1), wk(2.0, W1));
    tl.observe(T0 + 15 * MIN, fh(12.0, R1), wk(2.0, W1));
    assert_eq!(tl.types(), vec![QuotaEventType::BucketTopologyChanged]);
    assert_eq!(tl.closes(WindowType::FiveHour), 0, "codex history unaffected");
}

#[test]
fn small_drop_is_noise_and_moderate_drop_is_unknown_not_reset() {
    let mut tl = Timeline::new();
    tl.observe(T0, fh(13.0, R1), wk(2.0, W1));
    tl.observe(T0 + 5 * MIN, fh(12.0, R1), wk(2.0, W1)); // -1 pp: rounding noise
    assert!(tl.types().is_empty());
    tl.observe(T0 + 10 * MIN, fh(6.0, R1), wk(2.0, W1)); // -6 pp: candidate
    tl.observe(T0 + 15 * MIN, fh(6.0, R1), wk(2.0, W1));
    assert_eq!(tl.types(), vec![QuotaEventType::UnknownQuotaTransition]);
    assert_eq!(tl.events()[0].confidence, EvidenceConfidence::Low);
    assert!(reset_events(&tl).is_empty());
    assert_eq!(
        tl.track(WindowType::FiveHour).analysis_segment_start_snapshot_id,
        Some(3),
        "burn rate restarts after a confirmed correction"
    );
}

#[test]
fn one_poll_usage_spike_is_flagged_transient() {
    let mut tl = Timeline::new();
    tl.observe(T0, fh(12.0, R1), wk(2.0, W1));
    tl.observe(T0 + 5 * MIN, fh(20.0, R1), wk(2.0, W1));
    tl.observe(T0 + 10 * MIN, fh(12.0, R1), wk(2.0, W1));
    assert_eq!(tl.types(), vec![QuotaEventType::TransientBackendState]);
    assert_eq!(tl.final_quality(2, WindowType::FiveHour), Some(SnapshotQuality::SuspectTransient));
}

#[test]
fn confirmed_schedule_change_that_reverts_is_reconciled() {
    let mut tl = Timeline::new();
    tl.observe(T0, fh(12.0, R1), wk(2.0, W1));
    tl.observe(T0 + 5 * MIN, fh(12.0, R1 + HOUR), wk(2.0, W1));
    tl.observe(T0 + 10 * MIN, fh(12.0, R1 + HOUR), wk(2.0, W1));
    tl.observe(T0 + 15 * MIN, fh(12.0, R1), wk(2.0, W1));
    let types = tl.types();
    assert_eq!(types, vec![QuotaEventType::WindowScheduleChanged, QuotaEventType::BackendSnapshotReconciliation]);
    let rec = tl.events().into_iter().find(|e| e.event_type == QuotaEventType::BackendSnapshotReconciliation).unwrap();
    assert!(rec.supersedes_event_key.as_deref().unwrap().contains("window_schedule_changed"));
    assert_eq!(tl.track(WindowType::FiveHour).confirmed_reset_at_ms, Some(R1));
}

#[test]
fn confirmed_rollover_that_reverts_restores_the_cycle() {
    let mut tl = Timeline::new();
    tl.observe(T0, fh(90.0, R1), wk(2.0, W1));
    let t1 = R1 + MIN;
    tl.observe(t1, fh(1.0, t1 + 4 * HOUR), wk(2.0, W1));
    tl.observe(t1 + 5 * MIN, fh(1.0, t1 + 4 * HOUR), wk(2.0, W1));
    let first_cycle = tl.track(WindowType::FiveHour).current_cycle_key.clone();
    tl.observe(t1 + 10 * MIN, fh(90.0, R1), wk(2.0, W1));
    assert!(tl.types().contains(&QuotaEventType::BackendSnapshotReconciliation));
    let ops = tl.cycle_ops();
    assert!(ops.iter().any(|o| matches!(o, CycleOp::Void { cycle_key, .. } if Some(cycle_key) == first_cycle.as_ref())));
    assert!(ops.iter().any(|o| matches!(o, CycleOp::Reopen { .. })));
}

#[test]
fn rollover_into_an_idle_sliding_window() {
    let mut tl = Timeline::new();
    tl.observe(T0, fh(96.0, R1), wk(10.0, W1));
    tl.observe(R1 + MIN, fh(96.0, R1), wk(10.0, W1)); // server still shows the old window
    assert_eq!(tl.track(WindowType::FiveHour).observation_state, WindowObservationState::ExpiredUnconfirmed);
    for i in 0..4 {
        let at = R1 + 6 * MIN + i * 5 * MIN;
        tl.observe(at, fh(0.0, at + 5 * HOUR), wk(10.0, W1));
    }
    let rollover = tl.events().into_iter().find(|e| e.event_type == QuotaEventType::NaturalRolloverObserved).unwrap();
    assert_eq!(rollover.confidence, EvidenceConfidence::Medium, "new anchor is sliding, so not HIGH");
    assert!(tl.types().contains(&QuotaEventType::SlidingAnchorDetected));
    assert!(!tl.types().contains(&QuotaEventType::WindowScheduleChanged));
    let t = tl.track(WindowType::FiveHour);
    assert_eq!(t.observation_state, WindowObservationState::IdleOrUninitialized);
    assert_eq!(t.confirmed_reset_at_ms, None);
}

#[test]
fn uncorrelated_credit_drop_is_diagnostic_only() {
    let mut tl = Timeline::new();
    tl.observe(T0, fh(10.0, R1), wk(2.0, W1));
    tl.credits = Some(1);
    for i in 1..=4 {
        tl.observe(T0 + i * 5 * MIN, fh(10.0, R1), wk(2.0, W1));
    }
    assert_eq!(tl.types(), vec![QuotaEventType::ResetCreditCountChanged]);
    assert!(tl.user_visible().is_empty());
}

#[test]
fn unverified_identity_is_never_compared() {
    let mut tl = Timeline::new();
    tl.observe(T0, fh(80.0, R1), wk(2.0, W1));
    let obs = Observation {
        snapshot_id: 99,
        captured_at_ms: T0 + 5 * MIN,
        account_fingerprint: None,
        account_mismatch: false,
        ordinary_usage_allowed: Some(true),
        reset_credit_count: Some(2),
        bucket_ids: vec!["codex".into()],
        five_hour: fh(0.0, R1 + HOUR),
        weekly: wk(0.0, W1),
    };
    let (next, out) = tl.engine.step(&tl.state, &obs);
    assert!(out.events.is_empty());
    assert_eq!(next.context, tl.state.context, "context untouched");
}

#[test]
fn replay_is_deterministic() {
    let run = || {
        let mut tl = Timeline::new();
        tl.observe(T0, fh(98.0, R1), wk(40.0, W1));
        tl.observe(T0 + 5 * MIN, fh(98.0, R1 + 10 * MIN), wk(40.0, W1));
        tl.observe(T0 + 10 * MIN, fh(98.0, R1), wk(40.0, W1));
        tl.observe(R1 + MIN, fh(1.0, R1 + 5 * HOUR), wk(41.0, W1));
        tl.observe(R1 + 6 * MIN, fh(2.0, R1 + 5 * HOUR), wk(41.0, W1));
        (tl.outputs.clone(), tl.state.clone())
    };
    assert_eq!(run(), run());
}

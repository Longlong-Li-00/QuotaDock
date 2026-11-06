//! reset-detector-v2: an idle -> active transition restarts the burn-rate analysis segment
//! (same ObservedCycle, no event), on the real 2026-10-08 evidence and on synthetic variants.

mod common;
use common::*;
use quotadock_core::analytics::burn_rate::BurnPoint;
use quotadock_core::analytics::config::AnalyticsConfig;
use quotadock_core::analytics::forecast::{forecast, forecast_from};
use quotadock_core::analytics::*;
use serde_json::Value;

const W1: i64 = T0 + 5 * 24 * HOUR;

fn fixture() -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/real-5h-idle-to-active-2026-10-08.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("fixture present")).expect("fixture is JSON")
}

fn window(s: &Value, key: &str) -> WindowObs {
    let w = &s[key];
    WindowObs::new(w["usedPercent"].as_f64().unwrap(), w["resetAtMs"].as_i64().unwrap(), w["durationMins"].as_i64().unwrap())
}

fn opens(tl: &Timeline, t: WindowType) -> usize {
    tl.cycle_ops().iter().filter(|op| matches!(op, CycleOp::Open { window_type, .. } if *window_type == t)).count()
}

fn assert_no_false_transition(tl: &Timeline) {
    let types = tl.types();
    for bad in [
        QuotaEventType::NaturalRolloverObserved,
        QuotaEventType::UnscheduledQuotaRefresh,
        QuotaEventType::ResetCreditRedeemed,
        QuotaEventType::WindowScheduleChanged,
    ] {
        assert!(!types.contains(&bad), "unexpected {bad:?} in {types:?}");
    }
    assert!(tl.user_visible().is_empty(), "nothing user-visible: {:?}", tl.user_visible());
    assert_eq!(tl.closes(WindowType::FiveHour), 0, "no cycle boundary");
}

#[test]
fn idle_to_active_reanchors_analysis_segment() {
    let fx = fixture();
    let snaps = fx["snapshots"].as_array().unwrap();
    let mut tl = Timeline::new();
    let first_id = snaps[0]["id"].as_i64().unwrap();
    tl.next_id = first_id;
    let mut resumed: Option<(i64, i64)> = None; // (first snapshot with 5h usage, its resetsAt)
    for s in snaps {
        assert_eq!(s["id"].as_i64().unwrap(), tl.next_id, "fixture snapshot ids are consecutive");
        tl.allowed = s["ordinaryUsageAllowed"].as_bool();
        if resumed.is_none() && s["fiveHour"]["usedPercent"].as_f64().unwrap() > 0.0 {
            resumed = Some((s["id"].as_i64().unwrap(), s["fiveHour"]["resetAtMs"].as_i64().unwrap()));
        }
        tl.observe(s["capturedAtMs"].as_i64().unwrap(), window(s, "fiveHour"), window(s, "weekly"));
    }
    let (active_id, reset) = resumed.expect("the fixture contains the usage resumption");
    assert_eq!(fx["_meta"]["observed_segment_start_snapshot_id"], first_id, "reset-detector-v1 kept the idle start");

    let t = tl.track(WindowType::FiveHour);
    assert_eq!(t.observation_state, WindowObservationState::Active);
    assert_eq!(t.analysis_segment_start_snapshot_id, Some(active_id), "segment restarts at the first active observation");
    assert_eq!(t.active_origin_ms, Some(reset - 300 * MIN), "derived origin = resetsAt - 5 h");
    assert_eq!(t.confirmed_reset_at_ms, Some(reset));
    assert_eq!(opens(&tl, WindowType::FiveHour), 1, "same ObservedCycle: no new cycle for the segment");
    assert_no_false_transition(&tl);
    let log = tl.outputs.iter().flat_map(|o| o.log_lines.iter()).cloned().collect::<Vec<_>>().join("\n");
    assert!(log.contains("analysis segment re-anchored window=five_hour") && log.contains("derived"), "{log}");

    // The idle period is not in the active burn denominator.
    let points = |from: i64| -> Vec<BurnPoint> {
        let pts: Vec<BurnPoint> = snaps
            .iter()
            .filter(|s| s["id"].as_i64().unwrap() >= from)
            .filter(|s| {
                let q = tl.final_quality(s["id"].as_i64().unwrap(), WindowType::FiveHour);
                q.is_none() || q == Some(SnapshotQuality::Normal)
            })
            .map(|s| BurnPoint {
                captured_at_ms: s["capturedAtMs"].as_i64().unwrap(),
                used_percent: s["fiveHour"]["usedPercent"].as_f64().unwrap(),
            })
            .collect();
        pts
    };
    let cfg = AnalyticsConfig::default();
    let v2_points = points(active_id);
    assert!(v2_points.iter().all(|p| p.captured_at_ms > t.active_origin_ms.unwrap() && p.used_percent > 0.0), "no idle 0 % snapshot");
    let v2 = forecast(WindowType::FiveHour, &v2_points, Some(&t), &cfg);
    let v1_style = forecast_from(WindowType::FiveHour, &points(first_id), None, Some(&t), &cfg);
    let (r2, r1) = (v2.burn_rate_per_hour.unwrap(), v1_style.burn_rate_per_hour.unwrap());
    eprintln!("burn rate v1-style={r1:.3} pp/h, v2={r2:.3} pp/h");
    assert!(r1 < 3.0, "v1-style segment dilutes the rate with ~9 h of idle zeros: {r1}");
    // 27 % used between the derived start (11:10:08) and 14:05 is ~9.3 pp/h on average.
    let last = v2_points.last().unwrap();
    let average = last.used_percent / ((last.captured_at_ms - t.active_origin_ms.unwrap()) as f64 / 3_600_000.0);
    assert!(r2 > 3.0 * r1, "v2 is not diluted by the idle period: v1-style {r1}, v2 {r2}");
    assert!((r2 - average).abs() / average < 0.5, "v2 {r2} tracks the window average {average}");
    assert_eq!(v2.burn.points, v2_points.len() + 1, "origin + active observations");
}

#[test]
fn first_active_poll_with_a_new_reset_is_a_resumption_not_a_schedule_change() {
    let mut tl = Timeline::new();
    for i in 0..3 {
        let at = T0 + i * 5 * MIN;
        tl.observe(at, fh(0.0, at + 5 * HOUR), wk(2.0, W1));
    }
    assert!(tl.track(WindowType::FiveHour).sliding_anchor);
    // Usage started at T0 + 12 min; the next poll (T0 + 15 min) already shows it.
    let started = T0 + 12 * MIN;
    tl.observe(T0 + 15 * MIN, fh(4.0, started + 5 * HOUR), wk(2.0, W1));
    tl.observe(T0 + 20 * MIN, fh(6.0, started + 5 * HOUR), wk(2.0, W1));
    let t = tl.track(WindowType::FiveHour);
    assert_eq!(t.analysis_segment_start_snapshot_id, Some(4));
    assert_eq!(t.active_origin_ms, Some(started));
    assert_eq!(t.confirmed_reset_at_ms, Some(started + 5 * HOUR));
    assert!(!t.sliding_anchor);
    assert_no_false_transition(&tl);
    assert_eq!(tl.types(), vec![QuotaEventType::SlidingAnchorDetected], "only the earlier idle diagnostic");
    assert_eq!(tl.final_quality(4, WindowType::FiveHour), None, "accepted directly, never pending");
}

#[test]
fn frozen_zero_percent_anchor_then_usage_reanchors_at_the_usage() {
    let mut tl = Timeline::new();
    for i in 0..3 {
        let at = T0 + i * 5 * MIN;
        tl.observe(at, fh(0.0, at + 5 * HOUR), wk(2.0, W1));
    }
    let started = T0 + 11 * MIN;
    // 0 % (rounded) with the frozen anchor, then usage: the segment starts at the usage.
    tl.observe(T0 + 15 * MIN, fh(0.0, started + 5 * HOUR), wk(2.0, W1));
    tl.observe(T0 + 20 * MIN, fh(0.0, started + 5 * HOUR), wk(2.0, W1));
    tl.observe(T0 + 25 * MIN, fh(3.0, started + 5 * HOUR), wk(2.0, W1));
    let t = tl.track(WindowType::FiveHour);
    assert_eq!(t.analysis_segment_start_snapshot_id, Some(6));
    assert_eq!(t.active_origin_ms, Some(started));
    assert_eq!(opens(&tl, WindowType::FiveHour), 1);
    assert!(tl.user_visible().is_empty());
    assert_eq!(tl.closes(WindowType::FiveHour), 0);
}

#[test]
fn a_window_that_was_never_idle_is_not_reanchored() {
    let mut tl = Timeline::new();
    let r = T0 + 3 * HOUR;
    for (i, u) in [12.0, 13.0, 15.0, 18.0].into_iter().enumerate() {
        tl.observe(T0 + i as i64 * 5 * MIN, fh(u, r), wk(2.0, W1));
    }
    let t = tl.track(WindowType::FiveHour);
    assert_eq!(t.analysis_segment_start_snapshot_id, Some(1));
    assert_eq!(t.active_origin_ms, None);
    assert!(tl.types().is_empty());
}

#[test]
fn a_rollover_after_reanchoring_clears_the_derived_origin() {
    let mut tl = Timeline::new();
    for i in 0..3 {
        let at = T0 + i * 5 * MIN;
        tl.observe(at, fh(0.0, at + 5 * HOUR), wk(2.0, W1));
    }
    let started = T0 + 12 * MIN;
    let reset = started + 5 * HOUR;
    tl.observe(T0 + 15 * MIN, fh(4.0, reset), wk(2.0, W1));
    tl.observe(T0 + 20 * MIN, fh(30.0, reset), wk(3.0, W1));
    assert_eq!(tl.track(WindowType::FiveHour).active_origin_ms, Some(started));
    // The window expires and a new one is observed in use: a natural rollover.
    let next_reset = reset + 5 * HOUR;
    tl.observe(reset + 5 * MIN, fh(2.0, next_reset), wk(3.0, W1));
    tl.observe(reset + 10 * MIN, fh(3.0, next_reset), wk(3.0, W1));
    let t = tl.track(WindowType::FiveHour);
    assert!(tl.types().contains(&QuotaEventType::NaturalRolloverObserved), "{:?}", tl.types());
    assert_eq!(t.active_origin_ms, None, "a new cycle starts its own segment");
    assert_eq!(t.analysis_segment_start_snapshot_id, Some(6));
}

#[test]
fn version_free_dedup_scopes() {
    use quotadock_core::product::notifications::version_free_scope;
    assert_eq!(version_free_scope("reset-detector-v1|abc|weekly|cycle|13"), "abc|weekly|cycle|13");
    assert_eq!(version_free_scope("reset-detector-v2|abc|weekly|cycle|13"), "abc|weekly|cycle|13");
    assert_eq!(version_free_scope("blocked:42"), "blocked:42");
    assert_eq!(version_free_scope("failures:7"), "failures:7");
}

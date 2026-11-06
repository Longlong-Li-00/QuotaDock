//! Burn rate and forecast (all thresholds are QuotaDock heuristics).

use quotadock_core::analytics::burn_rate::{BurnPoint, BurnStatus, compute_burn, ols_slope_per_hour};
use quotadock_core::analytics::forecast::{ForecastStatus, ResetSchedule, Risk, forecast};
use quotadock_core::analytics::*;

const T0: i64 = 1_791_374_400_000;
const MIN: i64 = 60_000;
const HOUR: i64 = 60 * MIN;

fn pts(spec: &[(i64, f64)]) -> Vec<BurnPoint> {
    spec.iter().map(|(m, u)| BurnPoint { captured_at_ms: T0 + m * MIN, used_percent: *u }).collect()
}

fn track(confirmed_reset: Option<i64>, polls: u32, sliding: bool) -> WindowTrackView {
    WindowTrackView {
        account_fingerprint: "a".repeat(32),
        window_type: WindowType::FiveHour,
        observation_state: WindowObservationState::Active,
        candidate_state: ResetCandidateState::None,
        confirmed_reset_at_ms: confirmed_reset,
        anchor_polls: polls,
        sliding_anchor: sliding,
        missing_streak: 0,
        current_cycle_key: None,
        analysis_segment_start_snapshot_id: Some(1),
        active_origin_ms: None,
        last_snapshot_id: 1,
    }
}

#[test]
fn ols_recovers_a_known_line_with_uneven_spacing() {
    let p = pts(&[(0, 10.0), (7, 10.7), (31, 13.1), (60, 16.0)]);
    let s = ols_slope_per_hour(&p).unwrap();
    assert!((s - 6.0).abs() < 1e-9, "{s}");
    assert_eq!(ols_slope_per_hour(&p[..1]), None);
}

#[test]
fn first_snapshot_never_produces_a_rate() {
    // 12% at install time says nothing about the rate.
    let cfg = AnalyticsConfig::default();
    let b = compute_burn(&pts(&[(0, 12.0)]), &cfg);
    assert_eq!(b.status, BurnStatus::InsufficientData);
    let b = compute_burn(&pts(&[(0, 12.0), (10, 14.0), (20, 16.0)]), &cfg);
    assert_eq!(b.status, BurnStatus::InsufficientData, "20 min < 30 min minimum span");
    let f = forecast(WindowType::FiveHour, &pts(&[(0, 12.0)]), Some(&track(Some(T0 + 3 * HOUR), 3, false)), &cfg);
    assert_eq!(f.status, ForecastStatus::InsufficientData);
    assert_eq!(f.risk, Risk::InsufficientData);
    assert_eq!(f.estimated_exhaustion_at_ms, None);
}

#[test]
fn stable_usage_gives_zero_burn_and_no_projection() {
    let cfg = AnalyticsConfig::default();
    let p = pts(&[(0, 12.0), (15, 12.0), (30, 12.0), (45, 12.0)]);
    let f = forecast(WindowType::FiveHour, &p, Some(&track(Some(T0 + 4 * HOUR), 4, false)), &cfg);
    assert_eq!(f.burn.status, BurnStatus::Stable);
    assert_eq!(f.burn_rate_per_hour, Some(0.0));
    assert_eq!(f.status, ForecastStatus::NotProjected);
    assert_eq!(f.estimated_exhaustion_at_ms, None, "no infinite date");
    assert_eq!(f.risk, Risk::Low);
}

#[test]
fn consuming_with_confirmed_reset_gives_margin_pressure_and_risk() {
    let cfg = AnalyticsConfig::default();
    // 10 pp/hour from 10% -> remaining 85% at 30 min; exhaustion in 8.5 h.
    let p = pts(&[(0, 10.0), (10, 11.6667), (20, 13.3333), (30, 15.0)]);
    let reset = T0 + 30 * MIN + 4 * HOUR;
    let f = forecast(WindowType::FiveHour, &p, Some(&track(Some(reset), 3, false)), &cfg);
    assert_eq!(f.status, ForecastStatus::Ok);
    assert!((f.burn_rate_per_hour.unwrap() - 10.0).abs() < 0.01);
    let expected_exhaustion = T0 + 30 * MIN + (8.5 * HOUR as f64) as i64;
    assert!((f.estimated_exhaustion_at_ms.unwrap() - expected_exhaustion).abs() < 60_000);
    assert!((f.margin_hours.unwrap() - (8.5 - 4.0)).abs() < 0.02, "exhaustion 4.5 h after the reset = +4.5 h slack");
    // pressure = 10 pp/h * 4 h / 85 pp ≈ 0.47 -> Low
    assert!((f.pressure.unwrap() - 40.0 / 85.0).abs() < 0.01);
    assert_eq!(f.risk, Risk::Low);
    assert_eq!(f.reset_schedule, ResetSchedule::Confirmed);
    assert!(f.heuristic.contains("not an OpenAI metric"));

    let reset_far = T0 + 30 * MIN + 8 * HOUR; // pressure 80/85 ≈ 0.94 -> Watch
    assert_eq!(forecast(WindowType::FiveHour, &p, Some(&track(Some(reset_far), 3, false)), &cfg).risk, Risk::Watch);
    let reset_farther = T0 + 30 * MIN + 10 * HOUR; // 100/85 > 1 -> AtRisk
    assert_eq!(forecast(WindowType::FiveHour, &p, Some(&track(Some(reset_farther), 3, false)), &cfg).risk, Risk::AtRisk);
}

#[test]
fn sliding_or_unconfirmed_reset_is_never_used_for_margin() {
    let cfg = AnalyticsConfig::default();
    let p = pts(&[(0, 10.0), (10, 11.6667), (20, 13.3333), (30, 15.0)]);
    let sliding = forecast(WindowType::FiveHour, &p, Some(&track(None, 0, true)), &cfg);
    assert_eq!(sliding.reset_schedule, ResetSchedule::Sliding);
    assert_eq!(sliding.status, ForecastStatus::ResetScheduleUnconfirmed);
    assert_eq!(sliding.margin_hours, None);
    assert_eq!(sliding.pressure, None);
    assert!(sliding.estimated_exhaustion_at_ms.is_some(), "exhaustion itself is still projected");
    let unconfirmed = forecast(WindowType::FiveHour, &p, Some(&track(None, 1, false)), &cfg);
    assert_eq!(unconfirmed.reset_schedule, ResetSchedule::Unconfirmed);
    assert_eq!(unconfirmed.status, ForecastStatus::ResetScheduleUnconfirmed);
    let passed = forecast(WindowType::FiveHour, &p, Some(&track(Some(T0), 5, false)), &cfg);
    assert_eq!(passed.reset_schedule, ResetSchedule::Passed);
}

#[test]
fn in_cycle_negative_movement_is_a_correction_not_negative_burn() {
    let cfg = AnalyticsConfig::default();
    let p = pts(&[(0, 13.0), (15, 12.0), (30, 12.0), (45, 12.0)]);
    let b = compute_burn(&p, &cfg);
    assert_eq!(b.current_cycle_rate, Some(0.0));
}

/// Real run 2026-10-11: right after a Codex weekly refresh the window stayed at 0 % for about
/// 2.5 h, then one work session used 6 % in about 15 min. Projected over the week this read "may
/// run out early" and was notified. With less than a day of observation the weekly forecast now
/// reports insufficient data; the same pace over more than a day is still projected.
#[test]
fn a_weekly_forecast_needs_a_day_of_observation() {
    let cfg = AnalyticsConfig::default();
    let mut t = track(Some(T0 + 7 * 24 * HOUR), 5, false);
    t.window_type = WindowType::Weekly;
    t.active_origin_ms = Some(T0); // derived: resetsAt - 7 d
    let burst = pts(&[(164, 3.0), (169, 4.0), (174, 5.0), (179, 6.0)]);
    let f = forecast(WindowType::Weekly, &burst, Some(&t), &cfg);
    assert_eq!((f.status, f.risk), (ForecastStatus::InsufficientData, Risk::InsufficientData));
    assert_eq!(f.burn_rate_per_day, None, "no rate shown either");
    assert_eq!(f.required_span_ms, 24 * HOUR);
    let day = pts(&[(164, 3.0), (179, 6.0), (12 * 60, 20.0), (25 * 60, 40.0)]);
    let g = forecast(WindowType::Weekly, &day, Some(&t), &cfg);
    assert_ne!(g.risk, Risk::InsufficientData);
    assert!(g.burn_rate_per_day.is_some());
    // Five-hour windows keep the 30-minute minimum.
    assert_eq!(forecast(WindowType::FiveHour, &burst, Some(&track(Some(T0 + 5 * HOUR), 3, false)), &cfg).required_span_ms, 30 * MIN);
}

#[test]
fn weekly_reports_per_day_and_prefers_recent_24h() {
    let cfg = AnalyticsConfig::default();
    // 2 days of data: slow first day, faster last day.
    let mut spec = Vec::new();
    for h in 0..=24 {
        spec.push((h * 60, 10.0 + h as f64 * 0.1));
    }
    for h in 25..=48 {
        spec.push((h * 60, 12.4 + (h - 24) as f64 * 0.5));
    }
    let p = pts(&spec);
    let mut t = track(Some(T0 + 72 * HOUR), 5, false);
    t.window_type = WindowType::Weekly;
    let f = forecast(WindowType::Weekly, &p, Some(&t), &cfg);
    assert_eq!(f.rate_source, Some("recent_24h"));
    let per_hour = f.burn_rate_per_hour.unwrap();
    assert!(per_hour > 0.4, "recent pace dominates: {per_hour}");
    assert!((f.burn_rate_per_day.unwrap() - per_hour * 24.0).abs() < 1e-9);
}

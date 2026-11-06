//! Exhaustion forecast and quota pressure. [QUOTADOCK HEURISTIC] throughout: OpenAI publishes no
//! such metric. Forecasts never use an unconfirmed or sliding resetsAt.

use super::burn_rate::{BurnMetrics, BurnPoint, BurnStatus, compute_burn_from};
use super::config::AnalyticsConfig;
use super::model::{EvidenceConfidence, WindowTrackView, WindowType};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ForecastStatus {
    Ok,
    InsufficientData,
    /// Enough data but no meaningful consumption: burn rate reported, no exhaustion date.
    NotProjected,
    /// Exhaustion projected, but no confirmed future reset anchor to compare it with.
    ResetScheduleUnconfirmed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    InsufficientData,
    Low,
    Watch,
    AtRisk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResetSchedule {
    /// Same resetsAt across >= 2 polls, not sliding, still in the future.
    Confirmed,
    Unconfirmed,
    Sliding,
    /// The confirmed anchor is already in the past.
    Passed,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageForecast {
    pub window_type: WindowType,
    pub status: ForecastStatus,
    pub burn_rate_per_hour: Option<f64>,
    pub burn_rate_per_day: Option<f64>,
    pub rate_source: Option<&'static str>,
    pub current_used_percent: Option<f64>,
    pub remaining_percent: Option<f64>,
    pub estimated_exhaustion_at_ms: Option<i64>,
    pub server_reset_at_ms: Option<i64>,
    pub reset_schedule: ResetSchedule,
    /// Hours between projected exhaustion and the confirmed reset: positive = slack, negative = may run out before the reset.
    pub margin_hours: Option<f64>,
    pub pressure: Option<f64>,
    pub risk: Risk,
    pub confidence: EvidenceConfidence,
    pub burn: BurnMetrics,
    /// Observation span required before a rate is reported (shown with "not enough history").
    pub required_span_ms: i64,
    pub heuristic: &'static str,
}

pub const HEURISTIC_NOTE: &str = "QuotaDock heuristic, not an OpenAI metric: linear burn over the current observed cycle; pressure = expected consumption until the confirmed reset / remaining quota";

pub fn forecast(window_type: WindowType, points: &[BurnPoint], track: Option<&WindowTrackView>, cfg: &AnalyticsConfig) -> UsageForecast {
    forecast_from(window_type, points, track.and_then(|t| t.active_origin_ms), track, cfg)
}

/// `origin_ms`: the derived 0 % start of a segment re-anchored after idle (reset-detector-v2).
pub fn forecast_from(
    window_type: WindowType,
    points: &[BurnPoint],
    origin_ms: Option<i64>,
    track: Option<&WindowTrackView>,
    cfg: &AnalyticsConfig,
) -> UsageForecast {
    let burn = compute_burn_from(points, origin_ms, cfg);
    let latest_ms = burn.latest_captured_at_ms;
    let used = burn.latest_used_percent;
    let remaining = used.map(|u| (100.0 - u).clamp(0.0, 100.0));

    let (server_reset_at_ms, reset_schedule) = match track {
        Some(t) if t.sliding_anchor => (None, ResetSchedule::Sliding),
        Some(t) => match (t.confirmed_reset_at_ms, latest_ms) {
            (Some(r), Some(now)) if r > now => (Some(r), ResetSchedule::Confirmed),
            (Some(_), Some(_)) => (None, ResetSchedule::Passed),
            _ => (None, ResetSchedule::Unconfirmed),
        },
        None => (None, ResetSchedule::Unconfirmed),
    };

    // Five-hour: the whole (short) cycle. Weekly: the last 24 h when available (current pace).
    let (rate, rate_source) = match window_type {
        WindowType::FiveHour => (burn.current_cycle_rate, Some("current_cycle")),
        WindowType::Weekly => match burn.recent_24h_rate {
            Some(r) => (Some(r), Some("recent_24h")),
            None => (burn.current_cycle_rate, Some("current_cycle")),
        },
    };
    let anchor_polls = track.map(|t| t.anchor_polls).unwrap_or(0);
    let confidence = if burn.status == BurnStatus::InsufficientData {
        EvidenceConfidence::Low
    } else if burn.points >= 12 && burn.span_ms >= 2 * 3_600_000 && anchor_polls >= 3 {
        EvidenceConfidence::High
    } else if burn.points >= 6 && burn.span_ms >= 3_600_000 {
        EvidenceConfidence::Medium
    } else {
        EvidenceConfidence::Low
    };

    // Weekly: a projection over days needs at least a day of observation (both providers).
    let required_span_ms =
        if window_type == WindowType::Weekly { cfg.min_span_ms.max(cfg.weekly_min_span_ms) } else { cfg.min_span_ms };
    let enough_history = burn.status != BurnStatus::InsufficientData && burn.span_ms >= required_span_ms;
    let confidence = if enough_history { confidence } else { EvidenceConfidence::Low };
    let mut f = UsageForecast {
        window_type,
        status: ForecastStatus::InsufficientData,
        burn_rate_per_hour: None,
        burn_rate_per_day: None,
        rate_source: None,
        current_used_percent: used,
        remaining_percent: remaining,
        estimated_exhaustion_at_ms: None,
        server_reset_at_ms,
        reset_schedule,
        margin_hours: None,
        pressure: None,
        risk: Risk::InsufficientData,
        confidence,
        burn: burn.clone(),
        required_span_ms,
        heuristic: HEURISTIC_NOTE,
    };
    if !enough_history {
        return f;
    }
    let rate = rate.unwrap_or(0.0);
    f.burn_rate_per_hour = Some(rate);
    f.burn_rate_per_day = (window_type == WindowType::Weekly).then_some(rate * 24.0);
    f.rate_source = rate_source;
    let (Some(now), Some(remaining)) = (latest_ms, remaining) else { return f };

    let hours_to_reset = server_reset_at_ms.map(|r| (r - now) as f64 / 3_600_000.0);
    let pressure = hours_to_reset.map(|h| {
        let expected = rate * h;
        if remaining <= 0.0 { if expected > 0.0 { f64::INFINITY } else { 0.0 } } else { expected / remaining }
    });
    f.pressure = pressure.filter(|p| p.is_finite());
    f.risk = match pressure {
        None => Risk::InsufficientData,
        Some(p) if p > cfg.pressure_at_risk => Risk::AtRisk,
        Some(p) if p >= cfg.pressure_watch => Risk::Watch,
        Some(_) => Risk::Low,
    };

    if burn.status == BurnStatus::Stable || rate <= 0.0 {
        // Never an "infinite" date.
        f.status = ForecastStatus::NotProjected;
        return f;
    }
    let exhaustion = now + ((remaining / rate) * 3_600_000.0).round() as i64;
    f.estimated_exhaustion_at_ms = Some(exhaustion);
    match server_reset_at_ms {
        Some(r) => {
            // Positive = projected to last beyond the confirmed reset (slack); negative = runs out first.
            f.margin_hours = Some((exhaustion - r) as f64 / 3_600_000.0);
            f.status = ForecastStatus::Ok;
        }
        None => f.status = ForecastStatus::ResetScheduleUnconfirmed,
    }
    f
}

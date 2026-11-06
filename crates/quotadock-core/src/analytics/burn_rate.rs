//! Burn rate (percentage points per hour) over one observed cycle segment.
//!
//! Estimator: ordinary least squares slope of usedPercent over time.
//! - Outliers are handled upstream: only `Normal`-quality snapshots of the current cycle segment
//!   are passed in (transient, pending, missing and account-mismatch points are excluded, and no
//!   confirmed rollover/refresh/credit boundary is ever crossed).
//! - usedPercent is rounded to integers by the app-server; the rounding error is roughly symmetric,
//!   which OLS averages out, whereas median-of-pairwise-slopes (Theil–Sen) collapses towards 0 on
//!   step-shaped data because most short pairs have identical values.
//! - Uneven poll intervals are handled naturally (time is the regressor).
//! - Negative slopes inside a cycle are corrections, reported as 0.
//! - reset-detector-v2: a segment re-anchored after an idle period has a known origin (derived
//!   window start, 0 % used). Its cycle rate is the least-squares line through that origin; a free
//!   intercept would absorb the first burst of usage and understate the rate.

use super::config::AnalyticsConfig;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct BurnPoint {
    pub captured_at_ms: i64,
    pub used_percent: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BurnStatus {
    /// Fewer than `min_snapshots` points or less than `min_span_ms` of observation.
    InsufficientData,
    /// Enough data, but less than `min_consumption_pp` consumed: rate reported, no projection.
    Stable,
    Consuming,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BurnMetrics {
    pub status: BurnStatus,
    pub recent_6h_rate: Option<f64>,
    pub recent_24h_rate: Option<f64>,
    pub current_cycle_rate: Option<f64>,
    pub points: usize,
    pub span_ms: i64,
    pub observed_consumption_pp: f64,
    pub latest_used_percent: Option<f64>,
    pub latest_captured_at_ms: Option<i64>,
}

/// OLS slope in percentage points per hour; None with < 2 points or no time spread.
pub fn ols_slope_per_hour(points: &[BurnPoint]) -> Option<f64> {
    if points.len() < 2 {
        return None;
    }
    let t0 = points[0].captured_at_ms;
    let xs: Vec<f64> = points.iter().map(|p| (p.captured_at_ms - t0) as f64 / 3_600_000.0).collect();
    let n = points.len() as f64;
    let mean_x = xs.iter().sum::<f64>() / n;
    let mean_y = points.iter().map(|p| p.used_percent).sum::<f64>() / n;
    let mut sxx = 0.0;
    let mut sxy = 0.0;
    for (x, p) in xs.iter().zip(points) {
        sxx += (x - mean_x) * (x - mean_x);
        sxy += (x - mean_x) * (p.used_percent - mean_y);
    }
    if sxx <= f64::EPSILON {
        return None;
    }
    Some(sxy / sxx)
}

/// Least-squares slope (pp/h) of a line constrained through `(origin_ms, 0 %)`.
pub fn origin_slope_per_hour(origin_ms: i64, points: &[BurnPoint]) -> Option<f64> {
    let (mut sxx, mut sxy) = (0.0, 0.0);
    for p in points {
        let x = (p.captured_at_ms - origin_ms) as f64 / 3_600_000.0;
        if x > 0.0 {
            sxx += x * x;
            sxy += x * p.used_percent;
        }
    }
    (sxx > f64::EPSILON).then(|| sxy / sxx)
}

/// Diagnostics-only comparison for a re-anchored 5h segment (Windows V1 soak). Never used by the
/// dashboard, risk, tray, notifications or forecast.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShadowRates {
    /// Production: least squares through the derived origin (what the forecast shows).
    pub production: Option<f64>,
    /// Latest used % divided by the hours since the derived origin.
    pub average_since_origin: Option<f64>,
    /// OLS slope over the last 30 min (at least 3 points spanning 20 min), else None.
    pub recent_30m: Option<f64>,
    pub points: usize,
}

pub fn shadow_rates(origin_ms: i64, points: &[BurnPoint], production: Option<f64>) -> ShadowRates {
    let last = points.last();
    let average_since_origin = last
        .filter(|p| p.captured_at_ms > origin_ms)
        .map(|p| p.used_percent / ((p.captured_at_ms - origin_ms) as f64 / 3_600_000.0));
    let recent_30m = last.and_then(|l| {
        let recent: Vec<BurnPoint> = points.iter().copied().filter(|p| p.captured_at_ms >= l.captured_at_ms - 30 * 60_000).collect();
        let span = recent.last()?.captured_at_ms - recent.first()?.captured_at_ms;
        (recent.len() >= 3 && span >= 20 * 60_000).then(|| ols_slope_per_hour(&recent)).flatten().map(|s| s.max(0.0))
    });
    ShadowRates { production, average_since_origin, recent_30m, points: points.len() }
}

fn rate_over(points: &[BurnPoint], cfg: &AnalyticsConfig) -> Option<f64> {
    if points.len() < cfg.min_snapshots {
        return None;
    }
    let span = points.last()?.captured_at_ms - points.first()?.captured_at_ms;
    if span < cfg.min_span_ms {
        return None;
    }
    ols_slope_per_hour(points).map(|s| s.max(0.0))
}

/// `points` must be the cycle segment's Normal snapshots in time order.
pub fn compute_burn(points: &[BurnPoint], cfg: &AnalyticsConfig) -> BurnMetrics {
    compute_burn_from(points, None, cfg)
}

/// Like `compute_burn`; with `origin_ms` (a v2 re-anchored segment) the segment starts at that
/// derived 0 % point and the cycle rate is constrained through it. `points` exclude the origin.
pub fn compute_burn_from(observed: &[BurnPoint], origin_ms: Option<i64>, cfg: &AnalyticsConfig) -> BurnMetrics {
    let origin = origin_ms.filter(|o| observed.first().is_some_and(|p| *o < p.captured_at_ms));
    let mut with_origin = Vec::with_capacity(observed.len() + 1);
    if let Some(o) = origin {
        with_origin.push(BurnPoint { captured_at_ms: o, used_percent: 0.0 });
    }
    with_origin.extend_from_slice(observed);
    let points = with_origin.as_slice();
    let latest = points.last();
    let span_ms = match (points.first(), latest) {
        (Some(a), Some(b)) => b.captured_at_ms - a.captured_at_ms,
        _ => 0,
    };
    let consumption = match (points.first(), latest) {
        (Some(a), Some(b)) => (b.used_percent - a.used_percent).max(0.0),
        _ => 0.0,
    };
    let window = |ms: i64| -> Option<f64> {
        let end = latest?.captured_at_ms;
        let slice: Vec<BurnPoint> = points.iter().copied().filter(|p| p.captured_at_ms >= end - ms).collect();
        rate_over(&slice, cfg)
    };
    let current_cycle_rate = match origin {
        Some(o) => rate_over(points, cfg).and(origin_slope_per_hour(o, observed)).map(|s| s.max(0.0)),
        None => rate_over(points, cfg),
    };
    let status = if current_cycle_rate.is_none() {
        BurnStatus::InsufficientData
    } else if consumption < cfg.min_consumption_pp {
        BurnStatus::Stable
    } else {
        BurnStatus::Consuming
    };
    BurnMetrics {
        status,
        recent_6h_rate: window(cfg.recent_short_ms),
        recent_24h_rate: window(cfg.recent_long_ms),
        current_cycle_rate,
        points: points.len(),
        span_ms,
        observed_consumption_pp: consumption,
        latest_used_percent: latest.map(|p| p.used_percent),
        latest_captured_at_ms: latest.map(|p| p.captured_at_ms),
    }
}

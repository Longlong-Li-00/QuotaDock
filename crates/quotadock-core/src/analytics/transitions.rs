//! Transition evidence between two accepted observations of one window, observation
//! fingerprints for A->B->A detection, and sliding-anchor detection.
//! Fingerprints are observation comparisons only; they are not server window ids.

use super::config::EngineConfig;
use serde::{Deserialize, Serialize};

/// One accepted (or pending) observation of a window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObsPoint {
    pub snapshot_id: i64,
    pub captured_at_ms: i64,
    pub used: Option<f64>,
    pub reset_at_ms: Option<i64>,
    pub duration_mins: Option<i64>,
}

/// usedPercent / resetsAt / duration as compared across snapshots. Not a server window id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowFingerprint {
    pub used_percent: Option<f64>,
    pub resets_at_ms: Option<i64>,
    pub window_duration_mins: Option<i64>,
}

impl ObsPoint {
    pub fn fingerprint(&self) -> WindowFingerprint {
        WindowFingerprint { used_percent: self.used, resets_at_ms: self.reset_at_ms, window_duration_mins: self.duration_mins }
    }
}

/// Same reset time, same duration and usage within the noise tolerance.
pub fn same_state(a: &ObsPoint, b: &ObsPoint, cfg: &EngineConfig) -> bool {
    a.reset_at_ms == b.reset_at_ms
        && a.duration_mins == b.duration_mins
        && match (a.used, b.used) {
            (Some(x), Some(y)) => (x - y).abs() <= cfg.percent_noise_tolerance_pp,
            (None, None) => true,
            _ => false,
        }
}

/// The opposite of `same_state`: reset or duration differs, or usage moved beyond the noise band.
pub fn materially_differs(a: &ObsPoint, b: &ObsPoint, cfg: &EngineConfig) -> bool {
    !same_state(a, b, cfg)
}

/// A point that looks like an idle window whose reset is "now + duration".
pub fn sliding_like(p: &ObsPoint, cfg: &EngineConfig) -> bool {
    let (Some(used), Some(reset), Some(duration)) = (p.used, p.reset_at_ms, p.duration_mins) else { return false };
    if used != 0.0 {
        return false;
    }
    let offset_s = (reset - p.captured_at_ms) / 1000;
    (offset_s - duration * 60).abs() <= cfg.sliding_anchor_tolerance_secs
}

/// resetsAt moved forward together with the observation clock.
pub fn moves_with_clock(a: &ObsPoint, b: &ObsPoint, cfg: &EngineConfig) -> bool {
    let (Some(ra), Some(rb)) = (a.reset_at_ms, b.reset_at_ms) else { return false };
    let d_reset = rb - ra;
    let d_clock = b.captured_at_ms - a.captured_at_ms;
    d_clock > 0 && ((d_reset - d_clock).abs() / 1000) <= cfg.sliding_anchor_tolerance_secs
}

pub fn sliding_pair(a: &ObsPoint, b: &ObsPoint, cfg: &EngineConfig) -> bool {
    sliding_like(a, cfg) && sliding_like(b, cfg) && moves_with_clock(a, b, cfg)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlidingAnchorEvidence {
    pub samples: usize,
    pub expected_duration_seconds: i64,
    pub offset_variance_seconds: f64,
    pub detected: bool,
}

/// [QUOTADOCK HEURISTIC] Looks at the trailing run of points: each sliding-like
/// (used == 0, resetsAt - capturedAt ≈ duration) and each consecutive pair moving with the clock.
pub fn detect_sliding<'a>(points: impl DoubleEndedIterator<Item = &'a ObsPoint>, cfg: &EngineConfig) -> SlidingAnchorEvidence {
    let mut run: Vec<&ObsPoint> = Vec::new();
    for p in points.rev() {
        if !sliding_like(p, cfg) {
            break;
        }
        if let Some(later) = run.last()
            && !moves_with_clock(p, later, cfg)
        {
            break;
        }
        run.push(p);
    }
    let expected = run.first().and_then(|p| p.duration_mins).unwrap_or(0) * 60;
    let offsets: Vec<f64> =
        run.iter().filter_map(|p| p.reset_at_ms.map(|r| ((r - p.captured_at_ms) as f64 / 1000.0) - expected as f64)).collect();
    let variance = if offsets.len() > 1 {
        let mean = offsets.iter().sum::<f64>() / offsets.len() as f64;
        offsets.iter().map(|o| (o - mean).powi(2)).sum::<f64>() / offsets.len() as f64
    } else {
        0.0
    };
    SlidingAnchorEvidence {
        samples: run.len(),
        expected_duration_seconds: expected,
        offset_variance_seconds: variance,
        detected: run.len() >= cfg.sliding_min_samples,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Availability {
    Allowed,
    Blocked,
    Unknown,
}

impl From<Option<bool>> for Availability {
    fn from(v: Option<bool>) -> Self {
        match v {
            Some(true) => Self::Allowed,
            Some(false) => Self::Blocked,
            None => Self::Unknown,
        }
    }
}

/// Everything known about one window transition; persisted verbatim in `evidence_json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowTransitionEvidence {
    pub used_percent_before: Option<f64>,
    pub used_percent_after: Option<f64>,
    pub used_percent_delta: Option<f64>,
    pub reset_at_before: Option<i64>,
    pub reset_at_after: Option<i64>,
    pub reset_at_delta_seconds: Option<i64>,
    pub availability_before: Availability,
    pub availability_after: Availability,
    pub credits_before: Option<i64>,
    pub credits_after: Option<i64>,
    pub credits_delta: Option<i64>,
    pub crossed_previous_reset_time: bool,
    pub stable_new_reset_at_polls: usize,
    pub sliding_anchor_detected: bool,
    pub bucket_topology_changed: bool,
    pub snapshot_before: i64,
    pub snapshot_after: i64,
    pub captured_before_ms: i64,
    pub captured_after_ms: i64,
}

#[allow(clippy::too_many_arguments)]
pub fn extract_evidence(
    before: &ObsPoint,
    after: &ObsPoint,
    availability_before: Availability,
    availability_after: Availability,
    credits_before: Option<i64>,
    credits_after: Option<i64>,
    sliding_anchor_detected: bool,
    bucket_topology_changed: bool,
) -> WindowTransitionEvidence {
    WindowTransitionEvidence {
        used_percent_before: before.used,
        used_percent_after: after.used,
        used_percent_delta: match (before.used, after.used) {
            (Some(a), Some(b)) => Some(b - a),
            _ => None,
        },
        reset_at_before: before.reset_at_ms,
        reset_at_after: after.reset_at_ms,
        reset_at_delta_seconds: match (before.reset_at_ms, after.reset_at_ms) {
            (Some(a), Some(b)) => Some((b - a) / 1000),
            _ => None,
        },
        availability_before,
        availability_after,
        credits_before,
        credits_after,
        credits_delta: match (credits_before, credits_after) {
            (Some(a), Some(b)) => Some(b - a),
            _ => None,
        },
        crossed_previous_reset_time: before.reset_at_ms.is_some_and(|r| after.captured_at_ms >= r),
        stable_new_reset_at_polls: 1,
        sliding_anchor_detected,
        bucket_topology_changed,
        snapshot_before: before.snapshot_id,
        snapshot_after: after.snapshot_id,
        captured_before_ms: before.captured_at_ms,
        captured_after_ms: after.captured_at_ms,
    }
}

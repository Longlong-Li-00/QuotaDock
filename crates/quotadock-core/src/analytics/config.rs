//! Every tunable threshold in one place. All of these are [QUOTADOCK HEURISTIC]: chosen by
//! QuotaDock, not published by OpenAI.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EngineConfig {
    /// usedPercent is the backend float `.round()`ed [OPENAI CODEX SOURCE]; drops up to this many
    /// percentage points are treated as rounding/correction noise, never as reset evidence.
    pub percent_noise_tolerance_pp: f64,
    /// A drop of at least this many points is a "substantial" drop (strong but not sufficient evidence).
    pub substantial_drop_pp: f64,
    /// A new resetsAt must be seen in this many consecutive successful snapshots to be confirmed.
    pub schedule_confirm_polls: u32,
    /// Sliding-anchor detection: |(resetsAt - capturedAt) - duration| and |Δreset - Δcaptured|
    /// must stay within this tolerance.
    pub sliding_anchor_tolerance_secs: i64,
    /// Samples needed before a sliding anchor is declared.
    pub sliding_min_samples: usize,
    /// A missing window (or a changed bucket set) must persist this many successful polls before
    /// it is treated as a lasting topology change.
    pub missing_confirm_polls: u32,
    /// A reset-credit decrease and a quota refresh are correlated if they are at most this many
    /// successful polls apart (credit updates may not be atomic with the refresh).
    pub credit_correlation_polls: u32,
    /// After a confirmed change, a return to the pre-change state within this many polls is a
    /// backend reconciliation, not a new transition.
    pub reconciliation_polls: u32,
    /// Accepted points kept per window for A->B->A and sliding detection (persisted across restarts).
    pub history_len: usize,
    /// v3. A fixed-anchor window that turns into an idle sliding anchor (usedPercent 0, resetsAt =
    /// capturedAt + duration, moving with the clock) before its reset time is confirmed only after
    /// this many consecutive idle observations (opener included).
    #[serde(default = "default_idle_confirm_polls")]
    pub idle_confirm_polls: u32,
    /// v3. Between consecutive idle observations, |Δreset - Δcaptured| must stay within this
    /// tolerance. Real 2026-10-23 data: offsets 0-1 s, at most 2 s between polls; 30 s bounds the
    /// request latency (reads time out at 30 s). Stricter than `sliding_anchor_tolerance_secs`,
    /// which bounds the absolute offset.
    #[serde(default = "default_idle_anchor_drift_tolerance_secs")]
    pub idle_anchor_drift_tolerance_secs: i64,
    /// v3. After this many consecutive candidates against the same accepted reference were each
    /// superseded by yet another value, the latest observation becomes the reference (one
    /// diagnostic event) instead of opening schedule-change / unknown candidates forever.
    #[serde(default = "default_max_superseded_candidates")]
    pub max_superseded_candidates: u32,
}

fn default_idle_confirm_polls() -> u32 {
    3
}

fn default_idle_anchor_drift_tolerance_secs() -> i64 {
    30
}

fn default_max_superseded_candidates() -> u32 {
    12
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            percent_noise_tolerance_pp: 1.0,
            substantial_drop_pp: 10.0,
            schedule_confirm_polls: 2,
            sliding_anchor_tolerance_secs: 120,
            sliding_min_samples: 3,
            missing_confirm_polls: 3,
            credit_correlation_polls: 2,
            reconciliation_polls: 3,
            history_len: 5,
            idle_confirm_polls: default_idle_confirm_polls(),
            idle_anchor_drift_tolerance_secs: default_idle_anchor_drift_tolerance_secs(),
            max_superseded_candidates: default_max_superseded_candidates(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AnalyticsConfig {
    /// Minimum Normal snapshots in the analysed range before any burn rate is reported.
    pub min_snapshots: usize,
    /// Minimum observation span (ms) before any burn rate is reported.
    pub min_span_ms: i64,
    /// Minimum observed consumption (pp) before an exhaustion time is projected.
    pub min_consumption_pp: f64,
    pub recent_short_ms: i64,
    pub recent_long_ms: i64,
    /// Quota pressure bands: below `pressure_watch` = Low, up to `pressure_at_risk` = Watch, above = AtRisk.
    pub pressure_watch: f64,
    pub pressure_at_risk: f64,
    /// A weekly forecast (rate, risk) needs this much observation inside its segment; before
    /// that it reports insufficient data. Only gates the result: the rates are computed as usual.
    #[serde(default = "default_weekly_min_span_ms")]
    pub weekly_min_span_ms: i64,
}

/// 24 h [QUOTADOCK HEURISTIC]. Real run 2026-10-11: three hours of one work session right after a
/// Codex weekly refresh were projected over the week as "may run out early" (and notified).
pub const WEEKLY_MIN_SPAN_MS: i64 = 24 * 3_600_000;

fn default_weekly_min_span_ms() -> i64 {
    WEEKLY_MIN_SPAN_MS
}

impl Default for AnalyticsConfig {
    fn default() -> Self {
        Self {
            min_snapshots: 3,
            min_span_ms: 30 * 60_000,
            min_consumption_pp: 2.0,
            recent_short_ms: 6 * 3_600_000,
            recent_long_ms: 24 * 3_600_000,
            pressure_watch: 0.7,
            pressure_at_risk: 1.0,
            weekly_min_span_ms: WEEKLY_MIN_SPAN_MS,
        }
    }
}

/// Retention policy structure (V1: defined, cleanup not enabled by default).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RetentionConfig {
    pub raw_snapshot_days: u32,
    pub cleanup_enabled: bool,
}

impl Default for RetentionConfig {
    fn default() -> Self {
        Self { raw_snapshot_days: 90, cleanup_enabled: false }
    }
}

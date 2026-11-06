//! Engine-facing data model. Observations are built from persisted snapshot rows, so the live
//! path and the re-analysis path feed the engine exactly the same input.

pub use crate::codex::normalize::WindowType;
use serde::{Deserialize, Serialize};

/// One window as the engine sees it. `reset_at_ms` is None when the server sent no usable value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowObs {
    pub present: bool,
    pub used_percent: Option<f64>,
    pub reset_at_ms: Option<i64>,
    pub duration_mins: Option<i64>,
}

impl WindowObs {
    pub fn missing() -> Self {
        Self { present: false, used_percent: None, reset_at_ms: None, duration_mins: None }
    }

    pub fn new(used: f64, reset_at_ms: i64, duration_mins: i64) -> Self {
        Self { present: true, used_percent: Some(used), reset_at_ms: Some(reset_at_ms), duration_mins: Some(duration_mins) }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Observation {
    pub snapshot_id: i64,
    /// responseReceivedAt (local clock, UTC ms). Server clock is not assumed to match.
    pub captured_at_ms: i64,
    pub account_fingerprint: Option<String>,
    /// True when the rate-limit account differs from the active Codex account.
    pub account_mismatch: bool,
    pub ordinary_usage_allowed: Option<bool>,
    pub reset_credit_count: Option<i64>,
    /// Sorted limitIds of rateLimitsByLimitId.
    pub bucket_ids: Vec<String>,
    pub five_hour: WindowObs,
    pub weekly: WindowObs,
}

impl Observation {
    pub fn window(&self, t: WindowType) -> &WindowObs {
        match t {
            WindowType::FiveHour => &self.five_hour,
            WindowType::Weekly => &self.weekly,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WindowObservationState {
    /// usedPercent > 0 and a reset time is present.
    Active,
    /// usedPercent == 0 without a confirmed fixed anchor (possibly a sliding now+duration anchor).
    IdleOrUninitialized,
    /// The reported reset time has passed but no new window has been observed yet.
    ExpiredUnconfirmed,
    /// A new reset time was seen once and awaits confirmation.
    ScheduleChangeCandidate,
    /// usedPercent == 0 with a reset time confirmed across >= 2 polls (initialized, unused).
    StableSchedule,
    /// The window is absent from the latest snapshot(s).
    TemporarilyMissing,
    Unknown,
}

impl WindowObservationState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::IdleOrUninitialized => "idle_or_uninitialized",
            Self::ExpiredUnconfirmed => "expired_unconfirmed",
            Self::ScheduleChangeCandidate => "schedule_change_candidate",
            Self::StableSchedule => "stable_schedule",
            Self::TemporarilyMissing => "temporarily_missing",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResetCandidateState {
    None,
    Pending,
    Confirmed,
    Rejected,
}

impl ResetCandidateState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Pending => "pending",
            Self::Confirmed => "confirmed",
            Self::Rejected => "rejected",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum EvidenceConfidence {
    Low,
    Medium,
    High,
}

impl EvidenceConfidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum QuotaEventType {
    NaturalRolloverObserved,
    ResetCreditRedeemed,
    UnscheduledQuotaRefresh,
    WindowScheduleChanged,
    SlidingAnchorDetected,
    TransientBackendState,
    BackendSnapshotReconciliation,
    BucketTopologyChanged,
    AccountChanged,
    /// The reset-credit count changed without a correlated quota refresh.
    ResetCreditCountChanged,
    UnknownQuotaTransition,
}

impl QuotaEventType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NaturalRolloverObserved => "natural_rollover_observed",
            Self::ResetCreditRedeemed => "reset_credit_redeemed",
            Self::UnscheduledQuotaRefresh => "unscheduled_quota_refresh",
            Self::WindowScheduleChanged => "window_schedule_changed",
            Self::SlidingAnchorDetected => "sliding_anchor_detected",
            Self::TransientBackendState => "transient_backend_state",
            Self::BackendSnapshotReconciliation => "backend_snapshot_reconciliation",
            Self::BucketTopologyChanged => "bucket_topology_changed",
            Self::AccountChanged => "account_changed",
            Self::ResetCreditCountChanged => "reset_credit_count_changed",
            Self::UnknownQuotaTransition => "unknown_quota_transition",
        }
    }

    /// Only these reach the user by default; everything else is an internal diagnostic.
    pub fn visibility(self) -> EventVisibility {
        match self {
            Self::NaturalRolloverObserved | Self::ResetCreditRedeemed | Self::UnscheduledQuotaRefresh => EventVisibility::User,
            _ => EventVisibility::Diagnostic,
        }
    }

    /// Events that start a new observed cycle segment (burn rate never crosses them).
    pub fn is_cycle_boundary(self) -> bool {
        matches!(self, Self::NaturalRolloverObserved | Self::ResetCreditRedeemed | Self::UnscheduledQuotaRefresh)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventVisibility {
    User,
    Diagnostic,
}

impl EventVisibility {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Diagnostic => "diagnostic",
        }
    }
}

/// Per-window snapshot quality. Analytics use `Normal` only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SnapshotQuality {
    Normal,
    /// Part of an unresolved candidate; excluded until confirmed (then Normal) or rejected.
    PendingConfirmation,
    SuspectTransient,
    MissingWindow,
    AccountMismatch,
}

impl SnapshotQuality {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::PendingConfirmation => "pending_confirmation",
            Self::SuspectTransient => "suspect_transient",
            Self::MissingWindow => "missing_window",
            Self::AccountMismatch => "account_mismatch",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "normal" => Self::Normal,
            "pending_confirmation" => Self::PendingConfirmation,
            "suspect_transient" => Self::SuspectTransient,
            "missing_window" => Self::MissingWindow,
            _ => Self::AccountMismatch,
        }
    }
}

/// A derived event, keyed deterministically so replays are idempotent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewEvent {
    pub event_key: String,
    pub account_fingerprint: Option<String>,
    pub window_type: Option<WindowType>,
    pub event_type: QuotaEventType,
    pub observed_at_ms: i64,
    pub previous_snapshot_id: Option<i64>,
    pub current_snapshot_id: i64,
    pub confidence: EvidenceConfidence,
    pub confirmed: bool,
    pub evidence: serde_json::Value,
    /// Key of an earlier event this one supersedes (reconciliation).
    pub supersedes_event_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CycleOp {
    Open {
        cycle_key: String,
        account_fingerprint: String,
        window_type: WindowType,
        observed_started_at_ms: i64,
        first_snapshot_id: i64,
        initial_used_percent: Option<f64>,
        initial_reset_at_ms: Option<i64>,
        start_reason: String,
        start_event_key: Option<String>,
        confidence: EvidenceConfidence,
    },
    Extend {
        cycle_key: String,
        last_snapshot_id: i64,
        last_seen_at_ms: i64,
        final_used_percent: Option<f64>,
        final_reset_at_ms: Option<i64>,
        /// Derived (reset - duration) from a confirmed, non-sliding anchor. Never authoritative.
        nominal_window_start_ms: Option<i64>,
    },
    Close {
        cycle_key: String,
        observed_ended_at_ms: i64,
        end_reason: String,
        end_event_key: Option<String>,
    },
    /// Undo a boundary that a backend reconciliation proved spurious.
    Void {
        cycle_key: String,
        reason: String,
    },
    Reopen {
        cycle_key: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QualityUpdate {
    pub snapshot_id: i64,
    pub window_type: WindowType,
    pub quality: SnapshotQuality,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidateRecord {
    pub candidate_key: String,
    pub account_fingerprint: String,
    pub window_type: Option<WindowType>,
    pub kind: String,
    pub state: ResetCandidateState,
    pub opened_at_ms: i64,
    pub opened_snapshot_id: i64,
    pub baseline_snapshot_id: Option<i64>,
    pub last_snapshot_id: i64,
    pub resolved_at_ms: Option<i64>,
    pub resolution_event_key: Option<String>,
    pub evidence: serde_json::Value,
}

/// Per-window state as exposed to storage/UI after each step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowTrackView {
    pub account_fingerprint: String,
    pub window_type: WindowType,
    pub observation_state: WindowObservationState,
    pub candidate_state: ResetCandidateState,
    /// resetsAt confirmed across >= 2 polls and not sliding; the only value forecasts may use.
    pub confirmed_reset_at_ms: Option<i64>,
    pub anchor_polls: u32,
    pub sliding_anchor: bool,
    pub missing_streak: u32,
    pub current_cycle_key: Option<String>,
    /// Burn-rate analysis starts here (cycle start, the latest confirmed in-cycle correction, or the
    /// first active observation after an idle period).
    pub analysis_segment_start_snapshot_id: Option<i64>,
    /// Derived (resetsAt - duration) 0 % origin of a segment re-anchored after idle; not authoritative.
    #[serde(default)]
    pub active_origin_ms: Option<i64>,
    pub last_snapshot_id: i64,
}

//! The narrow API a UI may call. Six read/refresh operations, nothing else: no shell, no SQL,
//! no arbitrary RPC, no process control. Every response is sanitized (account fingerprint only).

use crate::analytics::burn_rate::BurnPoint;
use crate::analytics::forecast::{UsageForecast, forecast};
use crate::analytics::model::{WindowTrackView, WindowType};
use crate::codex::provider::{PollStatus, ProviderStatus};
use crate::pipeline::{CoreError, PollTrigger, QuotaDockCore};
use crate::polling::{PollingHandle, PollingStatus, RefreshDecision};
use crate::storage::database::DatabaseInfo;
use crate::storage::derived::{self, CycleView, EventView};
use crate::storage::snapshots::{self, PollSummary, SnapshotView};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowStatus {
    pub window_type: WindowType,
    pub present: bool,
    pub used_percent: Option<f64>,
    pub remaining_percent: Option<f64>,
    /// The latest value the server reported (may be transient or sliding).
    pub reported_reset_at_ms: Option<i64>,
    /// Same value across >= 2 polls and not sliding: the only reset time a countdown may use.
    pub confirmed_reset_at_ms: Option<i64>,
    pub reset_schedule: &'static str,
    pub observation_state: Option<String>,
    pub candidate_state: Option<String>,
    pub sliding_anchor: bool,
    pub snapshot_quality: String,
    pub current_cycle: Option<CycleView>,
    /// How exact the reset time is. Codex: None. Claude: 1000 from the status line, 60000 from
    /// `claude -p /usage`; above 1000 the time is approximate.
    pub reset_precision_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrentUsage {
    pub provider: &'static str,
    pub captured_at_ms: Option<i64>,
    /// Straight from ordinaryUsageAllowed; never derived from percentages or reset times.
    pub availability: Option<String>,
    pub ordinary_usage_allowed: Option<bool>,
    pub windows: Vec<WindowStatus>,
    pub reset_credit_count: Option<i64>,
    pub bucket_ids: Vec<String>,
    pub account: Option<String>,
    pub polls: PollSummary,
    pub classifier_version: &'static str,
}

fn track_for(core: &QuotaDockCore, t: WindowType) -> Option<WindowTrackView> {
    let state = core.engine_state();
    let ctx = state.context.as_ref()?;
    core.engine().views(ctx).into_iter().find(|v| v.window_type == t)
}

pub fn get_current_codex_usage(core: &QuotaDockCore) -> Result<CurrentUsage, CoreError> {
    let latest: Option<SnapshotView> = core.with_db(snapshots::latest_snapshot)?;
    let polls = core.with_db(snapshots::poll_summary)?;
    let mut windows = Vec::new();
    for t in WindowType::ALL {
        let track = track_for(core, t);
        let cycle = match track.as_ref().and_then(|v| v.current_cycle_key.clone()) {
            Some(k) => core.with_db(|c| derived::cycle_by_key(c, &k))?,
            None => None,
        };
        let w = latest.as_ref().map(|s| match t {
            WindowType::FiveHour => s.five_hour.clone(),
            WindowType::Weekly => s.weekly.clone(),
        });
        let now = latest.as_ref().map(|s| s.captured_at_ms).unwrap_or(0);
        let schedule = match &track {
            Some(v) if v.sliding_anchor => "sliding",
            Some(v) => match v.confirmed_reset_at_ms {
                Some(r) if r > now => "confirmed",
                Some(_) => "passed",
                None => "unconfirmed",
            },
            None => "unconfirmed",
        };
        windows.push(WindowStatus {
            window_type: t,
            present: w.as_ref().is_some_and(|w| w.present),
            used_percent: w.as_ref().and_then(|w| w.used_percent),
            remaining_percent: w.as_ref().and_then(|w| w.remaining_percent),
            reported_reset_at_ms: w.as_ref().and_then(|w| w.reset_at_ms),
            confirmed_reset_at_ms: track.as_ref().and_then(|v| v.confirmed_reset_at_ms).filter(|_| schedule == "confirmed"),
            reset_schedule: schedule,
            observation_state: track.as_ref().map(|v| v.observation_state.as_str().to_string()),
            candidate_state: track.as_ref().map(|v| v.candidate_state.as_str().to_string()),
            sliding_anchor: track.as_ref().is_some_and(|v| v.sliding_anchor),
            snapshot_quality: w.as_ref().map(|w| w.quality.clone()).unwrap_or_else(|| "none".into()),
            current_cycle: cycle,
            reset_precision_ms: None,
        });
    }
    Ok(CurrentUsage {
        provider: "codex",
        captured_at_ms: latest.as_ref().map(|s| s.captured_at_ms),
        availability: latest.as_ref().map(|s| s.availability.clone()),
        ordinary_usage_allowed: latest.as_ref().and_then(|s| s.ordinary_usage_allowed),
        windows,
        reset_credit_count: latest.as_ref().and_then(|s| s.reset_credit_count),
        bucket_ids: latest.as_ref().map(|s| s.bucket_ids.clone()).unwrap_or_default(),
        account: latest.as_ref().and_then(|s| s.account_fingerprint.as_deref().map(crate::codex::sanitize::short_fingerprint)),
        polls,
        classifier_version: crate::analytics::CLASSIFIER_VERSION,
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshResult {
    pub decision: RefreshDecision,
    pub status: Option<PollStatus>,
}

/// With a polling loop, goes through it (coalesced/throttled). Without one, reads directly.
pub async fn refresh_codex_usage(core: &QuotaDockCore, polling: Option<&PollingHandle>) -> RefreshResult {
    match polling {
        Some(p) => RefreshResult { decision: p.refresh_now().await, status: None },
        None => {
            let out = core.poll_once(PollTrigger::Manual).await;
            RefreshResult { decision: RefreshDecision::Started, status: Some(out.status) }
        }
    }
}

pub fn get_usage_history(core: &QuotaDockCore, since_ms: i64, limit: usize) -> Result<Vec<SnapshotView>, CoreError> {
    let account = core.engine_state().context.map(|c| c.account_fingerprint);
    core.with_db(|c| snapshots::window_history(c, account.as_deref(), since_ms, limit.min(5_000)))
}

pub fn get_quota_events(core: &QuotaDockCore, include_diagnostic: bool, limit: usize) -> Result<Vec<EventView>, CoreError> {
    core.with_db(|c| derived::events(c, include_diagnostic, limit.min(1_000)))
}

/// Computed from local SQLite only; never triggers a backend read.
pub fn get_usage_forecast(core: &QuotaDockCore, window: WindowType) -> Result<UsageForecast, CoreError> {
    let state = core.engine_state();
    let track = track_for(core, window);
    let points: Vec<BurnPoint> = match (state.context.as_ref(), track.as_ref().and_then(|t| t.analysis_segment_start_snapshot_id)) {
        (Some(ctx), Some(from)) => core.with_db(|c| snapshots::burn_points(c, &ctx.account_fingerprint, window, from))?,
        _ => vec![],
    };
    // A v2 segment re-anchored after idle starts at its derived 0 % origin (track.active_origin_ms);
    // `forecast` applies it, and the idle snapshots before the segment are not read at all.
    Ok(forecast(window, &points, track.as_ref(), &core.analytics))
}

/// Diagnostics-only shadow estimates for the current 5h segment, when it was re-anchored after
/// idle (reset-detector-v2). Read from the same points as the production forecast; never shown in
/// the dashboard and never used by risk, tray or notifications.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FiveHourShadow {
    /// Derived, not authoritative: resetsAt - 5 h.
    pub origin_ms: i64,
    pub segment_start_snapshot_id: i64,
    pub latest_snapshot_id: i64,
    pub latest_used_percent: Option<f64>,
    pub rates: crate::analytics::burn_rate::ShadowRates,
}

pub fn five_hour_shadow(core: &QuotaDockCore) -> Result<Option<FiveHourShadow>, CoreError> {
    let Some(track) = track_for(core, WindowType::FiveHour) else { return Ok(None) };
    let (Some(origin_ms), Some(from)) = (track.active_origin_ms, track.analysis_segment_start_snapshot_id) else { return Ok(None) };
    let state = core.engine_state();
    let Some(ctx) = state.context.as_ref() else { return Ok(None) };
    let points = core.with_db(|c| snapshots::burn_points(c, &ctx.account_fingerprint, WindowType::FiveHour, from))?;
    let production = forecast(WindowType::FiveHour, &points, Some(&track), &core.analytics).burn_rate_per_hour;
    Ok(Some(FiveHourShadow {
        origin_ms,
        segment_start_snapshot_id: from,
        latest_snapshot_id: track.last_snapshot_id,
        latest_used_percent: points.last().map(|p| p.used_percent),
        rates: crate::analytics::burn_rate::shadow_rates(origin_ms, &points, production),
    }))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FullProviderStatus {
    pub provider: ProviderStatus,
    pub database: DatabaseInfo,
    pub polling: Option<PollingStatus>,
    pub classifier_version: &'static str,
}

pub async fn get_provider_status(core: &QuotaDockCore, polling: Option<&PollingHandle>) -> FullProviderStatus {
    FullProviderStatus {
        provider: core.provider_status().await,
        database: core.database_info().clone(),
        polling: polling.map(|p| p.status()),
        classifier_version: crate::analytics::CLASSIFIER_VERSION,
    }
}

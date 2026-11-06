//! The single read model for the Overview page (provider-neutral shape, Codex only in V1).

use super::settings::{AppSettings, Lang};
use super::tray::TrayStatusModel;
use crate::analytics::forecast::UsageForecast;
use crate::codex::types::VersionGate;
use crate::commands::CurrentUsage;
use crate::polling::PollingStatus;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthState {
    Connecting,
    Ok,
    Paused,
    NotFound,
    NotAuthenticated,
    UnsupportedVersion,
    Reconnecting,
    // Providers with passive or optional sources (Claude):
    Stale,
    Waiting,
    NotConfigured,
    ApiBilling,
    SourceError,
    NeedsVerification,
    Disabled,
}

/// A limit the source names beyond 5-hour/weekly (e.g. a per-model weekly limit).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtraLimitStatus {
    pub key: String,
    pub label: String,
    pub used_percent: f64,
    pub remaining_percent: f64,
    pub resets_at_ms: Option<i64>,
    /// How exact `resets_at_ms` is, as on `WindowStatus`: above 1000 ms it is approximate.
    pub reset_precision_ms: Option<i64>,
    /// False once the source reset time has passed (the value is from the previous period).
    pub current: bool,
}

/// Where a provider's numbers come from and how fresh they are (diagnostics, tooltips).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSourceInfo {
    pub state: crate::claude::monitor::ClaudeSourceState,
    pub last_source: Option<String>,
    pub last_update_ms: Option<i64>,
    pub last_passive_ms: Option<i64>,
    pub last_active_ms: Option<i64>,
    pub live: bool,
    pub integration_enabled: bool,
    pub active_enabled: bool,
    pub active_suspended: bool,
    pub active_needs_verification: bool,
    pub active_next_allowed_ms: Option<i64>,
    pub billing: &'static str,
    pub cli_found: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderHealth {
    pub state: HealthState,
    pub consecutive_failures: u32,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderDashboard {
    pub provider: &'static str,
    pub display_name: &'static str,
    pub plan_type: Option<String>,
    pub cli_version: Option<String>,
    pub app_server_version: Option<String>,
    /// The CLI is newer than the verified version: reads are attempted, flagged in the UI.
    pub compatibility_mode: bool,
    pub health: ProviderHealth,
    pub usage: CurrentUsage,
    pub forecasts: Vec<UsageForecast>,
    pub extra_limits: Vec<ExtraLimitStatus>,
    /// active (QuotaDock can ask the source now) | passive (data arrives on its own) | unavailable
    pub refresh_capability: &'static str,
    pub source: Option<ProviderSourceInfo>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardDto {
    pub generated_at_ms: i64,
    pub paused: bool,
    pub polling: Option<PollingStatus>,
    pub tray: TrayStatusModel,
    pub providers: Vec<ProviderDashboard>,
    pub settings: AppSettings,
    pub effective_language: Lang,
}

pub fn health_from(
    paused: bool,
    has_snapshot: bool,
    trailing_failures: u32,
    last_status: Option<&str>,
    last_error: Option<&str>,
) -> HealthState {
    if paused {
        return HealthState::Paused;
    }
    if trailing_failures == 0 {
        return if has_snapshot { HealthState::Ok } else { HealthState::Connecting };
    }
    let err = last_error.unwrap_or("");
    match last_status {
        Some("authentication_error") => HealthState::NotAuthenticated,
        Some("provider_unavailable") if err.contains("no codex executable") => HealthState::NotFound,
        Some("provider_unavailable") if err.contains("unverified older") || err.contains("older codex") => HealthState::UnsupportedVersion,
        _ => HealthState::Reconnecting,
    }
}

pub fn compatibility_mode(gate: Option<VersionGate>) -> bool {
    gate == Some(VersionGate::UnverifiedNewer)
}

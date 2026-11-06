//! Shared Codex types. Wire names follow the app-server schema of codex-cli 0.162.0-alpha.2.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The codex-cli version verified on the reference machine (Phase 1B).
pub const VERIFIED_CODEX_VERSION: &str = "0.162.0-alpha.2";

/// The only requests QuotaDock may send. The app-server also exposes fs/writeFile,
/// process/spawn, account/logout, account/rateLimitResetCredit/consume, ... which are refused
/// before reaching the wire.
pub const READ_ONLY_REQUEST_METHODS: &[&str] = &["initialize", "account/read", "account/rateLimits/read"];

/// The only client notification QuotaDock sends (handshake hygiene; the server only logs it).
pub const CLIENT_NOTIFICATION_METHODS: &[&str] = &["initialized"];

/// Identifies QuotaDock in the app-server User-Agent. Never "Codex Desktop" (special-cased upstream).
pub const CLIENT_NAME: &str = "quotadock";
pub const CLIENT_TITLE: &str = "QuotaDock";
pub const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InstallationSource {
    Manual,
    Desktop,
    Standalone,
    Vscode,
    Unknown,
}

impl InstallationSource {
    pub fn priority(self) -> u8 {
        match self {
            Self::Manual => 0,
            Self::Desktop => 1,
            Self::Standalone => 2,
            Self::Vscode => 3,
            Self::Unknown => 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VersionGate {
    Verified,
    UnverifiedNewer,
    UnsupportedOlder,
    Unparseable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexInstallation {
    pub path: PathBuf,
    pub version: String,
    pub source: InstallationSource,
    pub detail: String,
    pub gate: VersionGate,
}

/// V1 production transport is the private stdio app-server. The daemon proxy exists only in the
/// TypeScript reference (experimental, disabled by default, unverified on a real daemon).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TransportKind {
    StdioAppServer,
    DaemonProxy,
}

impl TransportKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::StdioAppServer => "stdio-app-server",
            Self::DaemonProxy => "daemon-proxy",
        }
    }
}

/// One quota window exactly as the parity layer exposes it (mirrors the TS `QuotaWindow`).
/// Numbers are f64 like JavaScript numbers so parity with the reference is exact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWindow {
    pub used_percent: Option<f64>,
    pub remaining_percent: Option<f64>,
    pub window_duration_mins: Option<f64>,
    pub resets_at: Option<f64>,
    pub resets_at_iso: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitBucket {
    pub limit_id: String,
    pub primary: Option<QuotaWindow>,
    pub secondary: Option<QuotaWindow>,
    pub raw: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UsageAvailability {
    Allowed,
    Blocked,
    Unknown,
}

impl UsageAvailability {
    /// `ordinaryUsageAllowed` is the only availability signal. Nothing else may change it.
    pub fn from_flag(flag: Option<bool>) -> Self {
        match flag {
            Some(true) => Self::Allowed,
            Some(false) => Self::Blocked,
            None => Self::Unknown,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allowed => "allowed",
            Self::Blocked => "blocked",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceBucket {
    Codex,
    Legacy,
}

/// Mirrors the TS `CodexQuotaSnapshot` (minus transport metadata), used for parity and storage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalizedRateLimits {
    pub ordinary_usage_allowed: Option<bool>,
    pub availability: UsageAvailability,
    pub five_hour: Option<QuotaWindow>,
    pub weekly: Option<QuotaWindow>,
    pub source_bucket: Option<SourceBucket>,
    /// Every bucket of `rateLimitsByLimitId`, unknown ones included, in server order.
    pub buckets: Vec<RateLimitBucket>,
    pub reset_credit_count: Option<f64>,
    pub reset_credits_present: bool,
    /// Memory only. Never persisted, never logged.
    #[serde(skip_serializing)]
    pub account_id: Option<String>,
    pub warnings: Vec<String>,
}

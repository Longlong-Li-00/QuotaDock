//! Provider-neutral shapes for one Claude usage observation (not Codex structs).

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PROVIDER_CLAUDE: &str = "claude";
pub const FIVE_HOUR_KEY: &str = "five_hour";
pub const WEEKLY_KEY: &str = "weekly";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaudeSource {
    /// Pushed by Claude Code to the QuotaDock status-line bridge.
    StatusLine,
    /// `claude -p /usage` run by QuotaDock (optional, off by default).
    ActiveCli,
}

impl ClaudeSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::StatusLine => "statusline",
            Self::ActiveCli => "active_cli",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Billing {
    Subscription,
    /// Claude Code is billing an API key (or Bedrock/Vertex/Foundry): no subscription quota exists.
    ApiKey,
    Unknown,
}

impl Billing {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Subscription => "subscription",
            Self::ApiKey => "api_key",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowKind {
    FiveHour,
    Weekly,
    /// Any other limit the source names explicitly (e.g. a per-model weekly limit).
    Extra,
}

impl WindowKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FiveHour => "five_hour",
            Self::Weekly => "weekly",
            Self::Extra => "extra",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeWindowObs {
    /// Stable key from the source's own naming: `five_hour`, `weekly`, `weekly_model:<name>`.
    pub key: String,
    pub kind: WindowKind,
    /// The source's label for extra limits (e.g. "Fable").
    pub label: Option<String>,
    pub duration_mins: Option<i64>,
    /// Source precision is kept (statusline may send 42.7).
    pub used_percent: f64,
    /// Source-supplied; never computed as now + duration.
    pub resets_at_ms: Option<i64>,
    /// 1 000 for statusline epoch seconds, 60 000 for /usage minute text.
    pub reset_precision_ms: i64,
}

impl ClaudeWindowObs {
    pub fn remaining_percent(&self) -> f64 {
        (100.0 - self.used_percent).clamp(0.0, 100.0)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeObservation {
    pub source: ClaudeSource,
    /// When the source produced the data (bridge receive time / query time).
    pub captured_at_ms: i64,
    pub claude_version: Option<String>,
    pub billing: Billing,
    pub windows: Vec<ClaudeWindowObs>,
    /// Sanitized source payload (no tokens, emails, ids, paths).
    pub raw_sanitized: Value,
}

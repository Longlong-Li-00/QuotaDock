//! Claude source orchestration: passive status-line bridge (primary), optional active `/usage`
//! fallback (off by default), authentication-mode checks. No polling of Anthropic beyond the
//! user-enabled active fallback, which only runs when passive data is stale.
//!
//! Cadence and backoff are [QUOTADOCK HEURISTIC] values agreed for Phase 3.5:
//! active foreground >= 5 min, background >= 15 min, skipped while passive data is fresher than
//! that; HTTP 429 -> Retry-After or 15/30/60 min, three in a row -> suspended for the session.

use super::cli::{self, ClaudeInstallation};
use super::env::{self, AuthEnvFlags};
use super::integration::{self, IntegrationState};
use super::model::*;
use super::statusline;
use super::store::{self, IngestResult};
use super::usage::{self, AuthStatus, UsageQueryError};
use crate::pipeline::{CoreError, QuotaDockCore};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

pub const ACTIVE_FOREGROUND_MS: i64 = 5 * 60_000;
pub const ACTIVE_BACKGROUND_MS: i64 = 15 * 60_000;
pub const ACTIVE_MANUAL_MIN_GAP_MS: i64 = 60_000;
pub const BACKOFF_MS: [i64; 3] = [15 * 60_000, 30 * 60_000, 60 * 60_000];
pub const SUSPEND_AFTER_429: u32 = 3;
/// Passive data counts as live for 10 min. With a usable active source the data is refreshed on
/// schedule (background every 15 min), so it stays live for one background interval plus 5 min.
pub const LIVE_MS: i64 = 10 * 60_000;
pub const LIVE_WITH_ACTIVE_MS: i64 = ACTIVE_BACKGROUND_MS + 5 * 60_000;
pub const ENV_REFRESH_MS: i64 = 60 * 60_000;
const CLI_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaudeSourceState {
    Disabled,
    Paused,
    NotFound,
    NotAuthenticated,
    ApiBilling,
    NotConfigured,
    Waiting,
    Live,
    Stale,
    SourceError,
    NeedsVerification,
}

impl ClaudeSourceState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Paused => "paused",
            Self::NotFound => "not_found",
            Self::NotAuthenticated => "not_authenticated",
            Self::ApiBilling => "api_billing",
            Self::NotConfigured => "not_configured",
            Self::Waiting => "waiting",
            Self::Live => "live",
            Self::Stale => "stale",
            Self::SourceError => "source_error",
            Self::NeedsVerification => "needs_verification",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveSourceState {
    pub last_attempt_ms: Option<i64>,
    pub last_success_ms: Option<i64>,
    pub consecutive_rate_limits: u32,
    pub consecutive_errors: u32,
    pub next_allowed_ms: Option<i64>,
    pub suspended: bool,
    /// The envelope showed model usage or an unknown format: disabled until QuotaDock restarts.
    pub needs_verification: bool,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeEnvironment {
    pub installation: Option<ClaudeInstallation>,
    pub version: Option<String>,
    pub auth: Option<AuthStatus>,
    pub env_flags: AuthEnvFlags,
    pub checked_at_ms: Option<i64>,
    pub error: Option<String>,
}

impl ClaudeEnvironment {
    pub fn billing(&self) -> Billing {
        if self.env_flags.non_subscription_override() {
            return Billing::ApiKey;
        }
        self.auth.as_ref().map(AuthStatus::billing).unwrap_or(Billing::Unknown)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeStatus {
    pub state: ClaudeSourceState,
    pub environment: ClaudeEnvironment,
    pub integration: Option<IntegrationState>,
    pub active_enabled: bool,
    pub active: ActiveSourceState,
    pub last_passive_ms: Option<i64>,
    pub last_active_ms: Option<i64>,
    pub last_update_ms: Option<i64>,
    pub last_source: Option<String>,
    /// The newest data is within the live window (see `LIVE_MS` / `LIVE_WITH_ACTIVE_MS`).
    pub live: bool,
    /// active | passive | unavailable
    pub refresh_capability: &'static str,
}

#[derive(Debug, Default)]
struct Inner {
    env: ClaudeEnvironment,
    child_env: Vec<(String, String)>,
    active: ActiveSourceState,
    /// Bridge records at or before this time were already ingested or ignored (pause).
    passive_seen_ms: i64,
    paused_since_ms: Option<i64>,
    /// When a bridge record was last stored (watchdog; memory only).
    last_passive_ingest_ms: Option<i64>,
}

/// In-memory view for the watchdog: no database access, so it works while the database is stuck.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeActivity {
    pub last_attempt_ms: Option<i64>,
    pub last_success_ms: Option<i64>,
    pub last_passive_ms: Option<i64>,
    pub next_allowed_ms: Option<i64>,
    /// Claude Code found, subscription billing, active source neither suspended nor disabled.
    pub active_usable: bool,
    pub suspended: bool,
    pub needs_verification: bool,
}

pub struct ClaudeMonitor {
    data_dir: PathBuf,
    home: PathBuf,
    bridge_dir: PathBuf,
    /// Test seam: use this executable instead of locating claude.exe.
    cli_override: Option<PathBuf>,
    cli_timeout: Duration,
    inner: Mutex<Inner>,
}

#[derive(Debug, Clone)]
pub struct Ingested {
    pub result: IngestResult,
    pub source: ClaudeSource,
}

impl ClaudeMonitor {
    /// `data_dir`: QuotaDock's data folder; `home`: the user profile (bridge under `~/.quotadock`).
    pub fn new(data_dir: &Path, home: &Path) -> Self {
        Self {
            data_dir: data_dir.to_path_buf(),
            home: home.to_path_buf(),
            bridge_dir: statusline::bridge_dir(home),
            cli_override: None,
            cli_timeout: CLI_TIMEOUT,
            inner: Mutex::new(Inner::default()),
        }
    }

    pub fn with_cli_override(mut self, exe: PathBuf) -> Self {
        self.cli_override = Some(exe);
        self
    }

    pub fn with_cli_timeout(mut self, timeout: Duration) -> Self {
        self.cli_timeout = timeout;
        self
    }

    pub fn bridge_dir(&self) -> &Path {
        &self.bridge_dir
    }

    fn cli_cwd(&self) -> PathBuf {
        self.data_dir.join("claude").join("cwd")
    }

    pub fn environment(&self) -> ClaudeEnvironment {
        self.inner.lock().unwrap().env.clone()
    }

    pub fn active_state(&self) -> ActiveSourceState {
        self.inner.lock().unwrap().active.clone()
    }

    pub fn activity(&self) -> ClaudeActivity {
        let i = self.inner.lock().unwrap();
        ClaudeActivity {
            last_attempt_ms: i.active.last_attempt_ms,
            last_success_ms: i.active.last_success_ms,
            last_passive_ms: i.last_passive_ingest_ms,
            next_allowed_ms: i.active.next_allowed_ms,
            active_usable: i.env.installation.is_some()
                && i.env.billing() == Billing::Subscription
                && !i.active.suspended
                && !i.active.needs_verification,
            suspended: i.active.suspended,
            needs_verification: i.active.needs_verification,
        }
    }

    pub fn environment_due(&self, now_ms: i64) -> bool {
        self.inner.lock().unwrap().env.checked_at_ms.is_none_or(|t| now_ms - t >= ENV_REFRESH_MS)
    }

    /// Locates Claude Code and reads version + auth mode. Never reads credentials.
    pub async fn refresh_environment(&self, now_ms: i64) -> ClaudeEnvironment {
        let child_env = env::claude_child_env();
        let mut out = ClaudeEnvironment { env_flags: env::auth_env_flags(&child_env), checked_at_ms: Some(now_ms), ..Default::default() };
        out.installation = match &self.cli_override {
            Some(p) => Some(ClaudeInstallation { path: p.clone(), kind: cli::InstallKind::Path }),
            None => cli::locate(&child_env),
        };
        if let Some(inst) = &out.installation {
            let cwd = self.cli_cwd();
            match cli::run(&inst.path, &["--version"], &cwd, &child_env, self.cli_timeout).await {
                Ok(o) => {
                    out.version = o
                        .stdout
                        .split_whitespace()
                        .next()
                        .map(str::to_string)
                        .filter(|v| v.chars().next().is_some_and(|c| c.is_ascii_digit()))
                }
                Err(e) => out.error = Some(e.to_string()),
            }
            match cli::run(&inst.path, &["auth", "status", "--json"], &cwd, &child_env, self.cli_timeout).await {
                Ok(o) => out.auth = usage::parse_auth_status(&o.stdout),
                Err(e) => out.error = Some(e.to_string()),
            }
        }
        let mut inner = self.inner.lock().unwrap();
        inner.child_env = child_env;
        inner.env = out.clone();
        out
    }

    pub fn set_paused(&self, paused: bool, now_ms: i64) {
        let mut inner = self.inner.lock().unwrap();
        if paused {
            inner.paused_since_ms.get_or_insert(now_ms);
        } else if inner.paused_since_ms.take().is_some() {
            // Updates that arrived while paused are not stored.
            inner.passive_seen_ms = inner.passive_seen_ms.max(now_ms);
        }
    }

    fn is_paused(&self) -> bool {
        self.inner.lock().unwrap().paused_since_ms.is_some()
    }

    /// Ingests the newest bridge record if it is new. Cheap: one small file read.
    pub fn poll_passive(&self, core: &QuotaDockCore, now_ms: i64) -> Result<Option<Ingested>, CoreError> {
        if self.is_paused() {
            return Ok(None);
        }
        let Some(rec) = statusline::read_record(&self.bridge_dir) else { return Ok(None) };
        {
            let mut inner = self.inner.lock().unwrap();
            if rec.received_at_ms <= inner.passive_seen_ms {
                return Ok(None);
            }
            inner.passive_seen_ms = rec.received_at_ms;
        }
        let Some(obs) = statusline::to_observation(&rec) else { return Ok(None) };
        // The newest valid observation wins, whatever its source: a record older than anything
        // already stored (an active read, or the same record after a restart) is not ingested.
        let stored = core.with_db(|c| store::latest(c, PROVIDER_CLAUDE))?.map(|o| o.captured_at_ms);
        if stored.is_some_and(|t| t >= obs.captured_at_ms) {
            return Ok(None);
        }
        let account = self.environment().auth.and_then(|a| a.account_fingerprint);
        let result = core.with_db_mut(|c| store::ingest(c, &obs, account.as_deref(), now_ms))?;
        self.inner.lock().unwrap().last_passive_ingest_ms = Some(now_ms);
        Ok(Some(Ingested { result, source: ClaudeSource::StatusLine }))
    }

    /// Whether the optional active source should run now.
    pub fn active_due(&self, core: &QuotaDockCore, enabled: bool, foreground: bool, manual: bool, now_ms: i64) -> Result<bool, CoreError> {
        if !enabled || self.is_paused() {
            return Ok(false);
        }
        let (env, active) = {
            let i = self.inner.lock().unwrap();
            (i.env.clone(), i.active.clone())
        };
        if active.suspended || active.needs_verification || env.installation.is_none() || env.billing() != Billing::Subscription {
            return Ok(false);
        }
        if active.next_allowed_ms.is_some_and(|t| now_ms < t) {
            return Ok(false);
        }
        if manual {
            return Ok(active.last_attempt_ms.is_none_or(|t| now_ms - t >= ACTIVE_MANUAL_MIN_GAP_MS));
        }
        let interval = if foreground { ACTIVE_FOREGROUND_MS } else { ACTIVE_BACKGROUND_MS };
        // Stale-data fallback only: fresh passive data (or a recent active read) means no query.
        let newest = core.with_db(|c| store::latest(c, PROVIDER_CLAUDE))?.map(|o| o.captured_at_ms);
        Ok(newest.is_none_or(|t| now_ms - t >= interval) && active.last_attempt_ms.is_none_or(|t| now_ms - t >= interval))
    }

    /// Runs `claude -p /usage` once (local slash command, guarded model name) and ingests it.
    pub async fn run_active(&self, core: &QuotaDockCore, now_ms: i64) -> Result<Option<Ingested>, CoreError> {
        let (inst, child_env, account) = {
            let mut i = self.inner.lock().unwrap();
            i.active.last_attempt_ms = Some(now_ms);
            (i.env.installation.clone(), i.child_env.clone(), i.env.auth.as_ref().and_then(|a| a.account_fingerprint.clone()))
        };
        let Some(inst) = inst else { return Ok(None) };
        let started = crate::time::now_ms();
        let run = cli::run(&inst.path, &usage::USAGE_ARGS, &self.cli_cwd(), &child_env, self.cli_timeout).await;
        let finished = crate::time::now_ms();
        let parsed = match run {
            Err(e) => Err(UsageQueryError::Source(e.to_string())),
            Ok(o) => usage::check_envelope(&o.stdout).and_then(|text| usage::parse_usage_text(&text, finished).map(|p| (text, p))),
        };
        match parsed {
            Ok((text, p)) => {
                let obs = ClaudeObservation {
                    source: ClaudeSource::ActiveCli,
                    captured_at_ms: finished,
                    claude_version: self.environment().version,
                    billing: p.billing,
                    windows: p.windows.clone(),
                    raw_sanitized: usage::sanitized_usage_payload(&text, &p),
                };
                let ingested = if p.billing == Billing::ApiKey || obs.windows.is_empty() {
                    None
                } else {
                    Some(core.with_db_mut(|c| store::ingest(c, &obs, account.as_deref(), finished))?)
                };
                core.with_db(|c| {
                    store::record_run(
                        c,
                        PROVIDER_CLAUDE,
                        "active_cli",
                        started,
                        finished,
                        "success",
                        None,
                        None,
                        ingested.as_ref().map(|r| r.snapshot_id),
                    )
                })?;
                let mut i = self.inner.lock().unwrap();
                i.active.last_success_ms = Some(finished);
                i.active.consecutive_rate_limits = 0;
                i.active.consecutive_errors = 0;
                i.active.next_allowed_ms = None;
                i.active.last_error = None;
                Ok(ingested.map(|result| Ingested { result, source: ClaudeSource::ActiveCli }))
            }
            Err(e) => {
                let (status, retry) = match &e {
                    UsageQueryError::RateLimited { retry_after_secs } => ("rate_limited", *retry_after_secs),
                    UsageQueryError::InferenceDetected(_) => ("inference_detected", None),
                    UsageQueryError::Unrecognized(_) => ("unrecognized", None),
                    UsageQueryError::NoUsageLines(_) => ("no_usage_lines", None),
                    UsageQueryError::Source(_) => ("error", None),
                };
                core.with_db(|c| {
                    store::record_run(c, PROVIDER_CLAUDE, "active_cli", started, finished, status, Some(&e.to_string()), retry, None)
                })?;
                let mut i = self.inner.lock().unwrap();
                i.active.last_error = Some(e.to_string());
                match e {
                    UsageQueryError::RateLimited { retry_after_secs } => {
                        i.active.consecutive_rate_limits += 1;
                        let n = i.active.consecutive_rate_limits;
                        let backoff = BACKOFF_MS[(n as usize - 1).min(BACKOFF_MS.len() - 1)];
                        let wait =
                            retry_after_secs.filter(|s| (1..=24 * 3600).contains(s)).map(|s| (s * 1000).max(backoff)).unwrap_or(backoff);
                        i.active.next_allowed_ms = Some(finished + wait);
                        if n >= SUSPEND_AFTER_429 {
                            i.active.suspended = true;
                        }
                    }
                    UsageQueryError::InferenceDetected(_) | UsageQueryError::Unrecognized(_) => i.active.needs_verification = true,
                    // Temporary failures back off (15 / 30 / 60 min) and recover on their own. A /usage
                    // text without usage lines is one of them: the envelope already proved no model
                    // use, and latching here stopped Claude monitoring for a day after a network
                    // outage (soak, 2026-10-13).
                    UsageQueryError::Source(_) | UsageQueryError::NoUsageLines(_) => {
                        i.active.consecutive_errors += 1;
                        let n = i.active.consecutive_errors as usize;
                        i.active.next_allowed_ms = Some(finished + BACKOFF_MS[(n - 1).min(BACKOFF_MS.len() - 1)]);
                    }
                }
                Ok(None)
            }
        }
    }

    pub fn integration_state(core: &QuotaDockCore) -> Result<Option<IntegrationState>, CoreError> {
        core.with_db(|c| {
            let v: Option<String> = rusqlite::OptionalExtension::optional(c.query_row(
                "SELECT value_json FROM settings WHERE key = 'claude_integration'",
                [],
                |r| r.get(0),
            ))?;
            Ok(v.and_then(|s| serde_json::from_str::<IntegrationState>(&s).ok()).filter(|s| s.enabled))
        })
    }

    pub fn save_integration_state(core: &QuotaDockCore, state: Option<&IntegrationState>, now_ms: i64) -> Result<(), CoreError> {
        core.with_db(|c| {
            match state {
                Some(s) => c.execute(
                    "INSERT INTO settings (key, value_json, updated_at_ms) VALUES ('claude_integration', ?1, ?2)
                     ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at_ms = excluded.updated_at_ms",
                    rusqlite::params![serde_json::to_string(s).unwrap_or_default(), now_ms],
                )?,
                None => c.execute("DELETE FROM settings WHERE key = 'claude_integration'", [])?,
            };
            Ok(())
        })
    }

    pub fn status(&self, core: &QuotaDockCore, enabled: bool, active_enabled: bool, now_ms: i64) -> Result<ClaudeStatus, CoreError> {
        let (env, active, paused) = {
            let i = self.inner.lock().unwrap();
            (i.env.clone(), i.active.clone(), i.paused_since_ms.is_some())
        };
        let integration = Self::integration_state(core)?;
        let latest = core.with_db(|c| store::latest(c, PROVIDER_CLAUDE))?;
        let last_passive_ms = core.with_db(|c| store::latest_capture_by_source(c, PROVIDER_CLAUDE, "statusline"))?;
        let last_active_ms = core.with_db(|c| store::latest_capture_by_source(c, PROVIDER_CLAUDE, "active_cli"))?;
        let last_update_ms = latest.as_ref().map(|l| l.captured_at_ms);
        let active_usable = active_enabled && !active.suspended && !active.needs_verification;
        let scheduled = active_usable && env.installation.is_some() && env.billing() == Billing::Subscription;
        let live_window = if scheduled { LIVE_WITH_ACTIVE_MS } else { LIVE_MS };
        let fresh = last_update_ms.is_some_and(|t| now_ms - t <= live_window);
        let state = if !enabled {
            ClaudeSourceState::Disabled
        } else if paused {
            ClaudeSourceState::Paused
        } else if env.checked_at_ms.is_some() && env.installation.is_none() && latest.is_none() {
            ClaudeSourceState::NotFound
        } else if env.billing() == Billing::ApiKey {
            ClaudeSourceState::ApiBilling
        } else if env.auth.as_ref().is_some_and(|a| !a.logged_in) {
            ClaudeSourceState::NotAuthenticated
        } else if active_enabled && active.needs_verification && integration.is_none() {
            ClaudeSourceState::NeedsVerification
        } else if fresh {
            ClaudeSourceState::Live
        } else if integration.is_none() && !active_enabled {
            ClaudeSourceState::NotConfigured
        } else if active_usable && active.consecutive_errors > 0 && latest.is_none() {
            ClaudeSourceState::SourceError
        } else if latest.is_some() {
            ClaudeSourceState::Stale
        } else {
            ClaudeSourceState::Waiting
        };
        let refresh_capability = if active_usable && env.installation.is_some() {
            "active"
        } else if integration.is_some() {
            "passive"
        } else {
            "unavailable"
        };
        Ok(ClaudeStatus {
            state,
            environment: env,
            integration,
            active_enabled,
            active,
            last_passive_ms,
            last_active_ms,
            last_update_ms,
            last_source: latest.map(|l| l.source),
            live: fresh,
            refresh_capability,
        })
    }

    /// Previews the status-line change: nothing is written.
    pub fn integration_plan(&self, home: &Path, now_ms: i64) -> Result<integration::IntegrationPlan, String> {
        let settings = claude_settings_path(home);
        let bridge = statusline::bridge_bin_dir(&self.home).join(integration::BRIDGE_EXE);
        integration::plan(&settings, &bridge, home, &self.data_dir.join("claude"), now_ms)
    }

    /// Where the bridge executable is installed (`~/.quotadock/bin`).
    pub fn bridge_exe_path(&self) -> PathBuf {
        statusline::bridge_bin_dir(&self.home).join(integration::BRIDGE_EXE)
    }
}

/// `CLAUDE_CONFIG_DIR` (user/machine scope) overrides `~/.claude`.
pub fn claude_settings_path(home: &Path) -> PathBuf {
    let configured = env::claude_child_env()
        .into_iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("CLAUDE_CONFIG_DIR"))
        .map(|(_, v)| PathBuf::from(v))
        .filter(|p| p.is_absolute());
    configured.unwrap_or_else(|| home.join(".claude")).join("settings.json")
}

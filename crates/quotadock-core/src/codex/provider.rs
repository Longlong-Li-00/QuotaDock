//! CodexProvider: discovery -> version gate -> ONE persistent stdio app-server -> read-only RPC.
//!
//! QuotaDock never touches credentials: authentication, token refresh and the backend request
//! (`GET /backend-api/wham/usage`, live on every call) are performed by the official app-server.

use super::discovery::{DiscoveryOptions, DiscoveryResult, discover};
use super::process::{AppServerProcess, InitializeResult, parse_server_version};
use super::rpc::RpcError;
use super::sanitize::{account_fingerprint, sanitize_string};
use super::types::{CodexInstallation, TransportKind, VersionGate};
use crate::time::now_ms;
use serde::Serialize;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::Duration;

/// Background-poll params: skip the reset-credit detail lookup, never opt into Luna Reserve.
pub fn background_rate_limit_params() -> Value {
    json!({ "excludeResetCreditDetails": true, "supportsLunaReserve": false })
}

/// Poll outcome categories persisted in `poll_runs.status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PollStatus {
    Success,
    NetworkError,
    AuthenticationError,
    RpcError,
    ParseError,
    ProviderUnavailable,
}

impl PollStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::NetworkError => "network_error",
            Self::AuthenticationError => "authentication_error",
            Self::RpcError => "rpc_error",
            Self::ParseError => "parse_error",
            Self::ProviderUnavailable => "provider_unavailable",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProviderError {
    pub status: PollStatus,
    pub message: String,
}

impl ProviderError {
    fn new(status: PollStatus, message: impl AsRef<str>) -> Self {
        Self { status, message: sanitize_string(message.as_ref()) }
    }
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.status.as_str(), self.message)
    }
}

/// A successful read. `account_id` lives in memory only; callers persist the fingerprint.
#[derive(Debug, Clone)]
pub struct RawRead {
    pub request_started_at_ms: i64,
    pub response_received_at_ms: i64,
    pub result: Value,
    pub cli_version: String,
    pub transport: TransportKind,
    /// From account/read (`workspaceRouting.chatgptAccountId`), memory only.
    pub active_account_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStatus {
    pub installation: Option<CodexInstallation>,
    pub selection_reason: Option<String>,
    pub transport: TransportKind,
    pub daemon_proxy: &'static str,
    pub app_server_pid: Option<u32>,
    pub app_server_version: Option<String>,
    pub account_type: Option<String>,
    pub plan_type: Option<String>,
    pub account_fingerprint: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ProviderConfig {
    pub discovery: DiscoveryOptions,
    pub request_timeout: Option<Duration>,
    /// Test seam: run this program with these args instead of `<codex> app-server --listen stdio://`.
    pub override_command: Option<(PathBuf, Vec<String>)>,
    pub extra_env: Vec<(String, String)>,
}

struct AccountInfo {
    account_type: String,
    plan_type: Option<String>,
    chatgpt_account_id: Option<String>,
}

/// One app-server start, for soak diagnostics (persisted by the pipeline in `provider_sessions`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderStart {
    pub started_at_ms: i64,
    /// `initial`: first start in this QuotaDock run; `after_stop`: QuotaDock had stopped the previous
    /// child (pause, sign-in error); `after_exit`: the previous child exited on its own.
    pub reason: &'static str,
    pub pid: Option<u32>,
    pub cli_version: String,
}

pub struct CodexProvider {
    cfg: ProviderConfig,
    discovery: Option<DiscoveryResult>,
    installation: Option<CodexInstallation>,
    process: Option<AppServerProcess>,
    account: Option<AccountInfo>,
    last_error: Option<String>,
    starts: u32,
    pending_start: Option<ProviderStart>,
}

impl CodexProvider {
    pub fn new(cfg: ProviderConfig) -> Self {
        Self { cfg, discovery: None, installation: None, process: None, account: None, last_error: None, starts: 0, pending_start: None }
    }

    /// The app-server start made by the last read, if any (taken once).
    pub fn take_start(&mut self) -> Option<ProviderStart> {
        self.pending_start.take()
    }

    pub fn discovery(&self) -> Option<&DiscoveryResult> {
        self.discovery.as_ref()
    }

    fn timeout(&self) -> Duration {
        self.cfg.request_timeout.unwrap_or(Duration::from_secs(30))
    }

    pub fn status(&mut self) -> ProviderStatus {
        let alive = self.process.as_mut().map(|p| p.is_alive()).unwrap_or(false);
        let init: Option<&InitializeResult> = if alive { self.process.as_ref().map(|p| p.initialize_result()) } else { None };
        ProviderStatus {
            installation: self.installation.clone(),
            selection_reason: self.discovery.as_ref().map(|d| d.selection_reason.clone()),
            transport: TransportKind::StdioAppServer,
            daemon_proxy: "experimental, disabled by default, unverified on a real daemon (TypeScript reference only)",
            app_server_pid: if alive { self.process.as_ref().and_then(|p| p.pid()) } else { None },
            app_server_version: init.and_then(|i| parse_server_version(&i.user_agent)),
            account_type: self.account.as_ref().map(|a| a.account_type.clone()),
            plan_type: self.account.as_ref().and_then(|a| a.plan_type.clone()),
            account_fingerprint: self.account.as_ref().and_then(|a| a.chatgpt_account_id.as_deref().map(account_fingerprint)),
            last_error: self.last_error.clone(),
        }
    }

    async fn ensure_installation(&mut self) -> Result<CodexInstallation, ProviderError> {
        if let Some(i) = &self.installation {
            return Ok(i.clone());
        }
        if let Some((program, _)) = &self.cfg.override_command {
            let i = CodexInstallation {
                path: program.clone(),
                version: super::types::VERIFIED_CODEX_VERSION.into(),
                source: super::types::InstallationSource::Manual,
                detail: "test override".into(),
                gate: VersionGate::Verified,
            };
            self.installation = Some(i.clone());
            return Ok(i);
        }
        let d = discover(&self.cfg.discovery).await;
        let selected = d.selected.clone();
        let reason = d.selection_reason.clone();
        let older = d.candidates.iter().any(|c| c.gate == VersionGate::UnsupportedOlder);
        self.discovery = Some(d);
        match selected {
            Some(i) => {
                self.installation = Some(i.clone());
                Ok(i)
            }
            None if older => {
                Err(ProviderError::new(PollStatus::ProviderUnavailable, format!("only unverified older codex versions found; {reason}")))
            }
            None => Err(ProviderError::new(PollStatus::ProviderUnavailable, reason)),
        }
    }

    async fn ensure_process(&mut self, installation: &CodexInstallation) -> Result<(), ProviderError> {
        if let Some(p) = self.process.as_mut()
            && p.is_alive()
        {
            return Ok(());
        }
        // A child we still hold but that is not alive exited on its own.
        let reason = match (&self.process, self.starts) {
            (Some(_), _) => "after_exit",
            (None, 0) => "initial",
            (None, _) => "after_stop",
        };
        let started_at_ms = now_ms();
        if let Some(old) = self.process.take() {
            old.stop().await;
        }
        let (program, args): (PathBuf, Vec<String>) = match &self.cfg.override_command {
            Some((p, a)) => (p.clone(), a.clone()),
            None => (installation.path.clone(), vec!["app-server".into(), "--listen".into(), "stdio://".into()]),
        };
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let process = AppServerProcess::start(&program, &arg_refs, &self.cfg.extra_env, Duration::from_secs(20))
            .await
            .map_err(|e| ProviderError::new(PollStatus::ProviderUnavailable, e.to_string()))?;

        // Read-only auth check on every (re)connect. `refreshToken` is never requested.
        let account = process.request("account/read", Some(json!({})), self.timeout()).await;
        let account = match account {
            Ok(v) => v,
            Err(e) => {
                process.stop().await;
                return Err(map_rpc_error(e));
            }
        };
        let acct = account.get("account").filter(|a| !a.is_null());
        let Some(acct) = acct else {
            process.stop().await;
            self.account = None;
            return Err(ProviderError::new(
                PollStatus::AuthenticationError,
                "Codex is not signed in. Sign in with the official Codex app; QuotaDock never logs in itself.",
            ));
        };
        let account_type = acct.get("type").and_then(Value::as_str).unwrap_or("unknown").to_string();
        if account_type != "chatgpt" {
            process.stop().await;
            return Err(ProviderError::new(
                PollStatus::AuthenticationError,
                format!("Codex is signed in with {account_type}; ChatGPT plan rate limits only exist for ChatGPT sign-in."),
            ));
        }
        self.account = Some(AccountInfo {
            account_type,
            plan_type: acct.get("planType").and_then(Value::as_str).map(str::to_string),
            chatgpt_account_id: account
                .get("workspaceRouting")
                .and_then(|w| w.get("chatgptAccountId"))
                .and_then(Value::as_str)
                .map(str::to_string),
        });
        self.starts += 1;
        self.pending_start = Some(ProviderStart { started_at_ms, reason, pid: process.pid(), cli_version: installation.version.clone() });
        self.process = Some(process);
        Ok(())
    }

    /// One read. Reuses the persistent child; (re)starts it only when it is not alive.
    pub async fn read(&mut self) -> Result<RawRead, ProviderError> {
        let result = self.read_inner().await;
        self.last_error = result.as_ref().err().map(|e| e.to_string());
        if let Err(e) = &result
            && e.status == PollStatus::AuthenticationError
        {
            // Start fresh next time so a sign-in done in the official app is picked up.
            if let Some(p) = self.process.take() {
                p.stop().await;
            }
        }
        result
    }

    async fn read_inner(&mut self) -> Result<RawRead, ProviderError> {
        let installation = self.ensure_installation().await?;
        self.ensure_process(&installation).await?;
        let process = self.process.as_ref().expect("process ensured");
        let request_started_at_ms = now_ms();
        let result = process
            .request("account/rateLimits/read", Some(background_rate_limit_params()), self.timeout())
            .await
            .map_err(map_rpc_error)?;
        let response_received_at_ms = now_ms();
        if !result.is_object() {
            return Err(ProviderError::new(PollStatus::ParseError, "account/rateLimits/read result is not an object"));
        }
        Ok(RawRead {
            request_started_at_ms,
            response_received_at_ms,
            result,
            cli_version: installation.version.clone(),
            transport: TransportKind::StdioAppServer,
            active_account_id: self.account.as_ref().and_then(|a| a.chatgpt_account_id.clone()),
        })
    }

    pub async fn shutdown(&mut self) {
        if let Some(p) = self.process.take() {
            p.stop().await;
        }
    }
}

/// Maps app-server errors to poll categories. Messages from
/// app-server/src/request_processors/account_processor.rs @ rust-v0.162.0-alpha.2.
pub fn map_rpc_error(e: RpcError) -> ProviderError {
    match &e {
        RpcError::Server { message, .. } if message.contains("authentication required") => {
            ProviderError::new(PollStatus::AuthenticationError, e.to_string())
        }
        RpcError::Server { message, .. } if message.contains("failed to fetch codex rate limits") => {
            ProviderError::new(PollStatus::NetworkError, e.to_string())
        }
        RpcError::Server { .. } | RpcError::MethodNotAllowed(_) => ProviderError::new(PollStatus::RpcError, e.to_string()),
        RpcError::Timeout { .. } => ProviderError::new(PollStatus::NetworkError, e.to_string()),
        RpcError::Closed(_) | RpcError::Io(_) => ProviderError::new(PollStatus::ProviderUnavailable, e.to_string()),
    }
}

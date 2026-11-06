//! Orchestration. Each stage is its own function:
//!
//! Provider.read -> normalize (validate) -> persist raw (tx 1) -> engine.step -> persist derived (tx 2)
//!
//! Failed polls never produce a snapshot (no fake 0%); they are recorded in `poll_runs`.
//! On startup, snapshots left `pending` by a crash are analysed before anything else, and the
//! engine state (including open candidates) is restored from SQLite.

use crate::analytics::reset_detector::{Engine, EngineState, StepOutput};
use crate::analytics::{AnalyticsConfig, EngineConfig};
use crate::codex::normalize::normalize_rate_limits;
use crate::codex::provider::{CodexProvider, PollStatus, ProviderConfig, RawRead};
use crate::codex::sanitize::{account_fingerprint, sanitize_string};
use crate::storage::database::{DatabaseInfo, InstanceLock, acquire_instance_lock, open_database};
use crate::storage::{derived, snapshots};
use crate::time::now_ms;
use rusqlite::Connection;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PollTrigger {
    Startup,
    Scheduled,
    Manual,
}

impl PollTrigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Startup => "startup",
            Self::Scheduled => "scheduled",
            Self::Manual => "manual",
        }
    }
}

#[derive(Debug)]
pub enum CoreError {
    Database(rusqlite::Error),
    Config(String),
}

impl std::fmt::Display for CoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Database(e) => write!(f, "database error: {e}"),
            Self::Config(e) => write!(f, "configuration error: {e}"),
        }
    }
}

impl std::error::Error for CoreError {}

impl From<rusqlite::Error> for CoreError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Database(e)
    }
}

thread_local! {
    /// Set while this thread is inside `with_db` / `with_db_mut`.
    static DB_HELD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The database mutex is not re-entrant: a `with_db` call inside another one's closure would
/// block the thread forever and, with it, every later poll and UI read. Such a call fails instead.
struct DbHeld;

impl DbHeld {
    fn enter() -> Result<Self, CoreError> {
        if DB_HELD.get() {
            debug_assert!(false, "nested with_db: the database mutex is not re-entrant");
            return Err(CoreError::Config("nested database access (would deadlock)".into()));
        }
        DB_HELD.set(true);
        Ok(Self)
    }
}

impl Drop for DbHeld {
    fn drop(&mut self) {
        DB_HELD.set(false);
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PollOutcome {
    pub status: PollStatus,
    pub snapshot_id: Option<i64>,
    pub error: Option<String>,
    pub events: Vec<crate::analytics::NewEvent>,
    pub log_lines: Vec<String>,
}

pub type LogFn = Arc<dyn Fn(&str) + Send + Sync>;

pub struct CoreConfig {
    pub database_path: PathBuf,
    pub provider: ProviderConfig,
    pub engine: EngineConfig,
    pub analytics: AnalyticsConfig,
}

impl CoreConfig {
    pub fn with_database(database_path: PathBuf) -> Self {
        Self { database_path, provider: ProviderConfig::default(), engine: EngineConfig::default(), analytics: AnalyticsConfig::default() }
    }
}

pub struct QuotaDockCore {
    _instance_lock: InstanceLock,
    db: Mutex<Connection>,
    db_info: DatabaseInfo,
    provider: tokio::sync::Mutex<CodexProvider>,
    engine: Engine,
    state: Mutex<EngineState>,
    pub analytics: AnalyticsConfig,
    log: LogFn,
}

impl QuotaDockCore {
    pub fn open(cfg: CoreConfig, log: LogFn) -> Result<Self, CoreError> {
        let lock = acquire_instance_lock(&cfg.database_path).map_err(CoreError::Config)?;
        let (conn, db_info) = open_database(&cfg.database_path)?;
        let stored_state = derived::load_engine_state(&conn)?;
        // A new classifier version has no derived state yet: rebuild it from the raw snapshots.
        // Rows of earlier classifier versions are kept (all derived tables are version-keyed).
        let needs_reanalysis = stored_state.is_none() && snapshots::snapshot_count(&conn)? > 0;
        let core = Self {
            _instance_lock: lock,
            db: Mutex::new(conn),
            db_info,
            provider: tokio::sync::Mutex::new(CodexProvider::new(cfg.provider)),
            engine: Engine::new(cfg.engine),
            state: Mutex::new(stored_state.unwrap_or_default()),
            analytics: cfg.analytics,
            log,
        };
        if needs_reanalysis {
            let n = core.reanalyze_all()?;
            (core.log)(&format!(
                "classifier {}: re-analysed {n} stored snapshot(s); earlier classifier history kept",
                crate::analytics::CLASSIFIER_VERSION
            ));
        } else {
            let recovered = core.analyze_pending()?;
            if recovered > 0 {
                (core.log)(&format!("recovered {recovered} snapshot(s) left unanalysed by a previous run"));
            }
        }
        Ok(core)
    }

    pub fn database_info(&self) -> &DatabaseInfo {
        &self.db_info
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    pub fn engine_state(&self) -> EngineState {
        self.state.lock().unwrap().clone()
    }

    /// Read-only access to the database for query commands.
    pub fn with_db<T>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Result<T, CoreError> {
        let _held = DbHeld::enter()?;
        let conn = self.db.lock().unwrap();
        Ok(f(&conn)?)
    }

    /// Watchdog probe: whether the database mutex is held right now. Never blocks.
    pub fn db_busy(&self) -> bool {
        matches!(self.db.try_lock(), Err(std::sync::TryLockError::WouldBlock))
    }

    /// Transactional access for other providers' stores (e.g. Claude observations).
    pub fn with_db_mut<T>(&self, f: impl FnOnce(&mut Connection) -> rusqlite::Result<T>) -> Result<T, CoreError> {
        let _held = DbHeld::enter()?;
        let mut conn = self.db.lock().unwrap();
        Ok(f(&mut conn)?)
    }

    pub async fn provider_status(&self) -> crate::codex::provider::ProviderStatus {
        self.provider.lock().await.status()
    }

    /// One complete poll. Holding the provider lock serialises polls (no concurrent reads).
    pub async fn poll_once(&self, trigger: PollTrigger) -> PollOutcome {
        let started = now_ms();
        let (result, start) = {
            let mut provider = self.provider.lock().await;
            let result = provider.read().await;
            (result, provider.take_start())
        };
        if let Some(s) = start {
            let conn = self.db.lock().unwrap();
            if let Err(e) = snapshots::insert_provider_start(&conn, "codex", s.started_at_ms, s.reason, s.pid, Some(&s.cli_version)) {
                (self.log)(&format!("failed to record app-server start: {e}"));
            }
        }
        match result {
            Ok(read) => match self.ingest(&read, trigger) {
                Ok((snapshot_id, out)) => PollOutcome {
                    status: PollStatus::Success,
                    snapshot_id: Some(snapshot_id),
                    error: None,
                    events: out.events,
                    log_lines: out.log_lines,
                },
                Err(IngestError::Parse(msg)) => {
                    self.record_failure(started, trigger, PollStatus::ParseError, &msg, Some(&read));
                    PollOutcome { status: PollStatus::ParseError, snapshot_id: None, error: Some(msg), events: vec![], log_lines: vec![] }
                }
                Err(IngestError::Core(e)) => {
                    let msg = e.to_string();
                    (self.log)(&format!("poll stored but analysis failed: {msg}"));
                    PollOutcome { status: PollStatus::Success, snapshot_id: None, error: Some(msg), events: vec![], log_lines: vec![] }
                }
            },
            Err(e) => {
                self.record_failure(started, trigger, e.status, &e.message, None);
                PollOutcome { status: e.status, snapshot_id: None, error: Some(e.message), events: vec![], log_lines: vec![] }
            }
        }
    }

    fn record_failure(&self, started: i64, trigger: PollTrigger, status: PollStatus, message: &str, read: Option<&RawRead>) {
        let conn = self.db.lock().unwrap();
        let r = snapshots::insert_poll_run(
            &conn,
            started,
            now_ms(),
            trigger.as_str(),
            status.as_str(),
            Some(&sanitize_string(message)),
            None,
            read.map(|r| r.cli_version.as_str()),
            read.map(|r| r.transport.as_str()),
        );
        if let Err(e) = r {
            (self.log)(&format!("failed to record poll failure: {e}"));
        }
    }

    /// Validate + normalize + persist raw (tx 1), then analyse (tx 2).
    pub fn ingest(&self, read: &RawRead, trigger: PollTrigger) -> Result<(i64, StepOutput), IngestError> {
        let normalized = normalize_rate_limits(&read.result).map_err(|e| IngestError::Parse(e.to_string()))?;
        let rate_limit_account = normalized.account_id.as_deref();
        let active_account = read.active_account_id.as_deref();
        let account_check = match (rate_limit_account, active_account) {
            (Some(a), Some(b)) if a == b => "match",
            (Some(_), Some(_)) => "mismatch",
            _ => "unknown",
        };
        // Memory only beyond this point: just the fingerprint is stored.
        let fingerprint = rate_limit_account.or(active_account).map(account_fingerprint);
        let snapshot_id = {
            let mut conn = self.db.lock().unwrap();
            let tx = conn.transaction().map_err(CoreError::from)?;
            let id = snapshots::insert_snapshot(
                &tx,
                &snapshots::SnapshotInsert {
                    request_started_at_ms: read.request_started_at_ms,
                    response_received_at_ms: read.response_received_at_ms,
                    account_fingerprint: fingerprint,
                    account_check,
                    cli_version: Some(&read.cli_version),
                    transport: read.transport.as_str(),
                    normalized: &normalized,
                    raw: &read.result,
                },
            )
            .map_err(CoreError::from)?;
            snapshots::insert_poll_run(
                &tx,
                read.request_started_at_ms,
                read.response_received_at_ms,
                trigger.as_str(),
                PollStatus::Success.as_str(),
                None,
                Some(id),
                Some(&read.cli_version),
                Some(read.transport.as_str()),
            )
            .map_err(CoreError::from)?;
            tx.commit().map_err(CoreError::from)?;
            id
        };
        let out = self.analyze_snapshot(snapshot_id).map_err(IngestError::Core)?;
        Ok((snapshot_id, out))
    }

    /// Runs the engine on one stored snapshot and commits all derived changes atomically.
    pub fn analyze_snapshot(&self, snapshot_id: i64) -> Result<StepOutput, CoreError> {
        let mut conn = self.db.lock().unwrap();
        let obs = snapshots::load_observation(&conn, snapshot_id)?;
        let current = self.state.lock().unwrap().clone();
        let (next, out) = self.engine.step(&current, &obs);
        let result = (|| -> rusqlite::Result<()> {
            let tx = conn.transaction()?;
            derived::apply_step(&tx, snapshot_id, &out, &next, now_ms())?;
            tx.commit()
        })();
        match result {
            Ok(()) => {
                *self.state.lock().unwrap() = next;
                for line in &out.log_lines {
                    (self.log)(line);
                }
                Ok(out)
            }
            Err(e) => {
                // Raw snapshot stays; in-memory state is untouched; can be re-analysed later.
                let _ = snapshots::mark_analysis_failed(&conn, snapshot_id, &e.to_string());
                Err(e.into())
            }
        }
    }

    pub fn analyze_pending(&self) -> Result<usize, CoreError> {
        let ids = self.with_db(|c| snapshots::snapshot_ids_with_status(c, "pending"))?;
        for id in &ids {
            self.analyze_snapshot(*id)?;
        }
        Ok(ids.len())
    }

    /// Re-derives everything for the current classifier version from the raw snapshots.
    pub fn reanalyze_all(&self) -> Result<usize, CoreError> {
        {
            let mut conn = self.db.lock().unwrap();
            let tx = conn.transaction()?;
            derived::reset_derived(&tx)?;
            tx.commit()?;
        }
        *self.state.lock().unwrap() = EngineState::default();
        let ids = self.with_db(snapshots::all_snapshot_ids)?;
        for id in &ids {
            self.analyze_snapshot(*id)?;
        }
        Ok(ids.len())
    }

    pub async fn shutdown(&self) {
        self.provider.lock().await.shutdown().await;
    }
}

#[derive(Debug)]
pub enum IngestError {
    Parse(String),
    Core(CoreError),
}

impl From<CoreError> for IngestError {
    fn from(e: CoreError) -> Self {
        Self::Core(e)
    }
}

impl std::fmt::Display for IngestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(m) => write!(f, "parse error: {m}"),
            Self::Core(e) => write!(f, "{e}"),
        }
    }
}

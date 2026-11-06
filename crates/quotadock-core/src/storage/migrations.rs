//! Schema migrations. Append-only: never edit a released migration, add a new version.

use rusqlite::{Connection, OptionalExtension, params};

pub struct Migration {
    pub version: i64,
    pub name: &'static str,
    pub sql: &'static str,
}

pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "phase2_core",
        sql: r#"
CREATE TABLE snapshots (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  captured_at_ms INTEGER NOT NULL,
  request_started_at_ms INTEGER NOT NULL,
  response_received_at_ms INTEGER NOT NULL,
  account_fingerprint TEXT,
  account_check TEXT NOT NULL CHECK (account_check IN ('match','mismatch','unknown')),
  cli_version TEXT,
  transport TEXT NOT NULL,
  ordinary_usage_allowed INTEGER,
  availability TEXT NOT NULL CHECK (availability IN ('allowed','blocked','unknown')),
  five_hour_present INTEGER NOT NULL,
  five_hour_used_percent REAL,
  five_hour_remaining_percent REAL,
  five_hour_reset_at_ms INTEGER,
  five_hour_duration_mins INTEGER,
  weekly_present INTEGER NOT NULL,
  weekly_used_percent REAL,
  weekly_remaining_percent REAL,
  weekly_reset_at_ms INTEGER,
  weekly_duration_mins INTEGER,
  reset_credit_count INTEGER,
  source_bucket TEXT,
  bucket_ids_json TEXT NOT NULL,
  bucket_json TEXT NOT NULL,
  raw_sanitized_json TEXT NOT NULL,
  warnings_json TEXT NOT NULL,
  quality TEXT NOT NULL,
  five_hour_quality TEXT NOT NULL,
  weekly_quality TEXT NOT NULL,
  analysis_status TEXT NOT NULL DEFAULT 'pending' CHECK (analysis_status IN ('pending','done','failed')),
  analysis_json TEXT,
  analysis_classifier_version TEXT
);
CREATE INDEX idx_snapshots_account_time ON snapshots(account_fingerprint, captured_at_ms);
CREATE INDEX idx_snapshots_analysis ON snapshots(analysis_status, id);

CREATE TABLE poll_runs (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  started_at_ms INTEGER NOT NULL,
  finished_at_ms INTEGER NOT NULL,
  poll_trigger TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('success','network_error','authentication_error','rpc_error','parse_error','provider_unavailable')),
  error_message TEXT,
  snapshot_id INTEGER REFERENCES snapshots(id) ON DELETE SET NULL,
  cli_version TEXT,
  transport TEXT
);
CREATE INDEX idx_poll_runs_time ON poll_runs(started_at_ms);

CREATE TABLE observed_cycles (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  cycle_key TEXT NOT NULL UNIQUE,
  classifier_version TEXT NOT NULL,
  account_fingerprint TEXT NOT NULL,
  window_type TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('open','closed','voided')),
  observed_started_at_ms INTEGER NOT NULL,
  observed_ended_at_ms INTEGER,
  last_seen_at_ms INTEGER NOT NULL,
  first_snapshot_id INTEGER REFERENCES snapshots(id) ON DELETE SET NULL,
  last_snapshot_id INTEGER REFERENCES snapshots(id) ON DELETE SET NULL,
  initial_used_percent REAL,
  final_used_percent REAL,
  initial_reset_at_ms INTEGER,
  final_reset_at_ms INTEGER,
  nominal_window_start_ms INTEGER,
  start_reason TEXT NOT NULL,
  end_reason TEXT,
  start_event_key TEXT,
  end_event_key TEXT,
  confidence TEXT NOT NULL
);
CREATE INDEX idx_cycles_lookup ON observed_cycles(classifier_version, account_fingerprint, window_type, status);

CREATE TABLE quota_events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  event_key TEXT NOT NULL UNIQUE,
  classifier_version TEXT NOT NULL,
  account_fingerprint TEXT,
  window_type TEXT,
  event_type TEXT NOT NULL,
  visibility TEXT NOT NULL CHECK (visibility IN ('user','diagnostic')),
  observed_at_ms INTEGER NOT NULL,
  previous_snapshot_id INTEGER REFERENCES snapshots(id) ON DELETE SET NULL,
  current_snapshot_id INTEGER REFERENCES snapshots(id) ON DELETE SET NULL,
  confidence TEXT NOT NULL CHECK (confidence IN ('low','medium','high')),
  confirmed INTEGER NOT NULL,
  evidence_json TEXT NOT NULL,
  superseded_by_event_key TEXT,
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL
);
CREATE INDEX idx_events_time ON quota_events(classifier_version, observed_at_ms);

CREATE TABLE quota_candidates (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  candidate_key TEXT NOT NULL UNIQUE,
  classifier_version TEXT NOT NULL,
  account_fingerprint TEXT NOT NULL,
  window_type TEXT,
  kind TEXT NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('pending','confirmed','rejected')),
  opened_at_ms INTEGER NOT NULL,
  opened_snapshot_id INTEGER REFERENCES snapshots(id) ON DELETE SET NULL,
  baseline_snapshot_id INTEGER REFERENCES snapshots(id) ON DELETE SET NULL,
  last_snapshot_id INTEGER REFERENCES snapshots(id) ON DELETE SET NULL,
  resolved_at_ms INTEGER,
  resolution_event_key TEXT,
  evidence_json TEXT NOT NULL,
  updated_at_ms INTEGER NOT NULL
);

CREATE TABLE window_tracks (
  classifier_version TEXT NOT NULL,
  account_fingerprint TEXT NOT NULL,
  window_type TEXT NOT NULL,
  observation_state TEXT NOT NULL,
  candidate_state TEXT NOT NULL,
  confirmed_reset_at_ms INTEGER,
  anchor_polls INTEGER NOT NULL,
  sliding_anchor INTEGER NOT NULL,
  missing_streak INTEGER NOT NULL,
  current_cycle_key TEXT,
  analysis_segment_start_snapshot_id INTEGER,
  last_snapshot_id INTEGER,
  updated_at_ms INTEGER NOT NULL,
  PRIMARY KEY (classifier_version, account_fingerprint, window_type)
);

CREATE TABLE engine_state (
  classifier_version TEXT PRIMARY KEY,
  state_json TEXT NOT NULL,
  last_snapshot_id INTEGER,
  updated_at_ms INTEGER NOT NULL
);

CREATE TABLE settings (
  key TEXT PRIMARY KEY,
  value_json TEXT NOT NULL,
  updated_at_ms INTEGER NOT NULL
);
INSERT INTO settings (key, value_json, updated_at_ms)
VALUES ('retention', '{"raw_snapshot_days":90,"cleanup_enabled":false}', 0);
"#,
    },
    Migration {
        version: 2,
        name: "phase3_notifications",
        sql: r#"
-- Persistent notification dedup: one row per (rule, provider, window, scope). The scope is the
-- observed cycle key for quota/forecast rules, the event key for event rules, and an episode key
-- for availability recovery / provider errors. A new cycle is a new scope, i.e. re-armed.
CREATE TABLE notification_state (
  rule_id TEXT NOT NULL,
  provider TEXT NOT NULL,
  window_type TEXT NOT NULL,
  scope_key TEXT NOT NULL,
  armed INTEGER NOT NULL,
  sent_at_ms INTEGER,
  updated_at_ms INTEGER NOT NULL,
  PRIMARY KEY (rule_id, provider, window_type, scope_key)
);

-- What was actually shown (or suppressed into an aggregate), for auditing dedup behaviour.
CREATE TABLE notification_log (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  created_at_ms INTEGER NOT NULL,
  provider TEXT NOT NULL,
  priority INTEGER NOT NULL,
  rule_ids_json TEXT NOT NULL,
  language TEXT NOT NULL,
  title TEXT NOT NULL,
  body TEXT NOT NULL,
  delivered INTEGER,
  error TEXT
);
CREATE INDEX idx_notification_log_time ON notification_log(created_at_ms);
"#,
    },
    Migration {
        version: 3,
        name: "phase3_provider_sessions",
        sql: r#"
-- Soak diagnostics: one row per app-server start. reason = initial (first start in a QuotaDock
-- run) | after_stop (QuotaDock had stopped it: pause, sign-in error) | after_exit (the previous
-- child exited on its own, i.e. a provider restart). cli_version shows Codex updates.
CREATE TABLE provider_sessions (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  provider TEXT NOT NULL,
  started_at_ms INTEGER NOT NULL,
  reason TEXT NOT NULL CHECK (reason IN ('initial','after_stop','after_exit')),
  pid INTEGER,
  cli_version TEXT
);
CREATE INDEX idx_provider_sessions_time ON provider_sessions(started_at_ms);
"#,
    },
    Migration {
        version: 4,
        name: "phase35_version_free_dedup_scopes",
        sql: r#"
-- Dedup scopes no longer carry the classifier version (see notifications::version_free_scope), so
-- re-analysis with reset-detector-v2 keeps alerts already sent for the same cycle/event deduplicated.
UPDATE OR IGNORE notification_state
   SET scope_key = substr(scope_key, instr(scope_key, '|') + 1)
 WHERE scope_key LIKE 'reset-detector-%|%';
"#,
    },
    Migration {
        version: 5,
        name: "phase35_provider_neutral",
        sql: r#"
-- Provider-neutral quota storage (Claude first). Codex keeps its Phase 2 tables (the Codex reset
-- engine needs their columns); nothing existing is altered or moved.
CREATE TABLE provider_snapshots (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  provider TEXT NOT NULL,
  captured_at_ms INTEGER NOT NULL,
  received_at_ms INTEGER NOT NULL,
  source TEXT NOT NULL,
  source_version TEXT,
  account_fingerprint TEXT,
  billing TEXT NOT NULL CHECK (billing IN ('subscription','api_key','unknown')),
  availability TEXT NOT NULL DEFAULT 'unknown',
  raw_sanitized_json TEXT NOT NULL
);
CREATE INDEX idx_provider_snapshots_time ON provider_snapshots(provider, captured_at_ms);

CREATE TABLE provider_windows (
  snapshot_id INTEGER NOT NULL REFERENCES provider_snapshots(id) ON DELETE CASCADE,
  window_key TEXT NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('five_hour','weekly','extra')),
  label TEXT,
  duration_mins INTEGER,
  used_percent REAL NOT NULL,
  remaining_percent REAL NOT NULL,
  resets_at_ms INTEGER,
  reset_precision_ms INTEGER NOT NULL,
  instance_key TEXT,
  PRIMARY KEY (snapshot_id, window_key)
);
CREATE INDEX idx_provider_windows_key ON provider_windows(window_key, snapshot_id);

-- One row per observed window instance (a quota period as QuotaDock saw it). Notification dedup
-- for providers without a reset engine is scoped to these instances.
CREATE TABLE provider_window_instances (
  instance_key TEXT PRIMARY KEY,
  provider TEXT NOT NULL,
  account_fingerprint TEXT,
  window_key TEXT NOT NULL,
  opened_at_ms INTEGER NOT NULL,
  first_snapshot_id INTEGER REFERENCES provider_snapshots(id) ON DELETE SET NULL,
  last_snapshot_id INTEGER REFERENCES provider_snapshots(id) ON DELETE SET NULL,
  resets_at_ms INTEGER,
  status TEXT NOT NULL CHECK (status IN ('open','closed'))
);
CREATE INDEX idx_provider_instances_open ON provider_window_instances(provider, window_key, status);

CREATE TABLE provider_events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  event_key TEXT NOT NULL UNIQUE,
  provider TEXT NOT NULL,
  window_key TEXT,
  event_type TEXT NOT NULL,
  visibility TEXT NOT NULL CHECK (visibility IN ('user','diagnostic')),
  observed_at_ms INTEGER NOT NULL,
  previous_snapshot_id INTEGER REFERENCES provider_snapshots(id) ON DELETE SET NULL,
  current_snapshot_id INTEGER REFERENCES provider_snapshots(id) ON DELETE SET NULL,
  confidence TEXT NOT NULL CHECK (confidence IN ('low','medium','high')),
  evidence_json TEXT NOT NULL,
  created_at_ms INTEGER NOT NULL
);
CREATE INDEX idx_provider_events_time ON provider_events(provider, observed_at_ms);

-- Every attempt of an active source (statusline pushes are not runs).
CREATE TABLE provider_source_runs (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  provider TEXT NOT NULL,
  source TEXT NOT NULL,
  started_at_ms INTEGER NOT NULL,
  finished_at_ms INTEGER NOT NULL,
  status TEXT NOT NULL,
  error_message TEXT,
  retry_after_secs INTEGER,
  snapshot_id INTEGER REFERENCES provider_snapshots(id) ON DELETE SET NULL
);
CREATE INDEX idx_provider_runs_time ON provider_source_runs(provider, started_at_ms);
"#,
    },
    Migration {
        version: 6,
        name: "phase36_analysis_diagnostics",
        sql: r#"
-- Diagnostics only (Windows V1 soak): rows here never feed the dashboard, risk, tray,
-- notifications or forecasts. kind = reanchor (a reset-detector-v2 idle -> active re-anchor seen
-- after a poll) | shadow_estimate (production 5h burn vs. average since the derived origin vs.
-- recent 30-min slope).
CREATE TABLE analysis_diagnostics (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  created_at_ms INTEGER NOT NULL,
  kind TEXT NOT NULL,
  window_type TEXT,
  snapshot_id INTEGER REFERENCES snapshots(id) ON DELETE SET NULL,
  data_json TEXT NOT NULL
);
CREATE INDEX idx_analysis_diagnostics_kind ON analysis_diagnostics(kind, created_at_ms);
"#,
    },
];

pub fn current_version(conn: &Connection) -> rusqlite::Result<i64> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (version INTEGER PRIMARY KEY, name TEXT NOT NULL, applied_at_ms INTEGER NOT NULL);",
    )?;
    Ok(conn.query_row("SELECT MAX(version) FROM schema_migrations", [], |r| r.get::<_, Option<i64>>(0)).optional()?.flatten().unwrap_or(0))
}

/// Applies every pending migration, each in its own transaction. Idempotent.
pub fn migrate(conn: &mut Connection) -> rusqlite::Result<i64> {
    let start = current_version(conn)?;
    let mut version = start;
    for m in MIGRATIONS.iter().filter(|m| m.version > start) {
        let tx = conn.transaction()?;
        tx.execute_batch(m.sql)?;
        tx.execute(
            "INSERT INTO schema_migrations (version, name, applied_at_ms) VALUES (?1, ?2, ?3)",
            params![m.version, m.name, crate::time::now_ms()],
        )?;
        tx.commit()?;
        version = m.version;
    }
    Ok(version)
}

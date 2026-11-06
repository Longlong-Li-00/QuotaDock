//! SQLite persistence for provider-neutral observations (migration v5).

use super::model::*;
use super::tracker::{self, OpenInstance, TrackOutput};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredWindow {
    pub window_key: String,
    pub kind: String,
    pub label: Option<String>,
    pub duration_mins: Option<i64>,
    pub used_percent: f64,
    pub remaining_percent: f64,
    pub resets_at_ms: Option<i64>,
    pub reset_precision_ms: i64,
    pub instance_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredObservation {
    pub snapshot_id: i64,
    pub captured_at_ms: i64,
    pub received_at_ms: i64,
    pub source: String,
    pub source_version: Option<String>,
    pub billing: String,
    pub windows: Vec<StoredWindow>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct IngestResult {
    pub snapshot_id: i64,
    pub track: TrackOutput,
    /// Event ids inserted by this observation.
    pub event_ids: Vec<i64>,
}

pub fn open_instances(conn: &Connection, provider: &str, account: Option<&str>) -> rusqlite::Result<Vec<OpenInstance>> {
    let mut stmt = conn.prepare(
        "SELECT i.instance_key, i.window_key, i.resets_at_ms,
                (SELECT used_percent FROM provider_windows w WHERE w.instance_key = i.instance_key ORDER BY w.snapshot_id DESC LIMIT 1),
                EXISTS (SELECT 1 FROM provider_events e WHERE e.event_type = 'limit_reached_observed'
                        AND e.event_key LIKE i.instance_key || '|%')
         FROM provider_window_instances i
         WHERE i.provider = ?1 AND i.status = 'open' AND (i.account_fingerprint IS ?2)",
    )?;
    stmt.query_map(params![provider, account], |r| {
        Ok(OpenInstance {
            instance_key: r.get(0)?,
            window_key: r.get(1)?,
            resets_at_ms: r.get(2)?,
            last_used_percent: r.get(3)?,
            limit_reported: r.get(4)?,
        })
    })?
    .collect()
}

/// Stores one observation and its derived instance/event changes atomically. Instances of another
/// account are closed first (identity changes never mix quota periods).
pub fn ingest(conn: &mut Connection, obs: &ClaudeObservation, account: Option<&str>, now_ms: i64) -> rusqlite::Result<IngestResult> {
    let tx = conn.transaction()?;
    tx.execute(
        "UPDATE provider_window_instances SET status = 'closed' WHERE provider = ?1 AND status = 'open' AND NOT (account_fingerprint IS ?2)",
        params![PROVIDER_CLAUDE, account],
    )?;
    tx.execute(
        "INSERT INTO provider_snapshots (provider, captured_at_ms, received_at_ms, source, source_version, account_fingerprint, billing, availability, raw_sanitized_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'unknown', ?8)",
        params![
            PROVIDER_CLAUDE,
            obs.captured_at_ms,
            now_ms,
            obs.source.as_str(),
            obs.claude_version,
            account,
            obs.billing.as_str(),
            crate::codex::sanitize::sanitize_value(&obs.raw_sanitized).to_string()
        ],
    )?;
    let snapshot_id = tx.last_insert_rowid();
    let open = open_instances(&tx, PROVIDER_CLAUDE, account)?;
    let t = tracker::track(&open, obs, account, snapshot_id);
    for key in &t.closed {
        tx.execute("UPDATE provider_window_instances SET status = 'closed' WHERE instance_key = ?1", params![key])?;
    }
    for (window_key, key, _reason) in &t.opened {
        let resets = obs.windows.iter().find(|w| &w.key == window_key).and_then(|w| w.resets_at_ms);
        tx.execute(
            "INSERT OR IGNORE INTO provider_window_instances (instance_key, provider, account_fingerprint, window_key, opened_at_ms, first_snapshot_id, last_snapshot_id, resets_at_ms, status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7, 'open')",
            params![key, PROVIDER_CLAUDE, account, window_key, obs.captured_at_ms, snapshot_id, resets],
        )?;
    }
    for (key, resets) in &t.reset_updates {
        tx.execute("UPDATE provider_window_instances SET resets_at_ms = ?2 WHERE instance_key = ?1", params![key, resets])?;
    }
    for w in &obs.windows {
        let instance = t.instance_for.iter().find(|(k, _)| k == &w.key).map(|(_, i)| i.clone());
        tx.execute(
            "INSERT INTO provider_windows (snapshot_id, window_key, kind, label, duration_mins, used_percent, remaining_percent, resets_at_ms, reset_precision_ms, instance_key)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                snapshot_id,
                w.key,
                w.kind.as_str(),
                w.label,
                w.duration_mins,
                w.used_percent,
                w.remaining_percent(),
                w.resets_at_ms,
                w.reset_precision_ms,
                instance
            ],
        )?;
        if let Some(i) = &instance {
            tx.execute("UPDATE provider_window_instances SET last_snapshot_id = ?2 WHERE instance_key = ?1", params![i, snapshot_id])?;
        }
    }
    let mut event_ids = Vec::new();
    for e in &t.events {
        let scope = e.window_key.as_ref().and_then(|wk| t.instance_for.iter().find(|(k, _)| k == wk).map(|(_, i)| i.clone()));
        let key = format!(
            "{}|{}|{snapshot_id}",
            scope.unwrap_or_else(|| format!("claude|{}", e.window_key.clone().unwrap_or_default())),
            e.event_type
        );
        let previous: Option<i64> = tx.query_row(
            "SELECT MAX(id) FROM provider_snapshots WHERE provider = ?1 AND id < ?2",
            params![PROVIDER_CLAUDE, snapshot_id],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO provider_events (event_key, provider, window_key, event_type, visibility, observed_at_ms, previous_snapshot_id, current_snapshot_id, confidence, evidence_json, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![key, PROVIDER_CLAUDE, e.window_key, e.event_type, e.visibility, obs.captured_at_ms, previous, snapshot_id, e.confidence, e.evidence.to_string(), now_ms],
        )?;
        if tx.changes() > 0 {
            event_ids.push(tx.last_insert_rowid());
        }
    }
    tx.commit()?;
    Ok(IngestResult { snapshot_id, track: t, event_ids })
}

fn windows_of(conn: &Connection, snapshot_id: i64) -> rusqlite::Result<Vec<StoredWindow>> {
    let mut stmt = conn.prepare(
        "SELECT window_key, kind, label, duration_mins, used_percent, remaining_percent, resets_at_ms, reset_precision_ms, instance_key
         FROM provider_windows WHERE snapshot_id = ?1
         ORDER BY CASE kind WHEN 'five_hour' THEN 0 WHEN 'weekly' THEN 1 ELSE 2 END, window_key",
    )?;
    stmt.query_map(params![snapshot_id], |r| {
        Ok(StoredWindow {
            window_key: r.get(0)?,
            kind: r.get(1)?,
            label: r.get(2)?,
            duration_mins: r.get(3)?,
            used_percent: r.get(4)?,
            remaining_percent: r.get(5)?,
            resets_at_ms: r.get(6)?,
            reset_precision_ms: r.get(7)?,
            instance_key: r.get(8)?,
        })
    })?
    .collect()
}

/// The newest observation (by source capture time).
pub fn latest(conn: &Connection, provider: &str) -> rusqlite::Result<Option<StoredObservation>> {
    let row = conn
        .query_row(
            "SELECT id, captured_at_ms, received_at_ms, source, source_version, billing FROM provider_snapshots
             WHERE provider = ?1 ORDER BY captured_at_ms DESC, id DESC LIMIT 1",
            params![provider],
            |r| Ok((r.get::<_, i64>(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .optional()?;
    let Some((id, captured_at_ms, received_at_ms, source, source_version, billing)) = row else { return Ok(None) };
    Ok(Some(StoredObservation {
        snapshot_id: id,
        captured_at_ms,
        received_at_ms,
        source,
        source_version,
        billing,
        windows: windows_of(conn, id)?,
    }))
}

/// The newest capture time per source (for freshness / active-query skipping).
pub fn latest_capture_by_source(conn: &Connection, provider: &str, source: &str) -> rusqlite::Result<Option<i64>> {
    conn.query_row(
        "SELECT MAX(captured_at_ms) FROM provider_snapshots WHERE provider = ?1 AND source = ?2",
        params![provider, source],
        |r| r.get(0),
    )
}

/// (captured_at_ms, used_percent, instance_key) for one window, oldest first.
pub fn window_series(
    conn: &Connection,
    provider: &str,
    window_key: &str,
    from_ms: i64,
) -> rusqlite::Result<Vec<(i64, f64, Option<String>)>> {
    let mut stmt = conn.prepare(
        "SELECT s.captured_at_ms, w.used_percent, w.instance_key FROM provider_windows w JOIN provider_snapshots s ON s.id = w.snapshot_id
         WHERE s.provider = ?1 AND w.window_key = ?2 AND s.captured_at_ms >= ?3 ORDER BY s.captured_at_ms ASC, s.id ASC",
    )?;
    stmt.query_map(params![provider, window_key, from_ms], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect()
}

pub fn instance_opened_at(conn: &Connection, instance_key: &str) -> rusqlite::Result<Option<i64>> {
    conn.query_row("SELECT opened_at_ms FROM provider_window_instances WHERE instance_key = ?1", params![instance_key], |r| r.get(0))
        .optional()
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderEventRow {
    pub id: i64,
    pub provider: String,
    pub window_key: Option<String>,
    pub event_type: String,
    pub visibility: String,
    pub observed_at_ms: i64,
    pub confidence: String,
    pub evidence: serde_json::Value,
}

pub fn events(conn: &Connection, provider: &str, include_diagnostic: bool, limit: usize) -> rusqlite::Result<Vec<ProviderEventRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, provider, window_key, event_type, visibility, observed_at_ms, confidence, evidence_json FROM provider_events
         WHERE provider = ?1 AND (?2 OR visibility = 'user') ORDER BY observed_at_ms DESC, id DESC LIMIT ?3",
    )?;
    stmt.query_map(params![provider, include_diagnostic, limit as i64], |r| {
        Ok(ProviderEventRow {
            id: r.get(0)?,
            provider: r.get(1)?,
            window_key: r.get(2)?,
            event_type: r.get(3)?,
            visibility: r.get(4)?,
            observed_at_ms: r.get(5)?,
            confidence: r.get(6)?,
            evidence: serde_json::from_str(&r.get::<_, String>(7)?).unwrap_or_default(),
        })
    })?
    .collect()
}

pub fn events_by_ids(conn: &Connection, ids: &[i64]) -> rusqlite::Result<Vec<ProviderEventRow>> {
    let mut out = Vec::new();
    for id in ids {
        let row = conn
            .query_row(
                "SELECT id, provider, window_key, event_type, visibility, observed_at_ms, confidence, evidence_json FROM provider_events WHERE id = ?1",
                params![id],
                |r| {
                    Ok(ProviderEventRow {
                        id: r.get(0)?,
                        provider: r.get(1)?,
                        window_key: r.get(2)?,
                        event_type: r.get(3)?,
                        visibility: r.get(4)?,
                        observed_at_ms: r.get(5)?,
                        confidence: r.get(6)?,
                        evidence: serde_json::from_str(&r.get::<_, String>(7)?).unwrap_or_default(),
                    })
                },
            )
            .optional()?;
        out.extend(row);
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRun {
    pub started_at_ms: i64,
    pub finished_at_ms: i64,
    pub status: String,
    pub error: Option<String>,
    pub retry_after_secs: Option<i64>,
}

#[allow(clippy::too_many_arguments)] // one argument per persisted column
pub fn record_run(
    conn: &Connection,
    provider: &str,
    source: &str,
    started: i64,
    finished: i64,
    status: &str,
    error: Option<&str>,
    retry_after_secs: Option<i64>,
    snapshot_id: Option<i64>,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO provider_source_runs (provider, source, started_at_ms, finished_at_ms, status, error_message, retry_after_secs, snapshot_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![provider, source, started, finished, status, error.map(crate::codex::sanitize::sanitize_string), retry_after_secs, snapshot_id],
    )?;
    Ok(())
}

pub fn recent_runs(conn: &Connection, provider: &str, limit: usize) -> rusqlite::Result<Vec<SourceRun>> {
    let mut stmt = conn.prepare(
        "SELECT started_at_ms, finished_at_ms, status, error_message, retry_after_secs FROM provider_source_runs
         WHERE provider = ?1 ORDER BY id DESC LIMIT ?2",
    )?;
    stmt.query_map(params![provider, limit as i64], |r| {
        Ok(SourceRun {
            started_at_ms: r.get(0)?,
            finished_at_ms: r.get(1)?,
            status: r.get(2)?,
            error: r.get(3)?,
            retry_after_secs: r.get(4)?,
        })
    })?
    .collect()
}

/// Claude events as `EventView`s for the merged timeline (id negated to stay unique). See
/// `derived::events_before` for `hide_five_hour`; `weekly_model:<name>` windows are not affected.
pub fn event_views_before(
    conn: &Connection,
    include_diagnostic: bool,
    before_ms: Option<i64>,
    limit: usize,
    hide_five_hour: bool,
) -> rusqlite::Result<Vec<crate::storage::derived::EventView>> {
    let mut stmt = conn.prepare(
        "SELECT id, event_key, window_key, event_type, visibility, observed_at_ms, previous_snapshot_id, current_snapshot_id, confidence, evidence_json
         FROM provider_events WHERE provider = ?1 AND (?2 OR visibility = 'user') AND (?4 IS NULL OR observed_at_ms < ?4)
           AND (NOT ?5 OR window_key IS NOT 'five_hour')
         ORDER BY observed_at_ms DESC, id DESC LIMIT ?3",
    )?;
    stmt.query_map(params![PROVIDER_CLAUDE, include_diagnostic, limit as i64, before_ms, hide_five_hour], |r| {
        Ok(crate::storage::derived::EventView {
            id: -r.get::<_, i64>(0)?,
            provider: PROVIDER_CLAUDE.into(),
            event_key: r.get(1)?,
            classifier_version: "claude-observer-v1".into(),
            window_type: r.get(2)?,
            event_type: r.get(3)?,
            visibility: r.get(4)?,
            observed_at_ms: r.get(5)?,
            previous_snapshot_id: r.get(6)?,
            current_snapshot_id: r.get(7)?,
            confidence: r.get(8)?,
            confirmed: true,
            evidence: serde_json::from_str(&r.get::<_, String>(9)?).unwrap_or_default(),
            superseded_by_event_key: None,
        })
    })?
    .collect()
}

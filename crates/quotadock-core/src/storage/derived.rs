//! Derived state written by the engine: events, cycles, candidates, qualities, tracks, engine state.
//! Everything here is re-derivable from raw snapshots (see `reset_derived`).

use crate::analytics::CLASSIFIER_VERSION;
use crate::analytics::model::*;
use crate::analytics::reset_detector::{EngineState, StepOutput};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::Serialize;

/// Applies one engine step atomically (caller owns the transaction).
pub fn apply_step(tx: &Transaction, snapshot_id: i64, out: &StepOutput, state: &EngineState, now_ms: i64) -> rusqlite::Result<()> {
    for e in &out.events {
        tx.execute(
            "INSERT INTO quota_events (event_key, classifier_version, account_fingerprint, window_type, event_type, visibility,
                observed_at_ms, previous_snapshot_id, current_snapshot_id, confidence, confirmed, evidence_json,
                superseded_by_event_key, created_at_ms, updated_at_ms)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,NULL,?13,?13)
             ON CONFLICT(event_key) DO UPDATE SET
                observed_at_ms = excluded.observed_at_ms, current_snapshot_id = excluded.current_snapshot_id,
                confidence = excluded.confidence, confirmed = excluded.confirmed, evidence_json = excluded.evidence_json,
                updated_at_ms = excluded.updated_at_ms",
            params![
                e.event_key,
                CLASSIFIER_VERSION,
                e.account_fingerprint,
                e.window_type.map(|w| w.as_str()),
                e.event_type.as_str(),
                e.event_type.visibility().as_str(),
                e.observed_at_ms,
                e.previous_snapshot_id,
                e.current_snapshot_id,
                e.confidence.as_str(),
                e.confirmed,
                e.evidence.to_string(),
                now_ms,
            ],
        )?;
        if let Some(old) = &e.supersedes_event_key {
            tx.execute(
                "UPDATE quota_events SET superseded_by_event_key = ?2, updated_at_ms = ?3 WHERE event_key = ?1",
                params![old, e.event_key, now_ms],
            )?;
        }
    }
    for op in &out.cycle_ops {
        match op {
            CycleOp::Open {
                cycle_key,
                account_fingerprint,
                window_type,
                observed_started_at_ms,
                first_snapshot_id,
                initial_used_percent,
                initial_reset_at_ms,
                start_reason,
                start_event_key,
                confidence,
            } => {
                tx.execute(
                    "INSERT INTO observed_cycles (cycle_key, classifier_version, account_fingerprint, window_type, status,
                        observed_started_at_ms, last_seen_at_ms, first_snapshot_id, last_snapshot_id, initial_used_percent,
                        final_used_percent, initial_reset_at_ms, final_reset_at_ms, start_reason, start_event_key, confidence)
                     VALUES (?1,?2,?3,?4,'open',?5,?5,?6,?6,?7,?7,?8,?8,?9,?10,?11)
                     ON CONFLICT(cycle_key) DO NOTHING",
                    params![
                        cycle_key,
                        CLASSIFIER_VERSION,
                        account_fingerprint,
                        window_type.as_str(),
                        observed_started_at_ms,
                        first_snapshot_id,
                        initial_used_percent,
                        initial_reset_at_ms,
                        start_reason,
                        start_event_key,
                        confidence.as_str(),
                    ],
                )?;
            }
            CycleOp::Extend {
                cycle_key,
                last_snapshot_id,
                last_seen_at_ms,
                final_used_percent,
                final_reset_at_ms,
                nominal_window_start_ms,
            } => {
                tx.execute(
                    "UPDATE observed_cycles SET last_snapshot_id = ?2, last_seen_at_ms = ?3, final_used_percent = ?4,
                        final_reset_at_ms = ?5, nominal_window_start_ms = COALESCE(?6, nominal_window_start_ms)
                     WHERE cycle_key = ?1 AND status = 'open'",
                    params![cycle_key, last_snapshot_id, last_seen_at_ms, final_used_percent, final_reset_at_ms, nominal_window_start_ms],
                )?;
            }
            CycleOp::Close { cycle_key, observed_ended_at_ms, end_reason, end_event_key } => {
                tx.execute(
                    "UPDATE observed_cycles SET status = 'closed', observed_ended_at_ms = ?2, end_reason = ?3, end_event_key = ?4
                     WHERE cycle_key = ?1 AND status = 'open'",
                    params![cycle_key, observed_ended_at_ms, end_reason, end_event_key],
                )?;
            }
            CycleOp::Void { cycle_key, reason } => {
                tx.execute(
                    "UPDATE observed_cycles SET status = 'voided', end_reason = ?2 WHERE cycle_key = ?1",
                    params![cycle_key, reason],
                )?;
            }
            CycleOp::Reopen { cycle_key } => {
                tx.execute(
                    "UPDATE observed_cycles SET status = 'open', observed_ended_at_ms = NULL, end_reason = NULL, end_event_key = NULL
                     WHERE cycle_key = ?1",
                    params![cycle_key],
                )?;
            }
        }
    }
    for q in &out.quality_updates {
        let col = match q.window_type {
            WindowType::FiveHour => "five_hour_quality",
            WindowType::Weekly => "weekly_quality",
        };
        tx.execute(
            &format!("UPDATE snapshots SET {col} = ?2 WHERE id = ?1 AND quality != 'account_mismatch'"),
            params![q.snapshot_id, q.quality.as_str()],
        )?;
        recompute_overall_quality(tx, q.snapshot_id)?;
    }
    for c in &out.candidates {
        tx.execute(
            "INSERT INTO quota_candidates (candidate_key, classifier_version, account_fingerprint, window_type, kind, state,
                opened_at_ms, opened_snapshot_id, baseline_snapshot_id, last_snapshot_id, resolved_at_ms, resolution_event_key,
                evidence_json, updated_at_ms)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)
             ON CONFLICT(candidate_key) DO UPDATE SET state = excluded.state, last_snapshot_id = excluded.last_snapshot_id,
                resolved_at_ms = excluded.resolved_at_ms, resolution_event_key = excluded.resolution_event_key,
                updated_at_ms = excluded.updated_at_ms",
            params![
                c.candidate_key,
                CLASSIFIER_VERSION,
                c.account_fingerprint,
                c.window_type.map(|w| w.as_str()),
                c.kind,
                c.state.as_str(),
                c.opened_at_ms,
                c.opened_snapshot_id,
                c.baseline_snapshot_id,
                c.last_snapshot_id,
                c.resolved_at_ms,
                c.resolution_event_key,
                c.evidence.to_string(),
                now_ms,
            ],
        )?;
    }
    for t in &out.tracks {
        tx.execute(
            "INSERT INTO window_tracks (classifier_version, account_fingerprint, window_type, observation_state, candidate_state,
                confirmed_reset_at_ms, anchor_polls, sliding_anchor, missing_streak, current_cycle_key,
                analysis_segment_start_snapshot_id, last_snapshot_id, updated_at_ms)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)
             ON CONFLICT(classifier_version, account_fingerprint, window_type) DO UPDATE SET
                observation_state = excluded.observation_state, candidate_state = excluded.candidate_state,
                confirmed_reset_at_ms = excluded.confirmed_reset_at_ms, anchor_polls = excluded.anchor_polls,
                sliding_anchor = excluded.sliding_anchor, missing_streak = excluded.missing_streak,
                current_cycle_key = excluded.current_cycle_key,
                analysis_segment_start_snapshot_id = excluded.analysis_segment_start_snapshot_id,
                last_snapshot_id = excluded.last_snapshot_id, updated_at_ms = excluded.updated_at_ms",
            params![
                CLASSIFIER_VERSION,
                t.account_fingerprint,
                t.window_type.as_str(),
                t.observation_state.as_str(),
                t.candidate_state.as_str(),
                t.confirmed_reset_at_ms,
                t.anchor_polls,
                t.sliding_anchor,
                t.missing_streak,
                t.current_cycle_key,
                t.analysis_segment_start_snapshot_id,
                t.last_snapshot_id,
                now_ms,
            ],
        )?;
    }
    tx.execute(
        "INSERT INTO engine_state (classifier_version, state_json, last_snapshot_id, updated_at_ms) VALUES (?1,?2,?3,?4)
         ON CONFLICT(classifier_version) DO UPDATE SET state_json = excluded.state_json,
            last_snapshot_id = excluded.last_snapshot_id, updated_at_ms = excluded.updated_at_ms",
        params![CLASSIFIER_VERSION, serde_json::to_string(state).unwrap_or_default(), state.last_snapshot_id, now_ms],
    )?;
    tx.execute(
        "UPDATE snapshots SET analysis_status = 'done', analysis_json = ?2, analysis_classifier_version = ?3 WHERE id = ?1",
        params![snapshot_id, out.analysis.to_string(), CLASSIFIER_VERSION],
    )?;
    Ok(())
}

fn recompute_overall_quality(tx: &Transaction, id: i64) -> rusqlite::Result<()> {
    tx.execute(
        "UPDATE snapshots SET quality = CASE
            WHEN quality = 'account_mismatch' THEN 'account_mismatch'
            WHEN five_hour_quality = 'suspect_transient' OR weekly_quality = 'suspect_transient' THEN 'suspect_transient'
            WHEN five_hour_quality = 'pending_confirmation' OR weekly_quality = 'pending_confirmation' THEN 'pending_confirmation'
            WHEN five_hour_quality = 'missing_window' OR weekly_quality = 'missing_window' THEN 'missing_window'
            ELSE 'normal' END
         WHERE id = ?1",
        params![id],
    )?;
    Ok(())
}

pub fn load_engine_state(conn: &Connection) -> rusqlite::Result<Option<EngineState>> {
    let json: Option<String> = conn
        .query_row("SELECT state_json FROM engine_state WHERE classifier_version = ?1", params![CLASSIFIER_VERSION], |r| r.get(0))
        .optional()?;
    Ok(json.and_then(|j| serde_json::from_str(&j).ok()))
}

/// Drops everything the current classifier derived and resets snapshot qualities, so that all raw
/// snapshots can be replayed. Raw columns are untouched.
pub fn reset_derived(tx: &Transaction) -> rusqlite::Result<()> {
    for table in ["quota_events", "observed_cycles", "quota_candidates", "window_tracks", "engine_state"] {
        tx.execute(&format!("DELETE FROM {table} WHERE classifier_version = ?1"), params![CLASSIFIER_VERSION])?;
    }
    tx.execute_batch(
        "UPDATE snapshots SET
            five_hour_quality = CASE WHEN quality = 'account_mismatch' THEN 'account_mismatch'
                                     WHEN five_hour_present = 1 THEN 'normal' ELSE 'missing_window' END,
            weekly_quality = CASE WHEN quality = 'account_mismatch' THEN 'account_mismatch'
                                  WHEN weekly_present = 1 THEN 'normal' ELSE 'missing_window' END,
            analysis_status = 'pending', analysis_json = NULL, analysis_classifier_version = NULL;
         UPDATE snapshots SET quality = CASE
            WHEN quality = 'account_mismatch' THEN 'account_mismatch'
            WHEN five_hour_quality = 'missing_window' OR weekly_quality = 'missing_window' THEN 'missing_window'
            ELSE 'normal' END;",
    )?;
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventView {
    pub id: i64,
    /// "codex" (quota_events) or "claude" (provider_events; id is negated to stay unique).
    pub provider: String,
    pub event_key: String,
    pub classifier_version: String,
    pub window_type: Option<String>,
    pub event_type: String,
    pub visibility: String,
    pub observed_at_ms: i64,
    pub previous_snapshot_id: Option<i64>,
    pub current_snapshot_id: Option<i64>,
    pub confidence: String,
    pub confirmed: bool,
    pub evidence: serde_json::Value,
    pub superseded_by_event_key: Option<String>,
}

pub fn events(conn: &Connection, include_diagnostic: bool, limit: usize) -> rusqlite::Result<Vec<EventView>> {
    events_page(conn, include_diagnostic, None, limit)
}

/// Newest first; `before_id` continues a previous page (keyset pagination on id).
pub fn events_page(conn: &Connection, include_diagnostic: bool, before_id: Option<i64>, limit: usize) -> rusqlite::Result<Vec<EventView>> {
    let mut stmt = conn.prepare(
        "SELECT id, event_key, classifier_version, window_type, event_type, visibility, observed_at_ms, previous_snapshot_id,
                current_snapshot_id, confidence, confirmed, evidence_json, superseded_by_event_key
         FROM quota_events WHERE classifier_version = ?1 AND (?2 OR visibility = 'user') AND (?4 IS NULL OR id < ?4)
         ORDER BY id DESC LIMIT ?3",
    )?;
    let rows = stmt
        .query_map(params![CLASSIFIER_VERSION, include_diagnostic, limit as i64, before_id], |r| {
            let ev: String = r.get(11)?;
            Ok(EventView {
                id: r.get(0)?,
                provider: "codex".into(),
                event_key: r.get(1)?,
                classifier_version: r.get(2)?,
                window_type: r.get(3)?,
                event_type: r.get(4)?,
                visibility: r.get(5)?,
                observed_at_ms: r.get(6)?,
                previous_snapshot_id: r.get(7)?,
                current_snapshot_id: r.get(8)?,
                confidence: r.get(9)?,
                confirmed: r.get(10)?,
                evidence: serde_json::from_str(&ev).unwrap_or(serde_json::Value::Null),
                superseded_by_event_key: r.get(12)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Newest first by observation time; `before_ms` continues a previous page (exclusive). Used to
/// merge providers on one timeline. `hide_five_hour` filters in SQL, so a page stays full ("load
/// more" depends on it); `IS NOT` keeps events without a window, such as a redeemed reset credit.
pub fn events_before(
    conn: &Connection,
    include_diagnostic: bool,
    before_ms: Option<i64>,
    limit: usize,
    hide_five_hour: bool,
) -> rusqlite::Result<Vec<EventView>> {
    let mut stmt = conn.prepare(
        "SELECT id, event_key, classifier_version, window_type, event_type, visibility, observed_at_ms, previous_snapshot_id,
                current_snapshot_id, confidence, confirmed, evidence_json, superseded_by_event_key
         FROM quota_events WHERE classifier_version = ?1 AND (?2 OR visibility = 'user') AND (?4 IS NULL OR observed_at_ms < ?4)
           AND (NOT ?5 OR window_type IS NOT 'five_hour')
         ORDER BY observed_at_ms DESC, id DESC LIMIT ?3",
    )?;
    let rows = stmt
        .query_map(params![CLASSIFIER_VERSION, include_diagnostic, limit as i64, before_ms, hide_five_hour], |r| {
            let ev: String = r.get(11)?;
            Ok(EventView {
                id: r.get(0)?,
                provider: "codex".into(),
                event_key: r.get(1)?,
                classifier_version: r.get(2)?,
                window_type: r.get(3)?,
                event_type: r.get(4)?,
                visibility: r.get(5)?,
                observed_at_ms: r.get(6)?,
                previous_snapshot_id: r.get(7)?,
                current_snapshot_id: r.get(8)?,
                confidence: r.get(9)?,
                confirmed: r.get(10)?,
                evidence: serde_json::from_str(&ev).unwrap_or(serde_json::Value::Null),
                superseded_by_event_key: r.get(12)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CycleView {
    pub cycle_key: String,
    pub window_type: String,
    pub status: String,
    pub observed_started_at_ms: i64,
    pub observed_ended_at_ms: Option<i64>,
    pub last_seen_at_ms: i64,
    pub first_snapshot_id: Option<i64>,
    pub last_snapshot_id: Option<i64>,
    pub initial_used_percent: Option<f64>,
    pub final_used_percent: Option<f64>,
    pub initial_reset_at_ms: Option<i64>,
    pub final_reset_at_ms: Option<i64>,
    /// Derived from a confirmed anchor (reset - duration); never authoritative.
    pub nominal_window_start_ms: Option<i64>,
    pub start_reason: String,
    pub end_reason: Option<String>,
    pub confidence: String,
}

fn cycle_view(r: &rusqlite::Row) -> rusqlite::Result<CycleView> {
    Ok(CycleView {
        cycle_key: r.get("cycle_key")?,
        window_type: r.get("window_type")?,
        status: r.get("status")?,
        observed_started_at_ms: r.get("observed_started_at_ms")?,
        observed_ended_at_ms: r.get("observed_ended_at_ms")?,
        last_seen_at_ms: r.get("last_seen_at_ms")?,
        first_snapshot_id: r.get("first_snapshot_id")?,
        last_snapshot_id: r.get("last_snapshot_id")?,
        initial_used_percent: r.get("initial_used_percent")?,
        final_used_percent: r.get("final_used_percent")?,
        initial_reset_at_ms: r.get("initial_reset_at_ms")?,
        final_reset_at_ms: r.get("final_reset_at_ms")?,
        nominal_window_start_ms: r.get("nominal_window_start_ms")?,
        start_reason: r.get("start_reason")?,
        end_reason: r.get("end_reason")?,
        confidence: r.get("confidence")?,
    })
}

pub fn cycles(conn: &Connection, account: &str, window: WindowType) -> rusqlite::Result<Vec<CycleView>> {
    let mut stmt = conn.prepare(
        "SELECT * FROM observed_cycles WHERE classifier_version = ?1 AND account_fingerprint = ?2 AND window_type = ?3 ORDER BY id",
    )?;
    let rows = stmt.query_map(params![CLASSIFIER_VERSION, account, window.as_str()], cycle_view)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn cycle_by_key(conn: &Connection, key: &str) -> rusqlite::Result<Option<CycleView>> {
    conn.query_row("SELECT * FROM observed_cycles WHERE cycle_key = ?1", params![key], cycle_view).optional()
}

/// (candidate_key, kind, state, window_type)
pub type CandidateRow = (String, String, String, Option<String>);

pub fn candidates(conn: &Connection, state: Option<&str>) -> rusqlite::Result<Vec<CandidateRow>> {
    let mut stmt = conn.prepare(
        "SELECT candidate_key, kind, state, window_type FROM quota_candidates WHERE classifier_version = ?1 AND (?2 IS NULL OR state = ?2) ORDER BY id",
    )?;
    let rows = stmt
        .query_map(params![CLASSIFIER_VERSION, state], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

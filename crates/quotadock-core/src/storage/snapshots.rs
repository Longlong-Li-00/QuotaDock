//! Raw snapshots and poll runs.

use crate::analytics::burn_rate::BurnPoint;
use crate::analytics::model::{Observation, SnapshotQuality, WindowObs, WindowType};
use crate::codex::parser::{MAX_PLAUSIBLE_SECONDS, MIN_PLAUSIBLE_SECONDS};
use crate::codex::sanitize::sanitize_value;
use crate::codex::types::{NormalizedRateLimits, QuotaWindow};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;
use serde_json::{Value, json};

pub struct SnapshotInsert<'a> {
    pub request_started_at_ms: i64,
    pub response_received_at_ms: i64,
    pub account_fingerprint: Option<String>,
    /// 'match' | 'mismatch' | 'unknown' (rate-limit accountId vs active account).
    pub account_check: &'static str,
    pub cli_version: Option<&'a str>,
    pub transport: &'a str,
    pub normalized: &'a NormalizedRateLimits,
    /// Raw RPC result; sanitized here before it touches the disk.
    pub raw: &'a Value,
}

/// Server seconds -> UTC ms, only for plausible Unix-second values (otherwise unknown).
pub fn reset_at_ms(w: &QuotaWindow) -> Option<i64> {
    w.resets_at.filter(|r| (MIN_PLAUSIBLE_SECONDS..=MAX_PLAUSIBLE_SECONDS).contains(r)).map(|r| (r * 1000.0).round() as i64)
}

fn duration_i64(w: &QuotaWindow) -> Option<i64> {
    w.window_duration_mins.filter(|d| d.fract() == 0.0).map(|d| d as i64)
}

pub fn insert_snapshot(conn: &Connection, s: &SnapshotInsert) -> rusqlite::Result<i64> {
    let n = s.normalized;
    let identity_ok = s.account_fingerprint.is_some() && s.account_check != "mismatch";
    let window_quality = |w: &Option<QuotaWindow>| -> &'static str {
        if !identity_ok {
            SnapshotQuality::AccountMismatch.as_str()
        } else if w.is_some() {
            SnapshotQuality::Normal.as_str()
        } else {
            SnapshotQuality::MissingWindow.as_str()
        }
    };
    let fh_q = window_quality(&n.five_hour);
    let wk_q = window_quality(&n.weekly);
    let overall = if !identity_ok {
        SnapshotQuality::AccountMismatch.as_str()
    } else if fh_q != "normal" || wk_q != "normal" {
        SnapshotQuality::MissingWindow.as_str()
    } else {
        SnapshotQuality::Normal.as_str()
    };
    let mut ids: Vec<&str> = n.buckets.iter().map(|b| b.limit_id.as_str()).collect();
    ids.sort_unstable();
    let buckets: Vec<Value> =
        n.buckets.iter().map(|b| json!({ "limitId": b.limit_id, "primary": b.primary, "secondary": b.secondary })).collect();
    fn w(x: &Option<QuotaWindow>) -> Option<&QuotaWindow> {
        x.as_ref()
    }
    conn.execute(
        "INSERT INTO snapshots (
            captured_at_ms, request_started_at_ms, response_received_at_ms, account_fingerprint, account_check,
            cli_version, transport, ordinary_usage_allowed, availability,
            five_hour_present, five_hour_used_percent, five_hour_remaining_percent, five_hour_reset_at_ms, five_hour_duration_mins,
            weekly_present, weekly_used_percent, weekly_remaining_percent, weekly_reset_at_ms, weekly_duration_mins,
            reset_credit_count, source_bucket, bucket_ids_json, bucket_json, raw_sanitized_json, warnings_json,
            quality, five_hour_quality, weekly_quality, analysis_status)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,?28,'pending')",
        params![
            s.response_received_at_ms,
            s.request_started_at_ms,
            s.response_received_at_ms,
            s.account_fingerprint,
            s.account_check,
            s.cli_version,
            s.transport,
            n.ordinary_usage_allowed,
            n.availability.as_str(),
            n.five_hour.is_some(),
            w(&n.five_hour).and_then(|x| x.used_percent),
            w(&n.five_hour).and_then(|x| x.remaining_percent),
            w(&n.five_hour).and_then(reset_at_ms),
            w(&n.five_hour).and_then(duration_i64),
            n.weekly.is_some(),
            w(&n.weekly).and_then(|x| x.used_percent),
            w(&n.weekly).and_then(|x| x.remaining_percent),
            w(&n.weekly).and_then(reset_at_ms),
            w(&n.weekly).and_then(duration_i64),
            n.reset_credit_count.filter(|c| c.fract() == 0.0).map(|c| c as i64),
            n.source_bucket.and_then(|b| serde_json::to_value(b).ok().and_then(|v| v.as_str().map(str::to_string))),
            serde_json::to_string(&ids).unwrap_or_else(|_| "[]".into()),
            serde_json::to_string(&buckets).unwrap_or_else(|_| "[]".into()),
            serde_json::to_string(&sanitize_value(s.raw)).unwrap_or_else(|_| "{}".into()),
            serde_json::to_string(&n.warnings).unwrap_or_else(|_| "[]".into()),
            overall,
            fh_q,
            wk_q,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

#[allow(clippy::too_many_arguments)] // one argument per persisted column
pub fn insert_poll_run(
    conn: &Connection,
    started_at_ms: i64,
    finished_at_ms: i64,
    trigger: &str,
    status: &str,
    error_message: Option<&str>,
    snapshot_id: Option<i64>,
    cli_version: Option<&str>,
    transport: Option<&str>,
) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO poll_runs (started_at_ms, finished_at_ms, poll_trigger, status, error_message, snapshot_id, cli_version, transport)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![started_at_ms, finished_at_ms, trigger, status, error_message, snapshot_id, cli_version, transport],
    )?;
    Ok(conn.last_insert_rowid())
}

/// One app-server start (soak diagnostics). `reason`: initial | after_stop | after_exit.
pub fn insert_provider_start(
    conn: &Connection,
    provider: &str,
    started_at_ms: i64,
    reason: &str,
    pid: Option<u32>,
    cli_version: Option<&str>,
) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO provider_sessions (provider, started_at_ms, reason, pid, cli_version) VALUES (?1,?2,?3,?4,?5)",
        params![provider, started_at_ms, reason, pid, cli_version],
    )?;
    Ok(conn.last_insert_rowid())
}

fn window_from_row(row: &Row, prefix: &str) -> rusqlite::Result<WindowObs> {
    let present: bool = row.get(format!("{prefix}_present").as_str())?;
    if !present {
        return Ok(WindowObs::missing());
    }
    Ok(WindowObs {
        present: true,
        used_percent: row.get(format!("{prefix}_used_percent").as_str())?,
        reset_at_ms: row.get(format!("{prefix}_reset_at_ms").as_str())?,
        duration_mins: row.get(format!("{prefix}_duration_mins").as_str())?,
    })
}

/// The engine's input, rebuilt from the stored row (identical for live and re-analysis paths).
pub fn load_observation(conn: &Connection, id: i64) -> rusqlite::Result<Observation> {
    conn.query_row("SELECT * FROM snapshots WHERE id = ?1", params![id], |row| {
        let ids_json: String = row.get("bucket_ids_json")?;
        Ok(Observation {
            snapshot_id: row.get("id")?,
            captured_at_ms: row.get("captured_at_ms")?,
            account_fingerprint: row.get("account_fingerprint")?,
            account_mismatch: row.get::<_, String>("account_check")? == "mismatch",
            ordinary_usage_allowed: row.get("ordinary_usage_allowed")?,
            reset_credit_count: row.get("reset_credit_count")?,
            bucket_ids: serde_json::from_str(&ids_json).unwrap_or_default(),
            five_hour: window_from_row(row, "five_hour")?,
            weekly: window_from_row(row, "weekly")?,
        })
    })
}

pub fn snapshot_ids_with_status(conn: &Connection, status: &str) -> rusqlite::Result<Vec<i64>> {
    let mut stmt = conn.prepare("SELECT id FROM snapshots WHERE analysis_status = ?1 ORDER BY id")?;
    let ids = stmt.query_map(params![status], |r| r.get(0))?.collect::<rusqlite::Result<Vec<i64>>>()?;
    Ok(ids)
}

pub fn snapshot_count(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("SELECT COUNT(*) FROM snapshots", [], |r| r.get(0))
}

pub fn all_snapshot_ids(conn: &Connection) -> rusqlite::Result<Vec<i64>> {
    let mut stmt = conn.prepare("SELECT id FROM snapshots ORDER BY id")?;
    let ids = stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<Vec<i64>>>()?;
    Ok(ids)
}

pub fn mark_analysis_failed(conn: &Connection, id: i64, message: &str) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE snapshots SET analysis_status = 'failed', analysis_json = ?2 WHERE id = ?1",
        params![id, json!({ "error": message }).to_string()],
    )?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowView {
    pub present: bool,
    pub used_percent: Option<f64>,
    pub remaining_percent: Option<f64>,
    pub reset_at_ms: Option<i64>,
    pub duration_mins: Option<i64>,
    pub quality: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotView {
    pub id: i64,
    pub captured_at_ms: i64,
    pub account_fingerprint: Option<String>,
    pub account_check: String,
    pub cli_version: Option<String>,
    pub transport: String,
    pub ordinary_usage_allowed: Option<bool>,
    pub availability: String,
    pub five_hour: WindowView,
    pub weekly: WindowView,
    pub reset_credit_count: Option<i64>,
    pub bucket_ids: Vec<String>,
    pub quality: String,
    pub analysis_status: String,
}

fn view_window(row: &Row, prefix: &str) -> rusqlite::Result<WindowView> {
    Ok(WindowView {
        present: row.get(format!("{prefix}_present").as_str())?,
        used_percent: row.get(format!("{prefix}_used_percent").as_str())?,
        remaining_percent: row.get(format!("{prefix}_remaining_percent").as_str())?,
        reset_at_ms: row.get(format!("{prefix}_reset_at_ms").as_str())?,
        duration_mins: row.get(format!("{prefix}_duration_mins").as_str())?,
        quality: row.get(format!("{prefix}_quality").as_str())?,
    })
}

fn snapshot_view(row: &Row) -> rusqlite::Result<SnapshotView> {
    let ids_json: String = row.get("bucket_ids_json")?;
    Ok(SnapshotView {
        id: row.get("id")?,
        captured_at_ms: row.get("captured_at_ms")?,
        account_fingerprint: row.get("account_fingerprint")?,
        account_check: row.get("account_check")?,
        cli_version: row.get("cli_version")?,
        transport: row.get("transport")?,
        ordinary_usage_allowed: row.get("ordinary_usage_allowed")?,
        availability: row.get("availability")?,
        five_hour: view_window(row, "five_hour")?,
        weekly: view_window(row, "weekly")?,
        reset_credit_count: row.get("reset_credit_count")?,
        bucket_ids: serde_json::from_str(&ids_json).unwrap_or_default(),
        quality: row.get("quality")?,
        analysis_status: row.get("analysis_status")?,
    })
}

pub fn latest_snapshot(conn: &Connection) -> rusqlite::Result<Option<SnapshotView>> {
    conn.query_row("SELECT * FROM snapshots ORDER BY id DESC LIMIT 1", [], snapshot_view).optional()
}

pub fn snapshot_by_id(conn: &Connection, id: i64) -> rusqlite::Result<Option<SnapshotView>> {
    conn.query_row("SELECT * FROM snapshots WHERE id = ?1", params![id], snapshot_view).optional()
}

/// History for one window (newest last), quality included so callers can filter.
pub fn window_history(conn: &Connection, account: Option<&str>, since_ms: i64, limit: usize) -> rusqlite::Result<Vec<SnapshotView>> {
    let mut stmt = conn.prepare(
        "SELECT * FROM (SELECT * FROM snapshots WHERE captured_at_ms >= ?1 AND (?2 IS NULL OR account_fingerprint = ?2)
         ORDER BY captured_at_ms DESC LIMIT ?3) ORDER BY captured_at_ms ASC",
    )?;
    let rows = stmt.query_map(params![since_ms, account, limit as i64], snapshot_view)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Normal-quality points of one window from `from_snapshot_id` on (one cycle segment).
pub fn burn_points(conn: &Connection, account: &str, window: WindowType, from_snapshot_id: i64) -> rusqlite::Result<Vec<BurnPoint>> {
    let p = window.as_str();
    let sql = format!(
        "SELECT captured_at_ms, {p}_used_percent FROM snapshots
         WHERE account_fingerprint = ?1 AND id >= ?2 AND {p}_quality = 'normal' AND quality != 'account_mismatch'
           AND {p}_present = 1 AND {p}_used_percent IS NOT NULL
         ORDER BY captured_at_ms ASC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(params![account, from_snapshot_id], |r| Ok(BurnPoint { captured_at_ms: r.get(0)?, used_percent: r.get(1)? }))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PollSummary {
    pub snapshots: i64,
    pub poll_runs: i64,
    pub failed_polls: i64,
    pub last_status: Option<String>,
    pub last_error: Option<String>,
    pub last_poll_at_ms: Option<i64>,
}

pub fn poll_summary(conn: &Connection) -> rusqlite::Result<PollSummary> {
    let snapshots: i64 = conn.query_row("SELECT COUNT(*) FROM snapshots", [], |r| r.get(0))?;
    let poll_runs: i64 = conn.query_row("SELECT COUNT(*) FROM poll_runs", [], |r| r.get(0))?;
    let failed_polls: i64 = conn.query_row("SELECT COUNT(*) FROM poll_runs WHERE status != 'success'", [], |r| r.get(0))?;
    let last = conn
        .query_row("SELECT status, error_message, started_at_ms FROM poll_runs ORDER BY id DESC LIMIT 1", [], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, i64>(2)?))
        })
        .optional()?;
    Ok(PollSummary {
        snapshots,
        poll_runs,
        failed_polls,
        last_status: last.as_ref().map(|l| l.0.clone()),
        last_error: last.as_ref().and_then(|l| l.1.clone()),
        last_poll_at_ms: last.map(|l| l.2),
    })
}

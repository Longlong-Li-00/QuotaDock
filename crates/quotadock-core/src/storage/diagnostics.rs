//! `analysis_diagnostics` (migration v6): soak-only rows that nothing in the product reads back
//! except the diagnostics export and the soak report.

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;

pub const KIND_REANCHOR: &str = "reanchor";
pub const KIND_SHADOW: &str = "shadow_estimate";

pub fn insert(conn: &Connection, kind: &str, window_type: Option<&str>, snapshot_id: Option<i64>, data: &Value, now_ms: i64) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO analysis_diagnostics (created_at_ms, kind, window_type, snapshot_id, data_json) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![now_ms, kind, window_type, snapshot_id, data.to_string()],
    )?;
    Ok(conn.last_insert_rowid())
}

/// The newest row of `kind`, as JSON.
pub fn latest(conn: &Connection, kind: &str) -> rusqlite::Result<Option<Value>> {
    let text: Option<String> = conn
        .query_row("SELECT data_json FROM analysis_diagnostics WHERE kind = ?1 ORDER BY id DESC LIMIT 1", [kind], |r| r.get(0))
        .optional()?;
    Ok(text.and_then(|t| serde_json::from_str(&t).ok()))
}

pub fn recent(conn: &Connection, kind: &str, limit: usize) -> rusqlite::Result<Vec<Value>> {
    let mut s = conn.prepare("SELECT created_at_ms, snapshot_id, data_json FROM analysis_diagnostics WHERE kind = ?1 ORDER BY id DESC LIMIT ?2")?;
    let rows = s.query_map(params![kind, limit as i64], |r| {
        let data: String = r.get(2)?;
        let mut v: Value = serde_json::from_str(&data).unwrap_or(Value::Null);
        if let Some(o) = v.as_object_mut() {
            o.insert("createdAtMs".into(), r.get::<_, i64>(0)?.into());
            o.insert("snapshotId".into(), r.get::<_, Option<i64>>(1)?.into());
        }
        Ok(v)
    })?;
    rows.collect()
}

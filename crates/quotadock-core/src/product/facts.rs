//! Monitoring facts derived from SQLite only, so tray severity and notification rules behave the
//! same after a restart as during a session.

use crate::storage::snapshots::{self, SnapshotView};
use rusqlite::{Connection, params};

#[derive(Debug, Clone, PartialEq)]
pub struct AuthoritativeAvailability {
    pub allowed: bool,
    pub captured_at_ms: i64,
    pub snapshot_id: i64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MonitorFacts {
    pub latest: Option<SnapshotView>,
    /// The last non-null ordinaryUsageAllowed *before* the latest snapshot, same account.
    pub previous_authoritative: Option<AuthoritativeAvailability>,
    /// Trailing successful snapshots (including the latest) whose availability is null.
    pub consecutive_null_availability: u32,
    pub last_valid_snapshot_at_ms: Option<i64>,
    /// Failed poll runs since the last successful one.
    pub trailing_failures: u32,
    pub failure_streak_first_id: Option<i64>,
    pub failure_streak_started_at_ms: Option<i64>,
    pub last_poll_status: Option<String>,
    pub last_error: Option<String>,
    pub last_poll_at_ms: Option<i64>,
}

pub fn load_facts(conn: &Connection) -> rusqlite::Result<MonitorFacts> {
    let latest = snapshots::latest_snapshot(conn)?;
    let mut facts =
        MonitorFacts { last_valid_snapshot_at_ms: latest.as_ref().map(|s| s.captured_at_ms), latest: latest.clone(), ..Default::default() };

    if let Some(l) = &latest {
        let mut stmt = conn.prepare(
            "SELECT id, captured_at_ms, ordinary_usage_allowed FROM snapshots
             WHERE (account_fingerprint IS ?1) ORDER BY id DESC LIMIT 500",
        )?;
        let rows: Vec<(i64, i64, Option<bool>)> =
            stmt.query_map(params![l.account_fingerprint], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?;
        let mut counting_nulls = true;
        for (i, (id, at, allowed)) in rows.iter().enumerate() {
            if counting_nulls {
                if allowed.is_none() {
                    facts.consecutive_null_availability += 1;
                } else {
                    counting_nulls = false;
                }
            }
            if i > 0
                && let Some(a) = allowed
            {
                facts.previous_authoritative = Some(AuthoritativeAvailability { allowed: *a, captured_at_ms: *at, snapshot_id: *id });
                break;
            }
        }
    }

    let mut stmt = conn.prepare("SELECT id, started_at_ms, status, error_message FROM poll_runs ORDER BY id DESC LIMIT 500")?;
    let runs: Vec<(i64, i64, String, Option<String>)> =
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<Result<_, _>>()?;
    if let Some((_, at, status, err)) = runs.first() {
        facts.last_poll_status = Some(status.clone());
        facts.last_error = err.clone();
        facts.last_poll_at_ms = Some(*at);
    }
    for (id, at, status, _) in &runs {
        if status == "success" {
            break;
        }
        facts.trailing_failures += 1;
        facts.failure_streak_first_id = Some(*id);
        facts.failure_streak_started_at_ms = Some(*at);
    }
    Ok(facts)
}

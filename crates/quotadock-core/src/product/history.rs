//! History series for charts, computed in Rust from SQLite: downsampled, split into segments at
//! observed-cycle boundaries (never a continuous line across a rollover/refresh/credit reset),
//! normal-quality points only, plus user-visible event markers.

use crate::analytics::CLASSIFIER_VERSION;
use crate::analytics::model::WindowType;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryRange {
    SixHours,
    TwelveHours,
    Day,
    Week,
    Month,
    CurrentCycle,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPoint {
    pub t: i64,
    pub remaining: f64,
    pub used: f64,
    /// Observed-cycle index: the chart never connects points of different segments.
    pub segment: i64,
    /// A measurement gap precedes this point (e.g. QuotaDock was not running): do not connect.
    pub gap_before: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistorySeries {
    pub provider: &'static str,
    pub window_type: WindowType,
    pub points: Vec<HistoryPoint>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryMarker {
    pub provider: &'static str,
    pub t: i64,
    pub window_type: Option<String>,
    pub event_type: String,
    pub confidence: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryData {
    pub range: HistoryRange,
    pub from_ms: i64,
    pub to_ms: i64,
    /// None = raw points (5-min polls); otherwise the bucket size used for downsampling.
    pub resolution_ms: Option<i64>,
    pub series: Vec<HistorySeries>,
    pub markers: Vec<HistoryMarker>,
    pub source_snapshots: usize,
}

const HOUR: i64 = 3_600_000;
const DAY: i64 = 24 * HOUR;

pub fn range_bounds(range: HistoryRange, now_ms: i64, current_cycle_start_ms: Option<i64>) -> (i64, Option<i64>) {
    match range {
        // Up to a day: every poll (raw points).
        HistoryRange::SixHours => (now_ms - 6 * HOUR, None),
        HistoryRange::TwelveHours => (now_ms - 12 * HOUR, None),
        HistoryRange::Day => (now_ms - DAY, None),
        HistoryRange::Week => (now_ms - 7 * DAY, Some(15 * 60_000)),
        HistoryRange::Month => (now_ms - 30 * DAY, Some(HOUR)),
        HistoryRange::CurrentCycle => {
            let from = current_cycle_start_ms.unwrap_or(now_ms - DAY);
            (from, if now_ms - from > 2 * DAY { Some(15 * 60_000) } else { None })
        }
    }
}

struct Row {
    id: i64,
    t: i64,
    windows: [(Option<f64>, Option<f64>, bool); 2], // (used, remaining, usable)
}

fn segment_of(firsts: &[i64], id: i64) -> i64 {
    firsts.iter().filter(|f| **f <= id).count() as i64
}

pub fn load_history(
    conn: &Connection,
    account: &str,
    range: HistoryRange,
    now_ms: i64,
    current_cycle_start_ms: Option<i64>,
) -> rusqlite::Result<HistoryData> {
    let (from_ms, resolution_ms) = range_bounds(range, now_ms, current_cycle_start_ms);
    let mut stmt = conn.prepare(
        "SELECT id, captured_at_ms,
                five_hour_used_percent, five_hour_remaining_percent, five_hour_present, five_hour_quality,
                weekly_used_percent, weekly_remaining_percent, weekly_present, weekly_quality, quality
         FROM snapshots WHERE account_fingerprint = ?1 AND captured_at_ms >= ?2 AND captured_at_ms <= ?3 ORDER BY captured_at_ms",
    )?;
    let rows: Vec<Row> = stmt
        .query_map(params![account, from_ms, now_ms], |r| {
            let ok = |present: bool, q: String, overall: &str| present && q == "normal" && overall != "account_mismatch";
            let overall: String = r.get(10)?;
            Ok(Row {
                id: r.get(0)?,
                t: r.get(1)?,
                windows: [
                    (r.get(2)?, r.get(3)?, ok(r.get(4)?, r.get(5)?, &overall)),
                    (r.get(6)?, r.get(7)?, ok(r.get(8)?, r.get(9)?, &overall)),
                ],
            })
        })?
        .collect::<Result<_, _>>()?;

    let mut firsts: BTreeMap<String, Vec<i64>> = BTreeMap::new();
    let mut cstmt = conn.prepare(
        "SELECT window_type, first_snapshot_id FROM observed_cycles
         WHERE classifier_version = ?1 AND account_fingerprint = ?2 AND status != 'voided' AND first_snapshot_id IS NOT NULL
         ORDER BY first_snapshot_id",
    )?;
    for r in cstmt.query_map(params![CLASSIFIER_VERSION, account], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
        let (w, f) = r?;
        firsts.entry(w).or_default().push(f);
    }

    let gap_ms = resolution_ms.map(|r| 3 * r).unwrap_or(0).max(45 * 60_000);
    let mut series = Vec::new();
    for (i, wt) in [WindowType::FiveHour, WindowType::Weekly].into_iter().enumerate() {
        let f = firsts.get(wt.as_str()).cloned().unwrap_or_default();
        let mut pts: Vec<HistoryPoint> = Vec::new();
        for row in &rows {
            let (used, remaining, usable) = row.windows[i];
            let (Some(used), Some(remaining), true) = (used, remaining, usable) else { continue };
            let p = HistoryPoint { t: row.t, remaining, used, segment: segment_of(&f, row.id), gap_before: false };
            match (resolution_ms, pts.last_mut()) {
                // Same bucket and same segment: keep the latest value of the bucket.
                (Some(res), Some(last)) if last.t / res == p.t / res && last.segment == p.segment => {
                    *last = HistoryPoint { gap_before: last.gap_before, ..p }
                }
                _ => pts.push(p),
            }
        }
        for k in 1..pts.len() {
            pts[k].gap_before = pts[k].t - pts[k - 1].t > gap_ms;
        }
        series.push(HistorySeries { provider: "codex", window_type: wt, points: pts });
    }

    let mut mstmt = conn.prepare(
        "SELECT observed_at_ms, window_type, event_type, confidence FROM quota_events
         WHERE classifier_version = ?1 AND account_fingerprint = ?2 AND visibility = 'user' AND superseded_by_event_key IS NULL
           AND observed_at_ms >= ?3 AND observed_at_ms <= ?4 ORDER BY observed_at_ms",
    )?;
    let markers = mstmt
        .query_map(params![CLASSIFIER_VERSION, account, from_ms, now_ms], |r| {
            Ok(HistoryMarker { provider: "codex", t: r.get(0)?, window_type: r.get(1)?, event_type: r.get(2)?, confidence: r.get(3)? })
        })?
        .collect::<Result<_, _>>()?;

    Ok(HistoryData { range, from_ms, to_ms: now_ms, resolution_ms, series, markers, source_snapshots: rows.len() })
}

/// Adds Claude series (provider-neutral tables) to `data`, over the same range and resolution.
/// Segments are window instances: the chart never connects two quota periods.
pub fn append_claude(conn: &Connection, data: &mut HistoryData) -> rusqlite::Result<()> {
    use crate::claude::model::PROVIDER_CLAUDE;
    use crate::claude::store;
    let gap_ms = data.resolution_ms.map(|r| 3 * r).unwrap_or(0).max(45 * 60_000);
    for (key, wt) in [("five_hour", WindowType::FiveHour), ("weekly", WindowType::Weekly)] {
        let mut instances: Vec<String> = Vec::new();
        let mut pts: Vec<HistoryPoint> = Vec::new();
        for (t, used, instance) in store::window_series(conn, PROVIDER_CLAUDE, key, data.from_ms)? {
            if t > data.to_ms {
                continue;
            }
            let inst = instance.unwrap_or_default();
            let segment = match instances.iter().position(|i| *i == inst) {
                Some(i) => i as i64,
                None => {
                    instances.push(inst);
                    instances.len() as i64 - 1
                }
            };
            let p = HistoryPoint { t, remaining: (100.0 - used).clamp(0.0, 100.0), used, segment, gap_before: false };
            match (data.resolution_ms, pts.last_mut()) {
                (Some(res), Some(last)) if last.t / res == p.t / res && last.segment == p.segment => {
                    *last = HistoryPoint { gap_before: last.gap_before, ..p }
                }
                _ => pts.push(p),
            }
        }
        for k in 1..pts.len() {
            pts[k].gap_before = pts[k].t - pts[k - 1].t > gap_ms;
        }
        data.series.push(HistorySeries { provider: PROVIDER_CLAUDE, window_type: wt, points: pts });
    }
    for e in store::events(conn, PROVIDER_CLAUDE, false, 500)? {
        if e.observed_at_ms >= data.from_ms && e.observed_at_ms <= data.to_ms {
            data.markers.push(HistoryMarker {
                provider: PROVIDER_CLAUDE,
                t: e.observed_at_ms,
                window_type: e.window_key.clone(),
                event_type: e.event_type.clone(),
                confidence: e.confidence.clone(),
            });
        }
    }
    data.markers.sort_by_key(|m| m.t);
    Ok(())
}

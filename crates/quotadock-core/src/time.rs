//! Time helpers. All persisted timestamps are UTC Unix milliseconds; local time is display-only.

use chrono::{DateTime, Local, TimeZone, Utc};

pub fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

/// `2026-10-07T17:19:44.000Z` (same shape as JavaScript `Date#toISOString`).
pub fn iso_utc_ms(ms: i64) -> Option<String> {
    DateTime::<Utc>::from_timestamp_millis(ms).map(|d| d.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
}

/// Local wall-clock rendering for diagnostics output only.
pub fn local_display_ms(ms: i64) -> String {
    match Local.timestamp_millis_opt(ms).single() {
        Some(d) => d.format("%Y-%m-%d %H:%M:%S").to_string(),
        None => format!("{ms} ms"),
    }
}

/// "in 2h41m" / "17 min ago" relative to `now_ms`.
pub fn relative(ms: i64, now_ms: i64) -> String {
    let delta_min = (ms - now_ms) as f64 / 60_000.0;
    let m = delta_min.round() as i64;
    if m >= 0 {
        format!("in {}h{:02}m", m / 60, m % 60)
    } else {
        let a = -m;
        if a < 120 { format!("{a} min ago") } else { format!("{}h{:02}m ago", a / 60, a % 60) }
    }
}

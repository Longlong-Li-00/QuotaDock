//! The status-line bridge record. Claude Code runs the bridge with its session JSON on stdin; the
//! bridge keeps only the quota fields [ANTHROPIC DOCS: statusline `rate_limits`] and writes them
//! atomically to `%USERPROFILE%\.quotadock\claude\statusline.json` (a per-user folder). No transcript
//! path, cwd, prompt text or account data is kept.
//!
//! Why not AppData: Claude Desktop is an MSIX package, and Windows redirects AppData writes of
//! processes started inside it (e.g. a Claude Code session in its Code tab) to the package's private
//! store [LOCAL TEST]. A folder under the user profile outside AppData is the same for every caller.

use super::model::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const BRIDGE_SCHEMA: u32 = 1;
pub const BRIDGE_FILE: &str = "statusline.json";

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeWindow {
    pub used_percentage: f64,
    /// Unix seconds, as Claude Code sends it.
    pub resets_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeRecord {
    pub schema: u32,
    pub received_at_ms: i64,
    pub claude_version: Option<String>,
    pub model: Option<String>,
    /// First 8 hex of SHA-256(session_id): tells concurrent sessions apart, nothing more.
    pub session: Option<String>,
    /// Windows from `rate_limits` keyed by the source's own names (five_hour, seven_day, ...).
    /// Absent until Claude Code has received its first API response in the session.
    pub rate_limits: BTreeMap<String, BridgeWindow>,
}

/// Parses the bridge's stdin. A leading UTF-8 BOM is dropped: Windows PowerShell 5.1 adds one when
/// it pipes text to a native program (e.g. a user's PowerShell status-line wrapper) [LOCAL TEST].
pub fn parse_input(bytes: &[u8]) -> Value {
    let body = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    serde_json::from_slice(body).unwrap_or(Value::Null)
}

/// Extracts the bridge record from Claude Code's status-line JSON.
pub fn extract(input: &Value, now_ms: i64) -> BridgeRecord {
    let mut rate_limits = BTreeMap::new();
    if let Some(map) = input.get("rate_limits").and_then(Value::as_object) {
        for (k, w) in map {
            // spend_limit (Claude apps gateway) is spend, not a subscription window.
            if k == "spend_limit" {
                continue;
            }
            if let Some(used) = w.get("used_percentage").and_then(Value::as_f64) {
                rate_limits
                    .insert(k.clone(), BridgeWindow { used_percentage: used, resets_at: w.get("resets_at").and_then(Value::as_i64) });
            }
        }
    }
    let session = input.get("session_id").and_then(Value::as_str).map(|s| {
        let d = Sha256::digest(format!("quotadock:claude-session:v1:{s}").as_bytes());
        d.iter().take(4).map(|b| format!("{b:02x}")).collect()
    });
    BridgeRecord {
        schema: BRIDGE_SCHEMA,
        received_at_ms: now_ms,
        claude_version: input.get("version").and_then(Value::as_str).map(str::to_string),
        model: input.pointer("/model/display_name").and_then(Value::as_str).map(str::to_string),
        session,
        rate_limits,
    }
}

/// QuotaDock's display choices for the bridge's own line, beside the drop file. The bridge is a
/// separate process that does not read QuotaDock's settings; it reads this small file instead.
pub const DISPLAY_FILE: &str = "display.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeDisplay {
    pub show_five_hour: bool,
}

impl Default for BridgeDisplay {
    fn default() -> Self {
        Self { show_five_hour: true }
    }
}

/// Written by QuotaDock when the setting changes, only into a bridge folder that already exists:
/// QuotaDock never creates `~/.quotadock` for this.
pub fn write_display(dir: &Path, display: BridgeDisplay) -> std::io::Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    let tmp = dir.join(format!("{DISPLAY_FILE}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec(&display)?)?;
    std::fs::rename(&tmp, dir.join(DISPLAY_FILE))
}

/// Missing or unreadable: everything shown, as before the file existed.
pub fn read_display(dir: &Path) -> BridgeDisplay {
    std::fs::read(dir.join(DISPLAY_FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// What the bridge prints when there is no previous status line to chain.
pub fn status_text(rec: &BridgeRecord, display: BridgeDisplay) -> String {
    // Half away from zero, like the UI (format!("{:.0}") would round half to even).
    let pct = |k: &str| rec.rate_limits.get(k).map(|w| format!("{}%", w.used_percentage.round() as i64));
    let mut parts: Vec<String> = Vec::new();
    if let Some(m) = &rec.model {
        parts.push(m.clone());
    }
    let five = pct("five_hour").filter(|_| display.show_five_hour);
    match (five, pct("seven_day")) {
        (None, None) => parts.push("usage after first reply".into()),
        (a, b) => {
            if let Some(a) = a {
                parts.push(format!("5h {a} used"));
            }
            if let Some(b) = b {
                parts.push(format!("week {b} used"));
            }
        }
    }
    parts.join(" · ")
}

/// `~/.quotadock`: the bridge's home (executable + drop file), outside AppData on purpose.
pub fn bridge_root(home: &Path) -> PathBuf {
    home.join(".quotadock")
}

pub fn bridge_dir(home: &Path) -> PathBuf {
    bridge_root(home).join("claude")
}

pub fn bridge_bin_dir(home: &Path) -> PathBuf {
    bridge_root(home).join("bin")
}

/// Atomic replace: write a temp file next to the target, then rename over it.
pub fn write_record(dir: &Path, rec: &BridgeRecord) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!("{BRIDGE_FILE}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec(rec)?)?;
    std::fs::rename(&tmp, dir.join(BRIDGE_FILE))
}

pub fn read_record(dir: &Path) -> Option<BridgeRecord> {
    let bytes = std::fs::read(dir.join(BRIDGE_FILE)).ok()?;
    let rec: BridgeRecord = serde_json::from_slice(&bytes).ok()?;
    (rec.schema == BRIDGE_SCHEMA).then_some(rec)
}

/// Maps a bridge record to an observation. Only windows the source names are mapped; an unknown
/// key is kept as an extra limit under its own name, never re-interpreted.
pub fn to_observation(rec: &BridgeRecord) -> Option<ClaudeObservation> {
    if rec.rate_limits.is_empty() {
        return None;
    }
    let windows = rec
        .rate_limits
        .iter()
        .map(|(k, w)| {
            let (key, kind, label, duration) = match k.as_str() {
                "five_hour" => (FIVE_HOUR_KEY.to_string(), WindowKind::FiveHour, None, Some(300)),
                "seven_day" => (WEEKLY_KEY.to_string(), WindowKind::Weekly, None, Some(10_080)),
                other => (format!("statusline:{other}"), WindowKind::Extra, Some(other.to_string()), None),
            };
            ClaudeWindowObs {
                key,
                kind,
                label,
                duration_mins: duration,
                used_percent: w.used_percentage.clamp(0.0, 100.0),
                resets_at_ms: w.resets_at.map(|s| s * 1000),
                reset_precision_ms: 1000,
            }
        })
        .collect();
    Some(ClaudeObservation {
        source: ClaudeSource::StatusLine,
        captured_at_ms: rec.received_at_ms,
        claude_version: rec.claude_version.clone(),
        // rate_limits are only sent for claude.ai Pro/Max subscribers [ANTHROPIC DOCS].
        billing: Billing::Subscription,
        windows,
        raw_sanitized: json!({ "source": "statusline", "rateLimits": rec.rate_limits, "claudeVersion": rec.claude_version, "session": rec.session }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> Value {
        json!({
            "session_id": "abc-123", "transcript_path": "C:/Users/me/.claude/projects/x/abc.jsonl",
            "cwd": "D:/secret-project", "version": "2.1.278", "model": { "id": "claude-x", "display_name": "Opus" },
            "workspace": { "current_dir": "D:/secret-project" },
            "rate_limits": {
                "five_hour": { "used_percentage": 23.5, "resets_at": 1738425600 },
                "seven_day": { "used_percentage": 41.2, "resets_at": 1738857600 },
                "spend_limit": { "used_percentage": 62.8, "resets_at": 1740787200, "used_usd": 3.1 }
            }
        })
    }

    #[test]
    fn keeps_only_quota_fields() {
        let rec = extract(&input(), 1_000);
        let text = serde_json::to_string(&rec).unwrap();
        assert!(!text.contains("secret-project") && !text.contains("transcript") && !text.contains("abc-123"));
        assert_eq!(rec.rate_limits.len(), 2, "spend_limit is not a subscription window");
        assert_eq!(rec.rate_limits["five_hour"], BridgeWindow { used_percentage: 23.5, resets_at: Some(1738425600) });
        assert_eq!(rec.session.as_ref().map(String::len), Some(8));
        assert_eq!(status_text(&rec, BridgeDisplay::default()), "Opus · 5h 24% used · week 41% used");
        assert_eq!(status_text(&rec, BridgeDisplay { show_five_hour: false }), "Opus · week 41% used", "5-hour hidden in QuotaDock");
        let obs = to_observation(&rec).unwrap();
        let five = obs.windows.iter().find(|w| w.key == FIVE_HOUR_KEY).unwrap();
        assert_eq!((five.used_percent, five.resets_at_ms, five.reset_precision_ms), (23.5, Some(1_738_425_600_000), 1000));
        assert!((five.remaining_percent() - 76.5).abs() < 1e-9, "raw precision kept");
    }

    #[test]
    fn absent_rate_limits_yield_no_observation() {
        let rec = extract(&json!({ "version": "2.1.278", "model": { "display_name": "Opus" } }), 5);
        assert!(rec.rate_limits.is_empty());
        assert!(to_observation(&rec).is_none());
        assert_eq!(status_text(&rec, BridgeDisplay::default()), "Opus · usage after first reply");
    }

    #[test]
    fn stdin_with_a_utf8_bom_is_parsed() {
        let plain = serde_json::to_vec(&input()).unwrap();
        let mut bom = b"\xEF\xBB\xBF".to_vec();
        bom.extend_from_slice(&plain);
        assert_eq!(parse_input(&bom), input());
        assert_eq!(parse_input(&plain), input());
        assert_eq!(parse_input(b"not json"), Value::Null);
    }

    #[test]
    fn atomic_write_and_read_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let rec = extract(&input(), 42);
        write_record(dir.path(), &rec).unwrap();
        assert_eq!(read_record(dir.path()), Some(rec));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1, "no temp file left behind");
    }
}

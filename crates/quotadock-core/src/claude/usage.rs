//! The two CLI-derived inputs: `claude -p /usage --output-format json` (Claude Code's local usage
//! command, which never calls a model) and `claude auth status --json`.
//!
//! The /usage text is human-oriented, so parsing is strict: only lines in the exact observed shape
//! are accepted; anything else is "unrecognized" rather than guessed. Observed on Claude Code
//! 2.1.278 [CLAUDE CODE OBSERVED]:
//!
//! ```text
//! You are currently using your subscription to power your Claude Code usage
//! Current session: 6% used · resets Oct 9, 1:19am (Asia/Seoul)
//! Current week (all models): 29% used · resets Oct 12, 11:59pm (Asia/Seoul)
//! Current week (Fable): 1% used · resets Oct 12, 11:59pm (Asia/Seoul)
//! ```

use super::model::*;
use crate::codex::sanitize::sanitize_string;
use chrono::{Datelike, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone};
use regex::Regex;
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::OnceLock;

/// Deliberately invalid model name passed with every /usage query: if a future Claude Code ever sent
/// "/usage" to a model, the request would fail instead of consuming quota.
pub const GUARD_MODEL: &str = "quotadock-usage-guard-no-model";

pub const USAGE_ARGS: [&str; 8] =
    ["-p", "/usage", "--output-format", "json", "--model", GUARD_MODEL, "--no-session-persistence", "--strict-mcp-config"];

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "detail")]
pub enum UsageQueryError {
    /// Claude Code reported an error (message sanitized).
    Source(String),
    /// The answer looks like a server-side rate limit.
    RateLimited { retry_after_secs: Option<i64> },
    /// The envelope shows model usage: the active source must be disabled.
    InferenceDetected(String),
    /// Not the expected local /usage result, or usage lines in an unknown shape.
    Unrecognized(String),
    /// Claude Code's local /usage command ran (the envelope proves no model use) but printed no
    /// usage line at all, e.g. while it cannot reach Anthropic. Temporary: retried with back-off,
    /// never a reason to disable the source. Carries a description of the text, never the text.
    NoUsageLines(String),
}

impl std::fmt::Display for UsageQueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Source(m) => write!(f, "Claude Code error: {m}"),
            Self::RateLimited { retry_after_secs } => write!(f, "rate limited (retry after {retry_after_secs:?} s)"),
            Self::InferenceDetected(m) => write!(f, "model usage detected in a /usage query: {m}"),
            Self::Unrecognized(m) => write!(f, "unrecognized /usage output: {m}"),
            Self::NoUsageLines(m) => write!(f, "no usage lines in the /usage text ({m})"),
        }
    }
}

fn rate_limit_hint(text: &str) -> Option<UsageQueryError> {
    let lower = text.to_ascii_lowercase();
    if !(lower.contains("429") || lower.contains("rate limit") || lower.contains("rate-limit") || lower.contains("too many requests")) {
        return None;
    }
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE
        .get_or_init(|| Regex::new(r"(?i)(?:retry[- ]after|try again in)\D{0,4}(\d{1,6})\s*(s|sec|second|m|min|minute|h|hour)?").unwrap());
    let retry_after_secs = re.captures(text).and_then(|c| {
        let n: i64 = c.get(1)?.as_str().parse().ok()?;
        Some(match c.get(2).map(|u| u.as_str().to_ascii_lowercase()) {
            Some(u) if u.starts_with('h') => n * 3600,
            Some(u) if u.starts_with('m') => n * 60,
            _ => n,
        })
    });
    Some(UsageQueryError::RateLimited { retry_after_secs })
}

/// Validates the JSON envelope and returns the /usage text.
pub fn check_envelope(stdout: &str) -> Result<String, UsageQueryError> {
    let v: Value = serde_json::from_str(stdout.trim())
        .map_err(|_| rate_limit_hint(stdout).unwrap_or_else(|| UsageQueryError::Unrecognized("stdout is not one JSON object".into())))?;
    let result = v.get("result").and_then(Value::as_str).unwrap_or("").to_string();
    let num = |k: &str| v.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    let tokens: f64 = ["input_tokens", "output_tokens", "cache_creation_input_tokens", "cache_read_input_tokens"]
        .iter()
        .map(|k| v.pointer(&format!("/usage/{k}")).and_then(Value::as_f64).unwrap_or(0.0))
        .sum();
    let model_usage = v.get("modelUsage").and_then(Value::as_object).is_some_and(|m| !m.is_empty());
    if num("duration_api_ms") > 0.0 || num("num_turns") > 0.0 || num("total_cost_usd") > 0.0 || tokens > 0.0 || model_usage {
        return Err(UsageQueryError::InferenceDetected(format!(
            "duration_api_ms={} num_turns={} total_cost_usd={} tokens={tokens} modelUsage={model_usage}",
            num("duration_api_ms"),
            num("num_turns"),
            num("total_cost_usd")
        )));
    }
    if v.get("is_error").and_then(Value::as_bool).unwrap_or(false) {
        return Err(
            rate_limit_hint(&result).unwrap_or_else(|| UsageQueryError::Source(sanitize_string(&result).chars().take(300).collect()))
        );
    }
    if v.get("local_command").and_then(Value::as_str) != Some("usage") {
        return Err(UsageQueryError::Unrecognized("the result is not Claude Code's local /usage command".into()));
    }
    // The zero-inference evidence must be present, not merely absent: a missing field is an unknown
    // envelope format (fail closed), never read as 0.
    for p in ["/duration_api_ms", "/num_turns", "/usage/input_tokens", "/usage/output_tokens"] {
        if v.pointer(p).and_then(Value::as_f64).is_none() {
            return Err(UsageQueryError::Unrecognized(format!("envelope field {p} is missing")));
        }
    }
    if v.get("modelUsage").and_then(Value::as_object).is_none() {
        return Err(UsageQueryError::Unrecognized("envelope field /modelUsage is missing".into()));
    }
    Ok(result)
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedUsage {
    pub billing: Billing,
    pub windows: Vec<ClaudeWindowObs>,
    /// Timezone label printed by Claude Code (diagnostics only; times are read as local time).
    pub timezone_label: Option<String>,
}

fn line_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?m)^[ \t]*Current (?P<what>session|week \((?P<scope>[^)\n]+)\)):[ \t]*(?P<pct>\d{1,3}(?:\.\d+)?)%[ \t]*used(?:[ \t]*[·•\-][ \t]*resets[ \t]+(?P<reset>[^(\n]*?)[ \t]*(?:\((?P<tz>[^)\n]+)\))?)?[ \t]*$",
        )
        .unwrap()
    })
}

/// Parses "Oct 9, 1:19am" in the local time zone, picking the year that puts it closest to `now`
/// (resets lie within the next ~8 days). Minute precision.
pub fn parse_reset_text(text: &str, now_ms: i64) -> Option<i64> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"(?i)^([A-Z][a-z]{2,8})\.?\s+(\d{1,2}),?\s+(\d{1,2})(?::(\d{2}))?\s*(am|pm)$").unwrap());
    let c = re.captures(text.trim())?;
    let month = match c[1].to_ascii_lowercase().get(..3)? {
        "jan" => 1,
        "feb" => 2,
        "mar" => 3,
        "apr" => 4,
        "may" => 5,
        "jun" => 6,
        "jul" => 7,
        "aug" => 8,
        "sep" => 9,
        "oct" => 10,
        "nov" => 11,
        "dec" => 12,
        _ => return None,
    };
    let day: u32 = c[2].parse().ok()?;
    let mut hour: u32 = c[3].parse().ok()?;
    let minute: u32 = c.get(4).map(|m| m.as_str().parse().unwrap_or(0)).unwrap_or(0);
    if hour == 0 || hour > 12 || minute > 59 {
        return None;
    }
    let pm = c[5].eq_ignore_ascii_case("pm");
    hour = match (hour, pm) {
        (12, false) => 0,
        (12, true) => 12,
        (h, true) => h + 12,
        (h, false) => h,
    };
    let now_local = Local.timestamp_millis_opt(now_ms).single()?;
    let time = NaiveTime::from_hms_opt(hour, minute, 0)?;
    let mut best: Option<(i64, i64)> = None;
    for year in [now_local.year() - 1, now_local.year(), now_local.year() + 1] {
        let Some(date) = NaiveDate::from_ymd_opt(year, month, day) else { continue };
        let Some(local) = Local.from_local_datetime(&NaiveDateTime::new(date, time)).earliest() else { continue };
        let ms = local.timestamp_millis();
        let dist = (ms - now_ms).abs();
        if best.is_none_or(|(_, d)| dist < d) {
            best = Some((ms, dist));
        }
    }
    let (ms, _) = best?;
    // A reset is at most a week (+ slack) ahead and cannot be far in the past.
    (ms >= now_ms - 2 * 86_400_000 && ms <= now_ms + 9 * 86_400_000).then_some(ms)
}

pub fn parse_usage_text(text: &str, now_ms: i64) -> Result<ParsedUsage, UsageQueryError> {
    let lower = text.to_ascii_lowercase();
    let billing = if lower.contains("using your subscription") {
        Billing::Subscription
    } else if lower.contains("api key") || lower.contains("api usage") || lower.contains("usage-based") || lower.contains("api billing") {
        Billing::ApiKey
    } else {
        Billing::Unknown
    };
    let mut windows = Vec::new();
    let mut timezone_label = None;
    for c in line_re().captures_iter(text) {
        let used: f64 = c["pct"].parse().map_err(|_| UsageQueryError::Unrecognized("percentage".into()))?;
        if !(0.0..=100.0).contains(&used) {
            return Err(UsageQueryError::Unrecognized(format!("percentage out of range: {used}")));
        }
        let resets_at_ms = c.name("reset").map(|m| m.as_str()).filter(|s| !s.is_empty()).and_then(|s| parse_reset_text(s, now_ms));
        if let Some(tz) = c.name("tz") {
            timezone_label.get_or_insert_with(|| tz.as_str().trim().to_string());
        }
        let (key, kind, label, duration) = match c.name("scope").map(|s| s.as_str().trim()) {
            None => (FIVE_HOUR_KEY.to_string(), WindowKind::FiveHour, None, Some(300)),
            Some(s) if s.eq_ignore_ascii_case("all models") => (WEEKLY_KEY.to_string(), WindowKind::Weekly, None, Some(10_080)),
            Some(s) => (format!("weekly_model:{}", s.to_ascii_lowercase()), WindowKind::Extra, Some(s.to_string()), Some(10_080)),
        };
        if windows.iter().any(|w: &ClaudeWindowObs| w.key == key) {
            continue;
        }
        windows.push(ClaudeWindowObs {
            key,
            kind,
            label,
            duration_mins: duration,
            used_percent: used,
            resets_at_ms,
            reset_precision_ms: 60_000,
        });
    }
    if windows.is_empty() && billing != Billing::ApiKey {
        return Err(rate_limit_hint(text).unwrap_or_else(|| UsageQueryError::NoUsageLines(describe_text(text))));
    }
    Ok(ParsedUsage { billing, windows, timezone_label })
}

/// What a /usage text without usage lines looked like, for diagnostics: its size and which of a few
/// fixed words it mentions. The text itself is never stored (it is not in a known shape).
fn describe_text(text: &str) -> String {
    const WORDS: [&str; 10] = ["error", "failed", "unable", "unavailable", "network", "connect", "timeout", "timed out", "offline", "login"];
    let lower = text.to_ascii_lowercase();
    let mentions: Vec<&str> = WORDS.iter().copied().filter(|w| lower.contains(w)).collect();
    let mentions = if mentions.is_empty() { "-".to_string() } else { mentions.join(",") };
    format!("{} chars, {} lines, mentions: {mentions}", text.chars().count(), text.lines().filter(|l| !l.trim().is_empty()).count())
}

/// A sanitized copy of the /usage answer for storage: only the quota lines and the billing line.
pub fn sanitized_usage_payload(text: &str, parsed: &ParsedUsage) -> Value {
    let lines: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("Current ") || l.to_ascii_lowercase().contains("using your subscription"))
        .map(sanitize_string)
        .collect();
    json!({ "source": "claude -p /usage", "lines": lines, "timezoneLabel": parsed.timezone_label })
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthStatus {
    pub logged_in: bool,
    /// `loggedIn` exactly as reported: None when absent or not a boolean. `logged_in` treats a
    /// missing value as signed out; the authentication-loss notice requires an explicit `false`.
    pub logged_in_reported: Option<bool>,
    pub auth_method: Option<String>,
    pub api_provider: Option<String>,
    pub subscription_type: Option<String>,
    /// SHA-256 of org id + email (memory only before hashing); never the values themselves.
    pub account_fingerprint: Option<String>,
}

impl AuthStatus {
    pub fn billing(&self) -> Billing {
        match (self.logged_in, self.auth_method.as_deref(), self.api_provider.as_deref()) {
            (true, Some("claude.ai"), None | Some("firstParty")) => Billing::Subscription,
            (true, Some(_), _) => Billing::ApiKey,
            _ => Billing::Unknown,
        }
    }
}

pub fn parse_auth_status(stdout: &str) -> Option<AuthStatus> {
    let v: Value = serde_json::from_str(stdout.trim()).ok()?;
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    let identity = format!("{}|{}", s("orgId").unwrap_or_default(), s("email").unwrap_or_default());
    let fingerprint = (identity != "|").then(|| {
        let digest = Sha256::digest(format!("quotadock:claude-account:v1:{identity}").as_bytes());
        digest.iter().take(16).map(|b| format!("{b:02x}")).collect::<String>()
    });
    let logged_in = v.get("loggedIn").and_then(Value::as_bool);
    Some(AuthStatus {
        logged_in: logged_in.unwrap_or(false),
        logged_in_reported: logged_in,
        auth_method: s("authMethod"),
        api_provider: s("apiProvider"),
        subscription_type: s("subscriptionType"),
        account_fingerprint: fingerprint,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const OBSERVED: &str = "You are currently using your subscription to power your Claude Code usage\n\nCurrent session: 6% used · resets Oct 9, 1:19am (Asia/Seoul)\nCurrent week (all models): 29% used · resets Oct 12, 11:59pm (Asia/Seoul)\nCurrent week (Fable): 1% used · resets Oct 12, 11:59pm (Asia/Seoul)\n\nWhat's contributing to your limits usage?\nLast 24h · 4434 requests · 4 sessions\n";

    fn now() -> i64 {
        // 2026-10-08 20:43 local
        Local.with_ymd_and_hms(2026, 10, 8, 20, 43, 0).single().unwrap().timestamp_millis()
    }

    #[test]
    fn parses_the_observed_text() {
        let p = parse_usage_text(OBSERVED, now()).unwrap();
        assert_eq!(p.billing, Billing::Subscription);
        assert_eq!(p.timezone_label.as_deref(), Some("Asia/Seoul"));
        assert_eq!(p.windows.len(), 3);
        let five = &p.windows[0];
        assert_eq!((five.key.as_str(), five.kind, five.used_percent), (FIVE_HOUR_KEY, WindowKind::FiveHour, 6.0));
        let expect = Local.with_ymd_and_hms(2026, 10, 9, 1, 19, 0).single().unwrap().timestamp_millis();
        assert_eq!(five.resets_at_ms, Some(expect));
        assert_eq!(p.windows[1].key, WEEKLY_KEY);
        assert_eq!(p.windows[1].resets_at_ms, Some(Local.with_ymd_and_hms(2026, 10, 12, 23, 59, 0).single().unwrap().timestamp_millis()));
        assert_eq!((p.windows[2].key.as_str(), p.windows[2].label.as_deref()), ("weekly_model:fable", Some("Fable")));
        assert_eq!(p.windows[2].kind, WindowKind::Extra);
    }

    #[test]
    fn reset_text_edge_cases() {
        let n = now();
        assert_eq!(
            parse_reset_text("Oct 9, 12:00am", n),
            Some(Local.with_ymd_and_hms(2026, 10, 9, 0, 0, 0).single().unwrap().timestamp_millis())
        );
        assert_eq!(
            parse_reset_text("Oct 9, 12pm", n),
            Some(Local.with_ymd_and_hms(2026, 10, 9, 12, 0, 0).single().unwrap().timestamp_millis())
        );
        assert_eq!(parse_reset_text("Jan 3, 1:00am", n), None, "months away is not a quota reset");
        assert_eq!(parse_reset_text("tomorrow", n), None);
        assert_eq!(parse_reset_text("Oct 9, 13:00pm", n), None);
    }

    #[test]
    fn missing_reset_or_unknown_shapes_are_not_guessed() {
        let p = parse_usage_text("Current session: 0% used\nCurrent week (all models): 3% used · resets someday\n", now()).unwrap();
        assert_eq!(p.windows.len(), 2);
        assert_eq!(p.windows[0].resets_at_ms, None);
        assert_eq!(p.windows[1].resets_at_ms, None, "unparseable reset text -> null");
        // No usage line at all: temporary (Claude Code could not load usage), described but not kept.
        let e = parse_usage_text("Failed to load usage data: network error. Contact a@b.example", now()).unwrap_err();
        assert_eq!(e, UsageQueryError::NoUsageLines("61 chars, 1 lines, mentions: error,failed,network".into()));
        assert!(!e.to_string().contains("a@b"), "the text itself is never stored");
        // Usage lines in a shape we do not accept stay "unrecognized" (a format change).
        assert!(matches!(parse_usage_text("Current session: 150% used", now()), Err(UsageQueryError::Unrecognized(_))));
        assert!(matches!(
            parse_usage_text("Error: 429 Too Many Requests. Try again in 5 minutes.", now()),
            Err(UsageQueryError::RateLimited { retry_after_secs: Some(300) })
        ));
    }

    fn envelope(extra: Value) -> String {
        let mut base = json!({
            "type": "result", "subtype": "success", "is_error": false, "duration_ms": 695, "duration_api_ms": 0,
            "num_turns": 0, "total_cost_usd": 0, "local_command": "usage", "result": OBSERVED,
            "usage": { "input_tokens": 0, "output_tokens": 0, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0 },
            "modelUsage": {}
        });
        for (k, v) in extra.as_object().unwrap() {
            base[k] = v.clone();
        }
        base.to_string()
    }

    #[test]
    fn envelope_guards_against_model_calls_and_foreign_results() {
        assert!(check_envelope(&envelope(json!({}))).unwrap().contains("Current session"));
        assert!(matches!(check_envelope(&envelope(json!({ "duration_api_ms": 812 }))), Err(UsageQueryError::InferenceDetected(_))));
        assert!(matches!(
            check_envelope(&envelope(json!({ "usage": { "output_tokens": 12 } }))),
            Err(UsageQueryError::InferenceDetected(_))
        ));
        assert!(matches!(check_envelope(&envelope(json!({ "modelUsage": { "claude": {} } }))), Err(UsageQueryError::InferenceDetected(_))));
        assert!(matches!(check_envelope(&envelope(json!({ "local_command": null }))), Err(UsageQueryError::Unrecognized(_))));
        assert!(matches!(
            check_envelope(&envelope(json!({ "is_error": true, "result": "API Error: 429 rate limit" }))),
            Err(UsageQueryError::RateLimited { .. })
        ));
        assert!(matches!(check_envelope("not json"), Err(UsageQueryError::Unrecognized(_))));
    }

    #[test]
    fn missing_zero_inference_evidence_is_not_read_as_zero() {
        for missing in [json!({ "duration_api_ms": null }), json!({ "num_turns": null }), json!({ "usage": {} }), json!({ "modelUsage": null })] {
            assert!(matches!(check_envelope(&envelope(missing.clone())), Err(UsageQueryError::Unrecognized(_))), "{missing}");
        }
    }

    #[test]
    fn auth_status_reports_billing_and_hashes_identity() {
        let a = parse_auth_status(r#"{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty","email":"x@y.z","orgId":"o-1","subscriptionType":"max"}"#).unwrap();
        assert_eq!(a.billing(), Billing::Subscription);
        assert_eq!(a.subscription_type.as_deref(), Some("max"));
        let fp = a.account_fingerprint.clone().unwrap();
        assert_eq!(fp.len(), 32);
        assert!(!format!("{a:?}").contains("x@y.z") && !format!("{a:?}").contains("o-1"));
        let k = parse_auth_status(r#"{"loggedIn":true,"authMethod":"api_key","apiProvider":"firstParty"}"#).unwrap();
        assert_eq!(k.billing(), Billing::ApiKey);
        assert_eq!(parse_auth_status(r#"{"loggedIn":false}"#).unwrap().billing(), Billing::Unknown);
    }
}

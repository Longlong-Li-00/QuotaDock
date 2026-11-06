//! Defensive parser for the `account/rateLimits/read` result (schema v2/GetAccountRateLimitsResponse).
//! Port of `src/codex/parser/parseRateLimits.ts`; warning texts are kept identical for parity.

use super::types::{QuotaWindow, RateLimitBucket};
use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedRateLimits {
    pub ordinary_usage_allowed: Option<bool>,
    pub account_id: Option<String>,
    /// `rateLimits`: backward-compatible single-bucket view.
    pub legacy: Option<RateLimitBucket>,
    /// `rateLimitsByLimitId` in server order; `None` when absent or null.
    pub by_limit_id: Option<Vec<RateLimitBucket>>,
    pub reset_credits: Option<Option<f64>>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError(pub String);

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ParseError {}

/// Unix-seconds sanity range (2001 .. ~5138); outside it the unit is not what was observed.
pub const MIN_PLAUSIBLE_SECONDS: f64 = 1_000_000_000.0;
pub const MAX_PLAUSIBLE_SECONDS: f64 = 100_000_000_000.0;

fn finite_number(v: Option<&Value>) -> Option<f64> {
    v.and_then(Value::as_f64).filter(|n| n.is_finite())
}

fn present(v: Option<&Value>) -> bool {
    !matches!(v, None | Some(Value::Null))
}

/// JavaScript-style number rendering for warning texts (`120` not `120.0`).
pub fn js_number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e21 { format!("{}", n as i128) } else { format!("{n}") }
}

pub fn compute_remaining_percent(used: Option<f64>) -> Option<f64> {
    used.map(|u| (100.0 - u).clamp(0.0, 100.0))
}

pub fn resets_at_to_iso(resets_at: Option<f64>, warnings: &mut Vec<String>, place: &str) -> Option<String> {
    let r = resets_at?;
    if !(MIN_PLAUSIBLE_SECONDS..=MAX_PLAUSIBLE_SECONDS).contains(&r) {
        warnings.push(format!("{place}: resetsAt={} is outside the plausible Unix-seconds range; kept raw, not converted", js_number(r)));
        return None;
    }
    crate::time::iso_utc_ms((r * 1000.0).round() as i64)
}

pub fn parse_window(raw: Option<&Value>, place: &str, warnings: &mut Vec<String>) -> Option<QuotaWindow> {
    let raw = match raw {
        None | Some(Value::Null) => return None,
        Some(Value::Object(m)) => m,
        Some(_) => {
            warnings.push(format!("{place}: window is not an object; ignored"));
            return None;
        }
    };
    let used = finite_number(raw.get("usedPercent"));
    if used.is_none() && present(raw.get("usedPercent")) {
        warnings.push(format!("{place}: usedPercent is not a number"));
    }
    if let Some(u) = used
        && !(0.0..=100.0).contains(&u)
    {
        warnings.push(format!("{place}: usedPercent={} is outside 0..100; kept as reported", js_number(u)));
    }
    let duration = finite_number(raw.get("windowDurationMins"));
    let resets_at = finite_number(raw.get("resetsAt"));
    Some(QuotaWindow {
        used_percent: used,
        remaining_percent: compute_remaining_percent(used),
        window_duration_mins: duration,
        resets_at,
        resets_at_iso: resets_at_to_iso(resets_at, warnings, place),
    })
}

pub fn parse_bucket(raw: Option<&Value>, limit_id: &str, place: &str, warnings: &mut Vec<String>) -> Option<RateLimitBucket> {
    let obj = match raw {
        None | Some(Value::Null) => return None,
        Some(Value::Object(m)) => m,
        Some(_) => {
            warnings.push(format!("{place}: bucket is not an object; ignored"));
            return None;
        }
    };
    Some(RateLimitBucket {
        limit_id: limit_id.to_string(),
        primary: parse_window(obj.get("primary"), &format!("{place}.primary"), warnings),
        secondary: parse_window(obj.get("secondary"), &format!("{place}.secondary"), warnings),
        raw: Value::Object(obj.clone()),
    })
}

pub fn parse_rate_limits(raw: &Value) -> Result<ParsedRateLimits, ParseError> {
    let obj: &Map<String, Value> = raw.as_object().ok_or_else(|| ParseError("account/rateLimits/read result is not an object".into()))?;
    let mut warnings = Vec::new();

    let ordinary_usage_allowed = match obj.get("ordinaryUsageAllowed") {
        Some(Value::Bool(b)) => Some(*b),
        None | Some(Value::Null) => None,
        Some(_) => {
            warnings.push("ordinaryUsageAllowed is not a boolean; treated as unknown".to_string());
            None
        }
    };

    let legacy_raw = obj.get("rateLimits");
    let legacy_id =
        legacy_raw.and_then(Value::as_object).and_then(|m| m.get("limitId")).and_then(Value::as_str).unwrap_or("legacy").to_string();
    let legacy = parse_bucket(legacy_raw, &legacy_id, "rateLimits", &mut warnings);

    let by_limit_id = match obj.get("rateLimitsByLimitId") {
        Some(Value::Object(m)) => {
            let mut out = Vec::new();
            for (key, value) in m {
                let place = format!("rateLimitsByLimitId.{key}");
                if let Some(inner) = value.as_object().and_then(|o| o.get("limitId")).and_then(Value::as_str)
                    && inner != key
                {
                    warnings.push(format!("{place}: inner limitId \"{inner}\" differs from its key; keyed by \"{key}\""));
                }
                if let Some(b) = parse_bucket(Some(value), key, &place, &mut warnings) {
                    out.push(b);
                }
            }
            Some(out)
        }
        None | Some(Value::Null) => None,
        Some(_) => {
            warnings.push("rateLimitsByLimitId is not an object; ignored".to_string());
            None
        }
    };

    let reset_credits = match obj.get("rateLimitResetCredits") {
        Some(Value::Object(m)) => {
            let count = finite_number(m.get("availableCount"));
            if count.is_none() {
                warnings.push("rateLimitResetCredits.availableCount is not a number".to_string());
            }
            Some(count)
        }
        _ => None,
    };

    Ok(ParsedRateLimits {
        ordinary_usage_allowed,
        account_id: obj.get("accountId").and_then(Value::as_str).map(str::to_string),
        legacy,
        by_limit_id,
        reset_credits,
        warnings,
    })
}

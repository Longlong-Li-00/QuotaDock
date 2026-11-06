//! Duration-based window normalization and Codex bucket selection.
//! Port of `normalizeWindows.ts` + `buildSnapshot.ts`. `primary`/`secondary` are positions, not
//! meanings; classification uses `windowDurationMins` only.

use super::parser::{ParseError, ParsedRateLimits, js_number, parse_rate_limits};
use super::types::{NormalizedRateLimits, QuotaWindow, RateLimitBucket, SourceBucket, UsageAvailability};
use serde::{Deserialize, Serialize};

pub const CODEX_LIMIT_ID: &str = "codex";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowType {
    FiveHour,
    Weekly,
}

impl WindowType {
    pub const ALL: [WindowType; 2] = [WindowType::FiveHour, WindowType::Weekly];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::FiveHour => "five_hour",
            Self::Weekly => "weekly",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "five_hour" => Some(Self::FiveHour),
            "weekly" => Some(Self::Weekly),
            _ => None,
        }
    }

    /// Nominal duration used only by heuristics (sliding-anchor detection); never to infer starts.
    pub fn nominal_duration_mins(self) -> i64 {
        match self {
            Self::FiveHour => 300,
            Self::Weekly => 10080,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::FiveHour => "five-hour",
            Self::Weekly => "weekly",
        }
    }
}

/// Durations observed on codex-cli 0.162.0-alpha.2. Extend here when new windows appear.
pub fn classify_duration(duration_mins: Option<f64>) -> Option<WindowType> {
    match duration_mins {
        Some(300.0) => Some(WindowType::FiveHour),
        Some(10080.0) => Some(WindowType::Weekly),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NormalizedWindows {
    pub five_hour: Option<QuotaWindow>,
    pub weekly: Option<QuotaWindow>,
    /// (slot, window) pairs whose duration matched no known class. Kept, never force-classified.
    pub unknown: Vec<(&'static str, QuotaWindow)>,
    pub warnings: Vec<String>,
}

pub fn normalize_windows(bucket: Option<&RateLimitBucket>) -> NormalizedWindows {
    let mut out = NormalizedWindows { five_hour: None, weekly: None, unknown: Vec::new(), warnings: Vec::new() };
    let Some(bucket) = bucket else { return out };
    let mut found: Vec<(WindowType, &'static str, QuotaWindow)> = Vec::new();
    for (slot, window) in [("primary", &bucket.primary), ("secondary", &bucket.secondary)] {
        let Some(w) = window else { continue };
        match classify_duration(w.window_duration_mins) {
            Some(class) => found.push((class, slot, w.clone())),
            None => {
                out.warnings.push(format!(
                    "bucket {} {slot}: windowDurationMins={} has no known class; kept as unknown",
                    bucket.limit_id,
                    w.window_duration_mins.map(js_number).unwrap_or_else(|| "null".into())
                ));
                out.unknown.push((slot, w.clone()));
            }
        }
    }
    for class in WindowType::ALL {
        let hits: Vec<_> = found.iter().filter(|(c, _, _)| *c == class).collect();
        if hits.len() > 1 {
            // Two windows claim the same class: picking one would be arbitrary.
            let slots: Vec<&str> = hits.iter().map(|(_, s, _)| *s).collect();
            out.warnings.push(format!(
                "bucket {}: {} windows classify as {} ({}); {} left null",
                bucket.limit_id,
                hits.len(),
                class.label(),
                slots.join(", "),
                class.label()
            ));
            continue;
        }
        let only = hits.first().map(|(_, _, w)| w.clone());
        match class {
            WindowType::FiveHour => out.five_hour = only,
            WindowType::Weekly => out.weekly = only,
        }
    }
    out
}

pub struct BucketSelection<'a> {
    pub bucket: Option<&'a RateLimitBucket>,
    pub source: Option<SourceBucket>,
    pub warnings: Vec<String>,
}

/// rateLimitsByLimitId["codex"], else legacy `rateLimits` (when byLimitId is absent, or when it lacks
/// "codex" but legacy says it is "codex"). Never "the first bucket".
pub fn select_codex_bucket(parsed: &ParsedRateLimits) -> BucketSelection<'_> {
    if let Some(buckets) = &parsed.by_limit_id {
        if let Some(codex) = buckets.iter().find(|b| b.limit_id == CODEX_LIMIT_ID) {
            return BucketSelection { bucket: Some(codex), source: Some(SourceBucket::Codex), warnings: vec![] };
        }
        if let Some(legacy) = parsed.legacy.as_ref().filter(|l| l.limit_id == CODEX_LIMIT_ID) {
            return BucketSelection {
                bucket: Some(legacy),
                source: Some(SourceBucket::Legacy),
                warnings: vec![
                    "rateLimitsByLimitId has no \"codex\" bucket; normalized view taken from legacy rateLimits (limitId=codex)".into(),
                ],
            };
        }
        let keys: Vec<&str> = buckets.iter().map(|b| b.limit_id.as_str()).collect();
        let list = if keys.is_empty() { "none".to_string() } else { keys.join(", ") };
        return BucketSelection {
            bucket: None,
            source: None,
            warnings: vec![format!("no \"codex\" bucket (buckets: {list}); normalized view left empty")],
        };
    }
    if let Some(legacy) = &parsed.legacy {
        return BucketSelection {
            bucket: Some(legacy),
            source: Some(SourceBucket::Legacy),
            warnings: vec![format!(
                "rateLimitsByLimitId absent; normalized view taken from legacy rateLimits (limitId={})",
                legacy.limit_id
            )],
        };
    }
    BucketSelection { bucket: None, source: None, warnings: vec!["response contains neither rateLimitsByLimitId nor rateLimits".into()] }
}

/// Parse + select + normalize. Equivalent of the TS `buildCodexSnapshot` minus transport metadata.
pub fn normalize_rate_limits(raw: &serde_json::Value) -> Result<NormalizedRateLimits, ParseError> {
    let parsed = parse_rate_limits(raw)?;
    let selection = select_codex_bucket(&parsed);
    let windows = normalize_windows(selection.bucket);
    let mut warnings = parsed.warnings.clone();
    warnings.extend(selection.warnings.iter().cloned());
    warnings.extend(windows.warnings.iter().cloned());
    Ok(NormalizedRateLimits {
        ordinary_usage_allowed: parsed.ordinary_usage_allowed,
        availability: UsageAvailability::from_flag(parsed.ordinary_usage_allowed),
        five_hour: windows.five_hour,
        weekly: windows.weekly,
        source_bucket: selection.source,
        buckets: parsed.by_limit_id.clone().unwrap_or_default(),
        reset_credit_count: parsed.reset_credits.flatten(),
        reset_credits_present: parsed.reset_credits.is_some(),
        account_id: parsed.account_id.clone(),
        warnings,
    })
}

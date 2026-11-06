//! Notification rules, aggregation and persistent dedup. Quiet by default.
//!
//! After every poll the runtime evaluates all rules once, then shows at most ONE notification:
//! the highest-priority item (low quota + forecast AtRisk for the same window are merged into a
//! single message), with other triggered items appended as short "Also:" lines. Every included
//! rule is recorded in `notification_state`, so nothing fires twice for the same scope, including
//! across restarts. Scopes: observed cycle key (quota/forecast), event key (events), blocked
//! episode (availability), failure streak (provider errors). A new observed cycle re-arms.
//!
//! Thresholds and timings are [QUOTADOCK PRODUCT RULE].

use super::facts::MonitorFacts;
use super::i18n::{self, strings};
use super::settings::{Lang, NotificationSettings};
use crate::analytics::forecast::Risk;
use crate::analytics::model::{EvidenceConfidence, NewEvent, QuotaEventType, WindowType};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

pub const QUOTA_WARNING_PCT: f64 = 30.0;
pub const QUOTA_CRITICAL_PCT: f64 = 10.0;
/// A false -> true transition counts only if the last blocked snapshot is at most this old.
pub const AVAILABILITY_RECOVERY_MAX_GAP_MS: i64 = 30 * 60_000;
pub const PROVIDER_ERROR_MIN_FAILURES: u32 = 3;
pub const PROVIDER_ERROR_MIN_MS: i64 = 15 * 60_000;

pub mod priority {
    /// Claude monitoring stopped (signed out): always the title of a combined notification.
    pub const MONITORING_STOPPED: u8 = 0;
    pub const AVAILABILITY: u8 = 1;
    pub const CREDIT_OR_REFRESH: u8 = 2;
    pub const QUOTA_CRITICAL: u8 = 3;
    pub const FORECAST: u8 = 4;
    pub const QUOTA_WARNING: u8 = 5;
    pub const PROVIDER_ERROR: u8 = 6;
    pub const ALLOWANCE_REFRESH: u8 = 7;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleKey {
    pub rule_id: String,
    pub provider: String,
    pub window_type: String,
    pub scope_key: String,
}

impl RuleKey {
    pub(crate) fn new(rule: &str, provider: &str, window: &str, scope: &str) -> Self {
        Self { rule_id: rule.into(), provider: provider.into(), window_type: window.into(), scope_key: version_free_scope(scope).into() }
    }
}

/// Engine cycle/event keys start with the classifier version ("reset-detector-v2|..."). Dedup scopes
/// drop it, so re-analysing history with a new classifier does not re-arm alerts already sent for
/// the same cycle or event (the rest of the key, e.g. the cycle's first snapshot id, is unchanged).
pub fn version_free_scope(scope: &str) -> &str {
    match scope.split_once('|') {
        Some((head, rest)) if head.starts_with("reset-detector-") => rest,
        _ => scope,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateKind {
    AvailabilityRestored,
    ResetCredit,
    QuotaRefresh,
    AllowanceRefresh,
    QuotaCritical,
    QuotaWarning,
    ForecastAtRisk,
    ProviderError,
    /// Claude Code signed out after a confirmed sign-in (`claude_auth`).
    SignedOut,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub priority: u8,
    pub kind: CandidateKind,
    /// Display name of the provider ("Codex", "Claude"); used for cross-provider aggregation.
    pub provider: &'static str,
    pub window: Option<WindowType>,
    pub remaining: Option<f64>,
    pub keys: Vec<RuleKey>,
    pub title: String,
    pub body: String,
    /// One-line form used when this item is folded into another notification.
    pub short: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Notification {
    pub priority: u8,
    pub title: String,
    pub body: String,
    pub kinds: Vec<CandidateKind>,
    pub keys: Vec<RuleKey>,
}

/// Per-window facts needed by the quota/forecast rules.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowRuleInput {
    pub window: WindowType,
    /// Remaining % only when the window is present with normal quality; None blocks quota rules.
    pub remaining: Option<f64>,
    pub cycle_key: Option<String>,
    pub risk: Option<Risk>,
}

pub struct NotifyInput<'a> {
    pub provider: &'a str,
    pub facts: &'a MonitorFacts,
    /// This evaluation follows a successful poll (the latest snapshot is new).
    pub poll_success: bool,
    pub windows: &'a [WindowRuleInput],
    pub new_events: &'a [NewEvent],
    pub settings: &'a NotificationSettings,
    pub lang: Lang,
    pub now_ms: i64,
}

pub trait NotificationStore {
    fn is_sent(&self, key: &RuleKey) -> bool;
}

pub struct SqliteStore<'a>(pub &'a Connection);

impl NotificationStore for SqliteStore<'_> {
    fn is_sent(&self, key: &RuleKey) -> bool {
        self.0
            .query_row(
                "SELECT 1 FROM notification_state WHERE rule_id = ?1 AND provider = ?2 AND window_type = ?3 AND scope_key = ?4
                 AND sent_at_ms IS NOT NULL",
                params![key.rule_id, key.provider, key.window_type, key.scope_key],
                |_| Ok(()),
            )
            .optional()
            .ok()
            .flatten()
            .is_some()
    }
}

fn label(lang: Lang, w: WindowType) -> &'static str {
    i18n::window_label(lang, w == WindowType::FiveHour)
}

/// Evaluates every rule; returns the not-yet-sent candidates ordered by priority.
pub fn evaluate(input: &NotifyInput, store: &dyn NotificationStore) -> Vec<Candidate> {
    let s = strings(input.lang);
    let p = input.provider;
    let n = input.settings;
    let mut out: Vec<Candidate> = Vec::new();
    let push = |c: Candidate, out: &mut Vec<Candidate>| {
        if !c.keys.iter().take(1).any(|k| store.is_sent(k)) {
            out.push(c);
        }
    };

    // 1. Availability recovery: authoritative false -> true only (null -> true never counts).
    if n.availability_restored
        && input.poll_success
        && let (Some(latest), Some(prev)) = (input.facts.latest.as_ref(), input.facts.previous_authoritative.as_ref())
        && latest.ordinary_usage_allowed == Some(true)
        && !prev.allowed
        && latest.captured_at_ms - prev.captured_at_ms <= AVAILABILITY_RECOVERY_MAX_GAP_MS
    {
        push(
            Candidate {
                priority: priority::AVAILABILITY,
                kind: CandidateKind::AvailabilityRestored,
                provider: CODEX,
                window: None,
                remaining: None,
                keys: vec![RuleKey::new("availability_restored", p, "account", &format!("blocked:{}", prev.snapshot_id))],
                title: s.n_available_title.into(),
                body: s.n_available_body.into(),
                short: s.n_available_title.into(),
            },
            &mut out,
        );
    }

    // 2. User-visible events from this poll.
    for e in input.new_events {
        let window = e.window_type;
        let wkey = window.map(|w| w.as_str()).unwrap_or("account");
        match e.event_type {
            QuotaEventType::ResetCreditRedeemed if n.reset_credit && e.confidence == EvidenceConfidence::High => {
                let mut body = s.n_credit_body.to_string();
                if let (Some(b), Some(a)) = (e.evidence["creditBefore"].as_i64(), e.evidence["creditAfter"].as_i64()) {
                    body.push('\n');
                    body.push_str(&i18n::credits_line(input.lang, b, a));
                }
                push(
                    Candidate {
                        priority: priority::CREDIT_OR_REFRESH,
                        kind: CandidateKind::ResetCredit,
                        provider: CODEX,
                        window,
                        remaining: None,
                        keys: vec![RuleKey::new("reset_credit", p, wkey, &e.event_key)],
                        title: s.n_credit_title.into(),
                        body,
                        short: s.n_credit_title.into(),
                    },
                    &mut out,
                );
            }
            QuotaEventType::UnscheduledQuotaRefresh if n.quota_refresh => {
                let title = match window {
                    Some(w) => format!("{} ({})", s.n_refresh_title, label(input.lang, w)),
                    None => s.n_refresh_title.into(),
                };
                push(
                    Candidate {
                        priority: priority::CREDIT_OR_REFRESH,
                        kind: CandidateKind::QuotaRefresh,
                        provider: CODEX,
                        window,
                        remaining: None,
                        keys: vec![RuleKey::new("quota_refresh", p, wkey, &e.event_key)],
                        title: title.clone(),
                        body: i18n::early_refresh_body(input.lang, window == Some(WindowType::FiveHour)),
                        short: title,
                    },
                    &mut out,
                );
            }
            QuotaEventType::NaturalRolloverObserved if n.allowance_refresh => {
                let title = match window {
                    Some(w) => format!("{} ({})", s.n_allowance_title, label(input.lang, w)),
                    None => s.n_allowance_title.into(),
                };
                push(
                    Candidate {
                        priority: priority::ALLOWANCE_REFRESH,
                        kind: CandidateKind::AllowanceRefresh,
                        provider: CODEX,
                        window,
                        remaining: None,
                        keys: vec![RuleKey::new("allowance_refresh", p, wkey, &e.event_key)],
                        title: title.clone(),
                        body: s.n_allowance_body.into(),
                        short: title,
                    },
                    &mut out,
                );
            }
            _ => {}
        }
    }

    // 3-5. Quota thresholds and forecast, per enabled window, scoped to the observed cycle.
    if input.poll_success {
        window_rules(input, store, &mut out);
    }

    // 6. Provider errors: never on the first failure.
    if n.provider_errors && !input.poll_success && input.facts.trailing_failures > 0 {
        // "15 min without valid data" is measured from the first failure of this streak, not from the
        // last snapshot: after the app was closed for hours, one failed startup read is not an outage.
        let long = input.facts.failure_streak_started_at_ms.map(|t| input.now_ms - t > PROVIDER_ERROR_MIN_MS).unwrap_or(false);
        if input.facts.trailing_failures >= PROVIDER_ERROR_MIN_FAILURES || long {
            let scope = format!("failures:{}", input.facts.failure_streak_first_id.unwrap_or(0));
            push(
                Candidate {
                    priority: priority::PROVIDER_ERROR,
                    kind: CandidateKind::ProviderError,
                    provider: CODEX,
                    window: None,
                    remaining: None,
                    keys: vec![RuleKey::new("provider_error", p, "account", &scope)],
                    title: s.n_provider_error_title.into(),
                    body: s.n_provider_error_body.into(),
                    short: s.n_provider_error_title.into(),
                },
                &mut out,
            );
        }
    }

    out.sort_by_key(|c| c.priority);
    out
}

const CODEX: &str = "Codex";

/// Only the state-based quota/forecast rules (deduplicated per cycle), e.g. when another provider's
/// update triggers an evaluation and simultaneous lows should be merged into one notification.
pub fn evaluate_windows(input: &NotifyInput, store: &dyn NotificationStore) -> Vec<Candidate> {
    let mut out = Vec::new();
    window_rules(input, store, &mut out);
    out
}

fn window_rules(input: &NotifyInput, store: &dyn NotificationStore, out: &mut Vec<Candidate>) {
    let s = strings(input.lang);
    let p = input.provider;
    let n = input.settings;
    // 3-5. Quota thresholds and forecast, per enabled window, scoped to the observed cycle.
    {
        for w in input.windows {
            let (quota_on, forecast_on) = match w.window {
                WindowType::Weekly => (n.weekly_quota, n.forecast_weekly),
                WindowType::FiveHour => (n.five_hour_quota, n.forecast_five_hour),
            };
            let (Some(remaining), Some(cycle)) = (w.remaining, w.cycle_key.as_deref()) else { continue };
            let wt = w.window.as_str();
            let five = w.window == WindowType::FiveHour;
            if quota_on {
                let k10 = RuleKey::new("quota_below_10", p, wt, cycle);
                let k30 = RuleKey::new("quota_below_30", p, wt, cycle);
                if remaining <= QUOTA_CRITICAL_PCT && !store.is_sent(&k10) {
                    let title = i18n::quota_below(input.lang, five, 10);
                    out.push(Candidate {
                        priority: priority::QUOTA_CRITICAL,
                        kind: CandidateKind::QuotaCritical,
                        provider: CODEX,
                        window: Some(w.window),
                        remaining: Some(remaining),
                        // Crossing 10% also settles the 30% rule for this cycle.
                        keys: vec![k10, k30],
                        title: title.clone(),
                        body: i18n::remaining_line(input.lang, five, remaining),
                        short: title,
                    });
                } else if remaining <= QUOTA_WARNING_PCT && remaining > QUOTA_CRITICAL_PCT && !store.is_sent(&k30) {
                    let title = i18n::quota_below(input.lang, five, 30);
                    out.push(Candidate {
                        priority: priority::QUOTA_WARNING,
                        kind: CandidateKind::QuotaWarning,
                        provider: CODEX,
                        window: Some(w.window),
                        remaining: Some(remaining),
                        keys: vec![k30],
                        title: title.clone(),
                        body: i18n::remaining_line(input.lang, five, remaining),
                        short: title,
                    });
                }
            }
            if forecast_on && w.risk == Some(Risk::AtRisk) {
                let k = RuleKey::new("forecast_at_risk", p, wt, cycle);
                if !store.is_sent(&k) {
                    let title = format!("{} ({})", s.n_forecast_title, label(input.lang, w.window));
                    let short = i18n::forecast_short(input.lang, &title);
                    out.push(Candidate {
                        priority: priority::FORECAST,
                        kind: CandidateKind::ForecastAtRisk,
                        provider: CODEX,
                        window: Some(w.window),
                        remaining: Some(remaining),
                        keys: vec![k],
                        title,
                        body: s.n_forecast_body.into(),
                        short,
                    });
                }
            }
        }
    }
}
/// Folds all candidates of one poll into at most one notification.
pub fn aggregate(candidates: Vec<Candidate>, lang: Lang) -> Option<Notification> {
    if candidates.is_empty() {
        return None;
    }
    // Low quota + forecast AtRisk for the same provider and window become one message.
    let mut queue: std::collections::VecDeque<Candidate> = candidates.into();
    let mut merged: Vec<Candidate> = Vec::new();
    while let Some(c) = queue.pop_front() {
        let is_quota = matches!(c.kind, CandidateKind::QuotaCritical | CandidateKind::QuotaWarning);
        if is_quota
            && let Some(f) = queue
                .iter()
                .position(|o| o.kind == CandidateKind::ForecastAtRisk && o.window == c.window && o.provider == c.provider)
                .and_then(|i| queue.remove(i))
        {
            let five = c.window == Some(WindowType::FiveHour);
            let title = i18n::low_combined_title(lang, c.provider);
            merged.push(Candidate {
                priority: c.priority.min(f.priority),
                kind: c.kind,
                provider: c.provider,
                window: c.window,
                remaining: c.remaining,
                keys: c.keys.iter().chain(f.keys.iter()).cloned().collect(),
                title: title.clone(),
                body: i18n::combined_low_body(lang, five, c.remaining.unwrap_or(0.0)),
                short: title,
            });
            continue;
        }
        merged.push(c);
    }
    // Low quota on more than one provider in the same evaluation: one "AI quotas" message.
    let is_low = |c: &Candidate| matches!(c.kind, CandidateKind::QuotaCritical | CandidateKind::QuotaWarning);
    let providers: std::collections::BTreeSet<&str> = merged.iter().filter(|c| is_low(c)).map(|c| c.provider).collect();
    if providers.len() >= 2 {
        let (lows, rest): (Vec<Candidate>, Vec<Candidate>) = merged.into_iter().partition(|c| is_low(c));
        merged = rest;
        let lines: Vec<String> = lows
            .iter()
            .map(|c| i18n::provider_window_remaining(lang, c.provider, c.window == Some(WindowType::FiveHour), c.remaining.unwrap_or(0.0)))
            .collect();
        let title = i18n::multi_low_title(lang).to_string();
        let critical = lows.iter().any(|c| c.kind == CandidateKind::QuotaCritical);
        merged.push(Candidate {
            priority: lows.iter().map(|c| c.priority).min().unwrap_or(priority::QUOTA_WARNING),
            kind: if critical { CandidateKind::QuotaCritical } else { CandidateKind::QuotaWarning },
            provider: "AI",
            window: None,
            remaining: None,
            keys: lows.iter().flat_map(|c| c.keys.iter().cloned()).collect(),
            title: title.clone(),
            body: lines.join("\n"),
            short: title,
        });
    }
    merged.sort_by_key(|c| c.priority);
    let primary = merged.remove(0);
    let mut body = primary.body.clone();
    let also: Vec<&str> = merged.iter().take(2).map(|c| c.short.as_str()).collect();
    if !also.is_empty() {
        body.push_str(&format!("\n{}: {}", strings(lang).n_also, also.join(" · ")));
    }
    let mut kinds = vec![primary.kind];
    let mut keys = primary.keys.clone();
    for c in &merged {
        kinds.push(c.kind);
        keys.extend(c.keys.iter().cloned());
    }
    Some(Notification { priority: primary.priority, title: primary.title, body, kinds, keys })
}

/// Marks every included rule as sent and logs the notification. Returns the log row id.
pub fn record(conn: &Connection, provider: &str, n: &Notification, lang: Lang, now_ms: i64) -> rusqlite::Result<i64> {
    for k in &n.keys {
        conn.execute(
            "INSERT INTO notification_state (rule_id, provider, window_type, scope_key, armed, sent_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, 0, ?5, ?5)
             ON CONFLICT(rule_id, provider, window_type, scope_key) DO UPDATE SET armed = 0, sent_at_ms = excluded.sent_at_ms,
                updated_at_ms = excluded.updated_at_ms",
            params![k.rule_id, k.provider, k.window_type, k.scope_key, now_ms],
        )?;
    }
    let rule_ids: Vec<String> = n.keys.iter().map(|k| format!("{}:{}:{}", k.rule_id, k.window_type, k.scope_key)).collect();
    conn.execute(
        "INSERT INTO notification_log (created_at_ms, provider, priority, rule_ids_json, language, title, body, delivered, error)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, NULL)",
        params![
            now_ms,
            provider,
            n.priority,
            serde_json::to_string(&rule_ids).unwrap_or_default(),
            match lang {
                Lang::En => "en",
                Lang::ZhCn => "zh-CN",
            },
            n.title,
            n.body
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn mark_delivery(conn: &Connection, log_id: i64, delivered: bool, error: Option<&str>) -> rusqlite::Result<()> {
    conn.execute("UPDATE notification_log SET delivered = ?2, error = ?3 WHERE id = ?1", params![log_id, delivered, error])?;
    Ok(())
}

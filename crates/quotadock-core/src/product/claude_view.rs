//! Claude in the product layer: Overview card, tray line, notification rules and forecasts, all
//! from QuotaDock's own SQLite (provider-neutral tables) and the monitor's source status.
//!
//! Claude has no authoritative availability flag: availability is always "unknown" and is never
//! inferred from 100 % usage. A window whose source reset time has passed is shown as waiting for
//! the next quota state and never drives reminders.

use super::dashboard::{ExtraLimitStatus, HealthState, ProviderDashboard, ProviderHealth, ProviderSourceInfo};
use super::i18n;
use super::notifications::{Candidate, CandidateKind, NotificationStore, RuleKey, priority};
use super::settings::{ClaudeNotificationSettings, Lang};
use super::tray::{CRITICAL_REMAINING_PCT, ProviderTrayLine, Severity, WARNING_REMAINING_PCT, display_tone};
use crate::analytics::burn_rate::BurnPoint;
use crate::analytics::config::AnalyticsConfig;
use crate::analytics::forecast::{Risk, UsageForecast, forecast};
use crate::analytics::model::{ResetCandidateState, WindowObservationState, WindowTrackView, WindowType};
use crate::claude::model::*;
use crate::claude::monitor::{ClaudeSourceState, ClaudeStatus};
use crate::claude::store::{self, ProviderEventRow, StoredObservation, StoredWindow};
use crate::commands::{CurrentUsage, WindowStatus};
use rusqlite::Connection;

pub const DISPLAY_NAME: &str = "Claude";

/// A weekly forecast needs a day of observation inside the window instance (§33 "after enough
/// history"): a 2-hour evening session extrapolated over the rest of the week produced a false
/// "may run out early" [REAL RUN 2026-10-08]. Applied by `forecast` for every provider.
pub use crate::analytics::config::WEEKLY_MIN_SPAN_MS;

fn window_type(kind: &str) -> Option<WindowType> {
    match kind {
        "five_hour" => Some(WindowType::FiveHour),
        "weekly" => Some(WindowType::Weekly),
        _ => None,
    }
}

/// The window's period is still current (its source reset time has not passed).
fn current(w: &StoredWindow, now_ms: i64) -> bool {
    w.resets_at_ms.is_none_or(|r| r > now_ms)
}

pub fn health_state(s: ClaudeSourceState) -> HealthState {
    match s {
        ClaudeSourceState::Disabled => HealthState::Disabled,
        ClaudeSourceState::Paused => HealthState::Paused,
        ClaudeSourceState::NotFound => HealthState::NotFound,
        ClaudeSourceState::NotAuthenticated => HealthState::NotAuthenticated,
        ClaudeSourceState::ApiBilling => HealthState::ApiBilling,
        ClaudeSourceState::NotConfigured => HealthState::NotConfigured,
        ClaudeSourceState::Waiting => HealthState::Waiting,
        ClaudeSourceState::Live => HealthState::Ok,
        ClaudeSourceState::Stale => HealthState::Stale,
        ClaudeSourceState::SourceError => HealthState::SourceError,
        ClaudeSourceState::NeedsVerification => HealthState::NeedsVerification,
    }
}

/// Burn rate / forecast inside the current window instance only. The reset time is the
/// source-supplied value (never now + duration).
pub fn forecasts(
    conn: &Connection,
    latest: Option<&StoredObservation>,
    cfg: &AnalyticsConfig,
    now_ms: i64,
) -> rusqlite::Result<Vec<UsageForecast>> {
    let mut out = Vec::new();
    let Some(latest) = latest else { return Ok(out) };
    for w in &latest.windows {
        let Some(t) = window_type(&w.kind) else { continue };
        let Some(instance) = &w.instance_key else { continue };
        let from = store::instance_opened_at(conn, instance)?.unwrap_or(latest.captured_at_ms);
        let points: Vec<BurnPoint> = store::window_series(conn, PROVIDER_CLAUDE, &w.window_key, from)?
            .into_iter()
            .filter(|(_, _, i)| i.as_deref() == Some(instance.as_str()))
            .map(|(t, used, _)| BurnPoint { captured_at_ms: t, used_percent: used })
            .collect();
        let confirmed = w.resets_at_ms.filter(|r| *r > now_ms);
        let track = WindowTrackView {
            account_fingerprint: String::new(),
            window_type: t,
            observation_state: WindowObservationState::Active,
            candidate_state: ResetCandidateState::None,
            confirmed_reset_at_ms: confirmed,
            anchor_polls: points.len() as u32,
            sliding_anchor: false,
            missing_streak: 0,
            current_cycle_key: Some(instance.clone()),
            analysis_segment_start_snapshot_id: None,
            active_origin_ms: None,
            last_snapshot_id: latest.snapshot_id,
        };
        out.push(forecast(t, &points, Some(&track), cfg));
    }
    Ok(out)
}

fn window_status(w: Option<&StoredWindow>, t: WindowType, now_ms: i64) -> WindowStatus {
    let schedule = match w.and_then(|w| w.resets_at_ms) {
        Some(r) if r > now_ms => "confirmed",
        Some(_) => "passed",
        None => "unconfirmed",
    };
    // A passed reset keeps its time, not its value: the value is from the previous period.
    let cur = w.filter(|w| current(w, now_ms));
    WindowStatus {
        window_type: t,
        present: w.is_some(),
        used_percent: cur.map(|w| w.used_percent),
        remaining_percent: cur.map(|w| w.remaining_percent),
        reported_reset_at_ms: w.and_then(|w| w.resets_at_ms),
        confirmed_reset_at_ms: w.and_then(|w| w.resets_at_ms).filter(|r| *r > now_ms),
        reset_schedule: schedule,
        observation_state: None,
        candidate_state: None,
        sliding_anchor: false,
        snapshot_quality: if w.is_some() { "normal".into() } else { "none".into() },
        current_cycle: None,
        reset_precision_ms: w.map(|w| w.reset_precision_ms),
    }
}

/// The 5-hour and weekly status every surface shows (Overview card, widget).
pub fn window_statuses(latest: Option<&StoredObservation>, now_ms: i64) -> Vec<WindowStatus> {
    let find = |k: &str| latest.and_then(|l| l.windows.iter().find(|w| w.kind == k));
    vec![window_status(find("five_hour"), WindowType::FiveHour, now_ms), window_status(find("weekly"), WindowType::Weekly, now_ms)]
}

pub fn dashboard(
    status: &ClaudeStatus,
    latest: Option<&StoredObservation>,
    forecasts: Vec<UsageForecast>,
    now_ms: i64,
) -> ProviderDashboard {
    let usage = CurrentUsage {
        provider: PROVIDER_CLAUDE,
        captured_at_ms: latest.map(|l| l.captured_at_ms),
        availability: Some("unknown".into()),
        ordinary_usage_allowed: None,
        windows: window_statuses(latest, now_ms),
        reset_credit_count: None,
        bucket_ids: vec![],
        account: status
            .environment
            .auth
            .as_ref()
            .and_then(|a| a.account_fingerprint.as_deref().map(crate::codex::sanitize::short_fingerprint)),
        polls: Default::default(),
        classifier_version: "claude-observer-v1",
    };
    let extra_limits = latest
        .map(|l| {
            l.windows
                .iter()
                .filter(|w| w.kind == "extra")
                .map(|w| ExtraLimitStatus {
                    key: w.window_key.clone(),
                    label: w.label.clone().unwrap_or_else(|| w.window_key.clone()),
                    used_percent: w.used_percent,
                    remaining_percent: w.remaining_percent,
                    resets_at_ms: w.resets_at_ms,
                    reset_precision_ms: Some(w.reset_precision_ms),
                    current: current(w, now_ms),
                })
                .collect()
        })
        .unwrap_or_default();
    let a = &status.active;
    ProviderDashboard {
        provider: PROVIDER_CLAUDE,
        display_name: DISPLAY_NAME,
        plan_type: status.environment.auth.as_ref().and_then(|a| a.subscription_type.clone()),
        cli_version: status.environment.version.clone(),
        app_server_version: None,
        compatibility_mode: false,
        health: ProviderHealth {
            state: health_state(status.state),
            consecutive_failures: a.consecutive_errors + a.consecutive_rate_limits,
            last_error: a.last_error.clone(),
        },
        usage,
        forecasts,
        extra_limits,
        refresh_capability: status.refresh_capability,
        source: Some(ProviderSourceInfo {
            state: status.state,
            last_source: status.last_source.clone(),
            last_update_ms: status.last_update_ms,
            last_passive_ms: status.last_passive_ms,
            last_active_ms: status.last_active_ms,
            live: status.live,
            integration_enabled: status.integration.is_some(),
            active_enabled: status.active_enabled,
            active_suspended: a.suspended,
            active_needs_verification: a.needs_verification,
            active_next_allowed_ms: a.next_allowed_ms,
            billing: status.environment.billing().as_str(),
            cli_found: status.environment.installation.is_some(),
        }),
    }
}

/// Tray line for Claude. Only quota thresholds raise severity: stale, waiting or not-configured
/// Claude data never turns the tray red (Claude monitoring is optional and passive).
pub fn tray_line(
    status: &ClaudeStatus,
    latest: Option<&StoredObservation>,
    weekly_risk: Option<Risk>,
    n: &ClaudeNotificationSettings,
    lang: Lang,
    now_ms: i64,
) -> ProviderTrayLine {
    let s = i18n::strings(lang);
    let rem = |k: &str| latest.and_then(|l| l.windows.iter().find(|w| w.kind == k && current(w, now_ms))).map(|w| w.remaining_percent);
    let (five, weekly) = (rem("five_hour"), rem("weekly"));
    let mut severity = Severity::Normal;
    let mut reasons: Vec<&'static str> = Vec::new();
    let mut raise = |sev: Severity, why: &'static str| {
        reasons.push(why);
        severity = severity.max(sev);
    };
    if n.weekly_quota
        && let Some(r) = weekly
    {
        if r <= CRITICAL_REMAINING_PCT {
            raise(Severity::Critical, "weekly_remaining_10");
        } else if r <= WARNING_REMAINING_PCT {
            raise(Severity::Warning, "weekly_remaining_30");
        }
    }
    if n.five_hour_quota
        && let Some(r) = five
    {
        if r <= CRITICAL_REMAINING_PCT {
            raise(Severity::Critical, "five_hour_remaining_10");
        } else if r <= WARNING_REMAINING_PCT {
            raise(Severity::Warning, "five_hour_remaining_30");
        }
    }
    if weekly.is_some() && weekly_risk == Some(Risk::AtRisk) {
        raise(Severity::Warning, "weekly_forecast_at_risk");
    }
    let live = status.live;
    let tone_current = live && status.state != ClaudeSourceState::Paused;
    let pct =|v: Option<f64>| v.map(|x| format!("{}%", x.round() as i64)).unwrap_or_else(|| "—".into());
    let note = match status.state {
        ClaudeSourceState::Live => None,
        ClaudeSourceState::Stale | ClaudeSourceState::Waiting if latest.is_some() => Some(i18n::not_live(lang).to_string()),
        other => Some(i18n::claude_state_short(lang, other.as_str()).to_string()),
    };
    // No data, for a reason the state names: not a wait, so the widget does not call it one.
    let waiting_note = match status.state {
        ClaudeSourceState::NotConfigured
        | ClaudeSourceState::ApiBilling
        | ClaudeSourceState::NotFound
        | ClaudeSourceState::NotAuthenticated
        | ClaudeSourceState::NeedsVerification
        | ClaudeSourceState::SourceError
            if latest.is_none() =>
        {
            note.clone()
        }
        _ => None,
    };
    ProviderTrayLine {
        provider: PROVIDER_CLAUDE,
        display_name: DISPLAY_NAME,
        severity,
        reasons,
        five_hour_remaining: five,
        weekly_remaining: weekly,
        five_hour_tone: display_tone(five, tone_current),
        weekly_tone: display_tone(weekly, tone_current),
        live,
        has_data: latest.is_some(),
        polls_failing: false,
        not_found: false,
        unverified_version: false,
        show_five_hour: true,
        note: note.clone(),
        note_short: i18n::claude_state_compact(lang, status.state.as_str()).map(str::to_string),
        waiting_note,
        menu_header: match &note {
            Some(n) => format!("{DISPLAY_NAME} · {n}"),
            None => DISPLAY_NAME.to_string(),
        },
        menu_five_hour: format!("{} {}", s.five_hour, pct(five)),
        menu_weekly: format!("{} {}", s.weekly, pct(weekly)),
    }
}

pub struct ClaudeWindowInput {
    pub window: WindowType,
    /// Remaining % of a current (not yet reset) window; None blocks reminders.
    pub remaining: Option<f64>,
    pub instance_key: Option<String>,
    pub risk: Option<Risk>,
}

pub fn window_inputs(latest: Option<&StoredObservation>, forecasts: &[UsageForecast], now_ms: i64) -> Vec<ClaudeWindowInput> {
    let Some(l) = latest else { return vec![] };
    l.windows
        .iter()
        .filter_map(|w| {
            let t = window_type(&w.kind)?;
            Some(ClaudeWindowInput {
                window: t,
                remaining: current(w, now_ms).then_some(w.remaining_percent),
                instance_key: w.instance_key.clone(),
                risk: forecasts.iter().find(|f| f.window_type == t).map(|f| f.risk),
            })
        })
        .collect()
}

/// Claude reminder rules: thresholds and (optional) forecast per window instance; refresh notices
/// for user-visible refresh events (off by default). Deduplicated through `store`.
pub fn evaluate(
    windows: &[ClaudeWindowInput],
    new_events: &[ProviderEventRow],
    n: &ClaudeNotificationSettings,
    lang: Lang,
    store: &dyn NotificationStore,
) -> Vec<Candidate> {
    let p = PROVIDER_CLAUDE;
    let mut out = Vec::new();
    for w in windows {
        let (Some(remaining), Some(scope)) = (w.remaining, w.instance_key.as_deref()) else { continue };
        let five = w.window == WindowType::FiveHour;
        let wt = w.window.as_str();
        let quota_on = if five { n.five_hour_quota } else { n.weekly_quota };
        if quota_on {
            let k10 = RuleKey::new("quota_below_10", p, wt, scope);
            let k30 = RuleKey::new("quota_below_30", p, wt, scope);
            if remaining <= CRITICAL_REMAINING_PCT && !store.is_sent(&k10) {
                let title = i18n::quota_below_for(lang, DISPLAY_NAME, five, 10);
                out.push(Candidate {
                    priority: priority::QUOTA_CRITICAL,
                    kind: CandidateKind::QuotaCritical,
                    provider: DISPLAY_NAME,
                    window: Some(w.window),
                    remaining: Some(remaining),
                    keys: vec![k10, k30],
                    title: title.clone(),
                    body: i18n::remaining_line(lang, five, remaining),
                    short: title,
                });
            } else if remaining <= WARNING_REMAINING_PCT && remaining > CRITICAL_REMAINING_PCT && !store.is_sent(&k30) {
                let title = i18n::quota_below_for(lang, DISPLAY_NAME, five, 30);
                out.push(Candidate {
                    priority: priority::QUOTA_WARNING,
                    kind: CandidateKind::QuotaWarning,
                    provider: DISPLAY_NAME,
                    window: Some(w.window),
                    remaining: Some(remaining),
                    keys: vec![k30],
                    title: title.clone(),
                    body: i18n::remaining_line(lang, five, remaining),
                    short: title,
                });
            }
        }
        if !five && n.forecast_weekly && w.risk == Some(Risk::AtRisk) {
            let k = RuleKey::new("forecast_at_risk", p, wt, scope);
            if !store.is_sent(&k) {
                let title = i18n::forecast_title_for(lang, DISPLAY_NAME);
                let short = i18n::forecast_short(lang, &title);
                out.push(Candidate {
                    priority: priority::FORECAST,
                    kind: CandidateKind::ForecastAtRisk,
                    provider: DISPLAY_NAME,
                    window: Some(w.window),
                    remaining: Some(remaining),
                    keys: vec![k],
                    title,
                    body: i18n::strings(lang).n_forecast_body.into(),
                    short,
                });
            }
        }
    }
    if n.allowance_refresh {
        for e in new_events.iter().filter(|e| e.visibility == "user" && e.event_type == "quota_refreshed_observed") {
            let k = RuleKey::new("allowance_refresh", p, e.window_key.as_deref().unwrap_or("account"), &format!("event:{}", e.id));
            if store.is_sent(&k) {
                continue;
            }
            let title = i18n::allowance_refreshed_for(lang, DISPLAY_NAME, e.window_key.as_deref());
            out.push(Candidate {
                priority: priority::ALLOWANCE_REFRESH,
                kind: CandidateKind::AllowanceRefresh,
                provider: DISPLAY_NAME,
                window: e.window_key.as_deref().and_then(window_type),
                remaining: None,
                keys: vec![k],
                title: title.clone(),
                body: i18n::claude_refresh_body(lang).into(),
                short: title,
            });
        }
    }
    out.sort_by_key(|c| c.priority);
    out
}

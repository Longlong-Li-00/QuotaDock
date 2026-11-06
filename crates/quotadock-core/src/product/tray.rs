//! Tray status model: one severity derived in Rust from authoritative data + QuotaDock rules.
//! The dashboard and widget read the same model: values and availability are shared; display tone
//! follows remaining percent, while the tray icon and severity follow notification rules.
//!
//! Precedence: error > critical > warning > normal. All thresholds are [QUOTADOCK PRODUCT RULE].

use super::facts::MonitorFacts;
use super::i18n::{self, strings};
use super::settings::{Lang, NotificationSettings};
use crate::analytics::forecast::Risk;
use serde::Serialize;

pub const MAYBE_STALE_MS: i64 = 10 * 60_000;
pub const STALE_MS: i64 = 30 * 60_000;
pub const UNKNOWN_AVAILABILITY_PERSIST_SNAPSHOTS: u32 = 3;
pub const CRITICAL_REMAINING_PCT: f64 = 10.0;
pub const WARNING_REMAINING_PCT: f64 = 30.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Normal,
    Warning,
    Critical,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Freshness {
    NoData,
    Fresh,
    MaybeStale,
    Stale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TrayIconKind {
    Normal,
    Warning,
    Critical,
    Error,
    Paused,
}

/// How a remaining value is coloured. Display only: it never reads notification settings, and
/// notification rules never read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DisplayTone {
    Normal,
    Low,
    Critical,
    Muted,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrayStatusModel {
    pub severity: Severity,
    pub icon: TrayIconKind,
    pub paused: bool,
    pub availability: Option<bool>,
    pub availability_unknown_persistent: bool,
    pub five_hour_remaining: Option<f64>,
    pub weekly_remaining: Option<f64>,
    pub updated_at_ms: Option<i64>,
    pub freshness: Freshness,
    pub minutes_since_update: Option<i64>,
    /// Machine-readable reasons for the severity (for the dashboard and tests).
    pub reasons: Vec<&'static str>,
    pub tooltip_lines: Vec<String>,
    pub menu_five_hour: String,
    pub menu_weekly: String,
    /// Per-provider lines, Codex first. `severity`/`icon` above are the maximum over providers;
    /// the remaining fields above describe Codex (kept for the Phase 3 UI).
    pub providers: Vec<ProviderTrayLine>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderTrayLine {
    pub provider: &'static str,
    pub display_name: &'static str,
    pub severity: Severity,
    pub reasons: Vec<&'static str>,
    pub five_hour_remaining: Option<f64>,
    pub weekly_remaining: Option<f64>,
    pub five_hour_tone: DisplayTone,
    pub weekly_tone: DisplayTone,
    pub live: bool,
    pub has_data: bool,
    /// The latest polls failed. Display only: raises nothing and adds no reason.
    pub polls_failing: bool,
    /// Codex: those polls found no installation. Display only: raises nothing and adds no reason.
    pub not_found: bool,
    /// Codex: those polls found only an older, unverified version. Display only, like `not_found`.
    pub unverified_version: bool,
    /// False: the 5-hour value stays in this model but is not shown (the provider's own switch).
    pub show_five_hour: bool,
    /// Short state for menu/tooltip when not live ("Not live", "Not set up", ...).
    pub note: Option<String>,
    /// Claude: `note` in a form that fits the widget's compact slot, when the full words do not.
    pub note_short: Option<String>,
    /// Claude: `note` when there is no data for a reason the state names, so it is not a wait.
    pub waiting_note: Option<String>,
    pub menu_header: String,
    pub menu_five_hour: String,
    pub menu_weekly: String,
}

pub struct TrayInput<'a> {
    pub facts: &'a MonitorFacts,
    pub paused: bool,
    pub weekly_risk: Option<Risk>,
    /// Display only. Whether a 5-hour value can raise the level is decided by `settings`.
    pub show_five_hour: bool,
    pub settings: &'a NotificationSettings,
    pub lang: Lang,
    pub now_ms: i64,
}

/// Remaining % of a window that is valid for rules (present, normal quality, numeric).
pub fn rule_remaining(w: &crate::storage::snapshots::WindowView) -> Option<f64> {
    if !w.present || w.quality != "normal" {
        return None;
    }
    w.remaining_percent
}

/// The tone of a remaining value. Low tones survive stale data: inside one window the remaining
/// value can only fall, so a stale low value is an upper bound. A stale high value must not look
/// healthy, so it goes muted, and the UI shows the data age.
pub fn display_tone(remaining: Option<f64>, current: bool) -> DisplayTone {
    match remaining {
        None => DisplayTone::Unknown,
        Some(r) if r <= CRITICAL_REMAINING_PCT => DisplayTone::Critical,
        Some(r) if r <= WARNING_REMAINING_PCT => DisplayTone::Low,
        Some(_) if current => DisplayTone::Normal,
        Some(_) => DisplayTone::Muted,
    }
}

/// A Codex window's tone: rule-valid values only, and a window whose reset time has passed has no
/// current value.
fn codex_tone(w: Option<&crate::storage::snapshots::WindowView>, current: bool, now_ms: i64) -> DisplayTone {
    display_tone(w.filter(|w| w.reset_at_ms.is_none_or(|r| r > now_ms)).and_then(rule_remaining), current)
}

/// A Codex value that belongs to the last period: the read was taken before a reset time that has
/// passed since, and the window had been used. A read taken after that time is the service's current
/// answer, an idle window at 100 % has nothing left to reset, and no reset time is not a passed one, so
/// those keep their numbers. Display only: rules and reminders read the window itself.
pub fn value_from_last_period(captured_at_ms: i64, reset_at_ms: Option<i64>, remaining: Option<f64>, now_ms: i64) -> bool {
    reset_at_ms.is_some_and(|r| captured_at_ms < r && r <= now_ms) && remaining.is_some_and(|v| v < 100.0)
}

/// The value a surface shows for a Codex window. None: missing, or from the last period.
fn shown_remaining(l: &crate::storage::snapshots::SnapshotView, w: &crate::storage::snapshots::WindowView, now_ms: i64) -> Option<f64> {
    let value = w.present.then_some(w.remaining_percent).flatten();
    value.filter(|_| !value_from_last_period(l.captured_at_ms, w.reset_at_ms, value, now_ms))
}

fn value_line(lang: Lang, show_five_hour: bool, five_hour: Option<f64>, weekly: Option<f64>) -> String {
    if show_five_hour { i18n::percent_line(lang, five_hour, weekly) } else { i18n::weekly_line(lang, weekly) }
}

pub fn derive_tray_status(input: &TrayInput) -> TrayStatusModel {
    let f = input.facts;
    let s = strings(input.lang);
    let latest = f.latest.as_ref();
    let availability = latest.and_then(|l| l.ordinary_usage_allowed);
    let five_hour_remaining = latest.and_then(|l| shown_remaining(l, &l.five_hour, input.now_ms));
    let weekly_remaining = latest.and_then(|l| shown_remaining(l, &l.weekly, input.now_ms));
    let age_ms = f.last_valid_snapshot_at_ms.map(|t| (input.now_ms - t).max(0));
    let minutes = age_ms.map(|a| a / 60_000);
    let freshness = match age_ms {
        None => Freshness::NoData,
        Some(a) if a > STALE_MS => Freshness::Stale,
        Some(a) if a > MAYBE_STALE_MS => Freshness::MaybeStale,
        Some(_) => Freshness::Fresh,
    };
    let unknown_persistent =
        availability.is_none() && latest.is_some() && f.consecutive_null_availability >= UNKNOWN_AVAILABILITY_PERSIST_SNAPSHOTS;

    let mut reasons: Vec<&'static str> = Vec::new();
    let mut severity = Severity::Normal;
    let mut raise = |sev: Severity, why: &'static str, reasons: &mut Vec<&'static str>| {
        reasons.push(why);
        if sev > severity {
            severity = sev;
        }
    };

    // error
    if f.trailing_failures > 0 {
        match f.last_poll_status.as_deref() {
            Some("authentication_error") => raise(Severity::Error, "not_authenticated", &mut reasons),
            Some("provider_unavailable") => raise(Severity::Error, "provider_unavailable", &mut reasons),
            _ => {}
        }
    }
    if !input.paused && freshness == Freshness::Stale {
        raise(Severity::Error, "no_valid_snapshot_30m", &mut reasons);
    }
    // critical
    if availability == Some(false) {
        raise(Severity::Critical, "limit_reached", &mut reasons);
    }
    if let Some(l) = latest {
        if input.settings.weekly_quota && rule_remaining(&l.weekly).is_some_and(|r| r <= CRITICAL_REMAINING_PCT) {
            raise(Severity::Critical, "weekly_remaining_10", &mut reasons);
        }
        if input.settings.five_hour_quota && rule_remaining(&l.five_hour).is_some_and(|r| r <= CRITICAL_REMAINING_PCT) {
            raise(Severity::Critical, "five_hour_remaining_10", &mut reasons);
        }
        // warning
        if input.settings.weekly_quota
            && rule_remaining(&l.weekly).is_some_and(|r| r <= WARNING_REMAINING_PCT && r > CRITICAL_REMAINING_PCT)
        {
            raise(Severity::Warning, "weekly_remaining_30", &mut reasons);
        }
        if input.settings.five_hour_quota
            && rule_remaining(&l.five_hour).is_some_and(|r| r <= WARNING_REMAINING_PCT && r > CRITICAL_REMAINING_PCT)
        {
            raise(Severity::Warning, "five_hour_remaining_30", &mut reasons);
        }
    }
    if input.weekly_risk == Some(Risk::AtRisk) {
        raise(Severity::Warning, "weekly_forecast_at_risk", &mut reasons);
    }
    if unknown_persistent {
        raise(Severity::Warning, "availability_unknown_persistent", &mut reasons);
    }

    // tooltip
    let mut lines = Vec::new();
    lines.push(if input.paused { s.paused_title.to_string() } else { s.app.to_string() });
    if latest.is_some() {
        lines.push(value_line(input.lang, input.show_five_hour, five_hour_remaining, weekly_remaining));
    }
    if input.paused {
        if let Some(m) = minutes {
            lines.push(i18n::last_update(input.lang, m));
        }
    } else {
        match (freshness, minutes) {
            (Freshness::NoData, _) if f.trailing_failures == 0 => lines.push(s.no_data_yet.into()),
            // Failing with nothing stored: say so before the rules' reason exists (backlog 52).
            (Freshness::NoData, _) if !reasons.contains(&"provider_unavailable") && !reasons.contains(&"not_authenticated") => {
                lines.push(s.provider_unavailable.into())
            }
            (Freshness::Fresh, Some(m)) => lines.push(i18n::updated_ago(input.lang, m)),
            (Freshness::MaybeStale, Some(m)) => lines.push(i18n::maybe_stale(input.lang, m)),
            (Freshness::Stale, Some(m)) => lines.push(i18n::stale(input.lang, m)),
            _ => {}
        }
    }
    let not_found = reasons.contains(&"provider_unavailable") && f.last_error.as_deref().is_some_and(|e| e.contains("no codex executable"));
    // The same test as the dashboard's UnsupportedVersion health state.
    let unverified_version =
        reasons.contains(&"provider_unavailable") && f.last_error.as_deref().is_some_and(|e| e.contains("unverified older") || e.contains("older codex"));
    if reasons.contains(&"not_authenticated") {
        lines.push(s.not_signed_in.into());
    } else if reasons.contains(&"provider_unavailable") {
        lines.push(if not_found {
            s.codex_not_found.into()
        } else if unverified_version {
            s.codex_unverified.into()
        } else {
            s.provider_unavailable.into()
        });
    }
    if availability == Some(false) {
        lines.push(s.limit_reached.into());
    } else if latest.is_some() && availability.is_none() {
        lines.push(if unknown_persistent { s.availability_unknown_persistent.into() } else { s.availability_unknown.into() });
    }

    let pct = |v: Option<f64>| v.map(|x| format!("{}%", x.round() as i64)).unwrap_or_else(|| "—".into());
    let icon = if input.paused {
        TrayIconKind::Paused
    } else {
        match severity {
            Severity::Normal => TrayIconKind::Normal,
            Severity::Warning => TrayIconKind::Warning,
            Severity::Critical => TrayIconKind::Critical,
            Severity::Error => TrayIconKind::Error,
        }
    };
    let tone_current = !input.paused && matches!(freshness, Freshness::Fresh | Freshness::MaybeStale);
    let codex_line = ProviderTrayLine {
        provider: "codex",
        display_name: s.codex,
        severity,
        reasons: reasons.clone(),
        five_hour_remaining,
        weekly_remaining,
        five_hour_tone: codex_tone(latest.map(|l| &l.five_hour), tone_current, input.now_ms),
        weekly_tone: codex_tone(latest.map(|l| &l.weekly), tone_current, input.now_ms),
        live: freshness == Freshness::Fresh,
        has_data: latest.is_some(),
        polls_failing: f.trailing_failures > 0,
        not_found,
        unverified_version,
        show_five_hour: input.show_five_hour,
        note: None,
        note_short: None,
        waiting_note: None,
        menu_header: s.codex.to_string(),
        menu_five_hour: format!("{} {}", s.five_hour, pct(five_hour_remaining)),
        menu_weekly: format!("{} {}", s.weekly, pct(weekly_remaining)),
    };
    TrayStatusModel {
        providers: vec![codex_line],
        severity,
        icon,
        paused: input.paused,
        availability,
        availability_unknown_persistent: unknown_persistent,
        five_hour_remaining,
        weekly_remaining,
        updated_at_ms: f.last_valid_snapshot_at_ms,
        freshness,
        minutes_since_update: minutes,
        reasons,
        tooltip_lines: lines,
        menu_five_hour: format!("{} {}", s.five_hour, pct(five_hour_remaining)),
        menu_weekly: format!("{} {}", s.weekly, pct(weekly_remaining)),
    }
}

/// The model while Codex is turned off in Settings: no Codex line and nothing Codex could raise.
/// Other providers are added with `with_provider`.
pub fn without_codex(paused: bool, lang: Lang) -> TrayStatusModel {
    let s = strings(lang);
    TrayStatusModel {
        severity: Severity::Normal,
        icon: if paused { TrayIconKind::Paused } else { TrayIconKind::Normal },
        paused,
        availability: None,
        availability_unknown_persistent: false,
        five_hour_remaining: None,
        weekly_remaining: None,
        updated_at_ms: None,
        freshness: Freshness::NoData,
        minutes_since_update: None,
        reasons: vec![],
        tooltip_lines: vec![if paused { s.paused_title.to_string() } else { s.app.to_string() }],
        menu_five_hour: format!("{} —", s.five_hour),
        menu_weekly: format!("{} —", s.weekly),
        providers: vec![],
    }
}

/// With Codex off, the header's "updated N min ago" follows the provider that remains. A sparse
/// passive source is never called stale outright: not live is only "may be stale".
pub fn with_update_time(mut model: TrayStatusModel, updated_at_ms: Option<i64>, live: bool, now_ms: i64) -> TrayStatusModel {
    model.updated_at_ms = updated_at_ms;
    model.minutes_since_update = updated_at_ms.map(|t| (now_ms - t).max(0) / 60_000);
    model.freshness = match updated_at_ms {
        None => Freshness::NoData,
        Some(_) if live => Freshness::Fresh,
        Some(_) => Freshness::MaybeStale,
    };
    model
}

/// Adds another provider's line: the global severity is the maximum, the tooltip lists every
/// provider. A provider without live data never removes or hides the others.
pub fn with_provider(mut model: TrayStatusModel, line: ProviderTrayLine, lang: Lang) -> TrayStatusModel {
    let s = strings(lang);
    model.severity = model.severity.max(line.severity);
    if !model.paused {
        model.icon = match model.severity {
            Severity::Normal => TrayIconKind::Normal,
            Severity::Warning => TrayIconKind::Warning,
            Severity::Critical => TrayIconKind::Critical,
            Severity::Error => TrayIconKind::Error,
        };
    }
    model.providers.push(line);
    // Rebuild the tooltip: title, one line per provider, then the Codex status lines.
    let codex_has_line = model.updated_at_ms.is_some();
    let mut lines = vec![model.tooltip_lines.first().cloned().unwrap_or_else(|| s.app.to_string())];
    for p in &model.providers {
        let mut l = if p.has_data {
            format!("{} {}", p.display_name, value_line(lang, p.show_five_hour, p.five_hour_remaining, p.weekly_remaining))
        } else {
            p.display_name.to_string()
        };
        if let Some(n) = &p.note {
            l.push_str(&format!(" · {n}"));
        }
        lines.push(l);
    }
    lines.extend(model.tooltip_lines.iter().skip(if codex_has_line { 2 } else { 1 }).cloned());
    model.tooltip_lines = lines;
    model
}

/// One provider's row in the tray menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuRow {
    pub text: String,
    /// False only for a provider turned off in Settings; every other row opens the main window.
    pub enabled: bool,
}

/// Paused, or how far the Codex data can be trusted. With no data, polls that keep failing are not
/// "connecting".
fn codex_data_status(model: &TrayStatusModel, line: &ProviderTrayLine, lang: Lang) -> Option<String> {
    let s = strings(lang);
    if model.paused {
        return Some(i18n::claude_state_short(lang, "paused").to_string());
    }
    match (model.freshness, model.minutes_since_update) {
        (Freshness::NoData, _) if !line.has_data => Some(if line.polls_failing { s.provider_unavailable } else { s.no_data_yet }.to_string()),
        (Freshness::MaybeStale, Some(m)) => Some(i18n::maybe_stale(lang, m)),
        (Freshness::Stale, Some(m)) => Some(i18n::stale(lang, m)),
        _ => None,
    }
}

/// Codex's status for its menu row, in the order the widget's notes use. Limit reached comes only
/// from `ordinaryUsageAllowed`, through the rules' reason. A failed read stores nothing, so a limit
/// stands while reads fail and stays first on the row, and a limit from old data says so.
fn codex_menu_status(model: &TrayStatusModel, line: &ProviderTrayLine, lang: Lang) -> Option<String> {
    let s = strings(lang);
    let has = |r: &str| line.reasons.contains(&r);
    if has("not_authenticated") {
        Some(s.not_signed_in.to_string())
    } else if has("provider_unavailable") {
        let cause = if line.not_found {
            s.codex_not_found
        } else if line.unverified_version {
            s.codex_unverified
        } else {
            s.provider_unavailable
        };
        Some(if has("limit_reached") { format!("{} · {cause}", s.limit_reached) } else { cause.to_string() })
    } else if has("limit_reached") {
        Some(match codex_data_status(model, line, lang) {
            Some(data) => format!("{} · {data}", s.limit_reached),
            None => s.limit_reached.to_string(),
        })
    } else {
        codex_data_status(model, line, lang)
    }
}

/// `Codex  5小时 36% · 每周 9%`: the name, its status when there is one, then the values, without the
/// 5-hour value when that window is hidden. A signed-out Codex and a provider with no data show the
/// status alone; a turned-off provider is a disabled `Claude · 已关闭`. Claude's status is its own
/// note, so Claude can never say limit reached. No reset text here.
pub fn menu_line(model: &TrayStatusModel, provider: &str, lang: Lang) -> MenuRow {
    let s = strings(lang);
    let Some(line) = model.providers.iter().find(|p| p.provider == provider) else {
        let name = if provider == "codex" { s.codex } else { super::claude_view::DISPLAY_NAME };
        return MenuRow { text: format!("{name} · {}", i18n::claude_state_short(lang, "disabled")), enabled: false };
    };
    let status = if provider == "codex" { codex_menu_status(model, line, lang) } else { line.note.clone() };
    let mut text = line.display_name.to_string();
    if let Some(status) = status {
        text.push_str(&format!(" · {status}"));
    }
    if line.has_data && !line.reasons.contains(&"not_authenticated") {
        let pct = |v: Option<f64>| v.map(|x| format!("{}%", x.round() as i64)).unwrap_or_else(|| "—".into());
        text.push_str("  ");
        if line.show_five_hour {
            text.push_str(&format!("{} {} · ", s.five_hour, pct(line.five_hour_remaining)));
        }
        text.push_str(&format!("{} {}", s.weekly, pct(line.weekly_remaining)));
    }
    MenuRow { text, enabled: true }
}

/// Windows tray tooltips are limited to 127 characters.
pub fn tooltip_text(model: &TrayStatusModel) -> String {
    let t = model.tooltip_lines.join("\n");
    t.chars().take(127).collect()
}

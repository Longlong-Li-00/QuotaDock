//! Desktop widget (Phase 3.7): a small, read-only, always-available view of the quotas.
//!
//! Presentation only. The view is built from the same tray model that drives the tray icon and
//! the dashboard (`ProductRuntime::tray_with`): values and availability are shared; display tone
//! follows remaining percent, while the tray icon and severity follow notification rules. Nothing
//! here reads a provider, starts a CLI, polls, evaluates thresholds or touches the reset engine.
//!
//! The pure rules the window shell (src-tauri) executes also live here, so they are testable:
//!
//! ```text
//! provider_foreground   monitoring cadence follows the main window only; the widget never counts
//! decide                lifecycle: create / update / destroy, never a second widget window
//! window_spec           frameless, transparent, no taskbar entry, not resizable, not focused
//! place / save_position multi-monitor placement in logical units, off-screen recovery
//! resized_position      where the widget goes when its height changes, bottom edge kept at the margin
//! custom_icons          the user's own provider icons from the data folder, if any
//! ```

use super::i18n::{self, strings};
use super::settings::{AppSettings, DesktopWidgetSettings, Lang, ThemePref, WidgetMode, WidgetPosition};
use super::tray::{DisplayTone, Freshness, ProviderTrayLine, Severity, TrayStatusModel};
use crate::analytics::model::WindowType;
use crate::commands::WindowStatus;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

/// The one widget window's label. Tauri refuses a second webview with the same label, so the
/// process can never hold two widget windows.
pub const WIDGET_LABEL: &str = "widget";
pub const WIDGET_URL: &str = "widget.html";

// ---------------------------------------------------------------------------------------- view

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WidgetProviderState {
    /// Current data.
    Live,
    /// The last valid values, older than the provider's live window (shown with their age).
    Stale,
    /// No values yet (or none usable: not set up, API billing, ...). Never shown as 0 % or 100 %.
    Waiting,
    /// Monitoring is paused: the last known values, marked as such.
    Paused,
    /// The provider cannot be read (not signed in, not found, no valid data for 30 min).
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    Allowed,
    /// Codex reported `ordinaryUsageAllowed = false` (never inferred from remaining %).
    Blocked,
    /// No authoritative flag (always the case for Claude).
    Unknown,
}

/// Why a provider's values are not current, for the words beside them. Display only: `state`,
/// severity and reasons do not read it. A paused view keeps it, because the data is as old as it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StaleKind {
    /// Codex: older than the "may be stale" age, still counted as current.
    Maybe,
    /// Codex: older than the stale age, or no valid data for 30 min.
    Stale,
    /// Claude: a passive source between updates, its normal resting state. Not an alarm.
    NotLive,
}

/// When a window resets, exactly as the dashboard's `WindowStatus` says, so the two cannot disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WidgetReset {
    /// "confirmed", "sliding", "unconfirmed", "passed" or "missing" (no such window in the data).
    pub schedule: &'static str,
    /// Only with "confirmed": nothing else may become a countdown.
    pub at_ms: Option<i64>,
    /// See `WindowStatus::reset_precision_ms`.
    pub precision_ms: Option<i64>,
}

impl WidgetReset {
    pub const MISSING: Self = Self { schedule: "missing", at_ms: None, precision_ms: None };

    pub fn of(window: Option<&WindowStatus>) -> Self {
        match window {
            Some(w) if w.present => Self { schedule: w.reset_schedule, at_ms: w.confirmed_reset_at_ms, precision_ms: w.reset_precision_ms },
            _ => Self::MISSING,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WidgetProvider {
    pub id: &'static str,
    pub display_name: &'static str,
    /// One letter for the compact layout; the full name is always the accessible label.
    pub short_label: &'static str,
    /// Remaining %, exactly as in the tray and dashboard. None = no value ("—").
    pub five_hour_remaining: Option<f64>,
    pub weekly_remaining: Option<f64>,
    pub five_hour_tone: DisplayTone,
    pub weekly_tone: DisplayTone,
    pub five_hour_reset: WidgetReset,
    pub weekly_reset: WidgetReset,
    /// False: the provider's 5-hour row is not drawn at all (different from a value of "—").
    pub show_five_hour: bool,
    pub state: WidgetProviderState,
    /// None: the values are current, or there are none.
    pub stale_kind: Option<StaleKind>,
    pub availability: Availability,
    /// The provider's severity from the product layer (thresholds are never re-evaluated here).
    pub severity: Severity,
    pub updated_at_ms: Option<i64>,
    /// Short status text when not live, localized in Rust (the tray shows the same words).
    pub note: Option<String>,
    /// `note` in a form that fits the compact name slot, when the full words do not. None otherwise,
    /// a limit included. `note` stays the whole sentence for the detailed layout and the tooltip.
    pub note_short: Option<String>,
    /// Only on a waiting row: why it has no data when that is not really a wait. None: the true wait.
    pub waiting_note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WidgetView {
    pub generated_at_ms: i64,
    pub paused: bool,
    pub global_severity: Severity,
    /// The effective application language and theme (the widget has no settings of its own).
    pub language: Lang,
    pub theme: ThemePref,
    pub mode: WidgetMode,
    pub opacity: u8,
    pub locked: bool,
    pub providers: Vec<WidgetProvider>,
    /// Newest update among the providers.
    pub updated_at_ms: Option<i64>,
    /// Provider id -> a data URL of the user's own icon for the ring layout (`custom_icons`).
    /// Missing: the built-in icon.
    pub custom_icons: BTreeMap<&'static str, String>,
}

fn short_label(provider: &str) -> &'static str {
    match provider {
        "codex" => "C",
        "claude" => "A",
        _ => "?",
    }
}

/// Builds the widget view from the tray model. `claude_updated_at_ms` is the time of the newest
/// Claude observation (the Codex time is in the tray model). Resets stay "missing" until `with_resets`.
pub fn build_view(tray: &TrayStatusModel, claude_updated_at_ms: Option<i64>, settings: &AppSettings, now_ms: i64) -> WidgetView {
    let lang = settings.effective_language();
    let providers: Vec<WidgetProvider> = tray
        .providers
        .iter()
        .map(|line| match line.provider {
            "codex" => codex_provider(tray, line, lang),
            _ => other_provider(tray, line, claude_updated_at_ms),
        })
        .collect();
    let w = &settings.desktop_widget;
    WidgetView {
        generated_at_ms: now_ms,
        paused: tray.paused,
        global_severity: tray.severity,
        language: lang,
        theme: settings.theme,
        mode: w.mode,
        opacity: w.opacity_percent(),
        locked: w.locked,
        updated_at_ms: providers.iter().filter_map(|p| p.updated_at_ms).max(),
        providers,
        custom_icons: BTreeMap::new(),
    }
}

/// Adds each provider's resets from the `WindowStatus` values the dashboard shows.
pub fn with_resets(mut view: WidgetView, codex: &[WindowStatus], claude: &[WindowStatus]) -> WidgetView {
    for p in &mut view.providers {
        let windows = if p.id == "codex" { codex } else { claude };
        let reset = |t: WindowType| WidgetReset::of(windows.iter().find(|w| w.window_type == t));
        p.five_hour_reset = reset(WindowType::FiveHour);
        p.weekly_reset = reset(WindowType::Weekly);
    }
    view
}

fn codex_provider(tray: &TrayStatusModel, line: &ProviderTrayLine, lang: Lang) -> WidgetProvider {
    let s = strings(lang);
    let has = |r: &str| line.reasons.contains(&r);
    let unreadable = has("not_authenticated") || has("provider_unavailable") || has("no_valid_snapshot_30m");
    let state = if tray.paused {
        WidgetProviderState::Paused
    } else if unreadable {
        WidgetProviderState::Error
    } else {
        match tray.freshness {
            Freshness::Fresh => WidgetProviderState::Live,
            Freshness::MaybeStale | Freshness::Stale => WidgetProviderState::Stale,
            Freshness::NoData => WidgetProviderState::Waiting,
        }
    };
    let stale_kind = match tray.freshness {
        Freshness::MaybeStale => Some(StaleKind::Maybe),
        Freshness::Stale => Some(StaleKind::Stale),
        _ if has("no_valid_snapshot_30m") && tray.updated_at_ms.is_some() => Some(StaleKind::Stale),
        _ => None,
    };
    let availability = match tray.availability {
        Some(true) => Availability::Allowed,
        Some(false) => Availability::Blocked,
        None => Availability::Unknown,
    };
    let (note, note_short) = if has("not_authenticated") {
        (Some(s.not_signed_in), Some(s.not_signed_in_short))
    } else if has("provider_unavailable") && line.not_found {
        (Some(s.codex_not_found), Some(i18n::claude_state_short(lang, "not_found")))
    } else if has("provider_unavailable") && line.unverified_version {
        (Some(s.codex_unverified), Some(s.codex_unverified_short))
    } else if has("provider_unavailable") {
        (Some(s.provider_unavailable), Some(s.provider_unavailable_short))
    } else if availability == Availability::Blocked {
        (Some(s.limit_reached), None)
    } else {
        (None, None)
    };
    // Polls failing with nothing stored ever: they keep retrying, but "waiting" alone hides that.
    let waiting_note =
        (state == WidgetProviderState::Waiting && line.polls_failing && !line.has_data).then(|| s.provider_unavailable_short.to_string());
    WidgetProvider {
        id: line.provider,
        display_name: line.display_name,
        short_label: short_label(line.provider),
        five_hour_remaining: line.five_hour_remaining,
        weekly_remaining: line.weekly_remaining,
        five_hour_tone: line.five_hour_tone,
        weekly_tone: line.weekly_tone,
        five_hour_reset: WidgetReset::MISSING,
        weekly_reset: WidgetReset::MISSING,
        show_five_hour: line.show_five_hour,
        state,
        stale_kind,
        availability,
        severity: line.severity,
        updated_at_ms: tray.updated_at_ms,
        note: note.map(str::to_string),
        note_short: note_short.map(str::to_string),
        waiting_note,
    }
}

/// Claude (and any later provider without an authoritative availability flag).
fn other_provider(tray: &TrayStatusModel, line: &ProviderTrayLine, updated_at_ms: Option<i64>) -> WidgetProvider {
    let state = if tray.paused {
        WidgetProviderState::Paused
    } else if line.live {
        WidgetProviderState::Live
    } else if line.has_data {
        WidgetProviderState::Stale
    } else {
        WidgetProviderState::Waiting
    };
    WidgetProvider {
        id: line.provider,
        display_name: line.display_name,
        short_label: short_label(line.provider),
        five_hour_remaining: line.five_hour_remaining,
        weekly_remaining: line.weekly_remaining,
        five_hour_tone: line.five_hour_tone,
        weekly_tone: line.weekly_tone,
        five_hour_reset: WidgetReset::MISSING,
        weekly_reset: WidgetReset::MISSING,
        show_five_hour: line.show_five_hour,
        state,
        stale_kind: (!line.live && line.has_data).then_some(StaleKind::NotLive),
        availability: Availability::Unknown,
        severity: line.severity,
        updated_at_ms,
        note: line.note.clone(),
        note_short: line.note_short.clone(),
        waiting_note: line.waiting_note.clone().filter(|_| state == WidgetProviderState::Waiting),
    }
}

// ------------------------------------------------------------------------ foreground (P0 rule)

/// What the shell knows about its windows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SurfaceStates {
    pub main_visible: bool,
    pub main_minimized: bool,
    pub widget_visible: bool,
    pub widget_focused: bool,
}

/// Monitoring cadence (Codex 60 s vs. 300 s, Claude active 5 vs. 15 min) follows the main window
/// only. The widget is a glance surface: visible, focused or always-on-top, it never puts
/// monitoring into foreground cadence (Phase 3.7 §18–§19).
pub fn provider_foreground(s: &SurfaceStates) -> bool {
    s.main_visible && !s.main_minimized
}

// ---------------------------------------------------------------------------------- lifecycle

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidgetEvent {
    /// QuotaDock started (after monitoring started).
    Startup,
    /// Any settings save (from the page or the widget's own menu).
    SettingsChanged,
    /// A second QuotaDock launch was forwarded to this one: it shows the main window only.
    SecondInstance,
    /// The user closed the widget window itself.
    WidgetClosed,
    /// The main window went to the tray.
    MainHidden,
    Paused,
    Resumed,
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidgetAction {
    Create,
    /// Apply settings to the existing window (always-on-top, size, look).
    Update,
    Destroy,
    /// Re-read the view (the window exists and nothing else changed).
    Refresh,
    Keep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WidgetDecision {
    pub action: WidgetAction,
    /// Store `desktopWidget.enabled = false` (the user closed the widget).
    pub disable_setting: bool,
}

/// What to do with the widget window. Only `Startup` and `SettingsChanged` with the widget enabled
/// can create it, and only when no widget window exists.
pub fn decide(event: WidgetEvent, enabled: bool, window_exists: bool) -> WidgetDecision {
    use WidgetAction::*;
    let action = match event {
        WidgetEvent::Startup if enabled && !window_exists => Create,
        WidgetEvent::Startup => Keep,
        WidgetEvent::SettingsChanged => match (enabled, window_exists) {
            (true, false) => Create,
            (true, true) => Update,
            (false, true) => Destroy,
            (false, false) => Keep,
        },
        WidgetEvent::SecondInstance | WidgetEvent::MainHidden => Keep,
        WidgetEvent::WidgetClosed | WidgetEvent::Quit if window_exists => Destroy,
        WidgetEvent::WidgetClosed | WidgetEvent::Quit => Keep,
        WidgetEvent::Paused | WidgetEvent::Resumed if window_exists => Refresh,
        WidgetEvent::Paused | WidgetEvent::Resumed => Keep,
    };
    WidgetDecision { action, disable_setting: event == WidgetEvent::WidgetClosed }
}

// -------------------------------------------------------------------------------- window spec

/// What decides the widget's height: the providers turned on, and how many of them show 5-hour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WidgetRows {
    pub providers: usize,
    pub five_hour: usize,
}

impl WidgetRows {
    /// A provider that is off counts for nothing, whatever its own 5-hour switch says.
    pub fn of(s: &AppSettings) -> Self {
        let on = [(s.codex.enabled, s.codex.show_five_hour), (s.claude.enabled, s.claude.show_five_hour)];
        Self { providers: on.iter().filter(|p| p.0).count(), five_hour: on.iter().filter(|p| p.0 && p.1).count() }
    }
}

/// Logical size of the widget window: exactly the panel's content (widget.css), so no empty strip
/// is left at the bottom. The height never depends on the data: every text stays on its one line.
/// Compact: 38 = padding 16 + border 2 + header 20; a row is 20 + gap 4 (a hidden 5-hour value only
/// leaves its cell empty). Detailed, the ring layout, has no header: padding 16 + border 2, a provider
/// 60 (name 16, value 28, reset 16) plus 16 for its 5-hour line, and 17 between two providers
/// (margin 8, divider 1, padding 8). No provider turned on: only the one-line hint (16 + 2 + 20).
pub fn widget_size(mode: WidgetMode, rows: WidgetRows) -> (f64, f64) {
    let (n, five) = (rows.providers.min(2) as f64, rows.five_hour.min(2) as f64);
    match mode {
        _ if rows.providers == 0 => (224.0, 38.0),
        WidgetMode::Compact => (224.0, 38.0 + 24.0 * n),
        WidgetMode::Detailed => (224.0, 18.0 + 60.0 * n + 17.0 * (n - 1.0) + 16.0 * five),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WidgetWindowSpec {
    pub label: &'static str,
    pub url: &'static str,
    pub title: &'static str,
    pub logical_size: (f64, f64),
    pub decorations: bool,
    pub transparent: bool,
    /// No native shadow: it would be drawn around the transparent rectangle.
    pub shadow: bool,
    pub skip_taskbar: bool,
    pub resizable: bool,
    pub maximizable: bool,
    pub minimizable: bool,
    /// Created without taking focus from the user's work.
    pub focused: bool,
    pub always_on_top: bool,
}

pub fn window_spec(settings: &DesktopWidgetSettings, rows: WidgetRows) -> WidgetWindowSpec {
    WidgetWindowSpec {
        label: WIDGET_LABEL,
        url: WIDGET_URL,
        title: "QuotaDock Widget",
        logical_size: widget_size(settings.mode, rows),
        decorations: false,
        transparent: true,
        shadow: false,
        skip_taskbar: true,
        resizable: false,
        maximizable: false,
        minimizable: false,
        focused: false,
        always_on_top: settings.always_on_top,
    }
}

// ---------------------------------------------------------------------------------- placement

/// A monitor as the shell sees it (physical pixels, as Windows reports them).
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorGeom {
    pub name: Option<String>,
    pub primary: bool,
    pub scale: f64,
    /// Work area (the monitor minus the taskbar).
    pub work_x: i32,
    pub work_y: i32,
    pub work_w: u32,
    pub work_h: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub x: i32,
    pub y: i32,
    pub monitor_index: usize,
    /// The saved place was not usable as is (monitor gone, or off its work area) and was moved.
    pub recovered: bool,
}

/// Distance from the work-area edges for the default place, logical pixels.
pub const EDGE_MARGIN: f64 = 16.0;

/// The monitor holding the centre of a window at physical `(x, y)`, or the nearest one.
fn nearest_monitor<'a>(x: i32, y: i32, size: (u32, u32), monitors: impl Iterator<Item = &'a MonitorGeom>) -> Option<&'a MonitorGeom> {
    let (cx, cy) = (x as i64 + size.0 as i64 / 2, y as i64 + size.1 as i64 / 2);
    let distance = |m: &MonitorGeom| {
        let (l, t) = (m.work_x as i64, m.work_y as i64);
        let (r, b) = (l + m.work_w as i64, t + m.work_h as i64);
        let dx = if cx < l { l - cx } else if cx > r { cx - r } else { 0 };
        let dy = if cy < t { t - cy } else if cy > b { cy - b } else { 0 };
        dx * dx + dy * dy
    };
    monitors.min_by_key(|m| distance(m))
}

fn scaled(v: f64, scale: f64) -> i32 {
    (v * scale).round() as i32
}

/// Where the widget goes: its saved monitor and offset when that monitor still exists (clamped
/// into the work area), otherwise near the bottom-right corner of the primary monitor's work area,
/// EDGE_MARGIN from its right and bottom edges. A recovered place uses the same default.
/// Never returns a place outside every work area.
pub fn place(saved: Option<&WidgetPosition>, monitors: &[MonitorGeom], logical_size: (f64, f64)) -> Option<Placement> {
    let saved_monitor = saved.and_then(|p| monitors.iter().position(|m| m.name.as_deref() == Some(p.monitor.as_str())));
    match (saved, saved_monitor) {
        (Some(p), Some(i)) => {
            let m = &monitors[i];
            let (w, h) = (scaled(logical_size.0, m.scale), scaled(logical_size.1, m.scale));
            let x = m.work_x + scaled(p.x as f64, m.scale);
            let y = m.work_y + scaled(p.y as f64, m.scale);
            let max_x = m.work_x + (m.work_w as i32 - w).max(0);
            let max_y = m.work_y + (m.work_h as i32 - h).max(0);
            let (cx, cy) = (x.clamp(m.work_x, max_x), y.clamp(m.work_y, max_y));
            Some(Placement { x: cx, y: cy, monitor_index: i, recovered: (cx, cy) != (x, y) })
        }
        _ => {
            let i = monitors.iter().position(|m| m.primary).or(if monitors.is_empty() { None } else { Some(0) })?;
            let m = &monitors[i];
            let (w, h, margin) = (scaled(logical_size.0, m.scale), scaled(logical_size.1, m.scale), scaled(EDGE_MARGIN, m.scale));
            let x = (m.work_x + m.work_w as i32 - w - margin).max(m.work_x);
            let y = (m.work_y + m.work_h as i32 - h - margin).max(m.work_y);
            Some(Placement { x, y, monitor_index: i, recovered: saved.is_some() })
        }
    }
}

/// The position to remember for a window at physical `(x, y)` with physical `size`: the monitor
/// holding the window's centre (or the nearest one) and the logical offset from its work area.
pub fn save_position(x: i32, y: i32, size: (u32, u32), monitors: &[MonitorGeom]) -> Option<WidgetPosition> {
    let m = nearest_monitor(x, y, size, monitors.iter().filter(|m| m.name.is_some()))?;
    let off = |v: i32, origin: i32| (((v - origin) as f64) / m.scale).round().max(0.0) as i32;
    Some(WidgetPosition { monitor: m.name.clone()?, x: off(x, m.work_x), y: off(y, m.work_y) })
}

/// The top edge after the window's height changes from `old_h` to `new_h`, all in physical pixels.
/// At the bottom of the work area (no more than EDGE_MARGIN from it, plus one logical pixel) the
/// bottom edge stays where it is, so going to a taller layout and back returns to the same place, the
/// default one included. The extra pixel is the error a remembered place can pick up at a fractional
/// scale, going through the logical offset of `save_position` and back through `place`; a gap in that
/// pixel goes back to EDGE_MARGIN, so the error cannot build up. Anywhere else the top edge stays.
/// Compared in physical pixels, as `place` computes them, so a rounded size at a fractional scale still
/// matches. A window hanging below the work area counts as touching it. Never above the work area,
/// also when the new height does not fit.
pub fn resized_y(old_y: i32, old_h: u32, new_h: u32, m: &MonitorGeom) -> i32 {
    if old_h == new_h {
        return old_y;
    }
    let bottom = m.work_y + m.work_h as i32;
    let gap = bottom - (old_y + old_h as i32);
    if gap > scaled(EDGE_MARGIN + 1.0, m.scale) {
        return old_y;
    }
    (bottom - gap.clamp(0, scaled(EDGE_MARGIN, m.scale)) - new_h as i32).max(m.work_y)
}

/// A move that goes with a resize.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResizeMove {
    pub x: i32,
    pub y: i32,
    /// Move before resizing. Going up moves first and going down resizes first: the two calls are
    /// separate, and this order keeps the window's bottom edge from dipping below where it ends up.
    pub move_first: bool,
}

/// What resizing a window at physical `(x, y)` with physical size `old` to `logical_size` takes:
/// `resized_y`, then the clamp into the work area that `place` applies. None: it stays where it is.
pub fn resized_position(x: i32, y: i32, old: (u32, u32), logical_size: (f64, f64), monitors: &[MonitorGeom]) -> Option<ResizeMove> {
    let m = nearest_monitor(x, y, old, monitors.iter())?;
    let new = (scaled(logical_size.0, m.scale) as u32, scaled(logical_size.1, m.scale) as u32);
    let new_y = resized_y(y, old.1, new.1, m);
    let clamped = save_position(x, new_y, new, monitors).and_then(|here| place(Some(&here), monitors, logical_size)).filter(|p| p.recovered);
    let (to_x, to_y) = clamped.map_or((x, new_y), |p| (p.x, p.y));
    ((to_x, to_y) != (x, y)).then_some(ResizeMove { x: to_x, y: to_y, move_first: to_y < y })
}

// ------------------------------------------------------------------------------- custom icons

/// Largest icon file read; anything bigger is ignored.
pub const CUSTOM_ICON_MAX_BYTES: u64 = 256 * 1024;

/// The user's own icon for each provider, for the centre of its ring: `<data>/icons/codex.svg` or
/// `.png` (likewise `claude`), as a data URL. Nothing is shipped or downloaded; a missing, oversized
/// or malformed file just leaves the built-in icon. The page shows it as an `<img>`, where an SVG
/// can run no script and load nothing.
pub fn custom_icons(data_dir: &Path) -> BTreeMap<&'static str, String> {
    let mut out = BTreeMap::new();
    for id in ["codex", "claude"] {
        for (ext, mime) in [("svg", "image/svg+xml"), ("png", "image/png")] {
            let path = data_dir.join("icons").join(format!("{id}.{ext}"));
            let Ok(meta) = std::fs::metadata(&path) else { continue };
            if !meta.is_file() || meta.len() > CUSTOM_ICON_MAX_BYTES {
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else { continue };
            if icon_looks_valid(&bytes, ext) {
                out.insert(id, format!("data:{mime};base64,{}", base64(&bytes)));
                break;
            }
        }
    }
    out
}

fn icon_looks_valid(bytes: &[u8], ext: &str) -> bool {
    if ext == "png" {
        return bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    }
    let Ok(text) = std::str::from_utf8(bytes) else { return false };
    let text = text.trim_start_matches('\u{feff}').trim_start();
    (text.starts_with("<svg") || text.starts_with("<?xml")) && text.contains("<svg")
}

fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            out.push(if i <= chunk.len() { ABC[(n >> shift & 63) as usize] as char } else { '=' });
        }
    }
    out
}

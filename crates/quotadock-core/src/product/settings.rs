//! User settings, persisted as one JSON value in the `settings` table (key `app_settings`).
//! Unknown or missing fields fall back to the defaults below, so old rows stay readable.

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

pub const SETTINGS_KEY: &str = "app_settings";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LanguagePref {
    #[default]
    #[serde(rename = "system")]
    System,
    #[serde(rename = "zh-CN")]
    ZhCn,
    #[serde(rename = "en")]
    En,
}

/// The language actually used for text produced by Rust (tray, notifications).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Lang {
    #[serde(rename = "zh-CN")]
    ZhCn,
    #[serde(rename = "en")]
    En,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThemePref {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NotificationSettings {
    pub weekly_quota: bool,
    pub five_hour_quota: bool,
    pub forecast_weekly: bool,
    pub forecast_five_hour: bool,
    pub availability_restored: bool,
    pub quota_refresh: bool,
    pub reset_credit: bool,
    /// "Notify when allowance refreshes" (NaturalRolloverObserved). Off by default: it would fire every 5 h.
    pub allowance_refresh: bool,
    pub provider_errors: bool,
    /// Claude reminders (the fields above are Codex's, kept for compatibility with saved settings).
    pub claude: ClaudeNotificationSettings,
}

/// Same defaults as Codex: weekly reminders on, 5-hour off, refresh notices off. The Claude forecast
/// notice is off by default too: passive Claude data can be sparse, so AtRisk is shown, not pushed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ClaudeNotificationSettings {
    pub weekly_quota: bool,
    pub five_hour_quota: bool,
    pub forecast_weekly: bool,
    pub allowance_refresh: bool,
}

impl Default for ClaudeNotificationSettings {
    fn default() -> Self {
        Self { weekly_quota: true, five_hour_quota: false, forecast_weekly: false, allowance_refresh: false }
    }
}

/// On by default, so settings saved before these switches existed keep monitoring Codex.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CodexSettings {
    /// Show and monitor Codex. Off: no reads, no app-server, no reminders; stored history is kept.
    pub enabled: bool,
    /// Show the 5-hour window. Off: it leaves every view and raises no reminder or tray level; it is
    /// still read and stored, and the reminder settings for it are kept. A plan without this limit.
    pub show_five_hour: bool,
}

impl Default for CodexSettings {
    fn default() -> Self {
        Self { enabled: true, show_five_hour: true }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ClaudeSettings {
    /// Show and monitor Claude. Passive monitoring only reads QuotaDock's own bridge file.
    pub enabled: bool,
    /// Same as `CodexSettings::show_five_hour`.
    pub show_five_hour: bool,
    /// `claude -p /usage` fallback when passive data is stale. `None` = never chosen by the user,
    /// which means on (Windows V1 default: the Claude Desktop Code tab runs no status line). Any
    /// stored value is an explicit choice and is kept; this includes a `false` written by Phase 3.5
    /// builds, which cannot be told apart from a deliberate "off".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_refresh: Option<bool>,
}

impl Default for ClaudeSettings {
    fn default() -> Self {
        Self { enabled: true, show_five_hour: true, active_refresh: None }
    }
}

impl ClaudeSettings {
    pub fn active_refresh_enabled(&self) -> bool {
        self.active_refresh.unwrap_or(true)
    }
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self {
            weekly_quota: true,
            five_hour_quota: false,
            forecast_weekly: true,
            forecast_five_hour: false,
            availability_restored: true,
            quota_refresh: true,
            reset_credit: true,
            allowance_refresh: false,
            provider_errors: true,
            claude: ClaudeNotificationSettings::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum WidgetMode {
    #[default]
    Compact,
    Detailed,
}

/// Where the desktop widget was left: the monitor (its Windows device name, e.g. `\\.\DISPLAY2`)
/// and the offset of the widget's top-left corner from that monitor's work area, in logical
/// pixels, so a DPI change keeps the relative place.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WidgetPosition {
    pub monitor: String,
    pub x: i32,
    pub y: i32,
}

/// Desktop widget (Phase 3.7). Off by default; presentation only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DesktopWidgetSettings {
    pub enabled: bool,
    pub always_on_top: bool,
    /// 50–100 %. Applied to the widget's background only; text stays fully opaque.
    pub opacity: u8,
    pub mode: WidgetMode,
    pub remember_position: bool,
    /// Prevents dragging (not click-through).
    pub locked: bool,
    /// Owned by the backend (saved after a drag); a settings save from the page keeps the stored one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<WidgetPosition>,
}

impl Default for DesktopWidgetSettings {
    fn default() -> Self {
        Self { enabled: false, always_on_top: true, opacity: 85, mode: WidgetMode::Compact, remember_position: true, locked: false, position: None }
    }
}

impl DesktopWidgetSettings {
    pub const MIN_OPACITY: u8 = 50;
    pub const MAX_OPACITY: u8 = 100;

    pub fn opacity_percent(&self) -> u8 {
        self.opacity.clamp(Self::MIN_OPACITY, Self::MAX_OPACITY)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppSettings {
    pub language: LanguagePref,
    pub theme: ThemePref,
    pub close_to_tray: bool,
    /// The "QuotaDock keeps running in the tray" hint was dismissed with "Don't show again".
    pub close_hint_dismissed: bool,
    pub launch_at_startup: bool,
    pub show_diagnostics: bool,
    pub notifications: NotificationSettings,
    pub codex: CodexSettings,
    pub claude: ClaudeSettings,
    pub desktop_widget: DesktopWidgetSettings,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            language: LanguagePref::System,
            theme: ThemePref::System,
            close_to_tray: true,
            close_hint_dismissed: false,
            launch_at_startup: false,
            show_diagnostics: false,
            notifications: NotificationSettings::default(),
            codex: CodexSettings::default(),
            claude: ClaudeSettings::default(),
            desktop_widget: DesktopWidgetSettings::default(),
        }
    }
}

impl AppSettings {
    pub fn effective_language(&self) -> Lang {
        match self.language {
            LanguagePref::ZhCn => Lang::ZhCn,
            LanguagePref::En => Lang::En,
            LanguagePref::System => system_language(),
        }
    }
}

/// Windows UI language: Chinese (any variant) -> zh-CN, anything else -> English.
pub fn system_language() -> Lang {
    #[cfg(windows)]
    {
        // LANGID primary language id 0x04 = LANG_CHINESE.
        let id = unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() };
        if id & 0x3ff == 0x04 {
            return Lang::ZhCn;
        }
        Lang::En
    }
    #[cfg(not(windows))]
    {
        match std::env::var("LANG") {
            Ok(l) if l.starts_with("zh") => Lang::ZhCn,
            _ => Lang::En,
        }
    }
}

pub fn load_settings(conn: &Connection) -> rusqlite::Result<AppSettings> {
    let json: Option<String> =
        conn.query_row("SELECT value_json FROM settings WHERE key = ?1", params![SETTINGS_KEY], |r| r.get(0)).optional()?;
    Ok(json.and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default())
}

pub fn save_settings(conn: &Connection, s: &AppSettings) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO settings (key, value_json, updated_at_ms) VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at_ms = excluded.updated_at_ms",
        params![SETTINGS_KEY, serde_json::to_string(s).unwrap_or_default(), crate::time::now_ms()],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_stays_on_for_settings_saved_before_the_switch_existed() {
        let parse = |json: &str| serde_json::from_str::<AppSettings>(json).unwrap().codex.enabled;
        assert!(AppSettings::default().codex.enabled);
        assert!(parse("{}"));
        assert!(parse(r#"{"claude":{"enabled":false},"launchAtStartup":true}"#));
        assert!(!parse(r#"{"codex":{"enabled":false}}"#));
        let off = AppSettings { codex: CodexSettings { enabled: false, ..Default::default() }, ..Default::default() };
        assert!(!parse(&serde_json::to_string(&off).unwrap()), "the choice survives a save");
    }

    #[test]
    fn five_hour_stays_shown_for_saved_settings_and_each_provider_has_its_own_switch() {
        let parse = |json: &str| {
            let s = serde_json::from_str::<AppSettings>(json).unwrap();
            (s.codex.show_five_hour, s.claude.show_five_hour)
        };
        assert_eq!(parse("{}"), (true, true));
        assert_eq!(parse(r#"{"codex":{"enabled":false},"claude":{"enabled":true,"activeRefresh":false}}"#), (true, true));
        assert_eq!(parse(r#"{"codex":{"showFiveHour":false}}"#), (false, true));
        assert_eq!(parse(r#"{"claude":{"showFiveHour":false}}"#), (true, false));
        let mut s = AppSettings::default();
        s.codex.show_five_hour = false;
        let saved = serde_json::to_string(&s).unwrap();
        assert_eq!(parse(&saved), (false, true), "the choice survives a save");
        assert!(serde_json::from_str::<AppSettings>(&saved).unwrap().codex.enabled);
    }

    #[test]
    fn active_refresh_defaults_on_but_keeps_an_explicit_choice() {
        let parse = |json: &str| serde_json::from_str::<AppSettings>(json).unwrap().claude;
        // Never chosen (new install, or settings saved before the field existed): on.
        assert!(AppSettings::default().claude.active_refresh_enabled());
        assert!(parse(r#"{"claude":{"enabled":true}}"#).active_refresh_enabled());
        assert!(parse("{}").active_refresh_enabled());
        // An explicit choice survives, including a stored "off".
        assert!(!parse(r#"{"claude":{"enabled":true,"activeRefresh":false}}"#).active_refresh_enabled());
        assert!(parse(r#"{"claude":{"enabled":true,"activeRefresh":true}}"#).active_refresh_enabled());
        // "Unset" stays unset across a save of other settings.
        let saved = serde_json::to_string(&AppSettings::default()).unwrap();
        assert!(!saved.contains("activeRefresh"), "{saved}");
        assert_eq!(parse(&saved).active_refresh, None);
    }
}
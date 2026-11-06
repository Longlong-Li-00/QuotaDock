//! Phase 3.7 desktop widget: the view comes from the product layer only, the widget never moves
//! monitoring into foreground cadence, never makes provider requests, never exists twice, and
//! its position survives restarts and monitor changes. Temporary folders and fakes only.

use quotadock_core::analytics::CLASSIFIER_VERSION;
use quotadock_core::analytics::model::{EvidenceConfidence, NewEvent, QuotaEventType, WindowType};
use quotadock_core::claude::monitor::ClaudeMonitor;
use quotadock_core::codex::provider::{PollStatus, RawRead};
use quotadock_core::codex::types::TransportKind;
use quotadock_core::commands::{self, WindowStatus};
use quotadock_core::pipeline::{CoreConfig, PollOutcome, PollTrigger, QuotaDockCore};
use quotadock_core::polling::controller::PollFn;
use quotadock_core::polling::{PollingConfig, RefreshDecision, spawn_polling};
use quotadock_core::product::history::HistoryRange;
use quotadock_core::product::runtime::ProductRuntime;
use quotadock_core::product::settings::{
    AppSettings, DesktopWidgetSettings, Lang, LanguagePref, ThemePref, WidgetMode, WidgetPosition, load_settings, save_settings,
};
use quotadock_core::product::tray::{DisplayTone, Freshness, ProviderTrayLine, Severity, TrayIconKind, TrayStatusModel, display_tone, menu_line};
use quotadock_core::product::widget::{
    self, Availability, MonitorGeom, ResizeMove, StaleKind, SurfaceStates, WidgetAction, WidgetEvent, WidgetProvider, WidgetProviderState, WidgetReset, decide, place,
    provider_foreground, resized_position, resized_y, save_position, window_spec, WidgetRows,
};
use quotadock_core::time::now_ms;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;

const FAKE_CLAUDE: &str = env!("CARGO_BIN_EXE_fake-claude");

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
    core: Arc<QuotaDockCore>,
    rt: Arc<ProductRuntime>,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let core = Arc::new(QuotaDockCore::open(CoreConfig::with_database(root.join("q.sqlite3")), Arc::new(|_| {})).unwrap());
        let cwd = root.join("claude").join("cwd");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::write(cwd.join("mode.txt"), "ok").unwrap();
        let monitor = ClaudeMonitor::new(&root, &root.join("home")).with_cli_override(PathBuf::from(FAKE_CLAUDE));
        let rt = ProductRuntime::with_claude(core.clone(), monitor).unwrap();
        Self { _dir: dir, root, core, rt }
    }

    fn claude_calls(&self) -> usize {
        std::fs::read_to_string(self.root.join("claude").join("cwd").join("calls.log")).map(|s| s.lines().count()).unwrap_or(0)
    }

    fn poll_runs(&self) -> i64 {
        self.core.with_db(|c| c.query_row("SELECT COUNT(*) FROM poll_runs", [], |r| r.get(0))).unwrap()
    }

    /// One Codex observation through the real pipeline (no provider process).
    fn codex(&self, five_used: f64, weekly_used: f64, allowed: Option<bool>) {
        self.codex_at(now_ms(), five_used, weekly_used, allowed);
    }

    fn codex_at(&self, now: i64, five_used: f64, weekly_used: f64, allowed: Option<bool>) {
        self.codex_resetting(now, five_used, weekly_used, allowed, now / 1000 + 3 * 3600, now / 1000 + 4 * 86_400);
    }

    fn codex_resetting(&self, now: i64, five_used: f64, weekly_used: f64, allowed: Option<bool>, five_reset_s: i64, weekly_reset_s: i64) {
        self.codex_read(now, five_used, weekly_used, allowed, Some(five_reset_s), weekly_reset_s);
    }

    fn codex_read(&self, now: i64, five_used: f64, weekly_used: f64, allowed: Option<bool>, five_reset_s: Option<i64>, weekly_reset_s: i64) {
        let bucket = json!({
            "limitId": "codex",
            "primary": { "usedPercent": five_used, "windowDurationMins": 300, "resetsAt": five_reset_s },
            "secondary": { "usedPercent": weekly_used, "windowDurationMins": 10080, "resetsAt": weekly_reset_s },
        });
        let result = json!({ "ordinaryUsageAllowed": allowed, "rateLimits": bucket.clone(), "rateLimitsByLimitId": { "codex": bucket },
            "rateLimitResetCredits": { "availableCount": 2, "credits": null }, "accountId": "acct-widget-test" });
        let read = RawRead {
            request_started_at_ms: now - 300,
            response_received_at_ms: now,
            result,
            cli_version: "0.162.0-alpha.2".into(),
            transport: TransportKind::StdioAppServer,
            active_account_id: Some("acct-widget-test".into()),
        };
        self.core.ingest(&read, PollTrigger::Scheduled).unwrap();
    }

    /// A failed Codex read as the pipeline records it: a poll run, nothing stored.
    fn failed_poll(&self, status: &str, error: &str) {
        let at = now_ms();
        self.core.with_db(|c| quotadock_core::storage::snapshots::insert_poll_run(c, at, at, "scheduled", status, Some(error), None, None, None)).unwrap();
    }

    fn settings(&self, f: impl FnOnce(&mut AppSettings)) {
        let mut s = self.rt.settings();
        f(&mut s);
        self.rt.update_settings(s).unwrap();
    }
}

fn line(provider: &'static str, five: Option<f64>, weekly: Option<f64>, live: bool, severity: Severity, note: Option<&str>) -> ProviderTrayLine {
    ProviderTrayLine {
        provider,
        display_name: if provider == "codex" { "Codex" } else { "Claude" },
        severity,
        reasons: vec![],
        five_hour_remaining: five,
        weekly_remaining: weekly,
        five_hour_tone: display_tone(five, live),
        weekly_tone: display_tone(weekly, live),
        live,
        has_data: five.is_some() || weekly.is_some(),
        polls_failing: false,
        not_found: false,
        unverified_version: false,
        show_five_hour: true,
        note: note.map(str::to_string),
        note_short: None,
        waiting_note: None,
        menu_header: String::new(),
        menu_five_hour: String::new(),
        menu_weekly: String::new(),
    }
}

fn tray(paused: bool, availability: Option<bool>, providers: Vec<ProviderTrayLine>) -> TrayStatusModel {
    let codex = providers.iter().find(|p| p.provider == "codex").cloned();
    TrayStatusModel {
        severity: providers.iter().map(|p| p.severity).max().unwrap_or(Severity::Normal),
        icon: if paused { TrayIconKind::Paused } else { TrayIconKind::Normal },
        paused,
        availability,
        availability_unknown_persistent: false,
        five_hour_remaining: codex.as_ref().and_then(|c| c.five_hour_remaining),
        weekly_remaining: codex.as_ref().and_then(|c| c.weekly_remaining),
        updated_at_ms: Some(1_000),
        freshness: Freshness::Fresh,
        minutes_since_update: Some(0),
        reasons: vec![],
        tooltip_lines: vec![],
        menu_five_hour: String::new(),
        menu_weekly: String::new(),
        providers,
    }
}

// ------------------------------------------------------------------------------- settings

#[test]
fn widget_is_disabled_by_default() {
    let d = AppSettings::default().desktop_widget;
    assert_eq!(d, DesktopWidgetSettings::default());
    assert!(!d.enabled);
    assert!(d.always_on_top && d.remember_position && !d.locked);
    assert_eq!((d.opacity, d.mode), (85, WidgetMode::Compact));
    // Settings saved before Phase 3.7 have no desktopWidget object: defaults, widget off.
    let old: AppSettings = serde_json::from_str(r#"{"language":"zh-CN","closeToTray":true}"#).unwrap();
    assert!(!old.desktop_widget.enabled);
    assert_eq!(decide(WidgetEvent::Startup, old.desktop_widget.enabled, false).action, WidgetAction::Keep, "nothing is created");
}

#[test]
fn widget_settings_survive_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("q.sqlite3");
    let s = AppSettings {
        desktop_widget: DesktopWidgetSettings {
            enabled: true,
            always_on_top: false,
            opacity: 60,
            mode: WidgetMode::Detailed,
            remember_position: true,
            locked: true,
            position: Some(WidgetPosition { monitor: r"\\.\DISPLAY2".into(), x: 1200, y: 40 }),
        },
        ..AppSettings::default()
    };
    {
        let core = QuotaDockCore::open(CoreConfig::with_database(path.clone()), Arc::new(|_| {})).unwrap();
        core.with_db(|c| save_settings(c, &s)).unwrap();
    }
    let core = QuotaDockCore::open(CoreConfig::with_database(path), Arc::new(|_| {})).unwrap();
    assert_eq!(core.with_db(load_settings).unwrap(), s);
    // Out-of-range opacity from an edited file is clamped when used.
    let odd: DesktopWidgetSettings = serde_json::from_str(r#"{"opacity":20}"#).unwrap();
    assert_eq!(odd.opacity_percent(), 50);
}

// ------------------------------------------------------------------------------------- view

#[test]
fn widget_shows_remaining_not_used() {
    // Codex used 27 % (5h) and 6 % (weekly): the tray model carries remaining 73 / 94.
    let t = tray(false, Some(true), vec![line("codex", Some(73.0), Some(94.0), true, Severity::Normal, None)]);
    let v = widget::build_view(&t, None, &AppSettings::default(), 0);
    let c = &v.providers[0];
    assert_eq!((c.five_hour_remaining, c.weekly_remaining), (Some(73.0), Some(94.0)));
    assert_eq!((c.id, c.display_name, c.short_label), ("codex", "Codex", "C"));
    assert_eq!((c.state, c.availability), (WidgetProviderState::Live, Availability::Allowed));
}

#[test]
fn widget_handles_missing_claude() {
    let t = tray(
        false,
        Some(true),
        vec![
            line("codex", Some(73.0), Some(94.0), true, Severity::Normal, None),
            line("claude", None, None, false, Severity::Normal, Some("等待 Claude Code")),
        ],
    );
    let v = widget::build_view(&t, None, &AppSettings::default(), 0);
    let a = &v.providers[1];
    assert_eq!((a.short_label, a.display_name), ("A", "Claude"));
    assert_eq!(a.state, WidgetProviderState::Waiting);
    assert_eq!((a.five_hour_remaining, a.weekly_remaining), (None, None), "never 0 % or 100 %");
    assert_eq!(a.note.as_deref(), Some("等待 Claude Code"));
    assert_eq!(a.availability, Availability::Unknown);
}

#[test]
fn widget_handles_stale_claude() {
    let t = tray(
        false,
        Some(true),
        vec![
            line("codex", Some(73.0), Some(94.0), true, Severity::Normal, None),
            line("claude", Some(68.0), Some(66.0), false, Severity::Normal, Some("Not live")),
        ],
    );
    let v = widget::build_view(&t, Some(42), &AppSettings::default(), 0);
    let a = &v.providers[1];
    assert_eq!(a.state, WidgetProviderState::Stale);
    assert_eq!((a.five_hour_remaining, a.weekly_remaining), (Some(68.0), Some(66.0)), "last valid values kept");
    assert_eq!(a.updated_at_ms, Some(42), "the age is shown, not hidden");
}

#[test]
fn stale_kind_says_how_old_the_codex_data_is() {
    let kind = |freshness: Freshness, reasons: Vec<&'static str>, updated: Option<i64>| {
        let mut l = line("codex", Some(73.0), Some(94.0), true, Severity::Normal, None);
        l.reasons = reasons;
        let mut t = tray(false, Some(true), vec![l]);
        t.freshness = freshness;
        t.updated_at_ms = updated;
        let v = widget::build_view(&t, None, &AppSettings::default(), 0);
        (v.providers[0].state, v.providers[0].stale_kind)
    };
    assert_eq!(kind(Freshness::Fresh, vec![], Some(1)), (WidgetProviderState::Live, None));
    assert_eq!(kind(Freshness::NoData, vec![], None), (WidgetProviderState::Waiting, None));
    assert_eq!(kind(Freshness::MaybeStale, vec![], Some(1)), (WidgetProviderState::Stale, Some(StaleKind::Maybe)));
    assert_eq!(kind(Freshness::Stale, vec![], Some(1)), (WidgetProviderState::Stale, Some(StaleKind::Stale)));
    assert_eq!(kind(Freshness::Stale, vec!["no_valid_snapshot_30m"], Some(1)), (WidgetProviderState::Error, Some(StaleKind::Stale)));
    assert_eq!(kind(Freshness::Fresh, vec!["no_valid_snapshot_30m"], Some(1)), (WidgetProviderState::Error, Some(StaleKind::Stale)));
    assert_eq!(kind(Freshness::NoData, vec!["no_valid_snapshot_30m"], None), (WidgetProviderState::Error, None), "no known age: nothing to say");
}

#[test]
fn stale_kind_says_not_live_for_claude_with_data_only() {
    let kind = |five: Option<f64>, live: bool| {
        let t = tray(false, None, vec![line("claude", five, five, live, Severity::Normal, None)]);
        let v = widget::build_view(&t, Some(42), &AppSettings::default(), 0);
        (v.providers[0].state, v.providers[0].stale_kind)
    };
    assert_eq!(kind(Some(68.0), true), (WidgetProviderState::Live, None));
    assert_eq!(kind(Some(68.0), false), (WidgetProviderState::Stale, Some(StaleKind::NotLive)));
    assert_eq!(kind(None, false), (WidgetProviderState::Waiting, None));
}

#[test]
fn stale_kind_changes_neither_state_nor_severity_and_is_sent_as_a_plain_word() {
    let t = tray(
        false,
        Some(true),
        vec![
            line("codex", Some(73.0), Some(94.0), true, Severity::Normal, None),
            line("claude", Some(68.0), Some(66.0), false, Severity::Warning, Some("Not live")),
        ],
    );
    let v = widget::build_view(&t, Some(42), &AppSettings::default(), 0);
    assert_eq!(v.providers[1].severity, Severity::Warning);
    assert_eq!(v.global_severity, Severity::Warning);
    let json = serde_json::to_value(&v).unwrap();
    assert_eq!(json["providers"][0]["staleKind"], Value::Null);
    assert_eq!(json["providers"][1]["staleKind"], json!("not_live"));
    let mut maybe = t.clone();
    maybe.freshness = Freshness::MaybeStale;
    assert_eq!(serde_json::to_value(widget::build_view(&maybe, None, &AppSettings::default(), 0)).unwrap()["providers"][0]["staleKind"], json!("maybe"));
    maybe.freshness = Freshness::Stale;
    assert_eq!(serde_json::to_value(widget::build_view(&maybe, None, &AppSettings::default(), 0)).unwrap()["providers"][0]["staleKind"], json!("stale"));
}

#[test]
fn widget_handles_paused_state() {
    let t = tray(
        true,
        Some(true),
        vec![
            line("codex", Some(73.0), Some(94.0), true, Severity::Normal, None),
            line("claude", Some(68.0), Some(66.0), true, Severity::Normal, None),
        ],
    );
    let v = widget::build_view(&t, Some(5), &AppSettings::default(), 0);
    assert!(v.paused);
    assert!(v.providers.iter().all(|p| p.state == WidgetProviderState::Paused), "never shown as live while paused");
    assert_eq!(v.providers[1].weekly_remaining, Some(66.0), "last known quota kept");
    let mut old = t.clone();
    old.freshness = Freshness::Stale;
    assert_eq!(widget::build_view(&old, Some(5), &AppSettings::default(), 0).providers[0].state, WidgetProviderState::Paused, "paused wins over the kind");
}

#[test]
fn availability_is_authoritative_for_codex_and_never_inferred_for_claude() {
    let t = tray(
        false,
        Some(false),
        vec![
            line("codex", Some(40.0), Some(80.0), true, Severity::Critical, None),
            line("claude", Some(0.0), Some(0.0), true, Severity::Critical, None),
        ],
    );
    let v = widget::build_view(&t, None, &AppSettings { language: LanguagePref::En, ..Default::default() }, 0);
    assert_eq!(v.providers[0].availability, Availability::Blocked, "ordinaryUsageAllowed = false");
    assert_eq!(v.providers[0].note.as_deref(), Some("Limit reached"));
    assert_eq!(v.providers[1].availability, Availability::Unknown, "0 % remaining is not 'blocked'");
    assert_eq!(v.global_severity, Severity::Critical, "the product layer's severity, not re-evaluated");
}

#[test]
fn widget_language_follows_application() {
    let t = tray(false, None, vec![line("codex", Some(73.0), Some(94.0), true, Severity::Normal, None)]);
    let mut s = AppSettings { language: LanguagePref::ZhCn, ..Default::default() };
    assert_eq!(widget::build_view(&t, None, &s, 0).language, Lang::ZhCn);
    s.language = LanguagePref::En;
    assert_eq!(widget::build_view(&t, None, &s, 0).language, Lang::En);
    s.language = LanguagePref::System;
    assert_eq!(widget::build_view(&t, None, &s, 0).language, s.effective_language(), "the app's resolved language");
}

#[test]
fn widget_theme_follows_application() {
    let t = tray(false, None, vec![line("codex", Some(73.0), Some(94.0), true, Severity::Normal, None)]);
    for theme in [ThemePref::Dark, ThemePref::Light, ThemePref::System] {
        let s = AppSettings { theme, ..Default::default() };
        assert_eq!(widget::build_view(&t, None, &s, 0).theme, theme);
    }
}

#[test]
fn widget_view_carries_display_settings() {
    let t = tray(false, None, vec![line("codex", Some(73.0), Some(94.0), true, Severity::Normal, None)]);
    let mut s = AppSettings::default();
    s.desktop_widget.mode = WidgetMode::Detailed;
    s.desktop_widget.opacity = 100;
    s.desktop_widget.locked = true;
    let v = widget::build_view(&t, None, &s, 0);
    assert_eq!((v.mode, v.opacity, v.locked), (WidgetMode::Detailed, 100, true));
    s.desktop_widget.opacity = 12; // an edited settings file: clamped to the allowed range
    assert_eq!(widget::build_view(&t, None, &s, 0).opacity, 50);
}

#[tokio::test]
async fn widget_uses_product_state_only() {
    let e = Env::new();
    e.codex(27.0, 6.0, Some(true));
    e.rt.claude.refresh_environment(now_ms()).await;
    e.rt.claude.run_active(&e.core, now_ms()).await.unwrap().expect("claude stored");
    let tray = e.rt.tray_status().unwrap();
    let view = e.rt.widget_view().unwrap();
    assert_eq!(view.providers.len(), tray.providers.len());
    for (w, t) in view.providers.iter().zip(&tray.providers) {
        assert_eq!(w.id, t.provider);
        assert_eq!((w.five_hour_remaining, w.weekly_remaining), (t.five_hour_remaining, t.weekly_remaining), "same numbers as the tray");
        assert_eq!(w.severity, t.severity);
    }
    assert_eq!(view.providers[0].five_hour_remaining, Some(73.0));
    assert_eq!(view.global_severity, tray.severity);
}

// ------------------------------------------------------------------------- tones and resets

#[test]
fn widget_tone_comes_from_the_tray_line_and_not_from_severity() {
    // 5-hour at 4 % with its reminder off: the tray model's tone is critical, its severity normal.
    let e = Env::new();
    e.codex(96.0, 40.0, Some(true));
    e.settings(|s| s.claude.enabled = false);
    let v = e.rt.widget_view().unwrap();
    let c = &v.providers[0];
    assert_eq!((c.five_hour_tone, c.weekly_tone), (DisplayTone::Critical, DisplayTone::Normal));
    assert_eq!(c.severity, Severity::Normal);
    assert!(e.rt.tray_status().unwrap().reasons.is_empty());
    let page = serde_json::to_value(&v).unwrap();
    assert_eq!(page["providers"][0]["fiveHourTone"], "critical");
    assert_eq!(page["providers"][0]["weeklyTone"], "normal");
}

#[tokio::test]
async fn paused_widget_mutes_a_high_value_and_keeps_a_low_one() {
    let e = Env::new();
    e.codex(96.0, 40.0, Some(true));
    e.settings(|s| s.claude.enabled = false);
    e.rt.pause().await;
    let v = e.rt.widget_view().unwrap();
    assert_eq!((v.providers[0].five_hour_tone, v.providers[0].weekly_tone), (DisplayTone::Critical, DisplayTone::Muted));
}

fn status(present: bool, schedule: &'static str, reported: Option<i64>, confirmed: Option<i64>, precision: Option<i64>) -> WindowStatus {
    WindowStatus {
        window_type: WindowType::FiveHour,
        present,
        used_percent: present.then_some(20.0),
        remaining_percent: present.then_some(80.0),
        reported_reset_at_ms: reported,
        confirmed_reset_at_ms: confirmed,
        reset_schedule: schedule,
        observation_state: None,
        candidate_state: None,
        sliding_anchor: schedule == "sliding",
        snapshot_quality: "normal".into(),
        current_cycle: None,
        reset_precision_ms: precision,
    }
}

#[test]
fn only_a_confirmed_reset_carries_a_time() {
    let of = |w: &WindowStatus| WidgetReset::of(Some(w));
    assert_eq!(of(&status(true, "confirmed", Some(9), Some(9), None)), WidgetReset { schedule: "confirmed", at_ms: Some(9), precision_ms: None });
    assert_eq!(of(&status(true, "confirmed", Some(9), Some(9), Some(60_000))).precision_ms, Some(60_000));
    for schedule in ["sliding", "unconfirmed", "passed"] {
        // The server's reported time is not a confirmed one and never reaches the widget.
        assert_eq!(of(&status(true, schedule, Some(9), None, None)), WidgetReset { schedule, at_ms: None, precision_ms: None });
    }
    assert_eq!(of(&status(false, "unconfirmed", None, None, None)), WidgetReset::MISSING);
    assert_eq!(WidgetReset::of(None), WidgetReset::MISSING);
    let shape = serde_json::to_value(of(&status(true, "confirmed", Some(9), Some(9), Some(1000)))).unwrap();
    assert_eq!(shape, json!({ "schedule": "confirmed", "atMs": 9, "precisionMs": 1000 }));
}

#[tokio::test]
async fn widget_resets_are_the_ones_the_dashboard_shows() {
    let e = Env::new();
    let t = now_ms();
    let (five_s, weekly_s) = (t / 1000 + 3 * 3600, t / 1000 + 4 * 86_400);
    for i in (1..=3).rev() {
        e.codex_resetting(t - i * 60_000, 27.0, 6.0, Some(true), five_s, weekly_s);
    }
    e.rt.claude.refresh_environment(now_ms()).await;
    e.rt.claude.run_active(&e.core, now_ms()).await.unwrap().expect("claude stored");
    let view = e.rt.widget_view().unwrap();
    let dash = e.rt.dashboard().await.unwrap();
    assert_eq!(view.providers.len(), 2);
    for p in &view.providers {
        let windows = &dash.providers.iter().find(|d| d.provider == p.id).unwrap().usage.windows;
        let shown = |t: WindowType| WidgetReset::of(windows.iter().find(|w| w.window_type == t));
        assert_eq!(p.five_hour_reset, shown(WindowType::FiveHour), "{}", p.id);
        assert_eq!(p.weekly_reset, shown(WindowType::Weekly), "{}", p.id);
    }
    let codex = &view.providers[0];
    assert_eq!(codex.five_hour_reset, WidgetReset { schedule: "confirmed", at_ms: Some(five_s * 1000), precision_ms: None });
    assert_eq!(codex.weekly_reset, WidgetReset { schedule: "confirmed", at_ms: Some(weekly_s * 1000), precision_ms: None });
    let claude = &view.providers[1];
    assert_eq!((claude.five_hour_reset.schedule, claude.five_hour_reset.precision_ms), ("confirmed", Some(60_000)), "from claude -p /usage");
    assert!(claude.five_hour_reset.at_ms.is_some());
}

#[test]
fn widget_reports_missing_when_nothing_was_read_and_only_one_reading_is_unconfirmed() {
    let e = Env::new();
    let none = e.rt.widget_view().unwrap();
    for p in &none.providers {
        assert_eq!((p.five_hour_reset, p.weekly_reset), (WidgetReset::MISSING, WidgetReset::MISSING), "{}", p.id);
        assert_eq!((p.five_hour_tone, p.weekly_tone), (DisplayTone::Unknown, DisplayTone::Unknown), "{}", p.id);
    }
    e.codex(27.0, 6.0, Some(true));
    let once = e.rt.widget_view().unwrap();
    assert_eq!(once.providers[0].five_hour_reset, WidgetReset { schedule: "unconfirmed", at_ms: None, precision_ms: None }, "one reading confirms nothing");
}

#[test]
fn a_hidden_five_hour_keeps_its_tone_and_reset_in_the_view() {
    let e = Env::new();
    e.codex(96.0, 40.0, Some(true));
    e.settings(|s| s.codex.show_five_hour = false);
    let c = e.rt.widget_view().unwrap().providers.into_iter().find(|p| p.id == "codex").unwrap();
    assert!(!c.show_five_hour);
    assert_eq!(c.five_hour_tone, DisplayTone::Critical);
    assert_eq!(c.five_hour_reset.schedule, "unconfirmed");
}

// -------------------------------------------------------- no provider requests, no cadence change

#[tokio::test]
async fn widget_does_not_trigger_provider_refresh() {
    let e = Env::new();
    e.codex(27.0, 6.0, Some(true));
    let (polls, claude) = (e.poll_runs(), e.claude_calls());
    for _ in 0..50 {
        e.rt.widget_view().unwrap();
    }
    assert_eq!(e.poll_runs(), polls, "no Codex read");
    assert_eq!(e.claude_calls(), claude, "no Claude CLI call (not even --version)");
    let starts: i64 = e.core.with_db(|c| c.query_row("SELECT COUNT(*) FROM provider_sessions", [], |r| r.get(0))).unwrap();
    assert_eq!(starts, 0, "no app-server started");
    assert!(e.rt.claude.activity().last_attempt_ms.is_none());
}

#[test]
fn widget_does_not_change_foreground_monitoring_state() {
    for main_visible in [false, true] {
        for main_minimized in [false, true] {
            let expected = main_visible && !main_minimized;
            for widget_visible in [false, true] {
                for widget_focused in [false, true] {
                    let s = SurfaceStates { main_visible, main_minimized, widget_visible, widget_focused };
                    assert_eq!(provider_foreground(&s), expected, "{s:?}");
                }
            }
        }
    }
}

type Log = Arc<Mutex<Vec<Duration>>>;

fn recorder() -> (PollFn, Log) {
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let start = tokio::time::Instant::now();
    let l = log.clone();
    let f: PollFn = Arc::new(move |_| {
        let l = l.clone();
        Box::pin(async move {
            l.lock().unwrap().push(start.elapsed());
            true
        })
    });
    (f, log)
}

#[tokio::test(start_paused = true)]
async fn widget_does_not_change_codex_poll_cadence() {
    let e = Env::new();
    e.codex(27.0, 6.0, Some(true));
    let (f, log) = recorder();
    let handle = spawn_polling(f, PollingConfig::default());
    // Main window hidden, widget visible and focused: the shell asks the one rule there is.
    let surfaces = SurfaceStates { main_visible: false, main_minimized: false, widget_visible: true, widget_focused: true };
    handle.set_foreground(provider_foreground(&surfaces));
    e.rt.attach_polling(handle.clone());
    // Let the spawned controller run its immediate startup read (single-threaded test runtime).
    tokio::task::yield_now().await;
    tokio::task::yield_now().await;
    for _ in 0..20 {
        tokio::time::advance(Duration::from_secs(60)).await;
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
        e.rt.widget_view().unwrap();
    }
    let secs: Vec<u64> = log.lock().unwrap().iter().map(|d| d.as_secs()).collect();
    assert_eq!(secs, vec![0, 300, 600, 900, 1200], "background cadence (300 s) while the widget is shown and read 20 times");
    assert!(!handle.status().foreground);
    handle.stop().await;
}

#[tokio::test]
async fn widget_does_not_change_claude_active_cadence() {
    let e = Env::new();
    e.settings(|s| s.claude.active_refresh = Some(true));
    let surfaces = SurfaceStates { main_visible: false, main_minimized: false, widget_visible: true, widget_focused: true };
    let fg = provider_foreground(&surfaces);
    assert!(!fg, "the widget does not make Claude use the 5-minute foreground interval");
    e.rt.claude_tick(fg, false).await.unwrap();
    let after_first = (e.claude_calls(), e.rt.claude.activity().last_attempt_ms);
    assert!(after_first.1.is_some(), "the background tick ran the one due query");
    for _ in 0..20 {
        e.rt.widget_view().unwrap();
    }
    e.rt.claude_tick(fg, false).await.unwrap();
    assert_eq!((e.claude_calls(), e.rt.claude.activity().last_attempt_ms), after_first, "no extra query: data is fresh for 15 min");
}

// ------------------------------------------------------------------------------- lifecycle

/// The shell's window registry: labels are unique, like Tauri's.
#[derive(Default)]
struct Shell {
    widget: bool,
    max_seen: usize,
    enabled: bool,
}

impl Shell {
    fn on(&mut self, event: WidgetEvent) {
        let d = decide(event, self.enabled, self.widget);
        match d.action {
            WidgetAction::Create => {
                assert!(!self.widget, "create only when none exists ({event:?})");
                self.widget = true;
            }
            WidgetAction::Destroy => self.widget = false,
            WidgetAction::Update | WidgetAction::Refresh => assert!(self.widget),
            WidgetAction::Keep => {}
        }
        if d.disable_setting {
            self.enabled = false;
        }
        self.max_seen = self.max_seen.max(self.widget as usize);
    }
}

#[test]
fn enabling_widget_creates_one_window() {
    let mut sh = Shell::default();
    sh.on(WidgetEvent::Startup);
    assert!(!sh.widget, "off by default");
    sh.enabled = true;
    sh.on(WidgetEvent::SettingsChanged);
    assert!(sh.widget);
    for _ in 0..5 {
        sh.on(WidgetEvent::SettingsChanged); // opacity, mode, always-on-top...
    }
    assert_eq!(decide(WidgetEvent::SettingsChanged, true, true).action, WidgetAction::Update);
    assert_eq!(sh.max_seen, 1);
}

#[test]
fn single_instance_never_creates_second_widget() {
    let mut sh = Shell { enabled: true, ..Default::default() };
    sh.on(WidgetEvent::Startup);
    for _ in 0..3 {
        sh.on(WidgetEvent::SecondInstance);
        sh.on(WidgetEvent::Startup); // a stray repeated start signal must not duplicate either
        sh.on(WidgetEvent::MainHidden);
        sh.on(WidgetEvent::Paused);
        sh.on(WidgetEvent::Resumed);
    }
    // Enable / disable / enable / "restart" repeatedly.
    for _ in 0..10 {
        sh.enabled = false;
        sh.on(WidgetEvent::SettingsChanged);
        sh.enabled = true;
        sh.on(WidgetEvent::SettingsChanged);
        sh.on(WidgetEvent::Quit);
        sh.on(WidgetEvent::Startup);
    }
    assert_eq!(sh.max_seen, 1, "never more than one widget window");
    assert_eq!(decide(WidgetEvent::SecondInstance, true, false).action, WidgetAction::Keep, "a second launch never creates one");
}

#[test]
fn closing_widget_does_not_exit_monitoring() {
    let d = decide(WidgetEvent::WidgetClosed, true, true);
    assert_eq!(d.action, WidgetAction::Destroy);
    assert!(d.disable_setting, "closing the widget turns it off; QuotaDock keeps running");
    // Main window to the tray: the widget stays.
    assert_eq!(decide(WidgetEvent::MainHidden, true, true).action, WidgetAction::Keep);
}

#[test]
fn quitting_app_closes_widget() {
    assert_eq!(decide(WidgetEvent::Quit, true, true).action, WidgetAction::Destroy);
    assert!(!decide(WidgetEvent::Quit, true, true).disable_setting, "quitting keeps the widget on for the next start");
}

#[test]
fn widget_window_has_no_taskbar_entry() {
    let s = window_spec(&DesktopWidgetSettings { enabled: true, ..Default::default() }, WidgetRows { providers: 2, five_hour: 2 });
    assert!(s.skip_taskbar);
    assert!(!s.decorations && s.transparent && !s.shadow);
    assert!(!s.resizable && !s.maximizable && !s.minimizable);
    assert!(!s.focused, "created without stealing focus");
    assert!(s.always_on_top, "default on, configurable");
    assert_eq!((s.label, s.url), ("widget", "widget.html"));
    let off = window_spec(&DesktopWidgetSettings { always_on_top: false, ..Default::default() }, WidgetRows { providers: 1, five_hour: 1 });
    assert!(!off.always_on_top);
    assert!(off.logical_size.1 < s.logical_size.1, "one provider: a shorter widget");
}

#[test]
fn widget_window_is_exactly_as_tall_as_its_content() {
    // Measured against widget.css in a browser: a taller window leaves an empty strip at the bottom.
    let size = |mode, providers, five_hour| widget::widget_size(mode, WidgetRows { providers, five_hour });
    // The ring layout: 18 + 60 per provider + 16 per 5-hour line + 17 between two providers.
    assert_eq!(size(WidgetMode::Detailed, 2, 2), (224.0, 187.0));
    assert_eq!(size(WidgetMode::Detailed, 2, 1), (224.0, 171.0));
    assert_eq!(size(WidgetMode::Detailed, 2, 0), (224.0, 155.0));
    assert_eq!(size(WidgetMode::Detailed, 1, 1), (224.0, 94.0));
    assert_eq!(size(WidgetMode::Detailed, 1, 0), (224.0, 78.0));
    for five_hour in 0..=2 {
        assert_eq!(size(WidgetMode::Compact, 2, five_hour), (224.0, 86.0), "a hidden value only leaves its cell empty");
    }
    assert_eq!(size(WidgetMode::Compact, 1, 1), (224.0, 62.0));
    assert_eq!(size(WidgetMode::Compact, 1, 0), (224.0, 62.0));
    for mode in [WidgetMode::Compact, WidgetMode::Detailed] {
        assert_eq!(size(mode, 0, 0), (224.0, 38.0), "no provider: only the one-line hint");
    }
}

// ---------------------------------------------------------------------------- Codex switch

fn ids(t: &TrayStatusModel) -> Vec<&'static str> {
    t.providers.iter().map(|p| p.provider).collect()
}

#[tokio::test]
async fn codex_off_leaves_every_surface_and_keeps_its_history() {
    let e = Env::new();
    e.codex(27.0, 96.0, Some(false)); // weekly low and blocked: Codex alone would be critical
    e.rt.claude.refresh_environment(now_ms()).await;
    e.rt.claude.run_active(&e.core, now_ms()).await.unwrap().expect("claude stored");
    let has_codex_series = |e: &Env| e.rt.history(HistoryRange::Day).unwrap().unwrap().series.iter().any(|s| s.provider == "codex");
    let on = e.rt.tray_status().unwrap();
    assert_eq!(ids(&on), vec!["codex", "claude"]);
    assert_eq!(on.severity, Severity::Critical);
    assert!(has_codex_series(&e));

    e.settings(|s| s.codex.enabled = false);
    let off = e.rt.tray_status().unwrap();
    assert_eq!(ids(&off), vec!["claude"]);
    assert_eq!(off.severity, off.providers[0].severity, "only Claude's own level; Codex no longer raises the tray icon");
    assert!(off.severity < Severity::Critical);
    assert_eq!((off.availability, off.five_hour_remaining, off.weekly_remaining), (None, None, None));
    assert!(off.reasons.is_empty());
    assert!(off.updated_at_ms.is_some(), "the header follows Claude's data");
    assert_eq!(off.freshness, Freshness::Fresh);
    assert!(off.tooltip_lines.iter().all(|l| !l.contains("Codex")), "{:?}", off.tooltip_lines);
    assert_eq!(e.rt.widget_view().unwrap().providers.iter().map(|p| p.id).collect::<Vec<_>>(), vec!["claude"]);
    let dash = e.rt.dashboard().await.unwrap();
    assert_eq!(dash.providers.iter().map(|p| p.provider).collect::<Vec<_>>(), vec!["claude"]);
    assert!(!has_codex_series(&e));

    // Turned on again: nothing was deleted.
    e.settings(|s| s.codex.enabled = true);
    assert_eq!(ids(&e.rt.tray_status().unwrap()), vec!["codex", "claude"]);
    assert!(has_codex_series(&e));
}

#[test]
fn both_providers_off_is_a_quiet_empty_state() {
    let e = Env::new();
    e.settings(|s| {
        s.codex.enabled = false;
        s.claude.enabled = false;
    });
    let t = e.rt.tray_status().unwrap();
    assert!(t.providers.is_empty());
    assert_eq!((t.severity, t.icon), (Severity::Normal, TrayIconKind::Normal));
    assert_eq!(t.tooltip_lines.len(), 1, "the title only: {:?}", t.tooltip_lines);
    assert!(e.rt.widget_view().unwrap().providers.is_empty());
}

#[tokio::test]
async fn codex_off_sends_no_codex_reminder_and_on_sends_the_due_one() {
    let e = Env::new();
    e.codex(27.0, 96.0, Some(true)); // weekly at 4 % remaining: a reminder is due
    let poll = PollOutcome { status: PollStatus::Success, snapshot_id: Some(1), error: None, events: vec![], log_lines: vec![] };
    e.settings(|s| {
        s.codex.enabled = false;
        s.claude.enabled = false;
    });
    assert!(e.rt.after_poll(&poll).unwrap().notification.is_none(), "a read still in flight when Codex was turned off");
    e.settings(|s| s.codex.enabled = true);
    assert!(e.rt.after_poll(&poll).unwrap().notification.is_some(), "nothing was marked as sent while it was off");
}

#[tokio::test(start_paused = true)]
async fn codex_off_stops_reads_until_it_is_turned_on() {
    async fn run() {
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
    }
    let e = Env::new();
    let (f, log) = recorder();
    let handle = spawn_polling(f, PollingConfig::default());
    e.rt.attach_polling(handle.clone());
    run().await;
    let reads = || log.lock().unwrap().len();
    assert_eq!(reads(), 1, "the startup read");

    e.settings(|s| s.codex.enabled = false);
    e.rt.apply_codex_enabled().await;
    tokio::time::advance(Duration::from_secs(3600)).await;
    run().await;
    assert_eq!(reads(), 1, "an hour off: no reads");
    assert_eq!(e.rt.refresh().await, RefreshDecision::Started, "the page's refresh button still serves Claude");
    assert_eq!(reads(), 1, "and does not read Codex");

    e.rt.pause().await;
    e.rt.resume();
    run().await;
    assert_eq!(reads(), 1, "resuming monitoring does not restart Codex");

    e.rt.pause().await;
    e.settings(|s| s.codex.enabled = true);
    e.rt.apply_codex_enabled().await;
    run().await;
    assert_eq!(reads(), 1, "turned on during a pause: waits for the resume");
    e.rt.resume();
    run().await;
    assert_eq!(reads(), 2);

    e.settings(|s| s.codex.enabled = false);
    e.rt.apply_codex_enabled().await;
    e.settings(|s| s.codex.enabled = true);
    e.rt.apply_codex_enabled().await;
    run().await;
    assert_eq!(reads(), 3, "turned on: an immediate read");
    handle.stop().await;
}

/// Reads that stay open until the test lets one finish, as a slow or hung Codex read does.
fn held_reads() -> (PollFn, Arc<AtomicUsize>, Arc<Semaphore>) {
    let started = Arc::new(AtomicUsize::new(0));
    let finish = Arc::new(Semaphore::new(0));
    let (s, p) = (started.clone(), finish.clone());
    let f: PollFn = Arc::new(move |_| {
        let (s, p) = (s.clone(), p.clone());
        Box::pin(async move {
            s.fetch_add(1, Ordering::SeqCst);
            p.acquire().await.unwrap().forget();
            true
        })
    });
    (f, started, finish)
}

/// `update_settings` without the Tauri parts: stored at once, the switch applied in the background.
fn switch_codex(e: &Env, enabled: bool) -> JoinHandle<()> {
    let mut s = e.rt.settings();
    s.codex.enabled = enabled;
    e.rt.save_settings_from_page(s).unwrap();
    let rt = e.rt.clone();
    tokio::spawn(async move { rt.apply_codex_enabled().await })
}

/// Lets everything that can run do so: test time only moves once the runtime is idle.
async fn settle() {
    tokio::time::sleep(Duration::from_millis(1)).await;
}

/// A task that never ends fails here at once, on the test clock.
async fn ends(task: JoinHandle<()>) {
    tokio::time::timeout(Duration::from_secs(600), task).await.expect("still waiting").unwrap();
}

#[tokio::test(start_paused = true)]
async fn codex_off_then_on_during_a_read_ends_with_reads_running() {
    let e = Env::new();
    let (f, started, finish) = held_reads();
    let handle = spawn_polling(f, PollingConfig::default());
    e.rt.attach_polling(handle.clone());
    settle().await;
    let reads = || started.load(Ordering::SeqCst);
    assert_eq!(reads(), 1, "the startup read stays open");

    let off = switch_codex(&e, false);
    settle().await;
    assert!(!e.rt.settings().codex.enabled, "stored with the read still open");
    assert!(!off.is_finished(), "only the background task waits for the read");
    e.rt.modify_settings(|s| s.desktop_widget.locked = true).unwrap();
    let on = switch_codex(&e, true);
    settle().await;
    assert!(e.rt.settings().codex.enabled);
    assert!(e.rt.settings().desktop_widget.locked, "a widget menu change meanwhile is kept");
    assert_eq!(reads(), 1);

    finish.add_permits(1);
    ends(off).await;
    ends(on).await;
    settle().await;
    assert!(!handle.status().paused, "on again: the polling loop is not left paused");
    assert_eq!(reads(), 2, "the on's immediate read, after the one in flight");

    finish.add_permits(1);
    tokio::time::sleep(Duration::from_secs(301)).await;
    assert_eq!(reads(), 3, "and the normal schedule goes on");
    finish.add_permits(1);
    handle.stop().await;
}

#[tokio::test(start_paused = true)]
async fn each_codex_apply_uses_the_switch_stored_when_its_turn_comes() {
    let e = Env::new();
    let (f, started, finish) = held_reads();
    let handle = spawn_polling(f, PollingConfig::default());
    e.rt.attach_polling(handle.clone());
    settle().await;

    let off = switch_codex(&e, false);
    settle().await;
    let on = switch_codex(&e, true);
    settle().await;
    let off_again = switch_codex(&e, false);
    settle().await;

    finish.add_permits(1);
    ends(off).await;
    ends(on).await;
    ends(off_again).await;
    settle().await;
    assert!(handle.status().paused, "off is the newest");
    assert_eq!(started.load(Ordering::SeqCst), 1, "the on in between was off by its turn, so it started no read");
    handle.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_refresh_while_an_off_still_stops_does_not_say_paused() {
    let e = Env::new();
    let (f, _log) = recorder();
    let handle = spawn_polling(f, PollingConfig::default());
    e.rt.attach_polling(handle.clone());
    settle().await;

    // The moment in question: the Off's pause holds the loop, the newer On is stored but its
    // apply waits for the Off's stop.
    handle.pause().await;
    assert!(e.rt.settings().codex.enabled);
    assert_eq!(e.rt.refresh().await, RefreshDecision::Started);

    e.rt.pause().await;
    assert_eq!(e.rt.refresh().await, RefreshDecision::Paused, "the user's own pause still says so");
    e.rt.resume();
    handle.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_quit_during_a_background_codex_apply_does_not_hang() {
    let e = Env::new();
    let (f, started, finish) = held_reads();
    let handle = spawn_polling(f, PollingConfig::default());
    e.rt.attach_polling(handle.clone());
    settle().await;

    let off = switch_codex(&e, false);
    settle().await;
    let on = switch_codex(&e, true);
    settle().await;
    // main.rs quit(): stop polling, then the app-server.
    let quit = {
        let (handle, core) = (handle.clone(), e.core.clone());
        tokio::spawn(async move {
            handle.stop().await;
            core.shutdown().await;
        })
    };
    settle().await;
    assert!(!quit.is_finished(), "a quit waits for the read in flight, as it always has");

    finish.add_permits(1);
    ends(quit).await;
    ends(off).await;
    ends(on).await;
    assert!(!handle.status().running);
    assert_eq!(started.load(Ordering::SeqCst), 1, "the on has nothing left to resume");

    // Started after the loop ended, both directions return as well.
    ends(switch_codex(&e, false)).await;
    ends(switch_codex(&e, true)).await;
}

// ------------------------------------------------------------------------- 5-hour switches

fn codex_event(window: Option<WindowType>, event_type: QuotaEventType, key: &str) -> NewEvent {
    NewEvent {
        event_key: key.into(),
        account_fingerprint: None,
        window_type: window,
        event_type,
        observed_at_ms: now_ms(),
        previous_snapshot_id: None,
        current_snapshot_id: 1,
        confidence: EvidenceConfidence::High,
        confirmed: true,
        evidence: json!({}),
        supersedes_event_key: None,
    }
}

fn poll_with(events: Vec<NewEvent>) -> PollOutcome {
    PollOutcome { status: PollStatus::Success, snapshot_id: Some(1), error: None, events, log_lines: vec![] }
}

#[test]
fn widget_rows_count_only_providers_that_are_on() {
    let rows = |codex: (bool, bool), claude: (bool, bool)| {
        let mut s = AppSettings::default();
        (s.codex.enabled, s.codex.show_five_hour) = codex;
        (s.claude.enabled, s.claude.show_five_hour) = claude;
        let r = WidgetRows::of(&s);
        (r.providers, r.five_hour)
    };
    assert_eq!(WidgetRows::of(&AppSettings::default()), WidgetRows { providers: 2, five_hour: 2 });
    assert_eq!(rows((true, false), (true, true)), (2, 1));
    assert_eq!(rows((true, false), (true, false)), (2, 0));
    assert_eq!(rows((false, true), (true, false)), (1, 0), "Codex off: its own switch counts for nothing");
    assert_eq!(rows((false, true), (false, true)), (0, 0));
}

#[tokio::test]
async fn hidden_five_hour_leaves_the_views_but_not_the_data_or_the_other_provider() {
    let e = Env::new();
    e.codex(97.0, 6.0, Some(true)); // 3 % of 5-hour left, weekly fine
    e.rt.claude.refresh_environment(now_ms()).await;
    e.rt.claude.run_active(&e.core, now_ms()).await.unwrap().expect("claude stored");
    e.settings(|s| {
        s.language = LanguagePref::En;
        s.notifications.five_hour_quota = true;
        s.notifications.claude.weekly_quota = false;
    });
    let shown = e.rt.tray_status().unwrap();
    assert!(shown.reasons.contains(&"five_hour_remaining_10"));
    assert_eq!(shown.severity, Severity::Critical);
    assert!(shown.tooltip_lines.iter().any(|l| l.starts_with("Codex 5h: 3%")), "{:?}", shown.tooltip_lines);

    e.settings(|s| s.codex.show_five_hour = false);
    let t = e.rt.tray_status().unwrap();
    assert!(t.reasons.is_empty(), "a hidden window raises nothing: {:?}", t.reasons);
    assert_eq!(t.severity, Severity::Normal);
    assert!(t.tooltip_lines.iter().any(|l| l == "Codex Weekly: 94%"), "{:?}", t.tooltip_lines);
    assert!(t.tooltip_lines.iter().any(|l| l.starts_with("Claude 5h:")), "Claude has its own switch: {:?}", t.tooltip_lines);
    assert_eq!(t.five_hour_remaining, Some(3.0), "the number stays in the model, like the widget's");

    let v = e.rt.widget_view().unwrap();
    assert_eq!(v.providers.iter().map(|p| (p.id, p.show_five_hour)).collect::<Vec<_>>(), vec![("codex", false), ("claude", true)]);
    assert_eq!(serde_json::to_value(&v).unwrap()["providers"][0]["showFiveHour"], false);

    let dash = e.rt.dashboard().await.unwrap();
    let windows = |provider: &str| {
        let p = dash.providers.iter().find(|p| p.provider == provider).unwrap();
        (p.usage.windows.iter().map(|w| w.window_type).collect::<Vec<_>>(), p.forecasts.iter().map(|f| f.window_type).collect::<Vec<_>>())
    };
    assert_eq!(windows("codex"), (vec![WindowType::Weekly], vec![WindowType::Weekly]));
    assert!(windows("claude").0.contains(&WindowType::FiveHour));

    let series = |e: &Env| e.rt.history(HistoryRange::Day).unwrap().unwrap().series.iter().map(|s| (s.provider, s.window_type)).collect::<Vec<_>>();
    assert!(!series(&e).contains(&("codex", WindowType::FiveHour)));
    assert!(series(&e).contains(&("codex", WindowType::Weekly)));
    assert!(series(&e).contains(&("claude", WindowType::FiveHour)));

    // The choices behind it are untouched, also through what the page would save back.
    assert!(e.rt.settings().notifications.five_hour_quota);
    assert!(dash.settings.notifications.five_hour_quota);
    assert!(e.core.with_db(load_settings).unwrap().notifications.five_hour_quota);

    // The support bundle keeps the window.
    let bundle: Value = serde_json::from_str(&std::fs::read_to_string(e.rt.export_diagnostics().await.unwrap()).unwrap()).unwrap();
    let codex = bundle["dashboard"]["providers"].as_array().unwrap().iter().find(|p| p["provider"] == "codex").unwrap().clone();
    assert!(codex["usage"]["windows"].as_array().unwrap().iter().any(|w| w["windowType"] == "five_hour"));

    e.settings(|s| s.codex.show_five_hour = true);
    let back = e.rt.tray_status().unwrap();
    assert_eq!(back.severity, Severity::Critical, "shown again: back at once");
    assert!(series(&e).contains(&("codex", WindowType::FiveHour)), "nothing was deleted");
}

#[tokio::test]
async fn hidden_claude_five_hour_is_silent_too() {
    let e = Env::new();
    e.rt.claude.refresh_environment(now_ms()).await;
    e.rt.claude.run_active(&e.core, now_ms()).await.unwrap().expect("claude stored");
    e.settings(|s| {
        s.language = LanguagePref::En;
        s.codex.enabled = false;
        s.claude.show_five_hour = false;
    });
    let t = e.rt.tray_status().unwrap();
    assert!(!t.providers[0].show_five_hour);
    assert!(t.tooltip_lines.iter().any(|l| l.starts_with("Claude Weekly:")), "{:?}", t.tooltip_lines);
    assert!(t.tooltip_lines.iter().all(|l| !l.contains("5h")), "{:?}", t.tooltip_lines);
    let dash = e.rt.dashboard().await.unwrap();
    assert!(dash.providers[0].usage.windows.iter().all(|w| w.window_type != WindowType::FiveHour));
    assert!(dash.providers[0].forecasts.iter().all(|f| f.window_type != WindowType::FiveHour));
    let h = e.rt.history(HistoryRange::Day).unwrap().unwrap();
    assert!(h.series.iter().all(|s| s.window_type != WindowType::FiveHour), "{:?}", h.series.iter().map(|s| s.window_type).collect::<Vec<_>>());
}

#[test]
fn a_real_block_stays_visible_when_five_hour_is_hidden() {
    let e = Env::new();
    e.codex(100.0, 53.0, Some(false)); // the service says blocked; the flag does not say which window
    e.settings(|s| {
        s.codex.show_five_hour = false;
        s.language = LanguagePref::En;
    });
    let t = e.rt.tray_status().unwrap();
    assert_eq!(t.severity, Severity::Critical);
    assert!(t.reasons.contains(&"limit_reached"), "{:?}", t.reasons);
    assert_eq!(menu_line(&t, "codex", Lang::En).text, "Codex · Limit reached  Weekly 47%");
    let v = e.rt.widget_view().unwrap();
    assert_eq!(v.providers[0].availability, Availability::Blocked);
    assert_eq!(v.providers[0].note.as_deref(), Some("Limit reached"));

    // Reads start failing. A failed read stores nothing, so the block stands, and the row still says so.
    e.failed_poll("provider_unavailable", "app-server closed the connection");
    let t = e.rt.tray_status().unwrap();
    assert_eq!(t.severity, Severity::Error);
    assert!(t.reasons.contains(&"limit_reached") && t.reasons.contains(&"provider_unavailable"), "{:?}", t.reasons);
    assert_eq!(menu_line(&t, "codex", Lang::En).text, "Codex · Limit reached · Cannot refresh Codex usage  Weekly 47%");
    let v = e.rt.widget_view().unwrap();
    assert_eq!(v.providers[0].availability, Availability::Blocked, "the widget is told too; its one note goes to the error");
    assert_eq!(v.providers[0].note.as_deref(), Some("Cannot refresh Codex usage"));
}

#[tokio::test]
async fn a_reminder_waits_while_hidden_and_fires_once_when_shown() {
    let e = Env::new();
    e.codex(97.0, 6.0, Some(true));
    let title = |e: &Env| e.rt.after_poll(&poll_with(vec![])).unwrap().notification.map(|(n, _)| n.title);
    e.settings(|s| {
        s.claude.enabled = false;
        s.notifications.five_hour_quota = true;
        s.codex.show_five_hour = false;
    });
    assert_eq!(title(&e), None, "hidden");
    e.settings(|s| s.codex.show_five_hour = true);
    assert!(title(&e).is_some(), "not marked as sent while hidden");
    assert_eq!(title(&e), None, "and only once");
    e.settings(|s| s.codex.show_five_hour = false);
    e.settings(|s| s.codex.show_five_hour = true);
    assert_eq!(title(&e), None, "switching does not re-arm a reminder already sent");
}

#[test]
fn hidden_five_hour_silences_its_refresh_notices_only() {
    let e = Env::new();
    e.codex(10.0, 6.0, Some(true));
    let title = |e: &Env, ev: NewEvent| e.rt.after_poll(&poll_with(vec![ev])).unwrap().notification.map(|(n, _)| n.title);
    e.settings(|s| {
        s.claude.enabled = false;
        s.language = LanguagePref::En;
        s.notifications.allowance_refresh = true;
        s.codex.show_five_hour = false;
    });
    let five = Some(WindowType::FiveHour);
    assert_eq!(title(&e, codex_event(five, QuotaEventType::UnscheduledQuotaRefresh, "r5")), None);
    assert_eq!(title(&e, codex_event(five, QuotaEventType::NaturalRolloverObserved, "n5")), None);
    let week = Some(WindowType::Weekly);
    assert!(title(&e, codex_event(week, QuotaEventType::UnscheduledQuotaRefresh, "rw")).is_some_and(|t| t.contains("Weekly")));
    assert!(title(&e, codex_event(week, QuotaEventType::NaturalRolloverObserved, "nw")).is_some_and(|t| t.contains("Weekly")));
    assert!(title(&e, codex_event(None, QuotaEventType::ResetCreditRedeemed, "credit")).is_some(), "a redeemed credit concerns the account");
    e.settings(|s| s.codex.show_five_hour = true);
    assert!(title(&e, codex_event(five, QuotaEventType::UnscheduledQuotaRefresh, "r5b")).is_some_and(|t| t.contains("5")));
}

#[test]
fn hidden_five_hour_events_leave_full_pages_and_events_without_a_window_stay() {
    let e = Env::new();
    let t0 = now_ms();
    e.core
        .with_db(|c| {
            for (i, (window, key)) in [(Some("weekly"), "w"), (None, "credit"), (Some("five_hour"), "f1"), (Some("five_hour"), "f2")].iter().enumerate() {
                let at = t0 + i as i64;
                c.execute(
                    "INSERT INTO quota_events (event_key, classifier_version, window_type, event_type, visibility, observed_at_ms, confidence, confirmed, evidence_json, created_at_ms, updated_at_ms)
                     VALUES (?1, ?2, ?3, 'unscheduled_quota_refresh', 'user', ?4, 'high', 1, '{}', ?4, ?4)",
                    rusqlite::params![key, CLASSIFIER_VERSION, window, at],
                )?;
            }
            for (i, (window, key)) in [(Some("weekly_model:fable"), "cm"), (Some("weekly"), "cw"), (Some("five_hour"), "c5")].iter().enumerate() {
                let at = t0 + 10 + i as i64;
                c.execute(
                    "INSERT INTO provider_events (event_key, provider, window_key, event_type, visibility, observed_at_ms, confidence, evidence_json, created_at_ms)
                     VALUES (?1, 'claude', ?2, 'quota_refreshed_observed', 'user', ?3, 'medium', '{}', ?3)",
                    rusqlite::params![key, window, at],
                )?;
            }
            Ok(())
        })
        .unwrap();
    let keys = |e: &Env, limit| e.rt.events(false, None, limit).unwrap().into_iter().map(|v| v.event_key).collect::<Vec<_>>();
    assert_eq!(keys(&e, 50), vec!["c5", "cw", "cm", "f2", "f1", "credit", "w"]);
    e.settings(|s| s.codex.show_five_hour = false);
    assert_eq!(keys(&e, 50), vec!["c5", "cw", "cm", "credit", "w"]);
    e.settings(|s| {
        s.claude.enabled = false;
    });
    assert_eq!(keys(&e, 2), vec!["credit", "w"], "the hidden events do not use up the page");
    e.settings(|s| {
        s.claude.enabled = true;
        s.claude.show_five_hour = false;
    });
    assert_eq!(keys(&e, 50), vec!["cw", "cm", "credit", "w"]);
}

#[tokio::test]
async fn a_claude_update_sends_no_reminder_about_old_codex_data() {
    let quiet = |s: &mut AppSettings| {
        s.notifications.claude.weekly_quota = false;
        s.notifications.claude.forecast_weekly = false;
    };
    let old = Env::new();
    old.codex_at(now_ms() - 40 * 60_000, 6.0, 96.0, Some(true)); // weekly reminder due, but 40 min old
    old.settings(quiet);
    let after = old.rt.claude_tick(false, true).await.unwrap().expect("manual tick");
    assert!(old.rt.claude.activity().last_success_ms.is_some(), "the Claude query stored data");
    assert_eq!(after.notification.map(|(n, _)| n.title), None);

    let current = Env::new();
    current.codex(6.0, 96.0, Some(true));
    current.settings(quiet);
    let after = current.rt.claude_tick(false, true).await.unwrap().expect("manual tick");
    assert!(after.notification.is_some(), "current data merges with the Claude update as before");
}

// ----------------------------------------------------------------- short notes and waiting notes

/// A Codex row from a tray line with its values, changed by the test.
fn codex_row(language: LanguagePref, availability: Option<bool>, change: impl FnOnce(&mut ProviderTrayLine)) -> WidgetProvider {
    let mut l = line("codex", Some(73.0), Some(94.0), false, Severity::Normal, None);
    change(&mut l);
    let settings = AppSettings { language, ..Default::default() };
    widget::build_view(&tray(false, availability, vec![l]), None, &settings, 0).providers.remove(0)
}

#[test]
fn codex_notes_have_a_short_form_for_the_compact_slot() {
    let notes = |language, availability, reasons: Vec<&'static str>, not_found| {
        let p = codex_row(language, availability, |l| {
            l.reasons = reasons;
            l.not_found = not_found;
        });
        (p.note, p.note_short)
    };
    let both = |note: &str, short: &str| (Some(note.to_string()), Some(short.to_string()));
    let (en, zh) = (LanguagePref::En, LanguagePref::ZhCn);
    assert_eq!(notes(en, Some(true), vec!["not_authenticated"], false), both("Sign in to Codex first", "Sign in first"));
    assert_eq!(notes(zh, Some(true), vec!["not_authenticated"], false), both("请先登录 Codex", "请先登录"));
    assert_eq!(notes(en, Some(true), vec!["provider_unavailable"], false), both("Cannot refresh Codex usage", "Can't refresh"));
    assert_eq!(notes(zh, Some(true), vec!["provider_unavailable"], false), both("无法刷新 Codex 用量", "无法刷新"));
    // A missing installation is named; older versions and the rest arrive here with `not_found` false.
    assert_eq!(notes(en, None, vec!["provider_unavailable"], true), both("Codex installation not found", "Not installed"));
    assert_eq!(notes(zh, None, vec!["provider_unavailable"], true), both("未找到 Codex", "未安装"));
    // One note on the widget: the error's, a limit or not. The limit's own words need no short form.
    assert_eq!(notes(en, Some(false), vec!["provider_unavailable", "limit_reached"], false), both("Cannot refresh Codex usage", "Can't refresh"));
    assert_eq!(notes(en, Some(false), vec!["limit_reached"], false), (Some("Limit reached".to_string()), None));
    assert_eq!(notes(en, Some(true), vec![], false), (None, None));
    assert_eq!(notes(en, Some(true), vec!["no_valid_snapshot_30m"], false), (None, None));
}

#[test]
fn codex_polls_failing_with_no_data_are_not_a_plain_wait() {
    let row = |language, paused, reasons: Vec<&'static str>, has_data, polls_failing| {
        let mut l = line("codex", None, None, false, Severity::Normal, None);
        (l.reasons, l.has_data, l.polls_failing) = (reasons, has_data, polls_failing);
        let mut t = tray(paused, None, vec![l]);
        t.freshness = if has_data { Freshness::Fresh } else { Freshness::NoData };
        let settings = AppSettings { language, ..Default::default() };
        let p = widget::build_view(&t, None, &settings, 0).providers.remove(0);
        (p.state, p.waiting_note)
    };
    let (en, zh) = (LanguagePref::En, LanguagePref::ZhCn);
    let waiting = WidgetProviderState::Waiting;
    assert_eq!(row(en, false, vec![], false, true), (waiting, Some("Can't refresh".to_string())));
    assert_eq!(row(zh, false, vec![], false, true), (waiting, Some("无法刷新".to_string())));
    assert_eq!(row(en, false, vec![], false, false), (waiting, None), "first launch: the true wait");
    assert_eq!(row(en, true, vec![], false, true), (WidgetProviderState::Paused, None));
    assert_eq!(row(en, false, vec!["provider_unavailable"], false, true), (WidgetProviderState::Error, None), "its note says it already");
    assert_eq!(row(en, false, vec![], true, true), (WidgetProviderState::Live, None), "data: not a waiting row");
}

#[tokio::test]
async fn codex_row_with_no_data_from_real_failed_reads() {
    let e = Env::new();
    e.settings(|s| {
        s.language = LanguagePref::En;
        s.claude.enabled = false;
    });
    let codex = |e: &Env| e.rt.widget_view().unwrap().providers.remove(0);
    let first = codex(&e);
    assert_eq!((first.state, first.note, first.note_short, first.waiting_note), (WidgetProviderState::Waiting, None, None, None));

    e.failed_poll("network_error", "timeout");
    let failing = codex(&e);
    assert_eq!((failing.state, failing.note, failing.note_short), (WidgetProviderState::Waiting, None, None));
    assert_eq!(failing.waiting_note.as_deref(), Some("Can't refresh"));
    let page = serde_json::to_value(e.rt.widget_view().unwrap()).unwrap();
    assert_eq!((&page["providers"][0]["waitingNote"], &page["providers"][0]["noteShort"]), (&json!("Can't refresh"), &Value::Null));

    e.rt.pause().await;
    assert_eq!(codex(&e).waiting_note, None, "paused");
    e.rt.resume();

    e.failed_poll("provider_unavailable", "no codex executable found");
    let not_found = codex(&e);
    assert_eq!(not_found.state, WidgetProviderState::Error);
    assert_eq!((not_found.note.as_deref(), not_found.note_short.as_deref(), not_found.waiting_note), (Some("Codex installation not found"), Some("Not installed"), None));
    let t = e.rt.tray_status().unwrap();
    assert_eq!(menu_line(&t, "codex", Lang::En).text, "Codex · Codex installation not found");

    e.failed_poll("provider_unavailable", "only unverified older codex versions found; no executable with a verified or newer version");
    let older = codex(&e);
    assert_eq!((older.note.as_deref(), older.note_short.as_deref()), (Some("Codex version not yet verified"), Some("Unverified")), "the Overview's words");
    assert_eq!(menu_line(&e.rt.tray_status().unwrap(), "codex", Lang::En).text, "Codex · Codex version not yet verified");
}

#[test]
fn claude_short_and_waiting_notes_come_from_its_tray_line() {
    let mut l = line("claude", None, None, false, Severity::Normal, Some("Needs verification"));
    l.note_short = Some("Needs check".to_string());
    l.waiting_note = Some("Needs verification".to_string());
    let view = |paused| widget::build_view(&tray(paused, None, vec![l.clone()]), None, &AppSettings::default(), 0).providers.remove(0);
    let waiting = view(false);
    assert_eq!(waiting.state, WidgetProviderState::Waiting);
    assert_eq!((waiting.note.as_deref(), waiting.note_short.as_deref(), waiting.waiting_note.as_deref()), (Some("Needs verification"), Some("Needs check"), Some("Needs verification")));
    let paused = view(true);
    assert_eq!((paused.state, paused.waiting_note), (WidgetProviderState::Paused, None), "only on a waiting row");
}

#[tokio::test]
async fn a_claude_row_with_no_data_says_why_and_the_true_wait_stays_a_wait() {
    let e = Env::new();
    e.settings(|s| {
        s.language = LanguagePref::En;
        s.codex.enabled = false;
        s.claude.active_refresh = Some(true);
    });
    e.rt.claude.refresh_environment(now_ms()).await;
    let claude = |e: &Env| e.rt.widget_view().unwrap().providers.remove(0);
    let waiting = claude(&e);
    assert_eq!(waiting.state, WidgetProviderState::Waiting);
    assert_eq!((waiting.note.as_deref(), waiting.note_short, waiting.waiting_note), (Some("Waiting for Claude Code"), None, None));

    e.settings(|s| s.claude.active_refresh = Some(false));
    let not_set_up = claude(&e);
    assert_eq!(not_set_up.state, WidgetProviderState::Waiting);
    assert_eq!((not_set_up.note.as_deref(), not_set_up.note_short, not_set_up.waiting_note.as_deref()), (Some("Not set up"), None, Some("Not set up")));
}

// ------------------------------------------------------------------------- a passed Codex window

fn five_hour(windows: &[WindowStatus]) -> &WindowStatus {
    windows.iter().find(|w| w.window_type == WindowType::FiveHour).unwrap()
}

/// Codex's 5-hour value as the tray, the widget and the page show it.
async fn shown_five_hour(e: &Env) -> (Option<f64>, Option<f64>, Option<f64>) {
    let dash = e.rt.dashboard().await.unwrap();
    (
        e.rt.tray_status().unwrap().providers[0].five_hour_remaining,
        e.rt.widget_view().unwrap().providers[0].five_hour_remaining,
        five_hour(&dash.providers[0].usage.windows).remaining_percent,
    )
}

/// Three reads a minute apart, the last at `last_read`, 18 % of the weekly window used. In English, Claude off.
fn codex_read_at(last_read: i64, five_used: f64, allowed: Option<bool>, five_reset_s: Option<i64>) -> Env {
    let e = Env::new();
    e.settings(|s| {
        s.language = LanguagePref::En;
        s.claude.enabled = false;
    });
    let weekly_reset_s = now_ms() / 1000 + 4 * 86_400;
    for i in (0..3).rev() {
        e.codex_read(last_read - i * 60_000, five_used, 18.0, allowed, five_reset_s, weekly_reset_s);
    }
    e
}

#[tokio::test]
async fn a_codex_value_from_before_a_passed_reset_leaves_the_views_and_nothing_else() {
    let t = now_ms();
    let five_reset_s = (t - 60_000) / 1000;
    let e = codex_read_at(t - 3 * 60_000, 99.0, Some(true), Some(five_reset_s));
    assert_eq!(shown_five_hour(&e).await, (None, None, None));
    assert_eq!(menu_line(&e.rt.tray_status().unwrap(), "codex", Lang::En).text, "Codex  5h — · Weekly 82%");

    // The page: the look of a passed Claude window, the window and its reset stay.
    let dash = e.rt.dashboard().await.unwrap();
    let w = five_hour(&dash.providers[0].usage.windows);
    assert!(w.present);
    assert_eq!((w.used_percent, w.remaining_percent), (None, None));
    assert_eq!(w.reported_reset_at_ms, Some(five_reset_s * 1000));
    assert_eq!((w.reset_schedule, w.confirmed_reset_at_ms), ("confirmed", Some(five_reset_s * 1000)), "compared with the read's own time");
    assert_eq!((w.reset_precision_ms, w.snapshot_quality.as_str()), (None, "normal"));
    let weekly = dash.providers[0].usage.windows.iter().find(|w| w.window_type == WindowType::Weekly).unwrap();
    assert_eq!((weekly.used_percent, weekly.remaining_percent), (Some(18.0), Some(82.0)), "the current window keeps its value");
    // The widget: no tone, the same reset as the page.
    let c = e.rt.widget_view().unwrap().providers.remove(0);
    assert_eq!(c.five_hour_tone, DisplayTone::Unknown);
    assert_eq!(c.five_hour_reset, WidgetReset::of(Some(w)));

    // What the reminders read has not moved: `window_inputs` takes it from this call.
    let usage = commands::get_current_codex_usage(&e.core).unwrap();
    assert_eq!((five_hour(&usage.windows).used_percent, five_hour(&usage.windows).remaining_percent), (Some(99.0), Some(1.0)));
    e.settings(|s| s.notifications.five_hour_quota = true);
    assert_eq!(e.rt.tray_status().unwrap().reasons, vec!["five_hour_remaining_10"]);
    let reminder = e.rt.after_poll(&poll_with(vec![])).unwrap().notification.map(|(n, _)| n.title);
    assert_eq!(reminder.as_deref(), Some("Codex 5-hour quota below 10%"));

    // The support bundle keeps the stored value.
    let bundle: Value = serde_json::from_str(&std::fs::read_to_string(e.rt.export_diagnostics().await.unwrap()).unwrap()).unwrap();
    let exported = bundle["dashboard"]["providers"][0]["usage"]["windows"].as_array().unwrap().iter().find(|w| w["windowType"] == "five_hour").unwrap().clone();
    assert_eq!((&exported["usedPercent"], &exported["remainingPercent"]), (&json!(99.0), &json!(1.0)));
}

#[tokio::test]
async fn a_read_after_the_reset_an_idle_window_and_no_reset_time_keep_their_numbers_everywhere() {
    let t = now_ms();
    let after = codex_read_at(t - 60_000, 40.0, Some(true), Some((t - 5 * 60_000) / 1000));
    assert_eq!(shown_five_hour(&after).await, (Some(60.0), Some(60.0), Some(60.0)), "a read after the reset time");
    let idle = codex_read_at(t - 3 * 60_000, 0.0, Some(true), Some((t - 60_000) / 1000));
    assert_eq!(shown_five_hour(&idle).await, (Some(100.0), Some(100.0), Some(100.0)), "an idle window past its time");
    let none = codex_read_at(t - 3 * 60_000, 40.0, Some(true), None);
    assert_eq!(shown_five_hour(&none).await, (Some(60.0), Some(60.0), Some(60.0)), "no reset time");
    let ahead = codex_read_at(t - 3 * 60_000, 40.0, Some(true), Some((t + 3_600_000) / 1000));
    assert_eq!(shown_five_hour(&ahead).await, (Some(60.0), Some(60.0), Some(60.0)), "not passed yet");
}

#[tokio::test]
async fn a_limit_keeps_its_word_when_the_value_beside_it_is_from_the_last_period() {
    let t = now_ms();
    let e = codex_read_at(t - 3 * 60_000, 100.0, Some(false), Some((t - 60_000) / 1000));
    assert_eq!(shown_five_hour(&e).await, (None, None, None));
    assert_eq!(menu_line(&e.rt.tray_status().unwrap(), "codex", Lang::En).text, "Codex · Limit reached  5h — · Weekly 82%");
    let c = e.rt.widget_view().unwrap().providers.remove(0);
    assert_eq!((c.availability, c.note.as_deref(), c.note_short), (Availability::Blocked, Some("Limit reached"), None));
    assert_eq!(e.rt.dashboard().await.unwrap().providers[0].usage.ordinary_usage_allowed, Some(false));

    // Paused past the reset, the case the finding started from.
    e.rt.pause().await;
    assert_eq!(shown_five_hour(&e).await, (None, None, None));
    assert_eq!(menu_line(&e.rt.tray_status().unwrap(), "codex", Lang::En).text, "Codex · Limit reached · Paused  5h — · Weekly 82%");
    assert_eq!(e.rt.widget_view().unwrap().providers[0].state, WidgetProviderState::Paused);
}

// ------------------------------------------------------------------------------- placement

fn monitor(name: &str, primary: bool, scale: f64, x: i32, y: i32, w: u32, h: u32) -> MonitorGeom {
    // Work area = monitor minus a 48 px taskbar at the bottom.
    MonitorGeom { name: Some(name.into()), primary, scale, work_x: x, work_y: y, work_w: w, work_h: h - (48.0 * scale) as u32 }
}

#[test]
fn widget_position_persists() {
    let two = vec![monitor(r"\\.\DISPLAY1", true, 1.25, 0, 0, 2560, 1440), monitor(r"\\.\DISPLAY2", false, 1.0, 2560, 0, 1920, 1080)];
    let size = (224.0, 102.0);
    // The user dragged the widget to the top-right of monitor 2.
    let saved = save_position(2560 + 1920 - 240, 20, (224, 102), &two).unwrap();
    assert_eq!(saved, WidgetPosition { monitor: r"\\.\DISPLAY2".into(), x: 1680, y: 20 });
    let p = place(Some(&saved), &two, size).unwrap();
    assert_eq!((p.x, p.y, p.monitor_index, p.recovered), (2560 + 1680, 20, 1, false), "restored on monitor 2");
    // On a 125 % monitor the logical offset is stored, and restored exactly.
    let saved1 = save_position(1000, 500, (280, 128), &two).unwrap();
    assert_eq!(saved1, WidgetPosition { monitor: r"\\.\DISPLAY1".into(), x: 800, y: 400 });
    let p1 = place(Some(&saved1), &two, size).unwrap();
    assert_eq!((p1.x, p1.y, p1.recovered), (1000, 500, false));
}

#[test]
fn offscreen_position_recovers() {
    let size = (224.0, 102.0);
    let saved = WidgetPosition { monitor: r"\\.\DISPLAY2".into(), x: 1680, y: 20 };
    // Monitor 2 unplugged: back to the primary monitor's work area, bottom-right.
    let one = vec![monitor(r"\\.\DISPLAY1", true, 1.0, 0, 0, 1920, 1080)];
    let p = place(Some(&saved), &one, size).unwrap();
    assert!(p.recovered);
    assert_eq!((p.monitor_index, p.x, p.y), (0, 1920 - 224 - 16, 1080 - 48 - 102 - 16));
    // Same monitor, smaller resolution: clamped inside the work area.
    let small = vec![monitor(r"\\.\DISPLAY1", true, 1.0, 0, 0, 1920, 1080), monitor(r"\\.\DISPLAY2", false, 1.0, 1920, 0, 1280, 720)];
    let q = place(Some(&saved), &small, size).unwrap();
    assert!(q.recovered);
    assert_eq!((q.x, q.y), (1920 + 1280 - 224, 20));
    // DPI 100 % -> 150 %: the logical offset is kept and the widget stays fully visible.
    let hi = vec![monitor(r"\\.\DISPLAY1", true, 1.5, 0, 0, 3840, 2160)];
    let r = place(Some(&WidgetPosition { monitor: r"\\.\DISPLAY1".into(), x: 2000, y: 900 }), &hi, size).unwrap();
    assert_eq!((r.x, r.y, r.recovered), (3000, 1350, false));
    let near_edge = place(Some(&WidgetPosition { monitor: r"\\.\DISPLAY1".into(), x: 2500, y: 1400 }), &hi, size).unwrap();
    assert!(near_edge.recovered);
    assert!(near_edge.x + 336 <= 3840 && near_edge.y + 153 <= 2160 - 72, "fully inside the work area: {near_edge:?}");
    // No saved place: primary bottom-right; a monitor on the left with negative coordinates is fine.
    let left = vec![monitor(r"\\.\DISPLAY2", false, 1.0, -1920, 0, 1920, 1080), monitor(r"\\.\DISPLAY1", true, 1.0, 0, 0, 2560, 1440)];
    let d = place(None, &left, size).unwrap();
    assert_eq!((d.monitor_index, d.x, d.y, d.recovered), (1, 2560 - 224 - 16, 1440 - 48 - 102 - 16, false));
    // The margin and the size follow the monitor's scale, and an offset work area counts.
    let scaled = vec![MonitorGeom { name: Some("s".into()), primary: true, scale: 1.5, work_x: 100, work_y: 200, work_w: 3840, work_h: 2088 }];
    let b = place(None, &scaled, size).unwrap();
    assert_eq!((b.x, b.y), (100 + 3840 - 336 - 24, 200 + 2088 - 153 - 24));
    // A work area smaller than the widget: never above or left of it.
    let tiny = vec![MonitorGeom { name: None, primary: true, scale: 1.0, work_x: 10, work_y: 20, work_w: 100, work_h: 50 }];
    let c = place(None, &tiny, size).unwrap();
    assert_eq!((c.x, c.y), (10, 20));
    let s = save_position(-1000, 300, (224, 102), &left).unwrap();
    assert_eq!(s, WidgetPosition { monitor: r"\\.\DISPLAY2".into(), x: 920, y: 300 });
    assert_eq!(place(Some(&s), &left, size).unwrap().x, -1000);
    // No monitors reported at all: no placement (the shell lets Windows choose).
    assert!(place(Some(&saved), &[], size).is_none());
}

// Compact and detailed with both providers and both 5-hour windows.
const COMPACT: (f64, f64) = (224.0, 86.0);
const DETAILED: (f64, f64) = (224.0, 198.0);

/// Physical pixels at a scale, the way both `place` and the window's `set_size` round them.
fn phys(logical: f64, scale: f64) -> u32 {
    (logical * scale).round() as u32
}

#[test]
fn default_place_survives_a_trip_to_a_taller_layout_and_back() {
    // 1920x1080 at 100 %: first run y = 930, detailed 818, then 930 again, not 834 with no margin.
    let m = monitor("m", true, 1.0, 0, 0, 1920, 1080);
    assert_eq!(m.work_h, 1032);
    assert_eq!(place(None, std::slice::from_ref(&m), COMPACT).unwrap().y, 930);
    assert_eq!(resized_y(930, 86, 198, &m), 818);
    assert_eq!(resized_y(818, 198, 86, &m), 930);
    // At every scale the two places are exactly what `place` gives each height, 125 % rounding included.
    for (scale, w, h) in [(1.0, 1920, 1080), (1.25, 2400, 1350), (1.5, 2880, 1620), (1.75, 3360, 1890), (2.0, 3840, 2160)] {
        let mons = vec![monitor("m", true, scale, 0, 0, w, h)];
        let small = place(None, &mons, COMPACT).unwrap();
        let tall = place(None, &mons, DETAILED).unwrap();
        let (hs, ht) = (phys(86.0, scale), phys(198.0, scale));
        assert_eq!(resized_y(small.y, hs, ht, &mons[0]), tall.y, "{scale}");
        assert_eq!(resized_y(tall.y, ht, hs, &mons[0]), small.y, "{scale}");
        let up = resized_position(small.x, small.y, (phys(224.0, scale), hs), DETAILED, &mons).unwrap();
        assert_eq!(up, ResizeMove { x: small.x, y: tall.y, move_first: true }, "{scale}");
        let down = resized_position(small.x, tall.y, (phys(224.0, scale), ht), COMPACT, &mons).unwrap();
        assert_eq!(down, ResizeMove { x: small.x, y: small.y, move_first: false }, "{scale}");
    }
    let hi = monitor("m", true, 1.5, 0, 0, 2880, 1620);
    assert_eq!(hi.work_h, 1548);
    assert_eq!((resized_y(1395, 129, 297, &hi), resized_y(1227, 297, 129, &hi)), (1227, 1395));
}

#[test]
fn a_height_that_did_not_change_moves_nothing() {
    // update() runs on every settings change: not even an anchored or flush widget moves.
    let mons = vec![monitor("m", true, 1.0, 0, 0, 1920, 1080)];
    for y in [100, 930, 940, 946] {
        assert_eq!(resized_y(y, 86, 86, &mons[0]), y);
        assert_eq!(resized_position(1680, y, (224, 86), COMPACT, &mons), None, "{y}");
    }
}

#[test]
fn a_widget_elsewhere_keeps_its_top_edge() {
    for (scale, w, h) in [(1.0, 1920, 1080), (1.5, 2880, 1620)] {
        let mons = vec![monitor("m", true, scale, 0, 0, w, h)];
        let (hs, ht) = (phys(86.0, scale), phys(198.0, scale));
        let y = phys(400.0, scale) as i32;
        assert_eq!((resized_y(y, hs, ht, &mons[0]), resized_y(y, ht, hs, &mons[0])), (y, y), "{scale}");
        assert_eq!(resized_position(300, y, (phys(224.0, scale), hs), DETAILED, &mons), None, "{scale}");
        assert_eq!(resized_position(300, y, (phys(224.0, scale), ht), COMPACT, &mons), None, "{scale}");
        // The old top-right default too.
        let near_top = phys(16.0, scale) as i32;
        assert_eq!(resized_y(near_top, hs, ht, &mons[0]), near_top, "{scale}");
    }
}

#[test]
fn only_the_margin_decides_what_is_at_the_bottom() {
    // limit: the margin plus one logical pixel, in physical pixels.
    for (scale, w, h, margin, limit) in [(1.0, 1920, 1080, 16, 17), (1.25, 2400, 1350, 20, 21), (1.5, 2880, 1620, 24, 26)] {
        let mons = vec![monitor("m", true, scale, 0, 0, w, h)];
        let m = &mons[0];
        let (hs, ht) = (phys(86.0, scale), phys(198.0, scale));
        let bottom = m.work_h as i32;
        // Exactly at the margin, and inside it: that gap is kept.
        assert_eq!(resized_y(bottom - hs as i32 - margin, hs, ht, m), bottom - ht as i32 - margin, "{scale}");
        assert_eq!(resized_y(bottom - hs as i32 - 3, hs, ht, m), bottom - ht as i32 - 3, "{scale}");
        assert_eq!(resized_y(bottom - hs as i32, hs, ht, m), bottom - ht as i32, "{scale}: flush stays flush");
        // Over the margin by what a remembered place can pick up: back to the margin itself.
        for gap in margin + 1..=limit {
            assert_eq!(resized_y(bottom - hs as i32 - gap, hs, ht, m), bottom - ht as i32 - margin, "{scale} {gap}");
        }
        // One pixel above that: the top edge stays, and the clamp brings the taller window in.
        let above = bottom - hs as i32 - limit - 1;
        assert_eq!(resized_y(above, hs, ht, m), above, "{scale}");
        let clamped = resized_position(300, above, (phys(224.0, scale), hs), DETAILED, &mons).unwrap();
        assert_eq!((clamped.y, clamped.move_first), (bottom - ht as i32, true), "{scale}");
        // That clamp leaves it flush with the edge, so the way back keeps the bottom edge there.
        assert_eq!(resized_y(clamped.y, ht, hs, m), bottom - hs as i32, "{scale}");
        // Hanging below the work area: as if flush, and never further out.
        assert_eq!(resized_y(bottom - hs as i32 + 40, hs, ht, m), bottom - ht as i32, "{scale}");
    }
}

#[test]
fn a_restart_at_a_fractional_scale_never_strands_the_widget() {
    // A remembered place comes back through the logical offset, and at these scales that rounding can
    // lift the widget a pixel off the margin: 1920x1200 at 175 %, gap 29 with a margin of 28.
    // Remember, restart, change layout, over and over, from either layout.
    let panels = [(1.75, 1920, 1200), (1.75, 2880, 1620), (1.75, 5120, 2880), (2.25, 2736, 1824), (2.25, 3840, 2400), (2.5, 2048, 1536)];
    for (scale, w, h) in panels {
        let mons = vec![monitor("m", true, scale, 0, 0, w, h)];
        let (bottom, margin, width) = (mons[0].work_h as i32, phys(16.0, scale) as i32, phys(224.0, scale));
        for layouts in [[COMPACT, DETAILED], [DETAILED, COMPACT]] {
            let first = place(None, &mons, layouts[0]).unwrap();
            let (mut x, mut y) = (first.x, first.y);
            for round in 0..40 {
                let (now, next) = (layouts[round % 2], layouts[(round + 1) % 2]);
                let old = (width, phys(now.1, scale));
                let saved = save_position(x, y, old, &mons).unwrap();
                let restarted = place(Some(&saved), &mons, now).unwrap();
                (x, y) = (restarted.x, restarted.y);
                if let Some(m) = resized_position(x, y, old, next, &mons) {
                    (x, y) = (m.x, m.y);
                }
                // Neither stranded above the margin nor flush with the edge: exactly the margin.
                assert_eq!(bottom - y - phys(next.1, scale) as i32, margin, "{scale} {w}x{h} round {round}");
            }
        }
    }
}

#[test]
fn an_offset_work_area_counts_on_any_monitor() {
    // Taskbar on top: the work area starts at 48.
    let top = MonitorGeom { name: Some("top".into()), primary: true, scale: 1.0, work_x: 0, work_y: 48, work_w: 1920, work_h: 1032 };
    assert_eq!(place(None, std::slice::from_ref(&top), COMPACT).unwrap().y, 978);
    assert_eq!((resized_y(978, 86, 198, &top), resized_y(866, 198, 86, &top)), (866, 978));
    assert_eq!(resized_y(930, 86, 198, &top), 930, "the 100 % default of a bottom taskbar is above the margin here");
    // A secondary monitor left of and above the primary, at 150 %, while the primary is at 100 %.
    let mons = vec![monitor("p", true, 1.0, 0, 0, 1920, 1080), monitor("s", false, 1.5, -2880, -1620, 2880, 1620)];
    let bottom = -1620 + 1548;
    let (x, y) = (-400, bottom - 129 - 24);
    assert_eq!(resized_y(y, 129, 297, &mons[1]), bottom - 297 - 24);
    let up = resized_position(x, y, (336, 129), DETAILED, &mons).unwrap();
    assert_eq!((up.y, up.move_first), (bottom - 297 - 24, true), "that monitor's margin and scale, not the primary's");
    let back = resized_position(x, up.y, (336, 297), COMPACT, &mons).unwrap();
    assert_eq!((back.y, back.move_first), (y, false));
    // Middle of the negative monitor: stays.
    assert_eq!(resized_position(-2000, -1000, (336, 129), DETAILED, &mons), None);
}

#[test]
fn a_height_larger_than_the_work_area_stays_at_its_top() {
    let m = MonitorGeom { name: Some("tiny".into()), primary: true, scale: 1.0, work_x: 10, work_y: 20, work_w: 400, work_h: 150 };
    let mons = vec![m.clone()];
    assert_eq!(resized_y(68, 86, 198, &m), 20, "gap 16 kept as far as it fits");
    assert_eq!(resized_position(50, 68, (224, 86), DETAILED, &mons), Some(ResizeMove { x: 50, y: 20, move_first: true }));
    assert_eq!(resized_y(20, 198, 86, &m), 84, "it hung below the work area: flush on the way back");
    let negative = MonitorGeom { name: Some("n".into()), primary: true, scale: 1.0, work_x: -500, work_y: -300, work_w: 400, work_h: 150 };
    assert_eq!(resized_y(-252, 86, 198, &negative), -300);
}

#[test]
fn the_clamp_still_brings_a_widget_back_sideways() {
    // Partly off the right edge, as a changed work area can leave it: moved in, height or no height.
    let mons = vec![monitor("m", true, 1.0, 0, 0, 1920, 1080)];
    let same = resized_position(1800, 400, (224, 86), COMPACT, &mons).unwrap();
    assert_eq!((same.x, same.y), (1920 - 224, 400));
    let taller = resized_position(1800, 930, (224, 86), DETAILED, &mons).unwrap();
    assert_eq!((taller.x, taller.y), (1920 - 224, 818));
    assert_eq!(resized_position(100, 100, (224, 86), DETAILED, &[]), None, "no monitors reported: left alone");
}

#[test]
fn settings_page_save_keeps_the_widget_position() {
    let e = Env::new();
    let page_copy = e.rt.settings(); // the page loaded its settings before the drag
    let dragged = WidgetPosition { monitor: r"\\.\DISPLAY1".into(), x: 300, y: 40 };
    e.rt.modify_settings(|s| s.desktop_widget.position = Some(dragged.clone())).unwrap();
    let mut changed = page_copy;
    changed.theme = ThemePref::Dark;
    let saved = e.rt.save_settings_from_page(changed).unwrap();
    assert_eq!(saved.theme, ThemePref::Dark, "the page's change is saved");
    assert_eq!(saved.desktop_widget.position, Some(dragged.clone()), "the stale copy did not move the widget back");
    assert_eq!(e.core.with_db(load_settings).unwrap().desktop_widget.position, Some(dragged));
}

#[test]
fn concurrent_settings_writers_never_undo_each_other() {
    // Widget menu, position save and settings page may write at the same moment: no lost update.
    let e = Env::new();
    e.settings(|s| s.desktop_widget.opacity = 50);
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                for _ in 0..5 {
                    e.rt.modify_settings(|s| s.desktop_widget.opacity += 1).unwrap();
                }
            });
        }
    });
    assert_eq!(e.rt.settings().desktop_widget.opacity, 90);
    assert_eq!(e.core.with_db(load_settings).unwrap().desktop_widget.opacity, 90);
}

#[test]
fn json_shape_is_stable_for_the_page() {
    let t = tray(false, Some(true), vec![line("codex", Some(73.0), Some(94.0), true, Severity::Warning, None)]);
    let v: Value = serde_json::to_value(widget::build_view(&t, None, &AppSettings::default(), 7)).unwrap();
    assert_eq!(v["providers"][0]["fiveHourRemaining"], 73.0);
    assert_eq!(v["providers"][0]["state"], "live");
    assert_eq!(v["providers"][0]["severity"], "warning");
    assert_eq!(v["providers"][0]["availability"], "allowed");
    assert_eq!(v["providers"][0]["fiveHourTone"], "normal");
    assert_eq!(v["providers"][0]["weeklyTone"], "normal");
    assert_eq!(v["providers"][0]["fiveHourReset"], json!({ "schedule": "missing", "atMs": null, "precisionMs": null }));
    assert_eq!(v["providers"][0]["weeklyReset"]["schedule"], "missing");
    assert_eq!(v["mode"], "compact");
    assert_eq!(v["opacity"], 85);
    assert!(v.get("settings").is_none() && v.get("snapshot").is_none(), "no raw settings or snapshots are sent");
}

// ----------------------------------------------------------------------------- custom icons

#[test]
fn custom_icons_come_only_from_valid_small_files_in_the_icons_folder() {
    let dir = tempfile::tempdir().unwrap();
    assert!(widget::custom_icons(dir.path()).is_empty(), "no folder: built-in icons");
    let icons = dir.path().join("icons");
    std::fs::create_dir_all(&icons).unwrap();
    std::fs::write(icons.join("codex.svg"), "\u{feff}  <svg xmlns=\"http://www.w3.org/2000/svg\"/>").unwrap();
    std::fs::write(icons.join("codex.png"), [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0]).unwrap();
    std::fs::write(icons.join("claude.svg"), "<html><script>alert(1)</script></html>").unwrap();
    std::fs::write(icons.join("claude.png"), b"not a png").unwrap();
    std::fs::write(icons.join("other.svg"), "<svg/>").unwrap();
    let got = widget::custom_icons(dir.path());
    assert_eq!(got.keys().copied().collect::<Vec<_>>(), vec!["codex"], "malformed files and other names are ignored");
    assert!(got["codex"].starts_with("data:image/svg+xml;base64,"), "an SVG wins over a PNG: {}", got["codex"]);

    std::fs::remove_file(icons.join("codex.svg")).unwrap();
    assert_eq!(widget::custom_icons(dir.path())["codex"], "data:image/png;base64,iVBORw0KGgoA");

    std::fs::write(icons.join("claude.svg"), vec![b' '; widget::CUSTOM_ICON_MAX_BYTES as usize + 1]).unwrap();
    assert!(!widget::custom_icons(dir.path()).contains_key("claude"), "too large: ignored");
}

// ------------------------------------------------------------------- Claude tick and Codex reset

/// What a manual Claude tick sends, given a Codex weekly read of 5 % taken 3 min ago whose weekly
/// reset is `weekly_reset_in_s` from now.
async fn claude_tick_after_codex_weekly(weekly_reset_in_s: i64) -> Option<String> {
    let e = Env::new();
    e.settings(|s| {
        s.language = LanguagePref::En;
        s.claude.active_refresh = Some(true);
    });
    let read_at = now_ms() - 3 * 60_000;
    e.codex_resetting(read_at, 10.0, 95.0, Some(true), read_at / 1000 + 3 * 3600, now_ms() / 1000 + weekly_reset_in_s);
    let after = e.rt.claude_tick(false, true).await.unwrap().expect("the Claude read is stored");
    after.notification.map(|(n, _)| format!("{} | {}", n.title, n.body))
}

#[tokio::test]
async fn a_claude_tick_reminds_of_no_codex_window_whose_reset_passed_since_its_read() {
    let current = claude_tick_after_codex_weekly(3 * 86_400).await;
    assert!(current.as_deref().is_some_and(|n| n.contains("Codex")), "control: a current low Codex weekly is reminded: {current:?}");
    let passed = claude_tick_after_codex_weekly(-60).await;
    assert!(!passed.as_deref().is_some_and(|n| n.contains("Codex")), "the read is from the last period: {passed:?}");
}

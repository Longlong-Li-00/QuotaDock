//! Product runtime: ties polling, notifications, tray status and settings together for every
//! provider (Codex via the Phase 2 pipeline, Claude via `crate::claude`). The Tauri shell only
//! renders what this returns (toast text, tray model, dashboard).

use super::PROVIDER_CODEX;
use super::claude_auth;
use super::claude_view;
use super::dashboard::{DashboardDto, ProviderDashboard, ProviderHealth, compatibility_mode, health_from};
use super::facts::{MonitorFacts, load_facts};
use super::history::{self, HistoryData, HistoryRange, load_history};
use super::notifications::{self, Candidate, Notification, NotifyInput, SqliteStore, WindowRuleInput};
use super::settings::{AppSettings, Lang, NotificationSettings, load_settings, save_settings};
use super::tray::{
    MAYBE_STALE_MS, TrayInput, TrayStatusModel, derive_tray_status, value_from_last_period, with_provider, with_update_time, without_codex,
};
use super::widget::{self, WidgetView};
use crate::analytics::forecast::{Risk, UsageForecast};
use crate::analytics::model::WindowType;
use crate::claude::integration::{self, DisableOutcome, IntegrationPlan, IntegrationState};
use crate::claude::model::PROVIDER_CLAUDE;
use crate::claude::monitor::{ClaudeMonitor, ClaudeStatus};
use crate::claude::statusline;
use crate::claude::store::{self as claude_store, ProviderEventRow, StoredObservation};
use crate::codex::provider::PollStatus;
use crate::commands::{self, CurrentUsage};
use crate::pipeline::{CoreError, PollOutcome, QuotaDockCore};
use crate::platform::health::{self, GapChange, GapTracker, HealthLog};
use crate::polling::{PollingHandle, RefreshDecision};
use crate::storage::diagnostics;
use crate::time::now_ms;
use serde::Serialize;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Debug, Clone)]
pub struct AfterPoll {
    pub notification: Option<(Notification, i64)>,
    pub tray: TrayStatusModel,
}

pub struct ProductRuntime {
    pub core: Arc<QuotaDockCore>,
    pub claude: ClaudeMonitor,
    polling: OnceLock<PollingHandle>,
    paused: AtomicBool,
    settings: Mutex<AppSettings>,
    /// Serializes settings writers (the settings page, the widget's menu and position save), so a
    /// read-modify-write never undoes another writer's change.
    settings_writer: Mutex<()>,
    /// One `apply_codex_enabled` at a time. The page's save does not wait for them, and the polling loop
    /// drops a Resume that arrives behind a Pause during a read, so an on has to wait for the off before it.
    codex_apply: tokio::sync::Mutex<()>,
    health: Arc<HealthLog>,
    watch: Mutex<WatchState>,
}

/// Watchdog / diagnostics bookkeeping (memory only).
#[derive(Debug, Default)]
struct WatchState {
    codex_gap: GapTracker,
    claude_gap: GapTracker,
    db_busy_ticks: u32,
    last_resources_ms: Option<i64>,
    last_foreground: Option<bool>,
    /// When the main window last came to the front (None while it is not).
    foreground_since_ms: Option<i64>,
    /// Derived origin of the last re-anchor recorded in analysis_diagnostics (outer None = not loaded yet).
    last_reanchor_origin: Option<Option<i64>>,
    last_claude_env: Option<serde_json::Value>,
}

/// Resource samples every 30 min (plus one at start) [QUOTADOCK HEURISTIC].
const RESOURCE_SAMPLE_MS: i64 = 30 * 60_000;
/// Consecutive one-minute watchdog ticks that found the database mutex held.
const DB_BUSY_TICKS: u32 = 3;

/// What the Claude rules, tray and card need (local reads only; no CLI call).
struct ClaudeSnapshot {
    status: ClaudeStatus,
    latest: Option<StoredObservation>,
    forecasts: Vec<UsageForecast>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeIntegrationView {
    pub enabled: Option<IntegrationState>,
    pub plan: Option<IntegrationPlan>,
    pub plan_error: Option<String>,
}

fn window_inputs(usage: &CurrentUsage, forecasts: &[UsageForecast]) -> Vec<WindowRuleInput> {
    usage
        .windows
        .iter()
        .map(|w| WindowRuleInput {
            window: w.window_type,
            remaining: if w.present && w.snapshot_quality == "normal" { w.remaining_percent } else { None },
            cycle_key: w.current_cycle.as_ref().map(|c| c.cycle_key.clone()),
            risk: forecasts.iter().find(|f| f.window_type == w.window_type).map(|f| f.risk),
        })
        .collect()
}

/// The reminder settings the rules see: a hidden 5-hour window reminds nobody and raises no level.
/// In memory only: the stored choices come back unchanged when the window is shown again, so this must
/// never reach `store_settings` or the page (which saves its whole copy).
fn rule_settings(settings: &AppSettings) -> NotificationSettings {
    let mut n = settings.notifications.clone();
    if !settings.codex.show_five_hour {
        n.five_hour_quota = false;
        n.forecast_five_hour = false;
    }
    if !settings.claude.show_five_hour {
        n.claude.five_hour_quota = false;
    }
    n
}

/// Takes a provider's hidden 5-hour window out of what the page shows. The tray model in the
/// dashboard already came from `rule_settings`.
fn strip_hidden_five_hour(dashboard: &mut DashboardDto) {
    let settings = &dashboard.settings;
    for p in &mut dashboard.providers {
        let show = if p.provider == PROVIDER_CODEX { settings.codex.show_five_hour } else { settings.claude.show_five_hour };
        if !show {
            p.usage.windows.retain(|w| w.window_type != WindowType::FiveHour);
            p.forecasts.retain(|f| f.window_type != WindowType::FiveHour);
        }
    }
}

/// Takes a Codex value that belongs to the last period out of what the page shows, the look a passed
/// Claude window has: the window, its reset time and its schedule stay. Here and not in
/// `get_current_codex_usage`, because `window_inputs` reads that value for the reminders.
fn clear_last_period_values(dashboard: &mut DashboardDto) {
    let now = dashboard.generated_at_ms;
    for p in dashboard.providers.iter_mut().filter(|p| p.provider == PROVIDER_CODEX) {
        let Some(captured_at) = p.usage.captured_at_ms else { continue };
        for w in &mut p.usage.windows {
            if value_from_last_period(captured_at, w.reported_reset_at_ms, w.remaining_percent, now) {
                w.used_percent = None;
                w.remaining_percent = None;
            }
        }
    }
}

fn weekly_risk(forecasts: &[UsageForecast]) -> Option<Risk> {
    forecasts.iter().find(|f| f.window_type == WindowType::Weekly).map(|f| f.risk)
}

impl ProductRuntime {
    pub fn new(core: Arc<QuotaDockCore>) -> Result<Arc<Self>, CoreError> {
        let data_dir = core.database_info().path.parent().map(PathBuf::from).unwrap_or_default();
        // The status-line bridge lives under ~/.quotadock (outside AppData; see claude::statusline).
        let home = dirs::home_dir().unwrap_or_else(|| data_dir.clone());
        Self::with_claude(core, ClaudeMonitor::new(&data_dir, &home))
    }

    /// With an explicit Claude monitor (tests use a private bridge folder and a fake CLI).
    pub fn with_claude(core: Arc<QuotaDockCore>, claude: ClaudeMonitor) -> Result<Arc<Self>, CoreError> {
        let settings = core.with_db(load_settings)?;
        let data_dir = core.database_info().path.parent().map(PathBuf::from).unwrap_or_default();
        let display = statusline::BridgeDisplay { show_five_hour: settings.claude.show_five_hour };
        let _ = statusline::write_display(claude.bridge_dir(), display);
        Ok(Arc::new(Self {
            core,
            claude,
            polling: OnceLock::new(),
            paused: AtomicBool::new(false),
            settings: Mutex::new(settings),
            settings_writer: Mutex::new(()),
            codex_apply: tokio::sync::Mutex::new(()),
            health: Arc::new(HealthLog::new(&data_dir)),
            watch: Mutex::new(WatchState::default()),
        }))
    }

    /// The soak health log (`<data dir>\logs\health.jsonl`).
    pub fn health(&self) -> &Arc<HealthLog> {
        &self.health
    }

    pub fn record_start(&self, launch: &str, relaunched: bool) {
        self.health.record(
            "app_start",
            json!({ "version": env!("CARGO_PKG_VERSION"), "pid": std::process::id(), "launch": launch, "relaunched": relaunched,
                    "classifier": crate::analytics::CLASSIFIER_VERSION }),
        );
    }

    /// At most one `app_stop` per process (backlog 35), whichever exit path writes it first.
    pub fn record_stop(&self, reason: &str) {
        self.health.record_stop(reason);
    }

    /// Window shown / hidden (explains the cadence in the soak report). Logged on change only.
    pub fn note_foreground(&self, foreground: bool) {
        let mut w = self.watch.lock().unwrap();
        if w.last_foreground != Some(foreground) {
            w.last_foreground = Some(foreground);
            w.foreground_since_ms = foreground.then(now_ms);
            drop(w);
            self.health.record("foreground", json!({ "value": foreground }));
        }
    }

    /// Once a minute from the shell. Takes no database lock: a stuck database is one of the things
    /// it detects. MONITORING_GAP is a QuotaDock heuristic, not a quota event.
    pub fn watchdog_tick(&self, foreground: bool) {
        let now = now_ms();
        let paused = self.is_paused();
        let settings = self.settings();
        let mut w = self.watch.lock().unwrap();
        let mut lines: Vec<(&'static str, serde_json::Value)> = Vec::new();

        let status = self.polling().map(|p| p.status());
        let codex_threshold = status
            .as_ref()
            .filter(|s| s.running && !paused && settings.codex.enabled)
            .map(|s| health::codex_gap_threshold_ms(foreground, s.next_delay_ms));
        let codex_last = status.as_ref().and_then(|s| s.last_started_ms);
        if let Some(change) = w.codex_gap.observe(codex_last, codex_threshold, now) {
            let in_flight = status.as_ref().is_some_and(|s| s.last_started_ms > s.last_finished_ms);
            let extra = json!({ "consecutiveFailures": status.as_ref().map(|s| s.consecutive_failures), "readInFlight": in_flight });
            lines.push(("monitoring_gap", gap_line("codex", &change, foreground, codex_threshold, extra)));
        }

        let a = self.claude.activity();
        let claude_expected = settings.claude.enabled
            && settings.claude.active_refresh_enabled()
            && a.active_usable
            && !paused
            && a.next_allowed_ms.is_none_or(|t| t <= now);
        let foreground_for = w.foreground_since_ms.map(|since| now - since);
        let claude_threshold = claude_expected.then(|| health::claude_gap_threshold_ms(foreground, foreground_for));
        let claude_last = [a.last_attempt_ms, a.last_success_ms, a.last_passive_ms].into_iter().flatten().max();
        if let Some(change) = w.claude_gap.observe(claude_last, claude_threshold, now) {
            lines.push(("monitoring_gap", gap_line("claude", &change, foreground, claude_threshold, json!({}))));
        }

        if self.core.db_busy() {
            w.db_busy_ticks += 1;
            if w.db_busy_ticks == DB_BUSY_TICKS {
                lines.push(("db_busy", json!({ "ticks": w.db_busy_ticks })));
            }
        } else {
            if w.db_busy_ticks >= DB_BUSY_TICKS {
                lines.push(("db_busy_end", json!({ "ticks": w.db_busy_ticks })));
            }
            w.db_busy_ticks = 0;
        }
        let sample = w.last_resources_ms.is_none_or(|t| now - t >= RESOURCE_SAMPLE_MS);
        if sample {
            w.last_resources_ms = Some(now);
        }
        drop(w);
        for (kind, line) in lines {
            self.health.record(kind, line);
        }
        if sample {
            self.health.record("resources", health::resource_sample(foreground));
        }
    }

    /// Diagnostics only: shadow 5h estimates and re-anchor records (analysis_diagnostics). Errors
    /// are swallowed: diagnostics never affect monitoring.
    fn record_analysis_diagnostics(&self) {
        let Ok(shadow) = commands::five_hour_shadow(&self.core) else { return };
        let now = now_ms();
        let cached = self.watch.lock().unwrap().last_reanchor_origin;
        let known = match cached {
            Some(k) => k,
            None => self
                .core
                .with_db(|c| diagnostics::latest(c, diagnostics::KIND_REANCHOR))
                .ok()
                .flatten()
                .and_then(|v| v.get("originMs").and_then(serde_json::Value::as_i64)),
        };
        if let Some(s) = &shadow {
            let data = serde_json::to_value(s).unwrap_or_default();
            let _ = self.core.with_db(|c| {
                if Some(s.origin_ms) != known {
                    diagnostics::insert(c, diagnostics::KIND_REANCHOR, Some("five_hour"), Some(s.segment_start_snapshot_id), &data, now)?;
                }
                diagnostics::insert(c, diagnostics::KIND_SHADOW, Some("five_hour"), Some(s.latest_snapshot_id), &data, now)
            });
        }
        let origin = shadow.as_ref().map(|s| s.origin_ms);
        self.watch.lock().unwrap().last_reanchor_origin = Some(origin.or(known));
    }

    fn note_claude_env(&self) {
        let e = self.claude.environment();
        if e.checked_at_ms.is_none() {
            return;
        }
        let summary = json!({
            "version": e.version, "installKind": e.installation.as_ref().map(|i| i.kind), "billing": e.billing().as_str(),
            "loggedIn": e.auth.as_ref().map(|a| a.logged_in), "authMethod": e.auth.as_ref().and_then(|a| a.auth_method.clone()),
            "apiProvider": e.auth.as_ref().and_then(|a| a.api_provider.clone()),
            "subscriptionType": e.auth.as_ref().and_then(|a| a.subscription_type.clone()),
        });
        let mut w = self.watch.lock().unwrap();
        if w.last_claude_env.as_ref() != Some(&summary) {
            w.last_claude_env = Some(summary.clone());
            drop(w);
            self.health.record("claude_env", summary);
        }
    }

    pub fn attach_polling(&self, handle: PollingHandle) {
        let _ = self.polling.set(handle);
    }

    pub fn polling(&self) -> Option<&PollingHandle> {
        self.polling.get()
    }

    pub fn settings(&self) -> AppSettings {
        self.settings.lock().unwrap().clone()
    }

    pub fn lang(&self) -> Lang {
        self.settings().effective_language()
    }

    pub fn update_settings(&self, new: AppSettings) -> Result<AppSettings, CoreError> {
        let _writer = self.settings_writer.lock().unwrap();
        self.store_settings(new)
    }

    /// Read-modify-write of the settings under the writer lock: a change made meanwhile by another
    /// writer (e.g. the widget position saved after a drag) is kept, not overwritten.
    pub fn modify_settings(&self, change: impl FnOnce(&mut AppSettings)) -> Result<AppSettings, CoreError> {
        let _writer = self.settings_writer.lock().unwrap();
        let mut s = self.settings();
        change(&mut s);
        self.store_settings(s)
    }

    /// The settings page saves its whole copy, except the widget position: the backend owns it (saved
    /// after a drag), so a page holding an older copy never moves the widget back.
    pub fn save_settings_from_page(&self, new: AppSettings) -> Result<AppSettings, CoreError> {
        self.modify_settings(|s| {
            let position = s.desktop_widget.position.take();
            *s = new;
            s.desktop_widget.position = position;
        })
    }

    fn store_settings(&self, new: AppSettings) -> Result<AppSettings, CoreError> {
        self.core.with_db(|c| save_settings(c, &new))?;
        *self.settings.lock().unwrap() = new.clone();
        self.sync_bridge_display(&new);
        Ok(new)
    }

    /// Tells the status-line bridge whether to print 5-hour (backlog 37). Display only, best effort.
    pub fn sync_bridge_display(&self, s: &AppSettings) {
        let display = statusline::BridgeDisplay { show_five_hour: s.claude.show_five_hour };
        let _ = statusline::write_display(self.claude.bridge_dir(), display);
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    /// Pauses every provider: no Codex reads, QuotaDock's app-server stopped, no Claude queries,
    /// and Claude status-line updates arriving meanwhile are not stored. Not persisted.
    pub async fn pause(&self) {
        if let Some(p) = self.polling() {
            p.pause().await;
        }
        self.paused.store(true, Ordering::SeqCst);
        self.claude.set_paused(true, now_ms());
        self.health.record("paused", json!({ "value": true }));
        self.core.shutdown().await;
    }

    /// Immediate Codex read (the provider starts its app-server on demand), then the normal schedule.
    pub fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
        self.claude.set_paused(false, now_ms());
        self.health.record("paused", json!({ "value": false }));
        if self.settings().codex.enabled
            && let Some(p) = self.polling()
        {
            p.resume();
        }
    }

    /// After the Codex switch changed. Off: reads stop (waits for one in flight) and QuotaDock's
    /// app-server is stopped. On: an immediate read, unless monitoring is paused. Applies run one at
    /// a time, each on the switch as it is stored when its turn comes.
    pub async fn apply_codex_enabled(&self) {
        let _turn = self.codex_apply.lock().await;
        let enabled = self.settings().codex.enabled;
        self.health.record("codex_enabled", json!({ "value": enabled }));
        let Some(p) = self.polling() else { return };
        if !enabled {
            p.pause().await;
            self.core.shutdown().await;
        } else if !self.is_paused() {
            p.resume();
        }
    }

    pub async fn refresh(&self) -> RefreshDecision {
        if self.is_paused() {
            return RefreshDecision::Paused;
        }
        // Nothing to read for Codex; the caller already started the Claude step.
        if !self.settings().codex.enabled {
            return RefreshDecision::Started;
        }
        let decision = match self.polling() {
            Some(p) => p.refresh_now().await,
            None => RefreshDecision::Stopped,
        };
        // A loop still paused by the stop of an earlier Off: nobody paused monitoring, and the
        // apply of the newer On reads as soon as that stop ends.
        if decision == RefreshDecision::Paused && !self.is_paused() && self.settings().codex.enabled {
            return RefreshDecision::Started;
        }
        decision
    }

    fn forecasts(&self) -> Vec<UsageForecast> {
        WindowType::ALL.iter().filter_map(|w| commands::get_usage_forecast(&self.core, *w).ok()).collect()
    }

    fn claude_snapshot(&self, settings: &AppSettings, now: i64) -> Result<Option<ClaudeSnapshot>, CoreError> {
        if !settings.claude.enabled {
            return Ok(None);
        }
        let status = self.claude.status(&self.core, true, settings.claude.active_refresh_enabled(), now)?;
        let latest = self.core.with_db(|c| claude_store::latest(c, PROVIDER_CLAUDE))?;
        let forecasts = self.core.with_db(|c| claude_view::forecasts(c, latest.as_ref(), &self.core.analytics, now))?;
        Ok(Some(ClaudeSnapshot { status, latest, forecasts }))
    }

    /// Merges both providers' candidates of this evaluation into at most one notification.
    fn decide(
        &self,
        mut candidates: Vec<Candidate>,
        claude: Option<&ClaudeSnapshot>,
        claude_events: &[ProviderEventRow],
        settings: &AppSettings,
        now: i64,
    ) -> Result<Option<(Notification, i64)>, CoreError> {
        let lang = settings.effective_language();
        let rules = rule_settings(settings);
        self.core.with_db(|conn| {
            if let Some(cl) = claude {
                let inputs = claude_view::window_inputs(cl.latest.as_ref(), &cl.forecasts, now);
                candidates.extend(claude_view::evaluate(&inputs, claude_events, &rules.claude, lang, &SqliteStore(conn)));
            }
            candidates.sort_by_key(|c| c.priority);
            let Some(n) = notifications::aggregate(candidates, lang) else { return Ok(None) };
            let providers: std::collections::BTreeSet<&str> = n.keys.iter().map(|k| k.provider.as_str()).collect();
            let label = if providers.len() == 1 { providers.into_iter().next().unwrap_or(PROVIDER_CODEX) } else { "multi" };
            let id = notifications::record(conn, label, &n, lang, now)?;
            Ok(Some((n, id)))
        })
    }

    fn tray_with(
        &self,
        facts: &MonitorFacts,
        codex_risk: Option<Risk>,
        claude: Option<&ClaudeSnapshot>,
        settings: &AppSettings,
        now: i64,
    ) -> TrayStatusModel {
        let lang = settings.effective_language();
        let rules = rule_settings(settings);
        let base = if settings.codex.enabled {
            derive_tray_status(&TrayInput {
                facts,
                paused: self.is_paused(),
                weekly_risk: codex_risk,
                show_five_hour: settings.codex.show_five_hour,
                settings: &rules,
                lang,
                now_ms: now,
            })
        } else {
            without_codex(self.is_paused(), lang)
        };
        match claude {
            Some(cl) => {
                let mut line = claude_view::tray_line(&cl.status, cl.latest.as_ref(), weekly_risk(&cl.forecasts), &rules.claude, lang, now);
                line.show_five_hour = settings.claude.show_five_hour;
                let live = line.live;
                let model = with_provider(base, line, lang);
                if settings.codex.enabled { model } else { with_update_time(model, cl.status.last_update_ms, live, now) }
            }
            None => base,
        }
    }

    /// Evaluates notification rules for this Codex poll (at most one notification) and the tray.
    pub fn after_poll(&self, outcome: &PollOutcome) -> Result<AfterPoll, CoreError> {
        let settings = self.settings();
        let lang = settings.effective_language();
        let usage = commands::get_current_codex_usage(&self.core)?;
        let forecasts = self.forecasts();
        let windows = window_inputs(&usage, &forecasts);
        let poll_success = outcome.status == PollStatus::Success && outcome.snapshot_id.is_some();
        let now = now_ms();
        let facts = self.core.with_db(load_facts)?;
        // A read still in flight when Codex was turned off is stored, but no longer reminds anyone.
        let codex = if settings.codex.enabled {
            let rules = rule_settings(&settings);
            // Refresh notices have no setting of their own for the 5-hour window. Events without a
            // window (a redeemed reset credit) concern the account and stay.
            let new_events: Vec<_> =
                outcome.events.iter().filter(|e| settings.codex.show_five_hour || e.window_type != Some(WindowType::FiveHour)).cloned().collect();
            self.core.with_db(|conn| {
                let input = NotifyInput {
                    provider: PROVIDER_CODEX,
                    facts: &facts,
                    poll_success,
                    windows: &windows,
                    new_events: &new_events,
                    settings: &rules,
                    lang,
                    now_ms: now,
                };
                Ok(notifications::evaluate(&input, &SqliteStore(conn)))
            })?
        } else {
            vec![]
        };
        let claude = self.claude_snapshot(&settings, now)?;
        let notification = self.decide(codex, claude.as_ref(), &[], &settings, now)?;
        let tray = self.tray_with(&facts, weekly_risk(&forecasts), claude.as_ref(), &settings, now);
        self.record_analysis_diagnostics();
        Ok(AfterPoll { notification, tray })
    }

    /// One Claude step: environment check (hourly), the bridge file (cheap), and the optional
    /// active fallback when it is due. Returns Some when something new was stored or `manual`.
    pub async fn claude_tick(&self, foreground: bool, manual: bool) -> Result<Option<AfterPoll>, CoreError> {
        let mut settings = self.settings();
        if !settings.claude.enabled || self.is_paused() {
            return Ok(None);
        }
        let now = now_ms();
        let mut signed_out = None;
        if self.claude.environment_due(now) || manual {
            self.claude.refresh_environment(now).await;
            self.note_claude_env();
            // Paused or disabled while the check ran: no notice, nothing else this tick.
            settings = self.settings();
            if !settings.claude.enabled || self.is_paused() {
                return Ok(None);
            }
            let obs = claude_auth::AuthObservation::of(self.claude.environment().auth.as_ref());
            signed_out = self.core.with_db(|c| claude_auth::observe(c, obs, now))?;
        }
        let before = self.claude.activity();
        let mut event_ids = Vec::new();
        let mut stored = false;
        if let Some(i) = self.claude.poll_passive(&self.core, now)? {
            event_ids.extend(i.result.event_ids);
            stored = true;
        }
        if self.claude.active_due(&self.core, settings.claude.active_refresh_enabled(), foreground, manual, now)?
            && let Some(i) = self.claude.run_active(&self.core, now_ms()).await?
        {
            event_ids.extend(i.result.event_ids);
            stored = true;
        }
        let after = self.claude.activity();
        if after.suspended && !before.suspended {
            self.health.record("claude_active", json!({ "event": "suspended_after_rate_limits" }));
        }
        if after.needs_verification && !before.needs_verification {
            self.health.record("claude_active", json!({ "event": "needs_verification", "error": self.claude.active_state().last_error }));
        }
        if !stored && !manual && signed_out.is_none() {
            return Ok(None);
        }
        // The active query can take 45 s: a switch changed meanwhile applies to what comes next.
        settings = self.settings();
        let now = now_ms();
        let mut events = self.core.with_db(|c| claude_store::events_by_ids(c, &event_ids))?;
        if !settings.claude.show_five_hour {
            events.retain(|e| e.window_key.as_deref() != Some("five_hour"));
        }
        let claude = self.claude_snapshot(&settings, now)?;
        // Codex quota rules are state-based and deduplicated, so evaluating them here only merges a
        // simultaneous Codex low with a Claude low; nothing is sent twice.
        let usage = commands::get_current_codex_usage(&self.core)?;
        let forecasts = self.forecasts();
        let mut windows = window_inputs(&usage, &forecasts);
        // No read of its own here either: a window whose reset passed since the last read is from the
        // last period and reminds nobody, as Claude's own windows (`claude_view::current`). Backlog 38.
        if let Some(captured) = usage.captured_at_ms {
            for (input, w) in windows.iter_mut().zip(&usage.windows) {
                if super::tray::value_from_last_period(captured, w.reported_reset_at_ms, input.remaining, now) {
                    input.remaining = None;
                }
            }
        }
        let facts = self.core.with_db(load_facts)?;
        let lang = settings.effective_language();
        // Current Codex data only: this evaluation has no read of its own, and a reminder that was never
        // sent (its window was hidden, say) must not fire about a window that may have ended meanwhile.
        let codex_current = facts.last_valid_snapshot_at_ms.is_some_and(|t| now - t <= MAYBE_STALE_MS);
        let codex = if settings.codex.enabled && facts.latest.is_some() && codex_current {
            let rules = rule_settings(&settings);
            self.core.with_db(|conn| {
                let input = NotifyInput {
                    provider: PROVIDER_CODEX,
                    facts: &facts,
                    poll_success: true,
                    windows: &windows,
                    new_events: &[],
                    settings: &rules,
                    lang,
                    now_ms: now,
                };
                Ok(notifications::evaluate_windows(&input, &SqliteStore(conn)))
            })?
        } else {
            vec![]
        };
        let notification = if stored || signed_out.is_some() {
            let mut candidates = codex;
            candidates.extend(signed_out.as_ref().map(|d| claude_auth::candidate(lang, d.episode_ms)));
            let n = self.decide(candidates, claude.as_ref(), &events, &settings, now)?;
            // An explicit sign-out after a confirmed sign-in (`claude_auth`): the episode counts as
            // notified only now that its notice is recorded.
            if let Some(due) = &signed_out {
                self.core.with_db(|c| claude_auth::commit(c, due, now))?;
                self.health.record("claude_auth", json!({ "event": "signed_out_after_sign_in", "episodeMs": due.episode_ms }));
            }
            n
        } else {
            None
        };
        let tray = self.tray_with(&facts, weekly_risk(&forecasts), claude.as_ref(), &settings, now);
        Ok(Some(AfterPoll { notification, tray }))
    }

    pub fn tray_status(&self) -> Result<TrayStatusModel, CoreError> {
        let settings = self.settings();
        let now = now_ms();
        let forecasts = self.forecasts();
        let facts = self.core.with_db(load_facts)?;
        let claude = self.claude_snapshot(&settings, now)?;
        Ok(self.tray_with(&facts, weekly_risk(&forecasts), claude.as_ref(), &settings, now))
    }

    /// The desktop widget's view (Phase 3.7): the same tray model as the tray icon and dashboard,
    /// read from SQLite, with the resets the dashboard shows. Never takes the provider lock, never
    /// polls, never runs the Claude CLI.
    pub fn widget_view(&self) -> Result<WidgetView, CoreError> {
        let settings = self.settings();
        let now = now_ms();
        let forecasts = self.forecasts();
        let facts = self.core.with_db(load_facts)?;
        let claude = self.claude_snapshot(&settings, now)?;
        let tray = self.tray_with(&facts, weekly_risk(&forecasts), claude.as_ref(), &settings, now);
        let usage = commands::get_current_codex_usage(&self.core)?;
        let claude_windows = claude.as_ref().map(|c| claude_view::window_statuses(c.latest.as_ref(), now)).unwrap_or_default();
        let mut view = widget::build_view(&tray, claude.as_ref().and_then(|c| c.status.last_update_ms), &settings, now);
        view.custom_icons = widget::custom_icons(&self.data_dir());
        Ok(widget::with_resets(view, &usage.windows, &claude_windows))
    }

    pub fn mark_notification_delivery(&self, log_id: i64, delivered: bool, error: Option<&str>) {
        let _ = self.core.with_db(|c| notifications::mark_delivery(c, log_id, delivered, error));
    }

    /// What the page shows. The diagnostics export uses `dashboard_complete`: a support bundle should
    /// not depend on a display preference.
    pub async fn dashboard(&self) -> Result<DashboardDto, CoreError> {
        let mut dashboard = self.dashboard_complete().await?;
        strip_hidden_five_hour(&mut dashboard);
        clear_last_period_values(&mut dashboard);
        Ok(dashboard)
    }

    async fn dashboard_complete(&self) -> Result<DashboardDto, CoreError> {
        let settings = self.settings();
        let now = now_ms();
        let usage = commands::get_current_codex_usage(&self.core)?;
        let forecasts = self.forecasts();
        let facts = self.core.with_db(load_facts)?;
        let status = self.core.provider_status().await;
        let paused = self.is_paused();
        let claude = self.claude_snapshot(&settings, now)?;
        let tray = self.tray_with(&facts, weekly_risk(&forecasts), claude.as_ref(), &settings, now);
        let health = ProviderHealth {
            state: health_from(
                paused,
                facts.latest.is_some(),
                facts.trailing_failures,
                facts.last_poll_status.as_deref(),
                facts.last_error.as_deref(),
            ),
            consecutive_failures: facts.trailing_failures,
            last_error: facts.last_error.clone(),
        };
        let mut providers = Vec::new();
        if settings.codex.enabled {
            providers.push(ProviderDashboard {
                provider: PROVIDER_CODEX,
                display_name: "Codex",
                plan_type: status.plan_type.clone(),
                cli_version: status.installation.as_ref().map(|i| i.version.clone()),
                app_server_version: status.app_server_version.clone(),
                compatibility_mode: compatibility_mode(status.installation.as_ref().map(|i| i.gate)),
                health,
                usage,
                forecasts,
                extra_limits: vec![],
                refresh_capability: if paused { "unavailable" } else { "active" },
                source: None,
            });
        }
        if let Some(cl) = claude {
            providers.push(claude_view::dashboard(&cl.status, cl.latest.as_ref(), cl.forecasts, now));
        }
        Ok(DashboardDto {
            generated_at_ms: now,
            paused,
            polling: self.polling().map(|p| p.status()),
            tray,
            providers,
            effective_language: settings.effective_language(),
            settings,
        })
    }

    pub fn history(&self, range: HistoryRange) -> Result<Option<HistoryData>, CoreError> {
        let now = now_ms();
        let settings = self.settings();
        let state = self.core.engine_state();
        let usage = commands::get_current_codex_usage(&self.core)?;
        // A hidden window does not decide the visible "current cycle" range.
        let cycle_start = usage
            .windows
            .iter()
            .filter(|w| settings.codex.show_five_hour || w.window_type != WindowType::FiveHour)
            .filter_map(|w| w.current_cycle.as_ref().map(|c| c.observed_started_at_ms))
            .min();
        let mut data = match state.context.as_ref().filter(|_| settings.codex.enabled) {
            Some(ctx) => {
                let account = ctx.account_fingerprint.clone();
                self.core.with_db(|c| load_history(c, &account, range, now, cycle_start))?
            }
            None => {
                let (from_ms, resolution_ms) = history::range_bounds(range, now, cycle_start);
                HistoryData { range, from_ms, to_ms: now, resolution_ms, series: vec![], markers: vec![], source_snapshots: 0 }
            }
        };
        if settings.claude.enabled {
            self.core.with_db(|c| history::append_claude(c, &mut data))?;
        }
        let shown = |provider: &str| if provider == PROVIDER_CODEX { settings.codex.show_five_hour } else { settings.claude.show_five_hour };
        data.series.retain(|s| s.window_type != WindowType::FiveHour || shown(s.provider));
        data.markers.retain(|m| m.window_type.as_deref() != Some("five_hour") || shown(m.provider));
        Ok(Some(data))
    }

    /// Events of every enabled provider on one timeline, newest first (`before_ms` = next page).
    pub fn events(
        &self,
        include_diagnostic: bool,
        before_ms: Option<i64>,
        limit: usize,
    ) -> Result<Vec<crate::storage::derived::EventView>, CoreError> {
        let settings = self.settings();
        let (codex, claude) = (settings.codex.enabled, settings.claude.enabled);
        self.core.with_db(|c| {
            let mut all = if codex {
                crate::storage::derived::events_before(c, include_diagnostic, before_ms, limit, !settings.codex.show_five_hour)?
            } else {
                vec![]
            };
            if claude {
                all.extend(claude_store::event_views_before(c, include_diagnostic, before_ms, limit, !settings.claude.show_five_hour)?);
            }
            all.sort_by(|a, b| b.observed_at_ms.cmp(&a.observed_at_ms).then(b.id.cmp(&a.id)));
            all.truncate(limit);
            Ok(all)
        })
    }

    pub fn data_dir(&self) -> PathBuf {
        self.core.database_info().path.parent().map(PathBuf::from).unwrap_or_default()
    }

    // ------------------------------------------------------------------ Claude integration (opt-in)

    pub fn claude_integration_view(&self) -> Result<ClaudeIntegrationView, CoreError> {
        let enabled = ClaudeMonitor::integration_state(&self.core)?;
        let (plan, plan_error) = match dirs::home_dir() {
            Some(home) if enabled.is_none() => match self.claude.integration_plan(&home, now_ms()) {
                Ok(p) => (Some(p), None),
                Err(e) => (None, Some(e)),
            },
            Some(_) => (None, None),
            None => (None, Some("home directory not found".into())),
        };
        Ok(ClaudeIntegrationView { enabled, plan, plan_error })
    }

    /// Installs the bridge (copied from `bridge_source`) and writes the previewed statusLine. The
    /// preview is identified by its time and fingerprint; anything else on disk is refused.
    pub fn enable_claude_integration(&self, bridge_source: &Path, planned_at_ms: i64, plan_id: &str) -> Result<IntegrationState, String> {
        if ClaudeMonitor::integration_state(&self.core).map_err(|e| e.to_string())?.is_some() {
            return Err("Claude integration is already enabled".into());
        }
        let home = dirs::home_dir().ok_or("home directory not found")?;
        let now = now_ms();
        if planned_at_ms > now + 60_000 || now - planned_at_ms > 24 * 3_600_000 {
            return Err("the preview is out of date; review the change again".into());
        }
        let plan = self.claude.integration_plan(&home, planned_at_ms)?;
        integration::check_reviewed(&plan, plan_id)?;
        if !bridge_source.is_file() {
            return Err(format!("bridge executable not found next to QuotaDock ({})", integration::BRIDGE_EXE));
        }
        if let Some(dir) = plan.bridge_install_path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::copy(bridge_source, &plan.bridge_install_path).map_err(|e| format!("could not install the bridge: {e}"))?;
        let state = integration::enable(&plan, now)?;
        ClaudeMonitor::save_integration_state(&self.core, Some(&state), now).map_err(|e| e.to_string())?;
        // The bridge's own folder, so its first line already knows whether 5-hour is shown.
        let _ = std::fs::create_dir_all(self.claude.bridge_dir());
        self.sync_bridge_display(&self.settings());
        Ok(state)
    }

    /// Restores the previous statusLine (if it is still QuotaDock's) and removes the bridge.
    pub fn disable_claude_integration(&self) -> Result<DisableOutcome, String> {
        let Some(state) = ClaudeMonitor::integration_state(&self.core).map_err(|e| e.to_string())? else {
            return Ok(DisableOutcome::NothingToRestore);
        };
        let outcome = integration::disable(&state)?;
        ClaudeMonitor::save_integration_state(&self.core, None, now_ms()).map_err(|e| e.to_string())?;
        if outcome == DisableOutcome::Restored {
            let _ = std::fs::remove_file(self.claude.bridge_exe_path());
        }
        Ok(outcome)
    }

    /// Writes a sanitized diagnostics bundle into QuotaDock's own data folder and returns its path.
    pub async fn export_diagnostics(&self) -> Result<PathBuf, String> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Bundle {
            generated_at: String,
            app_version: &'static str,
            classifier_version: &'static str,
            dashboard: DashboardDto,
            events: Vec<crate::storage::derived::EventView>,
            recent_snapshots: Vec<crate::storage::snapshots::SnapshotView>,
            recent_poll_runs: Vec<serde_json::Value>,
            recent_notifications: Vec<serde_json::Value>,
            recent_provider_starts: Vec<serde_json::Value>,
            claude: serde_json::Value,
            /// Diagnostics only (never shown or used for decisions): 5h shadow estimates.
            analysis: serde_json::Value,
        }
        let dashboard = self.dashboard_complete().await.map_err(|e| e.to_string())?;
        let events = commands::get_quota_events(&self.core, true, 500).map_err(|e| e.to_string())?;
        let snaps = commands::get_usage_history(&self.core, 0, 300).map_err(|e| e.to_string())?;
        let (runs, notes, starts) = self
            .core
            .with_db(|c| {
                let mut s = c.prepare("SELECT id, started_at_ms, finished_at_ms, poll_trigger, status, error_message FROM poll_runs ORDER BY id DESC LIMIT 300")?;
                let runs = s
                    .query_map([], |r| {
                        Ok(serde_json::json!({ "id": r.get::<_, i64>(0)?, "startedAtMs": r.get::<_, i64>(1)?, "finishedAtMs": r.get::<_, i64>(2)?,
                            "trigger": r.get::<_, String>(3)?, "status": r.get::<_, String>(4)?, "error": r.get::<_, Option<String>>(5)? }))
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                let mut s = c.prepare("SELECT created_at_ms, priority, rule_ids_json, language, title, delivered, error FROM notification_log ORDER BY id DESC LIMIT 100")?;
                let notes = s
                    .query_map([], |r| {
                        Ok(serde_json::json!({ "createdAtMs": r.get::<_, i64>(0)?, "priority": r.get::<_, i64>(1)?, "rules": r.get::<_, String>(2)?,
                            "language": r.get::<_, String>(3)?, "title": r.get::<_, String>(4)?, "delivered": r.get::<_, Option<bool>>(5)?, "error": r.get::<_, Option<String>>(6)? }))
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                let mut s = c.prepare("SELECT started_at_ms, reason, pid, cli_version FROM provider_sessions ORDER BY id DESC LIMIT 100")?;
                let starts = s
                    .query_map([], |r| {
                        Ok(serde_json::json!({ "startedAtMs": r.get::<_, i64>(0)?, "reason": r.get::<_, String>(1)?,
                            "pid": r.get::<_, Option<i64>>(2)?, "cliVersion": r.get::<_, Option<String>>(3)? }))
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok((runs, notes, starts))
            })
            .map_err(|e| e.to_string())?;
        // Claude: versions, modes and counts only (no install paths, no account values).
        let env = self.claude.environment();
        // Read before the with_db below: the database mutex is not re-entrant.
        let integration_enabled = ClaudeMonitor::integration_state(&self.core).ok().flatten().is_some();
        let claude = self
            .core
            .with_db(|c| {
                Ok(serde_json::json!({
                    "installKind": env.installation.as_ref().map(|i| i.kind),
                    "version": env.version,
                    "billing": env.billing().as_str(),
                    "loggedIn": env.auth.as_ref().map(|a| a.logged_in),
                    "authMethod": env.auth.as_ref().and_then(|a| a.auth_method.clone()),
                    "subscriptionType": env.auth.as_ref().and_then(|a| a.subscription_type.clone()),
                    "envOverrides": env.env_flags,
                    "active": self.claude.active_state(),
                    "integrationEnabled": integration_enabled,
                    "recentRuns": claude_store::recent_runs(c, PROVIDER_CLAUDE, 50)?,
                    "events": claude_store::events(c, PROVIDER_CLAUDE, true, 200)?,
                }))
            })
            .map_err(|e| e.to_string())?;
        // Separate with_db calls (the database mutex is not re-entrant).
        let shadow = commands::five_hour_shadow(&self.core).ok().flatten();
        let reanchors = self.core.with_db(|c| diagnostics::recent(c, diagnostics::KIND_REANCHOR, 20)).unwrap_or_default();
        let analysis = json!({ "fiveHourShadow": shadow, "recentReanchors": reanchors, "healthLog": self.health.path().file_name().map(|n| n.to_string_lossy().into_owned()) });
        let bundle = Bundle {
            generated_at: crate::time::iso_utc_ms(now_ms()).unwrap_or_default(),
            app_version: env!("CARGO_PKG_VERSION"),
            classifier_version: crate::analytics::CLASSIFIER_VERSION,
            dashboard,
            events,
            recent_snapshots: snaps,
            recent_poll_runs: runs,
            recent_notifications: notes,
            recent_provider_starts: starts,
            claude,
            analysis,
        };
        let dir = self.data_dir().join("exports");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let name = format!("diagnostics-{}.json", chrono::Local::now().format("%Y%m%d-%H%M%S"));
        let path = dir.join(name);
        let text = serde_json::to_string_pretty(&bundle).map_err(|e| e.to_string())?;
        // Belt and braces: the bundle is built from sanitized views; scrub strings once more.
        let text = crate::codex::sanitize::sanitize_string(&text);
        std::fs::write(&path, text).map_err(|e| e.to_string())?;
        Ok(path)
    }
}

fn gap_line(provider: &str, change: &GapChange, foreground: bool, threshold_ms: Option<i64>, extra: serde_json::Value) -> serde_json::Value {
    let mut v = match change {
        GapChange::Opened { since_ms, idle_ms } => json!({ "provider": provider, "change": "opened", "sinceMs": since_ms, "idleMs": idle_ms }),
        GapChange::Closed { since_ms, duration_ms } => {
            json!({ "provider": provider, "change": "closed", "sinceMs": since_ms, "durationMs": duration_ms })
        }
    };
    if let Some(o) = v.as_object_mut() {
        o.insert("foreground".into(), foreground.into());
        o.insert("thresholdMs".into(), threshold_ms.into());
        if let serde_json::Value::Object(e) = extra {
            o.extend(e);
        }
    }
    v
}
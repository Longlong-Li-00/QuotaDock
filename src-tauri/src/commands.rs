//! The complete webview API: 18 narrow, typed, purpose-specific commands for the main page
//! (capabilities/default.json) and 4 for the desktop widget (capabilities/widget.json). There is
//! no command that runs a shell, executes SQL, sends JSON-RPC, or reads/writes arbitrary files.

use crate::AppState;
use quotadock_core::analytics::WindowType;
use quotadock_core::analytics::forecast::UsageForecast;
use quotadock_core::claude::integration::DisableOutcome;
use quotadock_core::commands::{self as core_cmd, CurrentUsage, FullProviderStatus};
use quotadock_core::polling::RefreshDecision;
use quotadock_core::product::dashboard::DashboardDto;
use quotadock_core::product::history::{HistoryData, HistoryRange};
use quotadock_core::product::runtime::ClaudeIntegrationView;
use quotadock_core::product::settings::{AppSettings, Lang};
use quotadock_core::product::widget::{WIDGET_LABEL, WidgetEvent, WidgetView};
use quotadock_core::storage::derived::EventView;
use serde::Serialize;
use tauri::{AppHandle, State, WebviewWindow};

fn err(e: impl std::fmt::Display) -> String {
    quotadock_core::codex::sanitize::sanitize_string(&e.to_string())
}

#[tauri::command]
pub async fn get_dashboard(state: State<'_, AppState>) -> Result<DashboardDto, String> {
    state.runtime.dashboard().await.map_err(err)
}

#[tauri::command]
pub async fn get_current_usage(state: State<'_, AppState>) -> Result<CurrentUsage, String> {
    core_cmd::get_current_codex_usage(&state.runtime.core).map_err(err)
}

#[tauri::command]
pub async fn get_usage_forecast(state: State<'_, AppState>, window: String) -> Result<UsageForecast, String> {
    let w = WindowType::parse(&window).ok_or_else(|| format!("unknown window {window:?}"))?;
    core_cmd::get_usage_forecast(&state.runtime.core, w).map_err(err)
}

/// Goes through the polling controller: coalesced/throttled, never a direct backend call.
/// Refreshes every refreshable provider: Codex through its polling controller, Claude through
/// the bridge file and (only if enabled and allowed) the local /usage fallback. Never launches an
/// interactive Claude session or sends a prompt.
#[tauri::command]
pub async fn refresh_usage(app: AppHandle, state: State<'_, AppState>) -> Result<RefreshDecision, String> {
    crate::refresh_claude(&app, true);
    Ok(state.runtime.refresh().await)
}

#[tauri::command]
pub async fn get_usage_history(state: State<'_, AppState>, range: HistoryRange) -> Result<Option<HistoryData>, String> {
    state.runtime.history(range).map_err(err)
}

/// Events of every enabled provider on one timeline; `before_ms` continues a page.
#[tauri::command]
pub async fn get_quota_events(
    state: State<'_, AppState>,
    include_diagnostic: bool,
    before_ms: Option<i64>,
    limit: Option<u32>,
) -> Result<Vec<EventView>, String> {
    let limit = limit.unwrap_or(50).clamp(1, 200) as usize;
    state.runtime.events(include_diagnostic, before_ms, limit).map_err(err)
}

/// Current Claude status-line integration and, when it is off, a preview of the exact change.
#[tauri::command]
pub async fn get_claude_integration(state: State<'_, AppState>) -> Result<ClaudeIntegrationView, String> {
    state.runtime.claude_integration_view().map_err(err)
}

/// Explicit user action: installs the bridge under `~/.quotadock` and writes the previewed
/// statusLine into Claude Code's settings (backed up first). Takes no path or content argument:
/// only the preview's time and fingerprint, and the backend refuses a plan that differs from it.
#[tauri::command]
pub async fn enable_claude_integration(
    app: AppHandle,
    state: State<'_, AppState>,
    planned_at_ms: i64,
    plan_id: String,
) -> Result<ClaudeIntegrationView, String> {
    let exe = std::env::current_exe().map_err(err)?;
    let bridge = exe.parent().map(|d| d.join(quotadock_core::claude::integration::BRIDGE_EXE)).ok_or("cannot locate QuotaDock's folder")?;
    state.runtime.enable_claude_integration(&bridge, planned_at_ms, &plan_id).map_err(err)?;
    crate::tray::refresh(&app);
    state.runtime.claude_integration_view().map_err(err)
}

/// Restores the previous statusLine (only if it is still QuotaDock's) and removes the bridge.
#[tauri::command]
pub async fn disable_claude_integration(app: AppHandle, state: State<'_, AppState>) -> Result<DisableOutcome, String> {
    let outcome = state.runtime.disable_claude_integration().map_err(err)?;
    crate::tray::refresh(&app);
    Ok(outcome)
}

#[tauri::command]
pub async fn get_provider_status(state: State<'_, AppState>) -> Result<FullProviderStatus, String> {
    Ok(core_cmd::get_provider_status(&state.runtime.core, state.runtime.polling()).await)
}

#[tauri::command]
pub async fn pause_monitoring(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    state.runtime.pause().await;
    crate::tray::refresh(&app);
    crate::window::refresh_ui(&app);
    crate::widget::on(&app, WidgetEvent::Paused);
    Ok(())
}

#[tauri::command]
pub async fn resume_monitoring(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    state.runtime.resume();
    crate::tray::refresh(&app);
    crate::widget::on(&app, WidgetEvent::Resumed);
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsDto {
    pub settings: AppSettings,
    pub effective_language: Lang,
    pub data_folder: String,
}

/// "Launch at startup" shows what Windows will actually do, not only the last choice made here (an
/// uninstall removes the Run value; Task Manager can disable it). Nothing is written.
fn settings_dto(app: &AppHandle, state: &AppState) -> SettingsDto {
    let mut settings = state.runtime.settings();
    if let Some(on) = crate::autostart_enabled(app) {
        settings.launch_at_startup = on;
    }
    SettingsDto { effective_language: settings.effective_language(), settings, data_folder: state.runtime.data_dir().display().to_string() }
}

#[tauri::command]
pub async fn get_settings(app: AppHandle, state: State<'_, AppState>) -> Result<SettingsDto, String> {
    Ok(settings_dto(&app, &state))
}

/// Persists settings and applies their side effects (autostart, theme, tray language, widget).
#[tauri::command]
pub async fn update_settings(app: AppHandle, state: State<'_, AppState>, settings: AppSettings) -> Result<SettingsDto, String> {
    let was_on = crate::autostart_enabled(&app).unwrap_or(state.runtime.settings().launch_at_startup);
    if settings.launch_at_startup != was_on {
        crate::set_autostart(&app, settings.launch_at_startup)?;
    }
    let codex_was_on = state.runtime.settings().codex.enabled;
    // Keeps the stored widget position (owned by the backend), under the settings writer lock.
    let settings = state.runtime.save_settings_from_page(settings).map_err(err)?;
    if settings.codex.enabled != codex_was_on {
        // In the background: turning Codex off waits for a read in flight and the app-server's stop, and the
        // page sends one save at a time. A reply that late would leave later changes only on the page, where
        // a quit or a widget menu change can lose them.
        let (runtime, app) = (state.runtime.clone(), app.clone());
        tauri::async_runtime::spawn(async move {
            runtime.apply_codex_enabled().await;
            // The read in flight has finished by now and has posted its tray model, computed before the
            // switch was stored. Show the stored switch after it. Harmless during a quit: both read
            // local data, and a destroyed widget or an ended event loop only makes them do nothing.
            crate::tray::refresh(&app);
            crate::widget::refresh(&app);
        });
    }
    crate::window::apply_theme(&app, settings.theme);
    crate::tray::refresh(&app);
    crate::widget::on(&app, WidgetEvent::SettingsChanged);
    Ok(settings_dto(&app, &state))
}

// ------------------------------------------------------------------ desktop widget (Phase 3.7)
// These four commands are granted to the widget window only (capabilities/widget.json), and each
// checks its caller too. The widget has no settings, provider, refresh or generic window access.

fn only_widget(caller: &WebviewWindow) -> Result<(), String> {
    if caller.label() == WIDGET_LABEL { Ok(()) } else { Err("not available for this window".into()) }
}

/// The widget's view: the same tray model as the tray icon and dashboard (local data only).
#[tauri::command]
pub async fn get_widget_view(caller: WebviewWindow, state: State<'_, AppState>) -> Result<WidgetView, String> {
    only_widget(&caller)?;
    state.runtime.widget_view().map_err(err)
}

/// Shows and focuses the main window. Never refreshes a provider.
#[tauri::command]
pub async fn open_main_window(app: AppHandle, caller: WebviewWindow) -> Result<(), String> {
    only_widget(&caller)?;
    crate::window::show_main(&app);
    Ok(())
}

/// Starts a system drag of the widget unless it is locked. Returns whether a drag started.
#[tauri::command]
pub async fn widget_start_drag(app: AppHandle, caller: WebviewWindow) -> Result<bool, String> {
    only_widget(&caller)?;
    crate::widget::start_drag(&app)
}

/// The widget's right-click menu (open, lock, always on top, detailed view, hide), built in Rust.
#[tauri::command]
pub async fn widget_context_menu(app: AppHandle, caller: WebviewWindow) -> Result<(), String> {
    only_widget(&caller)?;
    crate::widget::show_context_menu(&app)
}

/// Called by the close-to-tray hint: optionally never show it again, then hide the window.
#[tauri::command]
pub async fn acknowledge_close_hint(app: AppHandle, state: State<'_, AppState>, dont_show_again: bool) -> Result<(), String> {
    if dont_show_again {
        state.runtime.modify_settings(|s| s.close_hint_dismissed = true).map_err(err)?;
    }
    crate::window::hide_main(&app);
    Ok(())
}

/// Opens QuotaDock's own data folder (fixed path) in Explorer. Takes no path argument.
#[tauri::command]
pub async fn open_data_folder(state: State<'_, AppState>) -> Result<(), String> {
    let dir = state.runtime.data_dir();
    std::process::Command::new("explorer.exe").arg(&dir).spawn().map(|_| ()).map_err(err)
}

/// Writes a sanitized diagnostics bundle into QuotaDock's data folder; returns the file path.
#[tauri::command]
pub async fn export_diagnostics(state: State<'_, AppState>) -> Result<String, String> {
    state.runtime.export_diagnostics().await.map(|p| p.display().to_string())
}

/// Stops polling and the app-server, then exits.
#[tauri::command]
pub async fn quit_app(app: AppHandle) -> Result<(), String> {
    crate::quit(&app);
    Ok(())
}

//! QuotaDock desktop shell (Phase 3).
//!
//! The Rust backend owns the codex.exe child, the JSON-RPC transport, SQLite, polling,
//! notifications and the tray. The main page renders data from 18 narrow commands
//! (capabilities/default.json) and has no shell, filesystem, SQL, RPC, notification or tray access.
//! The optional desktop widget (Phase 3.7) is a second, read-only page with 4 widget-only commands
//! (capabilities/widget.json).
//! Claude data comes from the opt-in status-line bridge and the optional local `/usage` fallback.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod notify;
mod tray;
mod widget;
mod window;

use quotadock_core::codex::provider::PollStatus;
use quotadock_core::pipeline::{CoreConfig, LogFn, PollTrigger, QuotaDockCore};
use quotadock_core::polling::{PollingConfig, spawn_polling};
use quotadock_core::product::i18n::strings;
use quotadock_core::product::runtime::ProductRuntime;
use quotadock_core::product::widget::WidgetEvent;
use quotadock_core::platform::data_location::{self, StartupDecision};
use quotadock_core::platform::health;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Manager, RunEvent};
use tauri_plugin_autostart::ManagerExt;

pub struct AppState {
    pub runtime: Arc<ProductRuntime>,
    pub tray: Mutex<Option<tray::TrayHandles>>,
    pub foreground: AtomicBool,
    pub close_hint_shown: AtomicBool,
    /// A Claude step is running (ticker and manual refresh never overlap: at most one CLI query).
    pub claude_busy: AtomicBool,
}

pub fn request_refresh(app: &AppHandle) {
    refresh_claude(app, true);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let _ = state.runtime.refresh().await;
    });
}

/// One Claude step (bridge file; optional active fallback when due or `manual`). Applies the tray,
/// a notification and a UI refresh when something changed. Runs off the main thread.
async fn claude_step(app: &AppHandle, manual: bool) {
    let state = app.state::<AppState>();
    if state.claude_busy.swap(true, Ordering::SeqCst) {
        return;
    }
    let fg = state.foreground.load(Ordering::SeqCst);
    let result = state.runtime.claude_tick(fg, manual).await;
    state.claude_busy.store(false, Ordering::SeqCst);
    match result {
        Ok(Some(after)) => {
            tray::apply(app, &after.tray);
            if let Some((n, log_id)) = after.notification {
                notify::deliver(app, &n, log_id);
            }
            window::refresh_ui(app);
            widget::refresh(app);
        }
        Ok(None) => {}
        Err(e) => eprintln!("[quotadock] claude step failed: {e}"),
    }
}

pub fn refresh_claude(app: &AppHandle, manual: bool) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move { claude_step(&app, manual).await });
}

pub fn toggle_pause(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let event = if state.runtime.is_paused() {
            state.runtime.resume();
            WidgetEvent::Resumed
        } else {
            state.runtime.pause().await;
            WidgetEvent::Paused
        };
        tray::refresh(&app);
        window::refresh_ui(&app);
        widget::on(&app, event);
    });
}

/// The only full exit path: Tray -> Quit, Settings -> Quit, `--quit`. Polling (including a read in
/// flight) and the app-server are stopped off the main thread first, then the event loop exits.
/// RunEvent::Exit repeats the idempotent cleanup for exits that bypass this path.
pub fn quit(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // The widget goes first: it disappears at once instead of lingering while polling stops.
        widget::destroy(&app, "quit");
        let rt = app.state::<AppState>().runtime.clone();
        if let Some(p) = rt.polling() {
            p.stop().await;
        }
        rt.core.shutdown().await;
        rt.record_stop("quit");
        app.exit(0);
    });
}

pub fn set_autostart(app: &AppHandle, enable: bool) -> Result<(), String> {
    let launcher = app.autolaunch();
    if !enable {
        return launcher.disable().map_err(|e| e.to_string());
    }
    launcher.enable().map_err(|e| e.to_string())?;
    // The plugin writes the path without quotes; use the installer's quoted form.
    #[cfg(windows)]
    {
        // Autostart is on either way; a failed rewrite only leaves the plugin's unquoted form.
        if let Ok(exe) = std::env::current_exe() {
            let _ = quotadock_core::product::autostart::write_run_value(&app.package_info().name, &exe);
        }
    }
    Ok(())
}

/// Whether Windows will start QuotaDock at logon: the HKCU Run value exists and Task Manager has not
/// disabled it. Read-only. The stored setting alone can be stale: an uninstall removes the value.
pub fn autostart_enabled(app: &AppHandle) -> Option<bool> {
    app.autolaunch().is_enabled().ok()
}

/// Arguments understood by a running instance (forwarded by the single-instance plugin).
fn handle_args(app: &AppHandle, args: &[String], first_launch: bool) {
    let has = |a: &str| args.iter().any(|x| x == a);
    if has("--quit") {
        quit(app);
        return;
    }
    if has("--pause") || has("--resume") {
        let state = app.state::<AppState>();
        let want_pause = has("--pause");
        if want_pause != state.runtime.is_paused() {
            toggle_pause(app);
        }
    }
    if has("--refresh") {
        request_refresh(app);
    }
    if has("--test-notification") {
        let s = strings(app.state::<AppState>().runtime.lang());
        if let Err(e) = notify::show_plain(app, s.n_test_title, s.n_test_body) {
            eprintln!("[quotadock] test notification failed: {e}");
        }
    }
    // A plain launch (or a plain second launch) shows the window; scripted/autostart launches don't.
    let background_only =
        has("--autostart") || has("--hidden") || has("--pause") || has("--resume") || has("--refresh") || has("--test-notification");
    if !background_only {
        window::show_main(app);
    } else if first_launch {
        window::sync_foreground(app);
    }
}

fn main() {
    // No SQLite database may be opened before the data location is validated: inside another app's
    // MSIX package (e.g. a Claude Desktop shell) %LOCALAPPDATA% is virtualized and a second database
    // would appear. Packaged -> relaunch once through the Explorer shell; still packaged -> fail closed.
    let raw_args: Vec<String> = std::env::args().skip(1).collect();
    let (data_dir, relaunched) = match data_location::decide_current(&raw_args) {
        StartupDecision::Proceed { data_dir, relaunched } => (data_dir, relaunched),
        StartupDecision::Relaunch { args } => {
            let relaunch = std::env::current_exe().map_err(|e| e.to_string()).and_then(|exe| data_location::relaunch_via_shell(&exe, &args));
            if let Err(e) = relaunch {
                eprintln!("[quotadock] relaunch outside the package failed: {e}");
                data_location::show_fail_closed_message();
            }
            return;
        }
        StartupDecision::FailClosed(e) => {
            eprintln!("[quotadock] refusing to open the data store: {e}");
            data_location::show_fail_closed_message();
            return;
        }
    };
    let args = data_location::strip_marker(&raw_args);
    let launch = if args.iter().any(|a| a == "--autostart") { "autostart" } else if args.iter().any(|a| a == "--hidden") { "hidden" } else { "normal" };
    let log: LogFn = Arc::new(|line: &str| eprintln!("[quotadock] {line}"));
    let app = tauri::Builder::default()
        // Must be first: a second launch forwards its arguments here and exits.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            // A second launch while the first is still starting: nothing to forward to yet.
            if app.try_state::<AppState>().is_none() {
                return;
            }
            let args: Vec<String> = argv.into_iter().skip(1).collect();
            handle_args(app, &args, false);
        }))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, Some(vec!["--autostart"])))
        .setup(move |app| {
            let db = data_dir.join(data_location::DATABASE_FILE);
            let core = Arc::new(QuotaDockCore::open(CoreConfig::with_database(db), log.clone())?);
            let runtime = ProductRuntime::new(core)?;
            runtime.record_start(launch, relaunched);
            let power_watch = health::watch_power_events(runtime.health().clone());
            let session_watch = health::watch_session_end(runtime.health().clone());
            runtime.health().record("power_watch", serde_json::json!({ "registered": power_watch, "sessionEnd": session_watch }));
            app.manage(AppState {
                runtime: runtime.clone(),
                tray: Mutex::new(None),
                foreground: AtomicBool::new(false),
                close_hint_shown: AtomicBool::new(false),
                claude_busy: AtomicBool::new(false),
            });
            let handles = tray::build_tray(app.handle())?;
            *app.state::<AppState>().tray.lock().unwrap() = Some(handles);
            window::apply_theme(app.handle(), runtime.settings().theme);

            // Polling: every completed read updates notifications, tray and the page. Codex turned off
            // in Settings: no read (and so no app-server) until it is turned on.
            let codex_on = runtime.settings().codex.enabled;
            let handle = app.handle().clone();
            let rt = runtime.clone();
            let polling = tauri::async_runtime::block_on(async move {
                spawn_polling(
                    Arc::new(move |trigger: PollTrigger| {
                        let rt = rt.clone();
                        let handle = handle.clone();
                        Box::pin(async move {
                            let outcome = rt.core.poll_once(trigger).await;
                            match rt.after_poll(&outcome) {
                                Ok(after) => {
                                    tray::apply(&handle, &after.tray);
                                    if let Some((n, log_id)) = after.notification {
                                        notify::deliver(&handle, &n, log_id);
                                    }
                                }
                                Err(e) => eprintln!("[quotadock] post-poll evaluation failed: {e}"),
                            }
                            window::refresh_ui(&handle);
                            widget::refresh(&handle);
                            outcome.status == PollStatus::Success
                        })
                    }),
                    PollingConfig { start_paused: !codex_on, ..Default::default() },
                )
            });
            runtime.attach_polling(polling);

            // Claude: the bridge file every 15 s (a small local read); the optional active /usage
            // fallback only when the user enabled it and passive data is stale (5 / 15 min).
            let claude_app = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    claude_step(&claude_app, false).await;
                    tokio::time::sleep(Duration::from_secs(15)).await;
                }
            });

            // Keep "Updated N min ago" / staleness in the tooltip current (local data only), and run the
            // soak watchdog (MONITORING_GAP, stuck database, resource samples; no database lock).
            let ticker = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    let state = ticker.state::<AppState>();
                    state.runtime.watchdog_tick(state.foreground.load(Ordering::SeqCst));
                    tray::refresh(&ticker);
                }
            });

            handle_args(app.handle(), &args, true);
            // After monitoring started: the desktop widget, if the user turned it on (off by default).
            widget::on(app.handle(), WidgetEvent::Startup);
            Ok(())
        })
        .on_window_event(window::on_window_event)
        .invoke_handler(tauri::generate_handler![
            commands::get_dashboard,
            commands::get_current_usage,
            commands::get_usage_forecast,
            commands::refresh_usage,
            commands::get_usage_history,
            commands::get_quota_events,
            commands::get_provider_status,
            commands::pause_monitoring,
            commands::resume_monitoring,
            commands::get_settings,
            commands::update_settings,
            commands::acknowledge_close_hint,
            commands::open_data_folder,
            commands::export_diagnostics,
            commands::quit_app,
            commands::get_claude_integration,
            commands::enable_claude_integration,
            commands::disable_claude_integration,
            commands::get_widget_view,
            commands::open_main_window,
            commands::widget_start_drag,
            commands::widget_context_menu
        ])
        .build(tauri::generate_context!())
        .expect("error while building QuotaDock");

    app.run(|handle, event| {
        if let RunEvent::Exit = event
            && let Some(state) = handle.try_state::<AppState>()
        {
            let rt = state.runtime.clone();
            tauri::async_runtime::block_on(async move {
                if let Some(p) = rt.polling() {
                    p.stop().await;
                }
                rt.core.shutdown().await;
            });
            // After quit() this adds nothing; an exit that bypassed quit() still leaves its line.
            state.runtime.record_stop("exit");
        }
    });
}

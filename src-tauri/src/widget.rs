//! Desktop widget window (Phase 3.7): creation, placement, drag/lock, position memory, the
//! right-click menu and refresh nudges. The rules (lifecycle, window spec, placement, what counts
//! as foreground) live in `quotadock_core::product::widget`; this file only executes them.
//!
//! The widget is presentation only: it never polls, never refreshes a provider, and its
//! visibility never changes the monitoring cadence (that follows the main window only).

use crate::AppState;
use quotadock_core::product::i18n::widget_menu;
use quotadock_core::product::settings::{AppSettings, WidgetMode};
use quotadock_core::product::widget::{self as rules, MonitorGeom, WIDGET_LABEL, WidgetAction, WidgetEvent, WidgetRows};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::{AppHandle, LogicalSize, Manager, PhysicalPosition, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};

const MENU_OPEN: &str = "widget_open";
const MENU_LOCK: &str = "widget_lock";
const MENU_ON_TOP: &str = "widget_on_top";
const MENU_DETAILED: &str = "widget_detailed";
const MENU_HIDE: &str = "widget_hide";

/// After the last `Moved` event of a drag, wait this long before saving the position (one write
/// per drag, not one per mouse move).
const SAVE_DEBOUNCE: Duration = Duration::from_millis(700);
static MOVE_GENERATION: AtomicU64 = AtomicU64::new(0);

/// One `update` at a time. Each reads the window's geometry, decides, then posts its calls; two at once
/// can decide from the same old geometry and leave a wrong place, which the Moved event then saves.
static UPDATE_TURN: Mutex<()> = Mutex::new(());

pub fn widget_window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(WIDGET_LABEL)
}

pub fn is_visible(app: &AppHandle) -> bool {
    widget_window(app).is_some_and(|w| w.is_visible().unwrap_or(false))
}

pub fn is_focused(app: &AppHandle) -> bool {
    widget_window(app).is_some_and(|w| w.is_focused().unwrap_or(false))
}

/// Widget lifecycle in the health log: the evidence for the targeted widget soak (window count,
/// time shown, restored or recovered placement, always-on-top, exit cleanup).
fn record(app: &AppHandle, event: &str, mut fields: serde_json::Value) {
    let Some(state) = app.try_state::<AppState>() else { return };
    if let Some(obj) = fields.as_object_mut() {
        obj.insert("event".into(), event.into());
        obj.insert("widget".into(), usize::from(widget_window(app).is_some()).into());
        obj.insert("webviewWindows".into(), app.webview_windows().len().into());
    }
    state.runtime.health().record("widget", fields);
}

fn monitors(app: &AppHandle) -> Vec<MonitorGeom> {
    let primary = app.primary_monitor().ok().flatten().and_then(|m| m.name().cloned());
    app.available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|m| {
            let wa = m.work_area();
            MonitorGeom {
                name: m.name().cloned(),
                primary: primary.is_some() && m.name() == primary.as_ref(),
                scale: m.scale_factor(),
                work_x: wa.position.x,
                work_y: wa.position.y,
                work_w: wa.size.width,
                work_h: wa.size.height,
            }
        })
        .collect()
}

/// Applies a lifecycle event. Runs off the main thread: creating a window from a main-thread
/// event handler can deadlock on Windows.
pub fn on(app: &AppHandle, event: WidgetEvent) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move { handle(&app, event) });
}

fn handle(app: &AppHandle, event: WidgetEvent) {
    let Some(state) = app.try_state::<AppState>() else { return };
    let settings = state.runtime.settings();
    let decision = rules::decide(event, settings.desktop_widget.enabled, widget_window(app).is_some());
    if decision.disable_setting && settings.desktop_widget.enabled {
        if let Err(e) = state.runtime.modify_settings(|s| s.desktop_widget.enabled = false) {
            eprintln!("[quotadock] could not turn the widget off: {e}");
        }
        crate::window::reload_settings(app);
    }
    match decision.action {
        WidgetAction::Create => create(app, &state.runtime.settings()),
        WidgetAction::Update => update(app),
        WidgetAction::Destroy => destroy(
            app,
            match event {
                WidgetEvent::WidgetClosed => "closed",
                WidgetEvent::Quit => "quit",
                _ => "disabled",
            },
        ),
        WidgetAction::Refresh => refresh(app),
        WidgetAction::Keep => {}
    }
}

fn create(app: &AppHandle, s: &AppSettings) {
    if widget_window(app).is_some() {
        record(app, "create_skipped", serde_json::json!({})); // labels are unique anyway; never a second widget
        return;
    }
    let w = &s.desktop_widget;
    let spec = rules::window_spec(w, WidgetRows::of(s));
    let mons = monitors(app);
    let placement = rules::place(if w.remember_position { w.position.as_ref() } else { None }, &mons, spec.logical_size);
    let mut builder = WebviewWindowBuilder::new(app, spec.label, WebviewUrl::App(spec.url.into()))
        .title(spec.title)
        .inner_size(spec.logical_size.0, spec.logical_size.1)
        .decorations(spec.decorations)
        .transparent(spec.transparent)
        .shadow(spec.shadow)
        .skip_taskbar(spec.skip_taskbar)
        .resizable(spec.resizable)
        .maximizable(spec.maximizable)
        .minimizable(spec.minimizable)
        .always_on_top(spec.always_on_top)
        // Created visible but not focused (SW_SHOWNOACTIVATE): never takes focus from the user's work.
        .focused(spec.focused)
        .visible(true)
        .on_menu_event(|window, event| on_menu(window.app_handle(), event.id().as_ref()));
    if let Some(p) = placement {
        // Logical in the target monitor's own scale: Windows picks that monitor and its DPI.
        let m = &mons[p.monitor_index];
        builder = builder.position(p.x as f64 / m.scale, p.y as f64 / m.scale);
    }
    match builder.build() {
        Ok(win) => {
            if let Some(p) = placement {
                let _ = win.set_position(PhysicalPosition::new(p.x, p.y));
            }
            record(
                app,
                "created",
                serde_json::json!({
                    "mode": w.mode, "alwaysOnTop": w.always_on_top, "opacity": w.opacity_percent(), "locked": w.locked,
                    "restored": w.remember_position && w.position.is_some(), "recovered": placement.is_some_and(|p| p.recovered),
                    "monitors": mons.len(),
                }),
            );
        }
        Err(e) => {
            eprintln!("[quotadock] could not create the desktop widget: {e}");
            record(app, "create_failed", serde_json::json!({ "error": e.to_string() }));
        }
    }
}

/// Must never run on the main thread: the holder of `UPDATE_TURN` waits for that thread in its getters,
/// so a main-thread caller waiting for the turn would hang both. `on` and `on_menu` spawn. The next
/// holder's getters queue behind the calls the last one posted, so it sees the geometry they left.
fn update(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else { return };
    // It guards no data, so a panic in an earlier holder leaves nothing to repair.
    let turn = UPDATE_TURN.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(win) = widget_window(app) else { return };
    // Read inside the turn, not before it, so the newest settings land last.
    let s = state.runtime.settings();
    let spec = rules::window_spec(&s.desktop_widget, WidgetRows::of(&s));
    let _ = win.set_always_on_top(spec.always_on_top);
    // Read before resizing: where the bottom edge was decides where the new height goes. A locked
    // widget follows too: lock only stops dragging, and it could not be dragged back from a stranded
    // place. The Moved event saves the move only with remember position on, like any other move.
    let shift = match (win.outer_position(), win.outer_size()) {
        (Ok(pos), Ok(size)) => rules::resized_position(pos.x, pos.y, (size.width, size.height), spec.logical_size, &monitors(app)),
        _ => None,
    };
    let move_window = |m: rules::ResizeMove| {
        let _ = win.set_position(PhysicalPosition::new(m.x, m.y));
    };
    if let Some(m) = shift.filter(|m| m.move_first) {
        move_window(m);
    }
    let _ = win.set_size(LogicalSize::new(spec.logical_size.0, spec.logical_size.1));
    if let Some(m) = shift.filter(|m| !m.move_first) {
        move_window(m);
    }
    drop(turn);
    let w = &s.desktop_widget;
    record(app, "updated", serde_json::json!({ "mode": w.mode, "alwaysOnTop": w.always_on_top, "opacity": w.opacity_percent(), "locked": w.locked }));
    refresh(app);
}

/// `destroy`, not `close`: closing would raise CloseRequested, which means "the user closed it".
/// `reason`: "disabled", "closed" (Alt+F4 on the widget) or "quit".
pub fn destroy(app: &AppHandle, reason: &str) {
    if let Some(win) = widget_window(app) {
        let result = win.destroy();
        record(app, "destroyed", serde_json::json!({ "reason": reason, "ok": result.is_ok() }));
    }
}

/// Asks the widget page to re-read its view (a local read, no backend request).
pub fn refresh(app: &AppHandle) {
    if let Some(win) = widget_window(app) {
        let _ = win.eval("window.__quotadockWidget && window.__quotadockWidget.refresh()");
    }
}

pub fn on_window_event(window: &tauri::Window, event: &WindowEvent) {
    let app = window.app_handle();
    match event {
        WindowEvent::CloseRequested { api, .. } => {
            // Alt+F4 on the widget: turn the widget off; QuotaDock and monitoring keep running.
            api.prevent_close();
            on(app, WidgetEvent::WidgetClosed);
        }
        WindowEvent::Moved(_) => schedule_position_save(app),
        _ => {}
    }
}

fn schedule_position_save(app: &AppHandle) {
    let generation = MOVE_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(SAVE_DEBOUNCE).await;
        if MOVE_GENERATION.load(Ordering::SeqCst) == generation {
            save_position(&app);
        }
    });
}

fn save_position(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else { return };
    let Some(win) = widget_window(app) else { return };
    let s = state.runtime.settings();
    if !s.desktop_widget.remember_position {
        return;
    }
    let (Ok(pos), Ok(size)) = (win.outer_position(), win.outer_size()) else { return };
    let Some(p) = rules::save_position(pos.x, pos.y, (size.width, size.height), &monitors(app)) else { return };
    if s.desktop_widget.position.as_ref() != Some(&p)
        && let Err(e) = state.runtime.modify_settings(|s| {
            if s.desktop_widget.remember_position {
                s.desktop_widget.position = Some(p);
            }
        })
    {
        eprintln!("[quotadock] could not save the widget position: {e}");
    }
}

/// Starts a system drag unless the widget is locked (enforced here, not in the page).
pub fn start_drag(app: &AppHandle) -> Result<bool, String> {
    let Some(state) = app.try_state::<AppState>() else { return Ok(false) };
    if state.runtime.settings().desktop_widget.locked {
        return Ok(false);
    }
    let Some(win) = widget_window(app) else { return Ok(false) };
    win.start_dragging().map_err(|e| e.to_string())?;
    Ok(true)
}

pub fn show_context_menu(app: &AppHandle) -> Result<(), String> {
    let Some(state) = app.try_state::<AppState>() else { return Ok(()) };
    let Some(win) = widget_window(app) else { return Ok(()) };
    let s = state.runtime.settings();
    let t = widget_menu(state.runtime.lang());
    let w = &s.desktop_widget;
    let e = |r: tauri::Result<()>| r.map_err(|e| e.to_string());
    let open = MenuItem::with_id(app, MENU_OPEN, t.open, true, None::<&str>).map_err(|e| e.to_string())?;
    let lock = CheckMenuItem::with_id(app, MENU_LOCK, t.lock, true, w.locked, None::<&str>).map_err(|e| e.to_string())?;
    let on_top = CheckMenuItem::with_id(app, MENU_ON_TOP, t.on_top, true, w.always_on_top, None::<&str>).map_err(|e| e.to_string())?;
    let detailed =
        CheckMenuItem::with_id(app, MENU_DETAILED, t.detailed, true, w.mode == WidgetMode::Detailed, None::<&str>).map_err(|e| e.to_string())?;
    let hide = MenuItem::with_id(app, MENU_HIDE, t.hide, true, None::<&str>).map_err(|e| e.to_string())?;
    let sep1 = PredefinedMenuItem::separator(app).map_err(|e| e.to_string())?;
    let sep2 = PredefinedMenuItem::separator(app).map_err(|e| e.to_string())?;
    let menu = Menu::with_items(app, &[&open, &sep1, &lock, &on_top, &detailed, &sep2, &hide]).map_err(|e| e.to_string())?;
    e(win.popup_menu(&menu))
}

/// Every menu event of the app reaches this listener (and the tray's); only the widget's ids act.
fn on_menu(app: &AppHandle, id: &str) {
    match id {
        MENU_OPEN => crate::window::show_main(app),
        MENU_LOCK | MENU_ON_TOP | MENU_DETAILED | MENU_HIDE => {
            // Off the main thread: saving the settings takes the database lock.
            let (app, id) = (app.clone(), id.to_string());
            tauri::async_runtime::spawn(async move { apply_menu_choice(&app, &id) });
        }
        _ => {}
    }
}

fn apply_menu_choice(app: &AppHandle, id: &str) {
    let Some(state) = app.try_state::<AppState>() else { return };
    let saved = state.runtime.modify_settings(|s| {
        let w = &mut s.desktop_widget;
        match id {
            MENU_LOCK => w.locked = !w.locked,
            MENU_ON_TOP => w.always_on_top = !w.always_on_top,
            MENU_DETAILED => w.mode = if w.mode == WidgetMode::Detailed { WidgetMode::Compact } else { WidgetMode::Detailed },
            MENU_HIDE => w.enabled = false,
            _ => {}
        }
    });
    if let Err(e) = saved {
        eprintln!("[quotadock] widget menu: {e}");
        return;
    }
    crate::window::reload_settings(app);
    handle(app, WidgetEvent::SettingsChanged);
}

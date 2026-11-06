//! Main window behaviour: close-to-tray, show/focus (tray click, second instance), foreground
//! detection for 60 s polling, and nudging the page to re-read local data.

use crate::AppState;
use quotadock_core::product::settings::ThemePref;
use quotadock_core::product::widget::{SurfaceStates, WIDGET_LABEL, provider_foreground};
use std::sync::atomic::Ordering;
use tauri::{AppHandle, Manager, Theme, WebviewWindow, WindowEvent};

pub const MAIN: &str = "main";

pub fn main_window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(MAIN)
}

pub fn show_main(app: &AppHandle) {
    if let Some(w) = main_window(app) {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
    sync_foreground(app);
    refresh_ui(app);
}

pub fn hide_main(app: &AppHandle) {
    if let Some(w) = main_window(app) {
        // A close hint still open would otherwise reappear (and hide the window on OK) next time.
        let _ = w.eval("window.__quotadock && window.__quotadock.hideCloseHint && window.__quotadock.hideCloseHint()");
        let _ = w.hide();
    }
    sync_foreground(app);
}

/// Foreground = the main window is shown and not minimized (60 s polling); otherwise 5 min.
/// The desktop widget never counts, even visible and focused (`provider_foreground`, Phase 3.7).
/// The page is told too: WebView2 keeps running while the window sits in the tray, so the page
/// stops its clock and local re-reads until it is shown again.
pub fn sync_foreground(app: &AppHandle) {
    // Tauri builds the config window (pumping messages) before our setup hook manages AppState.
    let Some(state) = app.try_state::<AppState>() else { return };
    let window = main_window(app);
    let surfaces = SurfaceStates {
        main_visible: window.as_ref().is_some_and(|w| w.is_visible().unwrap_or(false)),
        main_minimized: window.as_ref().is_some_and(|w| w.is_minimized().unwrap_or(false)),
        widget_visible: crate::widget::is_visible(app),
        widget_focused: crate::widget::is_focused(app),
    };
    let fg = provider_foreground(&surfaces);
    if state.foreground.swap(fg, Ordering::SeqCst) != fg
        && let Some(p) = state.runtime.polling()
    {
        p.set_foreground(fg);
    }
    state.runtime.note_foreground(fg);
    if let Some(w) = window {
        let _ = w.eval(if fg {
            "window.__quotadock && window.__quotadock.setActive && window.__quotadock.setActive(true)"
        } else {
            "window.__quotadock && window.__quotadock.setActive && window.__quotadock.setActive(false)"
        });
    }
}

/// Asks the page to re-read local data now (no backend request). Uses a fixed hook defined by
/// our own page, so the webview needs no event permissions.
pub fn refresh_ui(app: &AppHandle) {
    if let Some(w) = main_window(app) {
        let _ = w.eval("window.__quotadock && window.__quotadock.refresh()");
    }
}

/// Settings changed outside the page (the widget's own menu): the page re-reads them.
pub fn reload_settings(app: &AppHandle) {
    if let Some(w) = main_window(app) {
        let _ = w.eval("window.__quotadock && window.__quotadock.reloadSettings && window.__quotadock.reloadSettings()");
    }
}

pub fn apply_theme(app: &AppHandle, theme: ThemePref) {
    if let Some(w) = main_window(app) {
        let _ = w.set_theme(match theme {
            ThemePref::System => None,
            ThemePref::Light => Some(Theme::Light),
            ThemePref::Dark => Some(Theme::Dark),
        });
    }
}

pub fn on_window_event(window: &tauri::Window, event: &WindowEvent) {
    if window.label() == WIDGET_LABEL {
        crate::widget::on_window_event(window, event);
        return;
    }
    if window.label() != MAIN {
        return;
    }
    let app = window.app_handle();
    match event {
        WindowEvent::CloseRequested { api, .. } => {
            // A close before setup finished (e.g. a scripted WM_CLOSE or a session shutdown while
            // the hidden window is still being created) used to panic on the missing state.
            let Some(state) = app.try_state::<AppState>() else {
                api.prevent_close();
                return;
            };
            let settings = state.runtime.settings();
            if !settings.close_to_tray {
                // A real close quits QuotaDock. Explicitly: an open desktop widget would otherwise
                // keep the process alive without its main window.
                api.prevent_close();
                crate::quit(app);
                return;
            }
            api.prevent_close();
            let first_time = !settings.close_hint_dismissed && !state.close_hint_shown.swap(true, Ordering::SeqCst);
            if first_time {
                // The page shows "QuotaDock will continue running in the system tray" with
                // "Don't show again" and then calls acknowledge_close_hint, which hides the window.
                if let Some(w) = main_window(app)
                    && w.eval("window.__quotadock && window.__quotadock.showCloseHint()").is_ok()
                {
                    return;
                }
            }
            hide_main(app);
        }
        WindowEvent::Focused(_) | WindowEvent::Resized(_) => sync_foreground(app),
        _ => {}
    }
}

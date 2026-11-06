//! System tray: two bars for the quotas while monitoring works (paused and error have their own
//! icons), localized tooltip and menu, all from the Rust `TrayStatusModel`. No webview tray access.

use crate::AppState;
use quotadock_core::product::i18n::strings;
use quotadock_core::product::tray::{MenuRow, TrayIconKind, TrayStatusModel, menu_line, tooltip_text};
use quotadock_core::product::tray_icon;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

pub struct TrayHandles {
    pub icon: TrayIcon<Wry>,
    pub open: MenuItem<Wry>,
    pub refresh: MenuItem<Wry>,
    pub codex: MenuItem<Wry>,
    pub claude: MenuItem<Wry>,
    pub pause: MenuItem<Wry>,
    pub quit: MenuItem<Wry>,
}

pub fn icon_image(kind: TrayIconKind) -> Image<'static> {
    let bytes: &'static [u8] = match kind {
        TrayIconKind::Normal => include_bytes!("../icons/tray/normal.png"),
        TrayIconKind::Warning => include_bytes!("../icons/tray/warning.png"),
        TrayIconKind::Critical => include_bytes!("../icons/tray/critical.png"),
        TrayIconKind::Error => include_bytes!("../icons/tray/error.png"),
        TrayIconKind::Paused => include_bytes!("../icons/tray/paused.png"),
    };
    Image::from_bytes(bytes).expect("embedded tray icon is a valid PNG")
}

pub fn build_tray(app: &AppHandle) -> tauri::Result<TrayHandles> {
    let state = app.state::<AppState>();
    let lang = state.runtime.lang();
    let s = strings(lang);
    // The provider rows are right from the first open: the next apply can be a minute away. A failed
    // read leaves only the names until then.
    let first = state.runtime.tray_status().ok();
    let row = |provider: &str, name: &str| match &first {
        Some(model) => menu_line(model, provider, lang),
        None => MenuRow { text: name.to_string(), enabled: true },
    };
    let (codex_row, claude_row) = (row("codex", s.codex), row("claude", "Claude"));
    let open = MenuItem::with_id(app, "open", s.menu_open, true, None::<&str>)?;
    let refresh = MenuItem::with_id(app, "refresh", s.menu_refresh, true, None::<&str>)?;
    let codex = MenuItem::with_id(app, "codex", codex_row.text, codex_row.enabled, None::<&str>)?;
    let claude = MenuItem::with_id(app, "claude", claude_row.text, claude_row.enabled, None::<&str>)?;
    let pause = MenuItem::with_id(app, "pause", s.menu_pause, true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", s.menu_quit, true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &open,
            &refresh,
            &PredefinedMenuItem::separator(app)?,
            &codex,
            &claude,
            &PredefinedMenuItem::separator(app)?,
            &pause,
            &quit,
        ],
    )?;
    let icon = TrayIconBuilder::with_id("quotadock")
        .icon(icon_image(TrayIconKind::Normal))
        .tooltip(s.app)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" | "codex" | "claude" => crate::window::show_main(app),
            "refresh" => crate::request_refresh(app),
            "pause" => crate::toggle_pause(app),
            "quit" => crate::quit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                crate::window::show_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(TrayHandles { icon, open, refresh, codex, claude, pause, quit })
}

/// Applies a model computed by the core. Cheap; called after polls, settings changes and every minute.
///
/// Tray and menu setters called off the main thread block until the main thread runs them, and
/// RunEvent::Exit blocks the main thread while it stops polling. If the poll task waited here at
/// that moment, neither side could proceed, so the update is posted to the main thread and this
/// returns immediately.
pub fn apply(app: &AppHandle, model: &TrayStatusModel) {
    let lang = app.state::<AppState>().runtime.settings().effective_language();
    let s = strings(lang);
    // While monitoring works the icon shows the quotas (two bars); paused and error keep theirs.
    let icon = match tray_icon::bars_for(model) {
        Some(bars) => Image::new_owned(tray_icon::render(bars), tray_icon::SIZE as u32, tray_icon::SIZE as u32),
        None => icon_image(model.icon),
    };
    let tooltip = tooltip_text(model);
    let paused = model.paused;
    let (codex, claude) = (menu_line(model, "codex", lang), menu_line(model, "claude", lang));
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let state = handle.state::<AppState>();
        let mut guard = state.tray.lock().unwrap();
        let Some(t) = guard.as_mut() else { return };
        let _ = t.icon.set_icon(Some(icon));
        let _ = t.icon.set_tooltip(Some(tooltip));
        let _ = t.open.set_text(s.menu_open);
        let _ = t.refresh.set_text(s.menu_refresh);
        let _ = t.refresh.set_enabled(!paused);
        let _ = t.codex.set_text(&codex.text);
        let _ = t.codex.set_enabled(codex.enabled);
        let _ = t.claude.set_text(&claude.text);
        let _ = t.claude.set_enabled(claude.enabled);
        let _ = t.pause.set_text(if paused { s.menu_resume } else { s.menu_pause });
        let _ = t.quit.set_text(s.menu_quit);
    });
}

pub fn refresh(app: &AppHandle) {
    let state = app.state::<AppState>();
    if let Ok(model) = state.runtime.tray_status() {
        apply(app, &model);
    }
}

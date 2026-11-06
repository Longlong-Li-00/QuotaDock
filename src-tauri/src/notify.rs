//! Windows notifications, sent only from Rust. The webview has no notification permission.
//!
//! Identity: the plugin uses the PowerShell identity only when the exe runs from `target\debug` or
//! `target\release`; elsewhere it uses `com.quotadock.app`. The installer registers that ID through
//! the Start Menu shortcut (System.AppUserModel.ID), so toasts show "QuotaDock"; a copy run from
//! another folder without that shortcut has its toasts silently dropped by Windows.
//! The plugin submits the toast on a background task and discards the OS result, so
//! `notification_log.delivered = 1` means "submitted to Windows", not "seen on screen".

use crate::AppState;
use quotadock_core::product::notifications::Notification;
use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;

pub fn deliver(app: &AppHandle, n: &Notification, log_id: i64) {
    let state = app.state::<AppState>();
    let result = app.notification().builder().title(&n.title).body(&n.body).show();
    match result {
        Ok(()) => state.runtime.mark_notification_delivery(log_id, true, None),
        Err(e) => {
            let msg = quotadock_core::codex::sanitize::sanitize_string(&e.to_string());
            eprintln!("[quotadock] notification failed: {msg}");
            state.runtime.mark_notification_delivery(log_id, false, Some(&msg));
        }
    }
}

pub fn show_plain(app: &AppHandle, title: &str, body: &str) -> Result<(), String> {
    app.notification().builder().title(title).body(body).show().map_err(|e| e.to_string())
}

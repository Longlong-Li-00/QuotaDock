// Generates one permission per app command (allow-<command>), so the capability file can grant
// exactly these commands and nothing else.
fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(&[
        "get_dashboard",
        "get_current_usage",
        "get_usage_forecast",
        "refresh_usage",
        "get_usage_history",
        "get_quota_events",
        "get_provider_status",
        "pause_monitoring",
        "resume_monitoring",
        "get_settings",
        "update_settings",
        "acknowledge_close_hint",
        "open_data_folder",
        "export_diagnostics",
        "quit_app",
        "get_claude_integration",
        "enable_claude_integration",
        "disable_claude_integration",
        "get_widget_view",
        "open_main_window",
        "widget_start_drag",
        "widget_context_menu",
    ])))
    .expect("failed to run tauri-build");
}

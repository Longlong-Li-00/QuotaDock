//! Phase 3 product layer: settings, tray status, notification rules, history series, dashboard.
//!
//! Everything here is derived from the Rust core and SQLite. The webview only renders what these
//! modules return; it never recomputes availability, schedules, cycles, forecasts or severities.

pub mod autostart;
pub mod claude_auth;
pub mod claude_view;
pub mod dashboard;
pub mod facts;
pub mod history;
pub mod i18n;
pub mod notifications;
pub mod runtime;
pub mod settings;
pub mod tray;
pub mod tray_icon;
pub mod widget;

/// The only provider in V1. Product-layer types carry a provider id so Claude can be added later.
pub const PROVIDER_CODEX: &str = "codex";

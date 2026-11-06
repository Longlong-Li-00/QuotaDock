//! Polling: 5 min in the background, 60 s while a UI is in the foreground, coalesced manual
//! refresh, and 1/2/5/10/15 min backoff after failures. Analytics never trigger extra reads.

pub mod controller;

pub use controller::{PollingConfig, PollingHandle, PollingStatus, RefreshDecision, spawn_polling, validate_interval};

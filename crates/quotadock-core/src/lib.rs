//! QuotaDock production core (Phase 2; Claude provider added in Phase 3.5).
//!
//! Read-only, local-first Codex quota monitoring:
//!
//! ```text
//! codex      persistent `codex app-server --listen stdio://` child, read-only JSON-RPC,
//!            parser + normalizer (ported from the Phase 1B TypeScript reference)
//! storage    QuotaDock's own SQLite (raw snapshots, poll runs, derived state)
//! analytics  observation state machine (reset detector), observed cycles, burn rate, forecast
//! polling    5 min background / 60 s foreground, coalesced manual refresh, backoff
//! pipeline   Provider -> Validate -> Normalize -> Persist -> Engine -> Persist derived
//! commands   the narrow API a UI may call (no shell, no SQL, no raw RPC)
//! ```

pub mod analytics;
pub mod claude;
pub mod codex;
pub mod commands;
pub mod pipeline;
pub mod platform;
pub mod polling;
pub mod product;
pub mod storage;
pub mod time;

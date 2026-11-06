//! QuotaDock's own SQLite database (never `~/.codex/*.sqlite`).
//!
//! Write discipline:
//! 1. `poll_runs` + raw `snapshots` are committed first, on their own (raw evidence is safe even
//!    if analysis fails);
//! 2. everything the engine derives (events, cycles, candidates, qualities, engine state) is
//!    committed atomically in a second transaction, together with `analysis_status = 'done'`.
//!
//! Raw snapshots are never rewritten except for the derived quality/analysis columns, so history
//! can be re-classified by a future classifier version.

pub mod database;
pub mod derived;
pub mod diagnostics;
pub mod migrations;
pub mod snapshots;

pub use database::open_database;

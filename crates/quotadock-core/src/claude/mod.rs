//! Claude provider (Phase 3.5). Read-only, credential-free, no model calls:
//!
//! ```text
//! statusline   Claude Code -> ~/.quotadock/bin/quotadock-claude-bridge.exe -> ~/.quotadock/claude/statusline.json
//!              (primary, passive; opt-in settings.json change with backup/restore)
//! usage        `claude -p /usage` local slash command (optional active fallback, off by default)
//!              + `claude auth status --json` for the billing/auth mode
//! tracker      conservative window instances (no reset classification yet)
//! store        provider-neutral tables (migration v5)
//! monitor      cadence, backoff, freshness and UI state
//! ```
//!
//! QuotaDock never reads `~/.claude/.credentials.json`, never calls Anthropic endpoints itself and
//! never sends a prompt.

pub mod cli;
pub mod env;
pub mod integration;
pub mod model;
pub mod monitor;
pub mod statusline;
pub mod store;
pub mod tracker;
pub mod usage;

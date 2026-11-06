//! Observation state machine, observed cycles, burn rate and forecast.
//!
//! This is an *observation* state machine, not a model of the OpenAI server: every transition is
//! inferred from successive snapshots, carries its evidence, and is versioned by
//! `CLASSIFIER_VERSION` so history can be re-analysed when the rules change.

pub mod burn_rate;
pub mod config;
pub mod forecast;
pub mod model;
pub mod reset_detector;
pub mod transitions;

pub use config::{AnalyticsConfig, EngineConfig};
pub use model::*;
pub use reset_detector::{Engine, EngineState, StepOutput};

/// Version tag written on every derived row (events, cycles, candidates, engine state).
pub const CLASSIFIER_VERSION: &str = "reset-detector-v3";

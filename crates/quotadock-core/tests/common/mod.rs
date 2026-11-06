#![allow(dead_code)]
//! Synthetic timeline builder for engine tests (no I/O, no clock).

use quotadock_core::analytics::reset_detector::{Engine, EngineState, StepOutput};
use quotadock_core::analytics::*;

pub const MIN: i64 = 60_000;
pub const HOUR: i64 = 60 * MIN;
/// 2026-10-07T12:00:00Z
pub const T0: i64 = 1_791_374_400_000;

pub fn fh(used: f64, reset: i64) -> WindowObs {
    WindowObs::new(used, reset, 300)
}

pub fn wk(used: f64, reset: i64) -> WindowObs {
    WindowObs::new(used, reset, 10080)
}

pub struct Timeline {
    pub engine: Engine,
    pub state: EngineState,
    pub next_id: i64,
    pub account: String,
    pub credits: Option<i64>,
    pub allowed: Option<bool>,
    pub buckets: Vec<String>,
    pub outputs: Vec<StepOutput>,
}

impl Timeline {
    pub fn new() -> Self {
        Self {
            engine: Engine::new(EngineConfig::default()),
            state: EngineState::default(),
            next_id: 1,
            account: "a".repeat(32),
            credits: Some(2),
            allowed: Some(true),
            buckets: vec!["codex".into()],
            outputs: vec![],
        }
    }

    pub fn observe(&mut self, at: i64, five_hour: WindowObs, weekly: WindowObs) -> &StepOutput {
        let obs = Observation {
            snapshot_id: self.next_id,
            captured_at_ms: at,
            account_fingerprint: Some(self.account.clone()),
            account_mismatch: false,
            ordinary_usage_allowed: self.allowed,
            reset_credit_count: self.credits,
            bucket_ids: self.buckets.clone(),
            five_hour,
            weekly,
        };
        self.next_id += 1;
        let (next, out) = self.engine.step(&self.state, &obs);
        // Every state must survive a JSON round trip (it is persisted after each step).
        let json = serde_json::to_string(&next).expect("state serializes");
        self.state = serde_json::from_str(&json).expect("state deserializes");
        assert_eq!(self.state, next, "state JSON round trip must be lossless");
        self.outputs.push(out);
        self.outputs.last().unwrap()
    }

    pub fn events(&self) -> Vec<NewEvent> {
        // Upserts (same key) collapse to the latest version, like the database.
        let mut by_key: Vec<NewEvent> = Vec::new();
        for e in self.outputs.iter().flat_map(|o| o.events.iter()) {
            if let Some(existing) = by_key.iter_mut().find(|x| x.event_key == e.event_key) {
                *existing = e.clone();
            } else {
                by_key.push(e.clone());
            }
        }
        by_key
    }

    pub fn types(&self) -> Vec<QuotaEventType> {
        self.events().iter().map(|e| e.event_type).collect()
    }

    pub fn user_visible(&self) -> Vec<NewEvent> {
        self.events().into_iter().filter(|e| e.event_type.visibility() == EventVisibility::User).collect()
    }

    pub fn track(&self, t: WindowType) -> WindowTrackView {
        let ctx = self.state.context.as_ref().expect("context");
        self.engine.views(ctx).into_iter().find(|v| v.window_type == t).unwrap()
    }

    pub fn cycle_ops(&self) -> Vec<CycleOp> {
        self.outputs.iter().flat_map(|o| o.cycle_ops.iter().cloned()).collect()
    }

    pub fn closes(&self, t: WindowType) -> usize {
        let prefix = format!("|{}|cycle|", t.as_str());
        self.cycle_ops().iter().filter(|op| matches!(op, CycleOp::Close { cycle_key, .. } if cycle_key.contains(&prefix))).count()
    }

    pub fn final_quality(&self, snapshot_id: i64, t: WindowType) -> Option<SnapshotQuality> {
        self.outputs
            .iter()
            .flat_map(|o| o.quality_updates.iter())
            .rfind(|q| q.snapshot_id == snapshot_id && q.window_type == t)
            .map(|q| q.quality)
    }
}

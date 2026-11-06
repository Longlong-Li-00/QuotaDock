//! reset-detector-v3: the observation state machine.
//!
//! Raw snapshot -> normalization -> transition extraction -> evidence evaluation -> candidate ->
//! confirmation -> quota event. Pure and deterministic: `step(state, observation)` never reads the
//! clock, so live processing and re-analysis of stored snapshots produce identical results.
//!
//! Rules (all thresholds in `EngineConfig`, all [QUOTADOCK HEURISTIC]):
//! - a changed resetsAt alone only opens a ScheduleChange candidate; it is confirmed after the new
//!   value is seen in `schedule_confirm_polls` consecutive snapshots, rejected on A->B->A;
//! - drops <= `percent_noise_tolerance_pp` are noise; a substantial drop opens a refresh/rollover
//!   candidate, never an immediate reset (except with correlated reset-credit evidence);
//! - an idle window whose resetsAt slides with the clock is recognised and never confirmed as anchor;
//! - v2: when usage resumes after such an idle period, the burn-rate analysis segment restarts at
//!   the first active observation (same ObservedCycle, no event), with the derived active-window
//!   start `resetsAt - duration` as its 0 % origin;
//! - v3: a fixed-anchor window that turns into an idle sliding anchor before its reset time opens a
//!   BecameIdle candidate, continued while each observation stays idle and its resetsAt moves with
//!   the clock; after `idle_confirm_polls` it is an unscheduled refresh (usage dropped beyond noise,
//!   cause unknown) or a schedule change (within noise). While such a candidate is pending the old
//!   reset time is presented as unconfirmed. v2 required an identical resetsAt and re-opened the
//!   candidate on every poll, so the accepted reference never advanced (real run 2026-10-23).
//!   Idle at, or within the clock tolerance before, the accepted reset time is the rollover;
//! - v3: after `max_superseded_candidates` schedule-change / unknown candidates against the same
//!   reference were each superseded by yet another value, the latest observation becomes the
//!   reference (a refresh, rollover or idle candidate always opens);
//! - availability is never derived here: it is `ordinaryUsageAllowed`, period.

use super::CLASSIFIER_VERSION;
use super::config::EngineConfig;
use super::model::*;
use super::transitions::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CandidateKind {
    ScheduleChange,
    QuotaRefresh,
    Rollover,
    Unknown,
    /// v3. A fixed-anchor window turned into an idle sliding anchor before its reset time.
    BecameIdle,
}

impl CandidateKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ScheduleChange => "schedule_change",
            Self::QuotaRefresh => "quota_refresh",
            Self::Rollover => "rollover",
            Self::Unknown => "unknown_transition",
            Self::BecameIdle => "became_idle",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    pub key: String,
    pub kind: CandidateKind,
    /// Last accepted point before the change.
    pub baseline: ObsPoint,
    /// Opener + confirmations; in limbo (PendingConfirmation) until resolved.
    pub points: Vec<ObsPoint>,
    pub drop_at_open: f64,
    pub opened_evidence: WindowTransitionEvidence,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CycleRef {
    pub key: String,
    pub first_snapshot_id: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ResetRun {
    pub reset_at_ms: Option<i64>,
    pub count: u32,
}

/// Remembered after a confirmed change so that a quick return to the pre-change state can be
/// recognised as backend reconciliation and undone instead of becoming a second transition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfirmedChange {
    pub event_key: String,
    pub event_type: QuotaEventType,
    pub pre: ObsPoint,
    pub post_points: Vec<i64>,
    pub polls_since: u32,
    pub boundary: Option<(Option<CycleRef>, CycleRef)>,
    pub history_before: VecDeque<ObsPoint>,
    pub reset_run_before: Option<ResetRun>,
    pub segment_before: Option<i64>,
    #[serde(default)]
    pub origin_before: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowTrack {
    pub window_type: WindowType,
    pub history: VecDeque<ObsPoint>,
    pub reset_run: Option<ResetRun>,
    pub sliding: bool,
    pub sliding_reported: bool,
    pub pending: Option<Candidate>,
    pub missing_streak: u32,
    pub missing_reported: bool,
    pub cycle: Option<CycleRef>,
    pub segment_start_snapshot_id: Option<i64>,
    /// v2: derived start of the active window (confirmed resetsAt - duration) after an idle -> active
    /// re-anchor. Not a server timestamp; only the 0 % origin of the burn regression.
    #[serde(default)]
    pub active_origin_ms: Option<i64>,
    /// v2: the window was seen idle (sliding anchor) and no usage has been accepted since.
    #[serde(default)]
    pub awaiting_usage: bool,
    /// v3: (snapshot id of the accepted reference, n): the last n candidates against that
    /// reference were each superseded by yet another value before confirmation.
    #[serde(default)]
    pub superseded: Option<(i64, u32)>,
    pub last_confirmed: Option<ConfirmedChange>,
    pub state: WindowObservationState,
    pub candidate_state: ResetCandidateState,
    pub last_snapshot_id: i64,
}

impl WindowTrack {
    fn new(window_type: WindowType, snapshot_id: i64) -> Self {
        Self {
            window_type,
            history: VecDeque::new(),
            reset_run: None,
            sliding: false,
            sliding_reported: false,
            pending: None,
            missing_streak: 0,
            missing_reported: false,
            cycle: None,
            segment_start_snapshot_id: None,
            active_origin_ms: None,
            awaiting_usage: false,
            superseded: None,
            last_confirmed: None,
            state: WindowObservationState::Unknown,
            candidate_state: ResetCandidateState::None,
            last_snapshot_id: snapshot_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreditDrop {
    pub from: i64,
    pub to: i64,
    pub snapshot_id: i64,
    pub observed_at_ms: i64,
    pub polls_since: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Redemption {
    pub event_key: String,
    pub credit_from: i64,
    pub credit_to: i64,
    pub credit_drop_snapshot_id: i64,
    pub first_observed_at_ms: i64,
    pub previous_snapshot_id: Option<i64>,
    pub polls_since: u32,
    pub windows: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CreditTracker {
    pub count: Option<i64>,
    pub pending_drop: Option<CreditDrop>,
    pub redemption: Option<Redemption>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TopologyTracker {
    pub confirmed: Vec<String>,
    pub pending: Option<(Vec<String>, i64, u32)>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountContext {
    pub account_fingerprint: String,
    pub opened_snapshot_id: i64,
    pub last_snapshot_id: i64,
    pub last_availability: Availability,
    pub credits: CreditTracker,
    pub topology: TopologyTracker,
    pub windows: BTreeMap<WindowType, WindowTrack>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EngineState {
    pub classifier_version: String,
    pub context: Option<AccountContext>,
    pub last_snapshot_id: Option<i64>,
}

impl Default for EngineState {
    fn default() -> Self {
        Self { classifier_version: CLASSIFIER_VERSION.to_string(), context: None, last_snapshot_id: None }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct StepOutput {
    pub events: Vec<NewEvent>,
    pub cycle_ops: Vec<CycleOp>,
    pub quality_updates: Vec<QualityUpdate>,
    pub candidates: Vec<CandidateRecord>,
    pub tracks: Vec<WindowTrackView>,
    /// Per-snapshot classification summary, stored in `snapshots.analysis_json`.
    pub analysis: Value,
    /// Diagnostic lines (resetsAt changes etc.); contain fingerprints only, never account ids.
    pub log_lines: Vec<String>,
}

/// Per-step facts shared by both windows.
struct StepFacts<'a> {
    fp: &'a str,
    obs: &'a Observation,
    availability_before: Availability,
    credits_before: Option<i64>,
    topology_changed: bool,
}

pub struct Engine {
    pub cfg: EngineConfig,
}

fn event_key(fp: &str, window: Option<WindowType>, ty: QuotaEventType, snapshot_id: i64) -> String {
    format!("{CLASSIFIER_VERSION}|{fp}|{}|{}|{snapshot_id}", window.map(|w| w.as_str()).unwrap_or("account"), ty.as_str())
}

fn cycle_key(fp: &str, t: WindowType, first_snapshot_id: i64) -> String {
    format!("{CLASSIFIER_VERSION}|{fp}|{}|cycle|{first_snapshot_id}", t.as_str())
}

fn candidate_key(fp: &str, t: Option<WindowType>, kind: &str, opened: i64) -> String {
    format!("{CLASSIFIER_VERSION}|{fp}|{}|{kind}|{opened}", t.map(|w| w.as_str()).unwrap_or("account"))
}

fn short(fp: &str) -> String {
    fp.chars().take(8).collect()
}

fn point_of(obs: &Observation, t: WindowType) -> ObsPoint {
    let w = obs.window(t);
    ObsPoint {
        snapshot_id: obs.snapshot_id,
        captured_at_ms: obs.captured_at_ms,
        used: w.used_percent,
        reset_at_ms: w.reset_at_ms,
        duration_mins: w.duration_mins,
    }
}

fn point_json(p: &ObsPoint) -> Value {
    json!({ "snapshotId": p.snapshot_id, "capturedAtMs": p.captured_at_ms, "usedPercent": p.used, "resetAtMs": p.reset_at_ms, "durationMins": p.duration_mins })
}

impl Engine {
    pub fn new(cfg: EngineConfig) -> Self {
        Self { cfg }
    }

    pub fn step(&self, state: &EngineState, obs: &Observation) -> (EngineState, StepOutput) {
        let mut st = state.clone();
        let mut out = StepOutput::default();
        st.last_snapshot_id = Some(obs.snapshot_id);

        let fp = match &obs.account_fingerprint {
            Some(fp) if !obs.account_mismatch => fp.clone(),
            _ => {
                // Identity unverified: stored, never compared.
                out.analysis = json!({ "skipped": "account identity unverified" });
                if let Some(ctx) = &st.context {
                    out.tracks = self.views(ctx);
                }
                return (st, out);
            }
        };

        match st.context.take() {
            Some(ctx) if ctx.account_fingerprint == fp => {
                let mut ctx = ctx;
                self.advance(&mut ctx, obs, &mut out);
                st.context = Some(ctx);
            }
            Some(old) => {
                // Never compare one account's usage with another's.
                self.close_context(old, obs, &fp, &mut out);
                st.context = Some(self.open_context(obs, &fp, &mut out, "account_changed"));
            }
            None => st.context = Some(self.open_context(obs, &fp, &mut out, "first_observation")),
        }

        let ctx = st.context.as_ref().expect("context set");
        out.tracks = self.views(ctx);
        let mut summary = serde_json::Map::new();
        for t in WindowType::ALL {
            let tr = &ctx.windows[&t];
            summary.insert(
                t.as_str().into(),
                json!({
                    "state": tr.state.as_str(),
                    "candidate": tr.candidate_state.as_str(),
                    "pendingKind": tr.pending.as_ref().map(|c| c.kind.as_str()),
                    "slidingAnchor": tr.sliding,
                    "confirmedResetAtMs": self.anchor(tr).map(|a| a.0),
                    "analysisSegmentStartSnapshotId": tr.segment_start_snapshot_id,
                    "activeOriginMs": tr.active_origin_ms,
                }),
            );
        }
        summary.insert("classifierVersion".into(), json!(CLASSIFIER_VERSION));
        if let Some(existing) = out.analysis.as_object() {
            for (k, v) in existing {
                summary.insert(k.clone(), v.clone());
            }
        }
        out.analysis = Value::Object(summary);
        (st, out)
    }

    // ------------------------------------------------------------------ context lifecycle

    fn open_context(&self, obs: &Observation, fp: &str, out: &mut StepOutput, reason: &str) -> AccountContext {
        let mut windows = BTreeMap::new();
        for t in WindowType::ALL {
            let mut track = WindowTrack::new(t, obs.snapshot_id);
            let w = obs.window(t);
            if w.present {
                self.first_observation(&mut track, fp, point_of(obs, t), reason, out);
            } else {
                track.missing_streak = 1;
            }
            track.state = self.derive_state(&track, w.present);
            windows.insert(t, track);
        }
        AccountContext {
            account_fingerprint: fp.to_string(),
            opened_snapshot_id: obs.snapshot_id,
            last_snapshot_id: obs.snapshot_id,
            last_availability: obs.ordinary_usage_allowed.into(),
            credits: CreditTracker { count: obs.reset_credit_count, pending_drop: None, redemption: None },
            topology: TopologyTracker { confirmed: obs.bucket_ids.clone(), pending: None },
            windows,
        }
    }

    fn close_context(&self, old: AccountContext, obs: &Observation, new_fp: &str, out: &mut StepOutput) {
        for (t, track) in &old.windows {
            if let Some(c) = &track.pending {
                self.resolve_candidate(&old.account_fingerprint, *t, c, ResetCandidateState::Rejected, obs, None, out);
                for p in &c.points {
                    out.quality_updates.push(QualityUpdate {
                        snapshot_id: p.snapshot_id,
                        window_type: *t,
                        quality: SnapshotQuality::SuspectTransient,
                    });
                }
            }
            if let (Some(cycle), Some(last)) = (&track.cycle, track.history.back()) {
                out.cycle_ops.push(CycleOp::Close {
                    cycle_key: cycle.key.clone(),
                    observed_ended_at_ms: last.captured_at_ms,
                    end_reason: "account_changed".into(),
                    end_event_key: None,
                });
            }
        }
        out.events.push(NewEvent {
            event_key: event_key(new_fp, None, QuotaEventType::AccountChanged, obs.snapshot_id),
            account_fingerprint: Some(new_fp.to_string()),
            window_type: None,
            event_type: QuotaEventType::AccountChanged,
            observed_at_ms: obs.captured_at_ms,
            previous_snapshot_id: Some(old.last_snapshot_id),
            current_snapshot_id: obs.snapshot_id,
            confidence: EvidenceConfidence::High,
            confirmed: true,
            evidence: json!({ "previousAccount": short(&old.account_fingerprint), "newAccount": short(new_fp), "comparisonContextClosed": true }),
            supersedes_event_key: None,
        });
        out.log_lines.push(format!("account changed {} -> {}; comparison context closed", short(&old.account_fingerprint), short(new_fp)));
    }

    fn first_observation(&self, track: &mut WindowTrack, fp: &str, p: ObsPoint, reason: &str, out: &mut StepOutput) {
        let key = cycle_key(fp, track.window_type, p.snapshot_id);
        out.cycle_ops.push(CycleOp::Open {
            cycle_key: key.clone(),
            account_fingerprint: fp.to_string(),
            window_type: track.window_type,
            observed_started_at_ms: p.captured_at_ms,
            first_snapshot_id: p.snapshot_id,
            initial_used_percent: p.used,
            initial_reset_at_ms: p.reset_at_ms,
            start_reason: reason.to_string(),
            start_event_key: None,
            // The real window start is unknown: QuotaDock only knows when it first saw it.
            confidence: EvidenceConfidence::Low,
        });
        track.cycle = Some(CycleRef { key, first_snapshot_id: p.snapshot_id });
        track.segment_start_snapshot_id = Some(p.snapshot_id);
        track.active_origin_ms = None;
        track.awaiting_usage = false;
        track.last_snapshot_id = p.snapshot_id;
        self.accept(track, p, out);
    }

    // ------------------------------------------------------------------ per-step advance

    fn advance(&self, ctx: &mut AccountContext, obs: &Observation, out: &mut StepOutput) {
        let fp = ctx.account_fingerprint.clone();
        let credits_before = ctx.credits.count;

        // Reset credits (account level).
        if let Some(d) = ctx.credits.pending_drop.as_mut() {
            d.polls_since += 1;
        }
        if let Some(r) = ctx.credits.redemption.as_mut() {
            r.polls_since += 1;
            if r.polls_since > self.cfg.credit_correlation_polls {
                ctx.credits.redemption = None;
            }
        }
        if let (Some(before), Some(now)) = (credits_before, obs.reset_credit_count) {
            if now < before {
                if let Some(old) = ctx.credits.pending_drop.take() {
                    self.emit_uncorrelated_credit_change(&fp, &old, obs, out);
                }
                ctx.credits.pending_drop = Some(CreditDrop {
                    from: before,
                    to: now,
                    snapshot_id: obs.snapshot_id,
                    observed_at_ms: obs.captured_at_ms,
                    polls_since: 0,
                });
            } else if now > before {
                out.events.push(NewEvent {
                    event_key: event_key(&fp, None, QuotaEventType::ResetCreditCountChanged, obs.snapshot_id),
                    account_fingerprint: Some(fp.clone()),
                    window_type: None,
                    event_type: QuotaEventType::ResetCreditCountChanged,
                    observed_at_ms: obs.captured_at_ms,
                    previous_snapshot_id: Some(ctx.last_snapshot_id),
                    current_snapshot_id: obs.snapshot_id,
                    confidence: EvidenceConfidence::Low,
                    confirmed: true,
                    evidence: json!({ "creditBefore": before, "creditAfter": now, "direction": "increase", "cause": "unknown" }),
                    supersedes_event_key: None,
                });
            }
        }
        if obs.reset_credit_count.is_some() {
            ctx.credits.count = obs.reset_credit_count;
        }

        let topology_changed = self.topology(ctx, &fp, obs, out);
        let facts = StepFacts { fp: &fp, obs, availability_before: ctx.last_availability, credits_before, topology_changed };

        let mut redemption_touched = false;
        {
            let AccountContext { windows, credits, .. } = ctx;
            for t in WindowType::ALL {
                let track = windows.get_mut(&t).expect("track per window");
                self.process_window(track, &facts, credits, &mut redemption_touched, out);
            }
        }

        if redemption_touched && let Some(r) = &ctx.credits.redemption {
            out.events.push(NewEvent {
                event_key: r.event_key.clone(),
                account_fingerprint: Some(fp.clone()),
                window_type: None,
                event_type: QuotaEventType::ResetCreditRedeemed,
                observed_at_ms: r.first_observed_at_ms,
                previous_snapshot_id: r.previous_snapshot_id,
                current_snapshot_id: obs.snapshot_id,
                confidence: EvidenceConfidence::High,
                confirmed: true,
                evidence: json!({
                    "creditBefore": r.credit_from,
                    "creditAfter": r.credit_to,
                    "creditDropSnapshotId": r.credit_drop_snapshot_id,
                    "windows": r.windows,
                    "note": "QuotaDock never consumes reset credits; redeemed by an official client",
                }),
                supersedes_event_key: None,
            });
        }
        if let Some(d) = ctx.credits.pending_drop.clone()
            && d.polls_since > self.cfg.credit_correlation_polls
        {
            self.emit_uncorrelated_credit_change(&fp, &d, obs, out);
            ctx.credits.pending_drop = None;
        }

        ctx.last_availability = obs.ordinary_usage_allowed.into();
        ctx.last_snapshot_id = obs.snapshot_id;
    }

    fn emit_uncorrelated_credit_change(&self, fp: &str, d: &CreditDrop, obs: &Observation, out: &mut StepOutput) {
        out.events.push(NewEvent {
            event_key: event_key(fp, None, QuotaEventType::ResetCreditCountChanged, d.snapshot_id),
            account_fingerprint: Some(fp.to_string()),
            window_type: None,
            event_type: QuotaEventType::ResetCreditCountChanged,
            observed_at_ms: d.observed_at_ms,
            previous_snapshot_id: None,
            current_snapshot_id: obs.snapshot_id,
            confidence: EvidenceConfidence::Low,
            confirmed: true,
            evidence: json!({
                "creditBefore": d.from,
                "creditAfter": d.to,
                "direction": "decrease",
                "correlatedQuotaRefresh": false,
                "cause": "unknown (expiry, redemption without visible refresh, or backend correction)",
            }),
            supersedes_event_key: None,
        });
    }

    /// Bucket set changes are confirmed like schedule changes; they never imply a reset.
    fn topology(&self, ctx: &mut AccountContext, fp: &str, obs: &Observation, out: &mut StepOutput) -> bool {
        let ids = &obs.bucket_ids;
        let topo = &mut ctx.topology;
        if *ids == topo.confirmed {
            if let Some((pending_ids, first, _)) = topo.pending.take() {
                out.events.push(NewEvent {
                    event_key: event_key(fp, None, QuotaEventType::TransientBackendState, obs.snapshot_id),
                    account_fingerprint: Some(fp.to_string()),
                    window_type: None,
                    event_type: QuotaEventType::TransientBackendState,
                    observed_at_ms: obs.captured_at_ms,
                    previous_snapshot_id: Some(first),
                    current_snapshot_id: obs.snapshot_id,
                    confidence: EvidenceConfidence::Low,
                    confirmed: true,
                    evidence: json!({ "subject": "bucket_topology", "confirmed": topo.confirmed, "transient": pending_ids }),
                    supersedes_event_key: None,
                });
            }
            return false;
        }
        let polls = match &topo.pending {
            Some((p, _, n)) if p == ids => n + 1,
            _ => 1,
        };
        let first = match &topo.pending {
            Some((p, f, _)) if p == ids => *f,
            _ => obs.snapshot_id,
        };
        if polls >= self.cfg.schedule_confirm_polls {
            out.events.push(NewEvent {
                event_key: event_key(fp, None, QuotaEventType::BucketTopologyChanged, obs.snapshot_id),
                account_fingerprint: Some(fp.to_string()),
                window_type: None,
                event_type: QuotaEventType::BucketTopologyChanged,
                observed_at_ms: obs.captured_at_ms,
                previous_snapshot_id: Some(first),
                current_snapshot_id: obs.snapshot_id,
                confidence: EvidenceConfidence::Medium,
                confirmed: true,
                evidence: json!({ "before": topo.confirmed, "after": ids, "polls": polls, "note": "bucket meanings are not inferred; codex cycles are unaffected" }),
                supersedes_event_key: None,
            });
            topo.confirmed = ids.clone();
            topo.pending = None;
            true
        } else {
            topo.pending = Some((ids.clone(), first, polls));
            false
        }
    }

    // ------------------------------------------------------------------ per-window processing

    fn process_window(
        &self,
        track: &mut WindowTrack,
        f: &StepFacts,
        credits: &mut CreditTracker,
        redemption_touched: &mut bool,
        out: &mut StepOutput,
    ) {
        let t = track.window_type;
        let w = f.obs.window(t);
        track.last_snapshot_id = f.obs.snapshot_id;
        track.candidate_state = if track.pending.is_some() { ResetCandidateState::Pending } else { ResetCandidateState::None };

        if !w.present {
            track.missing_streak += 1;
            if track.missing_streak >= self.cfg.missing_confirm_polls && !track.missing_reported {
                track.missing_reported = true;
                out.events.push(NewEvent {
                    event_key: event_key(f.fp, Some(t), QuotaEventType::BucketTopologyChanged, f.obs.snapshot_id),
                    account_fingerprint: Some(f.fp.to_string()),
                    window_type: Some(t),
                    event_type: QuotaEventType::BucketTopologyChanged,
                    observed_at_ms: f.obs.captured_at_ms,
                    previous_snapshot_id: track.history.back().map(|p| p.snapshot_id),
                    current_snapshot_id: f.obs.snapshot_id,
                    confidence: EvidenceConfidence::Medium,
                    confirmed: true,
                    evidence: json!({ "change": "window_absent", "consecutivePolls": track.missing_streak, "note": "not treated as exhaustion or reset" }),
                    supersedes_event_key: None,
                });
                if let Some(c) = track.pending.take() {
                    self.reject(track, f.fp, &c, f.obs, "window disappeared", out);
                }
                if let (Some(cycle), Some(last)) = (track.cycle.take(), track.history.back()) {
                    out.cycle_ops.push(CycleOp::Close {
                        cycle_key: cycle.key,
                        observed_ended_at_ms: last.captured_at_ms,
                        end_reason: "window_absent".into(),
                        end_event_key: None,
                    });
                }
                track.history.clear();
                track.reset_run = None;
                track.last_confirmed = None;
            }
            track.state = self.derive_state(track, false);
            return;
        }

        let cur = point_of(f.obs, t);
        if track.missing_reported {
            out.events.push(NewEvent {
                event_key: event_key(f.fp, Some(t), QuotaEventType::BucketTopologyChanged, f.obs.snapshot_id),
                account_fingerprint: Some(f.fp.to_string()),
                window_type: Some(t),
                event_type: QuotaEventType::BucketTopologyChanged,
                observed_at_ms: f.obs.captured_at_ms,
                previous_snapshot_id: None,
                current_snapshot_id: f.obs.snapshot_id,
                confidence: EvidenceConfidence::Medium,
                confirmed: true,
                evidence: json!({ "change": "window_reappeared", "afterMissingPolls": track.missing_streak }),
                supersedes_event_key: None,
            });
        }
        let was_missing_long = track.missing_reported;
        track.missing_streak = 0;
        track.missing_reported = false;

        if track.history.is_empty() && track.pending.is_none() {
            let reason = if was_missing_long { "window_reappeared" } else { "first_observation" };
            self.first_observation(track, f.fp, cur, reason, out);
            track.state = self.derive_state(track, true);
            return;
        }

        // The previous observation (pending or accepted), for diagnostics only.
        let previous = track.pending.as_ref().and_then(|c| c.points.last().cloned()).or_else(|| track.history.back().cloned());
        let reset_before = previous.as_ref().and_then(|p| p.reset_at_ms);
        let before_used = previous.as_ref().and_then(|p| p.used);
        let classification = self.classify_and_apply(track, f, credits, redemption_touched, cur.clone(), out);

        if previous.is_some() && reset_before != cur.reset_at_ms {
            out.log_lines.push(format!(
                "resetsAt changed window={} old={} new={} deltaSeconds={} usedBefore={} usedAfter={} creditBefore={} creditAfter={} classification={}",
                t.as_str(),
                reset_before.map(|v| v.to_string()).unwrap_or_else(|| "null".into()),
                cur.reset_at_ms.map(|v| v.to_string()).unwrap_or_else(|| "null".into()),
                match (reset_before, cur.reset_at_ms) {
                    (Some(a), Some(b)) => ((b - a) / 1000).to_string(),
                    _ => "n/a".into(),
                },
                before_used.map(|v| v.to_string()).unwrap_or_else(|| "?".into()),
                cur.used.map(|v| v.to_string()).unwrap_or_else(|| "null".into()),
                f.credits_before.map(|v| v.to_string()).unwrap_or_else(|| "null".into()),
                f.obs.reset_credit_count.map(|v| v.to_string()).unwrap_or_else(|| "null".into()),
                classification
            ));
        }
        track.state = self.derive_state(track, true);
    }

    /// Returns a short classification label for diagnostics.
    fn classify_and_apply(
        &self,
        track: &mut WindowTrack,
        f: &StepFacts,
        credits: &mut CreditTracker,
        redemption_touched: &mut bool,
        cur: ObsPoint,
        out: &mut StepOutput,
    ) -> String {
        let cfg = &self.cfg;

        // 1. Backend reconciliation: a confirmed change quickly reverted to the pre-change state.
        if let Some(lc) = track.last_confirmed.as_mut() {
            lc.polls_since += 1;
        }
        if let Some(lc) = track.last_confirmed.clone() {
            if lc.polls_since > cfg.reconciliation_polls {
                track.last_confirmed = None;
            } else if track.pending.is_none() && same_state(&cur, &lc.pre, cfg) {
                self.reconcile(track, f, &lc, &cur, out);
                return format!("backend_reconciliation_of:{}", lc.event_type.as_str());
            }
        }

        // 2. A pending candidate is confirmed, rejected (A->B->A) or superseded.
        if let Some(mut cand) = track.pending.take() {
            if same_state(&cur, &cand.baseline, cfg) {
                self.reject(track, f.fp, &cand, f.obs, "returned to the pre-change state (A->B->A)", out);
                self.accept(track, cur, out);
                return "transient_rejected".into();
            }
            // v3: the scheduled reset time arrived (or is within the clock tolerance) before an idle
            // refresh was confirmed: it is the rollover, not an early refresh.
            let at_reset = self.idle_at_reset(cand.baseline.reset_at_ms, &cur);
            let crossed_reset = cand.baseline.reset_at_ms.is_some_and(|r| cur.captured_at_ms >= r);
            if (cand.kind == CandidateKind::BecameIdle && (at_reset || crossed_reset))
                || (cand.kind == CandidateKind::QuotaRefresh && at_reset)
            {
                cand.kind = CandidateKind::Rollover;
            }
            if self.continues(&cand, &cur) {
                cand.points.push(cur.clone());
                out.quality_updates.push(QualityUpdate {
                    snapshot_id: cur.snapshot_id,
                    window_type: track.window_type,
                    quality: SnapshotQuality::PendingConfirmation,
                });
                let credit_ok = self.credit_signal_active(credits) && (cand.points.len() as u32 - 1) <= cfg.credit_correlation_polls;
                if self.refresh_like(cand.kind, cand.drop_at_open) && credit_ok {
                    self.confirm_credit(track, f, credits, redemption_touched, cand, out);
                    return "reset_credit_redeemed".into();
                }
                let ready = match cand.kind {
                    CandidateKind::ScheduleChange | CandidateKind::Rollover | CandidateKind::Unknown => {
                        cand.points.len() as u32 >= cfg.schedule_confirm_polls
                    }
                    // Wait out the credit-correlation window so a late credit update cannot turn an
                    // already-announced unscheduled refresh into a second event.
                    CandidateKind::QuotaRefresh => cand.points.len() as u32 > cfg.credit_correlation_polls,
                    CandidateKind::BecameIdle => cand.points.len() as u32 >= cfg.idle_confirm_polls,
                };
                if ready {
                    let label = self.confirm(track, f, cand, out);
                    return label;
                }
                self.upsert_candidate(f.fp, track.window_type, &cand, ResetCandidateState::Pending, None, None, out);
                track.pending = Some(cand);
                track.candidate_state = ResetCandidateState::Pending;
                return "candidate_pending".into();
            }
            // Diverged to a third state: the earlier candidate was not stable. v3: only the first of a
            // run of such rejections against one reference is written as a diagnostic event; the run
            // is summarised if the reference is re-based (step 5).
            let n = match track.superseded {
                Some((id, n)) if id == cand.baseline.snapshot_id => n + 1,
                _ => 1,
            };
            track.superseded = Some((cand.baseline.snapshot_id, n));
            self.reject_as(track, f.fp, &cand, f.obs, "superseded by a different value before confirmation", n == 1, out);
        }

        // 3. Generic A->B->A among accepted points (e.g. a one-poll usage spike).
        if track.history.len() >= 2 {
            let n = track.history.len();
            let (a, b) = (track.history[n - 2].clone(), track.history[n - 1].clone());
            if same_state(&cur, &a, cfg) && materially_differs(&b, &a, cfg) {
                out.quality_updates.push(QualityUpdate {
                    snapshot_id: b.snapshot_id,
                    window_type: track.window_type,
                    quality: SnapshotQuality::SuspectTransient,
                });
                track.history.pop_back();
                self.rebuild_reset_run(track);
                out.events.push(NewEvent {
                    event_key: event_key(f.fp, Some(track.window_type), QuotaEventType::TransientBackendState, f.obs.snapshot_id),
                    account_fingerprint: Some(f.fp.to_string()),
                    window_type: Some(track.window_type),
                    event_type: QuotaEventType::TransientBackendState,
                    observed_at_ms: b.captured_at_ms,
                    previous_snapshot_id: Some(a.snapshot_id),
                    current_snapshot_id: f.obs.snapshot_id,
                    confidence: EvidenceConfidence::Low,
                    confirmed: true,
                    evidence: json!({ "pattern": "A->B->A", "a": point_json(&a), "transient": point_json(&b), "current": point_json(&cur) }),
                    supersedes_event_key: None,
                });
                self.accept(track, cur, out);
                return "transient_accepted_point_flagged".into();
            }
        }

        let reference = track.history.back().cloned().expect("history not empty");

        // 4. Idle window with a sliding "now + duration" anchor: never a schedule change.
        if sliding_pair(&reference, &cur, cfg) {
            self.accept(track, cur, out);
            let ev = detect_sliding(track.history.iter(), cfg);
            if ev.detected {
                track.sliding = true;
                track.awaiting_usage = true;
                if !track.sliding_reported {
                    track.sliding_reported = true;
                    out.events.push(NewEvent {
                        event_key: event_key(f.fp, Some(track.window_type), QuotaEventType::SlidingAnchorDetected, f.obs.snapshot_id),
                        account_fingerprint: Some(f.fp.to_string()),
                        window_type: Some(track.window_type),
                        event_type: QuotaEventType::SlidingAnchorDetected,
                        observed_at_ms: f.obs.captured_at_ms,
                        previous_snapshot_id: Some(reference.snapshot_id),
                        current_snapshot_id: f.obs.snapshot_id,
                        confidence: EvidenceConfidence::Medium,
                        confirmed: true,
                        evidence: serde_json::to_value(&ev).unwrap_or(Value::Null),
                        supersedes_event_key: None,
                    });
                }
                return "sliding_anchor".into();
            }
            return "sliding_anchor_suspected".into();
        }
        if !sliding_like(&cur, cfg) {
            track.sliding = false;
            track.sliding_reported = false;
        }

        // 4b (v2). First usage after an idle period: a new analysis segment, not a transition.
        if let Some(label) = self.reanchor_after_idle(track, &reference, &cur, out) {
            self.accept(track, cur, out);
            return label;
        }

        // 5. Evaluate the change against the last accepted point.
        let drop = match (reference.used, cur.used) {
            (Some(a), Some(b)) => a - b,
            _ => 0.0,
        };
        let reset_changed = reference.reset_at_ms != cur.reset_at_ms;
        let reset_advanced = matches!((reference.reset_at_ms, cur.reset_at_ms), (Some(a), Some(b)) if b > a);
        let crossed = reference.reset_at_ms.is_some_and(|r| cur.captured_at_ms >= r);
        let substantial = drop >= cfg.substantial_drop_pp;
        let both_resets = reference.reset_at_ms.is_some() && cur.reset_at_ms.is_some();
        // v3: idle within the clock tolerance of the accepted reset time is the scheduled rollover seen
        // slightly early (the backend clock runs ahead of the local one), never an early refresh.
        let idle_at_reset = self.idle_at_reset(reference.reset_at_ms, &cur);
        // v3: idle (0 %, resetsAt = now + duration) well before the accepted reset time. A substantial
        // drop stays a QuotaRefresh candidate (it is confirmed by the drop, idle or not).
        let became_idle = !crossed
            && !idle_at_reset
            && !substantial
            && sliding_like(&cur, cfg)
            && !sliding_like(&reference, cfg)
            && cur.duration_mins == reference.duration_mins;
        let kind = if (crossed || idle_at_reset) && (reset_advanced || substantial) {
            Some(CandidateKind::Rollover)
        } else if became_idle {
            Some(CandidateKind::BecameIdle)
        } else if substantial {
            Some(CandidateKind::QuotaRefresh)
        } else if reset_changed && drop <= cfg.percent_noise_tolerance_pp && both_resets {
            Some(CandidateKind::ScheduleChange)
        } else if reset_changed || drop > cfg.percent_noise_tolerance_pp {
            Some(CandidateKind::Unknown)
        } else {
            None
        };
        let Some(kind) = kind else {
            self.accept(track, cur, out);
            return if crossed { "expired_unconfirmed".into() } else { "no_material_change".into() };
        };
        // A refresh, rollover or idle transition always opens its candidate: re-basing would lose it.
        if matches!(kind, CandidateKind::ScheduleChange | CandidateKind::Unknown)
            && let Some((id, n)) = track.superseded
            && id == reference.snapshot_id
            && n >= cfg.max_superseded_candidates
        {
            return self.rebase(track, f, &reference, cur, drop, n, out);
        }

        let evidence = extract_evidence(
            &reference,
            &cur,
            f.availability_before,
            f.obs.ordinary_usage_allowed.into(),
            f.credits_before,
            f.obs.reset_credit_count,
            track.sliding,
            f.topology_changed,
        );
        let cand = Candidate {
            key: candidate_key(f.fp, Some(track.window_type), kind.as_str(), cur.snapshot_id),
            kind,
            baseline: reference,
            points: vec![cur.clone()],
            drop_at_open: drop,
            opened_evidence: evidence,
        };
        out.quality_updates.push(QualityUpdate {
            snapshot_id: cur.snapshot_id,
            window_type: track.window_type,
            quality: SnapshotQuality::PendingConfirmation,
        });

        // Strong evidence exception: refresh + correlated reset-credit decrease confirms at once.
        if self.refresh_like(kind, drop) && self.credit_signal_active(credits) {
            self.confirm_credit(track, f, credits, redemption_touched, cand, out);
            return "reset_credit_redeemed".into();
        }
        self.upsert_candidate(f.fp, track.window_type, &cand, ResetCandidateState::Pending, None, None, out);
        track.pending = Some(cand);
        track.candidate_state = ResetCandidateState::Pending;
        format!("candidate_opened:{}", kind.as_str())
    }

    fn continues(&self, cand: &Candidate, cur: &ObsPoint) -> bool {
        let cfg = &self.cfg;
        let opener = &cand.points[0];
        match cand.kind {
            CandidateKind::ScheduleChange => {
                cur.reset_at_ms == opener.reset_at_ms
                    && cur.duration_mins == opener.duration_mins
                    && match (cur.used, opener.used) {
                        (Some(c), Some(o)) => c >= o - cfg.percent_noise_tolerance_pp,
                        _ => true,
                    }
            }
            CandidateKind::QuotaRefresh | CandidateKind::Rollover if cand.drop_at_open >= cfg.substantial_drop_pp => {
                match (cur.used, cand.baseline.used) {
                    (Some(c), Some(b)) => c <= b - cfg.substantial_drop_pp,
                    _ => false,
                }
            }
            CandidateKind::QuotaRefresh | CandidateKind::Rollover => cur.reset_at_ms != cand.baseline.reset_at_ms,
            CandidateKind::Unknown => same_state(cur, opener, cfg),
            // v3: still idle, same window length, resetsAt moved with the clock since the last point.
            CandidateKind::BecameIdle => {
                let last = cand.points.last().expect("points");
                sliding_like(cur, cfg) && cur.duration_mins == opener.duration_mins && self.idle_step(last, cur)
            }
        }
    }

    /// v3. Between two idle observations resetsAt moved forward with the observation clock, within
    /// `idle_anchor_drift_tolerance_secs`.
    fn idle_step(&self, a: &ObsPoint, b: &ObsPoint) -> bool {
        let (Some(ra), Some(rb)) = (a.reset_at_ms, b.reset_at_ms) else { return false };
        let d_clock = b.captured_at_ms - a.captured_at_ms;
        d_clock > 0 && (rb - ra - d_clock).abs() <= self.cfg.idle_anchor_drift_tolerance_secs * 1000
    }

    /// v3. `p` shows the window idle at, or within `sliding_anchor_tolerance_secs` before, the
    /// accepted reset time `reset`: the scheduled rollover (the backend clock may run ahead).
    fn idle_at_reset(&self, reset: Option<i64>, p: &ObsPoint) -> bool {
        reset.is_some_and(|r| p.captured_at_ms >= r - self.cfg.sliding_anchor_tolerance_secs * 1000) && sliding_like(p, &self.cfg)
    }

    /// v3. Trailing points that are idle sliding observations, each moving with the clock.
    fn idle_run(&self, points: &[ObsPoint]) -> usize {
        let mut n = 0;
        for (i, p) in points.iter().enumerate().rev() {
            if !sliding_like(p, &self.cfg) || points.get(i + 1).is_some_and(|next| !self.idle_step(p, next)) {
                break;
            }
            n += 1;
        }
        n
    }

    /// Candidate kinds that a correlated reset-credit decrease can confirm as a redemption.
    fn refresh_like(&self, kind: CandidateKind, drop: f64) -> bool {
        match kind {
            CandidateKind::QuotaRefresh | CandidateKind::Rollover => true,
            CandidateKind::BecameIdle => drop > self.cfg.percent_noise_tolerance_pp,
            CandidateKind::ScheduleChange | CandidateKind::Unknown => false,
        }
    }

    fn credit_signal_active(&self, credits: &CreditTracker) -> bool {
        credits.pending_drop.as_ref().is_some_and(|d| d.polls_since <= self.cfg.credit_correlation_polls) || credits.redemption.is_some()
    }

    fn stable_new_reset_polls(points: &[ObsPoint]) -> usize {
        let Some(last) = points.last() else { return 0 };
        points.iter().rev().take_while(|p| p.reset_at_ms == last.reset_at_ms).count()
    }

    fn confirm(&self, track: &mut WindowTrack, f: &StepFacts, cand: Candidate, out: &mut StepOutput) -> String {
        let cfg = &self.cfg;
        let stable_polls = Self::stable_new_reset_polls(&cand.points);
        let last = cand.points.last().expect("points");
        let anchor_stable = stable_polls >= cfg.schedule_confirm_polls as usize && !sliding_like(last, cfg);
        let substantial = cand.drop_at_open >= cfg.substantial_drop_pp;
        let (ty, confidence) = match cand.kind {
            CandidateKind::Rollover => (
                QuotaEventType::NaturalRolloverObserved,
                if substantial && anchor_stable {
                    EvidenceConfidence::High
                } else if substantial || anchor_stable {
                    EvidenceConfidence::Medium
                } else {
                    EvidenceConfidence::Low
                },
            ),
            CandidateKind::QuotaRefresh => (
                QuotaEventType::UnscheduledQuotaRefresh,
                // v3: a consistent idle run is stable evidence too (its resetsAt never repeats).
                if stable_polls >= cfg.schedule_confirm_polls as usize || self.idle_run(&cand.points) >= cfg.idle_confirm_polls as usize {
                    EvidenceConfidence::Medium
                } else {
                    EvidenceConfidence::Low
                },
            ),
            // v3: usage dropped beyond noise into a confirmed idle run before the reset time. What
            // caused it is not observable: the event only says an early refresh was observed.
            CandidateKind::BecameIdle if cand.drop_at_open > cfg.percent_noise_tolerance_pp => {
                (QuotaEventType::UnscheduledQuotaRefresh, EvidenceConfidence::Medium)
            }
            CandidateKind::ScheduleChange | CandidateKind::BecameIdle => {
                (QuotaEventType::WindowScheduleChanged, EvidenceConfidence::Medium)
            }
            CandidateKind::Unknown => (QuotaEventType::UnknownQuotaTransition, EvidenceConfidence::Low),
        };
        let key = event_key(f.fp, Some(track.window_type), ty, f.obs.snapshot_id);
        let mut evidence = serde_json::to_value(&cand.opened_evidence).unwrap_or(Value::Null);
        if let Some(o) = evidence.as_object_mut() {
            o.insert("stableNewResetAtPolls".into(), json!(stable_polls));
            o.insert("slidingAnchorDetected".into(), json!(sliding_like(last, cfg)));
            o.insert("candidateKind".into(), json!(cand.kind.as_str()));
            o.insert("points".into(), json!(cand.points.iter().map(point_json).collect::<Vec<_>>()));
            o.insert("firstObservedAtMs".into(), json!(cand.points[0].captured_at_ms));
            o.insert("confirmedAtMs".into(), json!(f.obs.captured_at_ms));
            if ty == QuotaEventType::UnscheduledQuotaRefresh || ty == QuotaEventType::UnknownQuotaTransition {
                o.insert("cause".into(), json!("unknown"));
            }
            if ty == QuotaEventType::WindowScheduleChanged && sliding_like(&cand.baseline, cfg) {
                o.insert("fromIdleSlidingAnchor".into(), json!(true));
            }
            if cand.kind == CandidateKind::BecameIdle {
                o.insert("toIdleSlidingAnchor".into(), json!(true));
                o.insert("idleObservations".into(), json!(self.idle_run(&cand.points)));
            }
        }
        out.events.push(NewEvent {
            event_key: key.clone(),
            account_fingerprint: Some(f.fp.to_string()),
            window_type: Some(track.window_type),
            event_type: ty,
            observed_at_ms: cand.points[0].captured_at_ms,
            previous_snapshot_id: Some(cand.baseline.snapshot_id),
            current_snapshot_id: f.obs.snapshot_id,
            confidence,
            confirmed: true,
            evidence,
            supersedes_event_key: None,
        });
        self.apply_confirmed(track, f.fp, &cand, ty, confidence, &key, out);
        // v3: the idle run is the evidence; the window is a sliding anchor from here on (reported
        // with this event, no separate SlidingAnchorDetected).
        if cand.kind == CandidateKind::BecameIdle && detect_sliding(track.history.iter(), cfg).detected {
            track.sliding = true;
            track.sliding_reported = true;
            track.awaiting_usage = true;
        }
        format!("confirmed:{}", ty.as_str())
    }

    fn confirm_credit(
        &self,
        track: &mut WindowTrack,
        f: &StepFacts,
        credits: &mut CreditTracker,
        redemption_touched: &mut bool,
        cand: Candidate,
        out: &mut StepOutput,
    ) {
        if credits.redemption.is_none() {
            let d = credits.pending_drop.take().expect("credit signal implies a drop or an open redemption");
            credits.redemption = Some(Redemption {
                event_key: event_key(f.fp, None, QuotaEventType::ResetCreditRedeemed, f.obs.snapshot_id),
                credit_from: d.from,
                credit_to: d.to,
                credit_drop_snapshot_id: d.snapshot_id,
                first_observed_at_ms: d.observed_at_ms.min(cand.points[0].captured_at_ms),
                previous_snapshot_id: Some(cand.baseline.snapshot_id),
                polls_since: 0,
                windows: BTreeMap::new(),
            });
        }
        let r = credits.redemption.as_mut().expect("redemption");
        r.first_observed_at_ms = r.first_observed_at_ms.min(cand.points[0].captured_at_ms);
        let mut ev = serde_json::to_value(&cand.opened_evidence).unwrap_or(Value::Null);
        if let Some(o) = ev.as_object_mut() {
            o.insert("candidateKind".into(), json!(cand.kind.as_str()));
            o.insert("points".into(), json!(cand.points.iter().map(point_json).collect::<Vec<_>>()));
            o.insert("stableNewResetAtPolls".into(), json!(Self::stable_new_reset_polls(&cand.points)));
        }
        r.windows.insert(track.window_type.as_str().into(), ev);
        let key = r.event_key.clone();
        *redemption_touched = true;
        self.apply_confirmed(track, f.fp, &cand, QuotaEventType::ResetCreditRedeemed, EvidenceConfidence::High, &key, out);
    }

    /// Bookkeeping shared by every confirmation: candidate row, qualities, cycles, history.
    #[allow(clippy::too_many_arguments)]
    fn apply_confirmed(
        &self,
        track: &mut WindowTrack,
        fp: &str,
        cand: &Candidate,
        ty: QuotaEventType,
        confidence: EvidenceConfidence,
        key: &str,
        out: &mut StepOutput,
    ) {
        let t = track.window_type;
        let last = cand.points.last().expect("points");
        self.upsert_candidate(fp, t, cand, ResetCandidateState::Confirmed, Some(last.captured_at_ms), Some(key.to_string()), out);
        for p in &cand.points {
            out.quality_updates.push(QualityUpdate { snapshot_id: p.snapshot_id, window_type: t, quality: SnapshotQuality::Normal });
        }
        let history_before = track.history.clone();
        let reset_run_before = track.reset_run;
        let segment_before = track.segment_start_snapshot_id;
        let origin_before = track.active_origin_ms;
        let mut boundary = None;
        if ty.is_cycle_boundary() {
            let old = track.cycle.take();
            if let Some(old) = &old {
                out.cycle_ops.push(CycleOp::Close {
                    cycle_key: old.key.clone(),
                    observed_ended_at_ms: cand.baseline.captured_at_ms,
                    end_reason: ty.as_str().into(),
                    end_event_key: Some(key.to_string()),
                });
            }
            let opener = &cand.points[0];
            let new_key = cycle_key(fp, t, opener.snapshot_id);
            out.cycle_ops.push(CycleOp::Open {
                cycle_key: new_key.clone(),
                account_fingerprint: fp.to_string(),
                window_type: t,
                observed_started_at_ms: opener.captured_at_ms,
                first_snapshot_id: opener.snapshot_id,
                initial_used_percent: opener.used,
                initial_reset_at_ms: opener.reset_at_ms,
                start_reason: ty.as_str().into(),
                start_event_key: Some(key.to_string()),
                confidence,
            });
            let new_ref = CycleRef { key: new_key, first_snapshot_id: opener.snapshot_id };
            track.cycle = Some(new_ref.clone());
            track.history.clear();
            track.reset_run = None;
            track.segment_start_snapshot_id = Some(opener.snapshot_id);
            track.active_origin_ms = None;
            track.awaiting_usage = false;
            boundary = Some((old, new_ref));
        } else if ty == QuotaEventType::UnknownQuotaTransition && cand.drop_at_open > self.cfg.percent_noise_tolerance_pp {
            // A confirmed in-cycle correction: burn rate restarts after it (no cycle boundary).
            track.segment_start_snapshot_id = Some(cand.points[0].snapshot_id);
            track.active_origin_ms = None;
        }
        track.sliding = false;
        track.sliding_reported = false;
        for p in &cand.points {
            self.accept(track, p.clone(), out);
        }
        track.last_confirmed = Some(ConfirmedChange {
            event_key: key.to_string(),
            event_type: ty,
            pre: cand.baseline.clone(),
            post_points: cand.points.iter().map(|p| p.snapshot_id).collect(),
            polls_since: 0,
            boundary,
            history_before,
            reset_run_before,
            segment_before,
            origin_before,
        });
        track.candidate_state = ResetCandidateState::Confirmed;
    }

    fn reject(&self, track: &mut WindowTrack, fp: &str, cand: &Candidate, obs: &Observation, why: &str, out: &mut StepOutput) {
        self.reject_as(track, fp, cand, obs, why, true, out);
    }

    /// `emit`: write the TransientBackendState event (the candidate row and qualities always are).
    #[allow(clippy::too_many_arguments)]
    fn reject_as(
        &self,
        track: &mut WindowTrack,
        fp: &str,
        cand: &Candidate,
        obs: &Observation,
        why: &str,
        emit: bool,
        out: &mut StepOutput,
    ) {
        let t = track.window_type;
        for p in &cand.points {
            out.quality_updates.push(QualityUpdate {
                snapshot_id: p.snapshot_id,
                window_type: t,
                quality: SnapshotQuality::SuspectTransient,
            });
        }
        let key = emit.then(|| event_key(fp, Some(t), QuotaEventType::TransientBackendState, obs.snapshot_id));
        if let Some(key) = &key {
            out.events.push(NewEvent {
                event_key: key.clone(),
                account_fingerprint: Some(fp.to_string()),
                window_type: Some(t),
                event_type: QuotaEventType::TransientBackendState,
                observed_at_ms: cand.points[0].captured_at_ms,
                previous_snapshot_id: Some(cand.baseline.snapshot_id),
                current_snapshot_id: obs.snapshot_id,
                confidence: EvidenceConfidence::Low,
                confirmed: true,
                evidence: json!({
                    "rejectedCandidateKind": cand.kind.as_str(),
                    "reason": why,
                    "baseline": point_json(&cand.baseline),
                    "transient": cand.points.iter().map(point_json).collect::<Vec<_>>(),
                    "resetAtDeltaSeconds": match (cand.baseline.reset_at_ms, cand.points[0].reset_at_ms) {
                        (Some(a), Some(b)) => Some((b - a) / 1000),
                        _ => None,
                    },
                }),
                supersedes_event_key: None,
            });
        }
        self.upsert_candidate(fp, t, cand, ResetCandidateState::Rejected, Some(obs.captured_at_ms), key, out);
        track.candidate_state = ResetCandidateState::Rejected;
    }

    /// v3. `n` candidates in a row against `reference` were superseded before confirmation: the
    /// backend value is unstable and the reference is stale. The current observation becomes the
    /// reference (one diagnostic event, no cycle boundary) instead of opening yet another candidate.
    #[allow(clippy::too_many_arguments)]
    fn rebase(
        &self,
        track: &mut WindowTrack,
        f: &StepFacts,
        reference: &ObsPoint,
        cur: ObsPoint,
        drop: f64,
        n: u32,
        out: &mut StepOutput,
    ) -> String {
        let t = track.window_type;
        out.events.push(NewEvent {
            event_key: event_key(f.fp, Some(t), QuotaEventType::UnknownQuotaTransition, f.obs.snapshot_id),
            account_fingerprint: Some(f.fp.to_string()),
            window_type: Some(t),
            event_type: QuotaEventType::UnknownQuotaTransition,
            observed_at_ms: f.obs.captured_at_ms,
            previous_snapshot_id: Some(reference.snapshot_id),
            current_snapshot_id: f.obs.snapshot_id,
            confidence: EvidenceConfidence::Low,
            confirmed: true,
            evidence: json!({
                "pattern": "unstable_backend_value",
                "supersededCandidates": n,
                "staleReference": point_json(reference),
                "current": point_json(&cur),
                "cause": "unknown",
            }),
            supersedes_event_key: None,
        });
        if drop > self.cfg.percent_noise_tolerance_pp {
            // Like a confirmed in-cycle correction: burn rate restarts after it.
            track.segment_start_snapshot_id = Some(cur.snapshot_id);
            track.active_origin_ms = None;
        }
        track.superseded = None;
        self.accept(track, cur, out);
        format!("reference_rebased_after:{n}")
    }

    fn reconcile(&self, track: &mut WindowTrack, f: &StepFacts, lc: &ConfirmedChange, cur: &ObsPoint, out: &mut StepOutput) {
        let t = track.window_type;
        out.events.push(NewEvent {
            event_key: event_key(f.fp, Some(t), QuotaEventType::BackendSnapshotReconciliation, f.obs.snapshot_id),
            account_fingerprint: Some(f.fp.to_string()),
            window_type: Some(t),
            event_type: QuotaEventType::BackendSnapshotReconciliation,
            observed_at_ms: f.obs.captured_at_ms,
            previous_snapshot_id: lc.post_points.last().copied(),
            current_snapshot_id: f.obs.snapshot_id,
            confidence: EvidenceConfidence::Medium,
            confirmed: true,
            evidence: json!({
                "revertedEventKey": lc.event_key,
                "revertedEventType": lc.event_type.as_str(),
                "pre": point_json(&lc.pre),
                "current": point_json(cur),
                "pollsSinceConfirmation": lc.polls_since,
                "postSnapshots": lc.post_points,
            }),
            supersedes_event_key: Some(lc.event_key.clone()),
        });
        for id in &lc.post_points {
            out.quality_updates.push(QualityUpdate { snapshot_id: *id, window_type: t, quality: SnapshotQuality::SuspectTransient });
        }
        if let Some((old, new)) = &lc.boundary {
            out.cycle_ops.push(CycleOp::Void { cycle_key: new.key.clone(), reason: "backend_snapshot_reconciliation".into() });
            if let Some(old) = old {
                out.cycle_ops.push(CycleOp::Reopen { cycle_key: old.key.clone() });
            }
            track.cycle = old.clone();
        }
        track.history = lc.history_before.clone();
        track.reset_run = lc.reset_run_before;
        track.segment_start_snapshot_id = lc.segment_before;
        track.active_origin_ms = lc.origin_before;
        track.last_confirmed = None;
        if !sliding_like(cur, &self.cfg) {
            // v3: a reverted BecameIdle confirmation had already declared the sliding anchor.
            track.sliding = false;
            track.sliding_reported = false;
            track.awaiting_usage = false;
        }
        self.accept(track, cur.clone(), out);
    }

    #[allow(clippy::too_many_arguments)]
    fn upsert_candidate(
        &self,
        fp: &str,
        t: WindowType,
        cand: &Candidate,
        state: ResetCandidateState,
        resolved_at_ms: Option<i64>,
        resolution_event_key: Option<String>,
        out: &mut StepOutput,
    ) {
        out.candidates.push(CandidateRecord {
            candidate_key: cand.key.clone(),
            account_fingerprint: fp.to_string(),
            window_type: Some(t),
            kind: cand.kind.as_str().into(),
            state,
            opened_at_ms: cand.points[0].captured_at_ms,
            opened_snapshot_id: cand.points[0].snapshot_id,
            baseline_snapshot_id: Some(cand.baseline.snapshot_id),
            last_snapshot_id: cand.points.last().map(|p| p.snapshot_id).unwrap_or(cand.points[0].snapshot_id),
            resolved_at_ms,
            resolution_event_key,
            evidence: serde_json::to_value(&cand.opened_evidence).unwrap_or(Value::Null),
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn resolve_candidate(
        &self,
        fp: &str,
        t: WindowType,
        cand: &Candidate,
        state: ResetCandidateState,
        obs: &Observation,
        key: Option<String>,
        out: &mut StepOutput,
    ) {
        self.upsert_candidate(fp, t, cand, state, Some(obs.captured_at_ms), key, out);
    }

    // ------------------------------------------------------------------ helpers

    /// v2. The window was idle (sliding anchor, 0 % since) and this observation shows usage with a
    /// resetsAt whose derived start (`resetsAt - duration`) lies within the last window length: the
    /// active window began after the idle period. Restart the burn-rate analysis segment here, with
    /// the derived start as its 0 % origin. Same ObservedCycle, no event, no candidate.
    fn reanchor_after_idle(&self, track: &mut WindowTrack, reference: &ObsPoint, cur: &ObsPoint, out: &mut StepOutput) -> Option<String> {
        if !track.awaiting_usage || reference.used.is_some_and(|u| u > 0.0) {
            return None;
        }
        let (Some(used), Some(reset), Some(duration)) = (cur.used, cur.reset_at_ms, cur.duration_mins) else { return None };
        if used <= 0.0 || reference.reset_at_ms.is_some_and(|r| cur.captured_at_ms >= r) {
            return None;
        }
        let origin = reset - duration * 60_000;
        let tol = self.cfg.sliding_anchor_tolerance_secs * 1000;
        if origin > cur.captured_at_ms + tol || origin < cur.captured_at_ms - duration * 60_000 - tol {
            return None;
        }
        let from = track.segment_start_snapshot_id;
        track.segment_start_snapshot_id = Some(cur.snapshot_id);
        track.active_origin_ms = Some(origin);
        track.awaiting_usage = false;
        out.log_lines.push(format!(
            "analysis segment re-anchored window={} fromSnapshot={} toSnapshot={} nominalActiveWindowStartMs={} (derived: resetsAt - duration, not a server timestamp)",
            track.window_type.as_str(),
            from.map(|v| v.to_string()).unwrap_or_else(|| "none".into()),
            cur.snapshot_id,
            origin
        ));
        Some("usage_resumed_segment_reanchored".into())
    }

    fn accept(&self, track: &mut WindowTrack, p: ObsPoint, out: &mut StepOutput) {
        if p.used.is_some_and(|u| u > 0.0) {
            track.awaiting_usage = false;
        }
        track.reset_run = match track.reset_run {
            Some(r) if r.reset_at_ms == p.reset_at_ms => Some(ResetRun { reset_at_ms: r.reset_at_ms, count: r.count + 1 }),
            _ => Some(ResetRun { reset_at_ms: p.reset_at_ms, count: 1 }),
        };
        if let Some(lc) = track.last_confirmed.as_mut()
            && !lc.post_points.contains(&p.snapshot_id)
        {
            lc.post_points.push(p.snapshot_id);
        }
        track.history.push_back(p.clone());
        while track.history.len() > self.cfg.history_len {
            track.history.pop_front();
        }
        if let Some(c) = &track.cycle {
            let nominal = self.anchor(track).zip(p.duration_mins).map(|((reset, _), d)| reset - d * 60_000);
            out.cycle_ops.push(CycleOp::Extend {
                cycle_key: c.key.clone(),
                last_snapshot_id: p.snapshot_id,
                last_seen_at_ms: p.captured_at_ms,
                final_used_percent: p.used,
                final_reset_at_ms: p.reset_at_ms,
                nominal_window_start_ms: nominal,
            });
        }
    }

    fn rebuild_reset_run(&self, track: &mut WindowTrack) {
        track.reset_run = track.history.back().map(|last| ResetRun {
            reset_at_ms: last.reset_at_ms,
            count: track.history.iter().rev().take_while(|p| p.reset_at_ms == last.reset_at_ms).count() as u32,
        });
    }

    /// Confirmed reset anchor: same resetsAt in >= `schedule_confirm_polls` consecutive accepted
    /// observations, and not a sliding idle anchor. v3: not while a pending observation shows the
    /// window idle (it contradicts the accepted reset time until resolved either way).
    pub fn anchor(&self, track: &WindowTrack) -> Option<(i64, u32)> {
        if track.sliding {
            return None;
        }
        if track.pending.as_ref().and_then(|c| c.points.last()).is_some_and(|p| sliding_like(p, &self.cfg)) {
            return None;
        }
        let run = track.reset_run?;
        let last = track.history.back()?;
        if sliding_like(last, &self.cfg) {
            return None;
        }
        (run.count >= self.cfg.schedule_confirm_polls).then_some(()).and(run.reset_at_ms.map(|r| (r, run.count)))
    }

    fn derive_state(&self, track: &WindowTrack, present: bool) -> WindowObservationState {
        if !present {
            return if track.missing_reported { WindowObservationState::Unknown } else { WindowObservationState::TemporarilyMissing };
        }
        if let Some(c) = &track.pending
            && c.kind == CandidateKind::ScheduleChange
        {
            return WindowObservationState::ScheduleChangeCandidate;
        }
        let latest = track.pending.as_ref().and_then(|c| c.points.last()).or(track.history.back());
        let Some(p) = latest else { return WindowObservationState::Unknown };
        if let Some(r) = p.reset_at_ms
            && p.captured_at_ms >= r
        {
            return WindowObservationState::ExpiredUnconfirmed;
        }
        match p.used {
            Some(0.0) => {
                if track.sliding || sliding_like(p, &self.cfg) || self.anchor(track).is_none() {
                    WindowObservationState::IdleOrUninitialized
                } else {
                    WindowObservationState::StableSchedule
                }
            }
            Some(_) if p.reset_at_ms.is_some() => WindowObservationState::Active,
            _ => WindowObservationState::Unknown,
        }
    }

    pub fn views(&self, ctx: &AccountContext) -> Vec<WindowTrackView> {
        WindowType::ALL
            .iter()
            .map(|t| {
                let tr = &ctx.windows[t];
                let anchor = self.anchor(tr);
                WindowTrackView {
                    account_fingerprint: ctx.account_fingerprint.clone(),
                    window_type: *t,
                    observation_state: tr.state,
                    candidate_state: if tr.pending.is_some() { ResetCandidateState::Pending } else { tr.candidate_state },
                    confirmed_reset_at_ms: anchor.map(|a| a.0),
                    anchor_polls: anchor.map(|a| a.1).unwrap_or(0),
                    sliding_anchor: tr.sliding,
                    missing_streak: tr.missing_streak,
                    current_cycle_key: tr.cycle.as_ref().map(|c| c.key.clone()),
                    analysis_segment_start_snapshot_id: tr.segment_start_snapshot_id,
                    active_origin_ms: tr.active_origin_ms,
                    last_snapshot_id: tr.last_snapshot_id,
                }
            })
            .collect()
    }
}

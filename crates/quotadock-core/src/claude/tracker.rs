//! Claude window instances: which quota period an observation belongs to. Deliberately simpler than
//! the Codex reset detector: there is no Claude evidence history yet, so it only
//!
//! - keeps an observation in the open instance while its resetsAt stays within
//!   `SAME_INSTANCE_TOLERANCE_MS` (the two sources differ in precision: seconds vs minutes);
//! - opens a new instance when the previous reset time has passed and a later one appears
//!   (QuotaRefreshedObserved, user-visible) or when the reset moved and usage dropped substantially
//!   before it was due (QuotaWindowChanged, diagnostic);
//! - records LimitReachedObserved (diagnostic) once per instance when usage reaches 100 %.
//!
//! It never infers availability, a promotion, or a reset from a reset time alone. [QUOTADOCK HEURISTIC]

use super::model::*;
use serde::Serialize;
use serde_json::{Value, json};

pub const SAME_INSTANCE_TOLERANCE_MS: i64 = 15 * 60_000;
pub const SUBSTANTIAL_DROP_PP: f64 = 10.0;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenInstance {
    pub instance_key: String,
    pub window_key: String,
    pub resets_at_ms: Option<i64>,
    pub last_used_percent: Option<f64>,
    pub limit_reported: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderEventDraft {
    pub event_type: &'static str,
    pub window_key: Option<String>,
    pub visibility: &'static str,
    pub confidence: &'static str,
    pub evidence: Value,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct TrackOutput {
    /// window_key -> instance key for this observation.
    pub instance_for: Vec<(String, String)>,
    /// Instances opened by this observation (window_key, key, reason).
    pub opened: Vec<(String, String, &'static str)>,
    pub closed: Vec<String>,
    /// (instance key, new resetsAt) for in-instance precision refinements.
    pub reset_updates: Vec<(String, Option<i64>)>,
    pub limit_reported: Vec<String>,
    pub events: Vec<ProviderEventDraft>,
}

pub fn instance_key(account: Option<&str>, window_key: &str, snapshot_id: i64) -> String {
    format!("claude|{}|{window_key}|{snapshot_id}", account.map(|a| &a[..a.len().min(8)]).unwrap_or("-"))
}

/// `open`: currently open instances for this provider/account; `snapshot_id`: the stored observation.
pub fn track(open: &[OpenInstance], obs: &ClaudeObservation, account: Option<&str>, snapshot_id: i64) -> TrackOutput {
    let mut out = TrackOutput::default();
    for w in &obs.windows {
        let current = open.iter().find(|i| i.window_key == w.key);
        let open_new = |reason: &'static str, out: &mut TrackOutput| {
            let key = instance_key(account, &w.key, snapshot_id);
            out.opened.push((w.key.clone(), key.clone(), reason));
            out.instance_for.push((w.key.clone(), key.clone()));
            key
        };
        let key = match current {
            None => open_new("first_observation", &mut out),
            Some(cur) => {
                let moved = match (cur.resets_at_ms, w.resets_at_ms) {
                    (Some(a), Some(b)) => (b - a).abs() > SAME_INSTANCE_TOLERANCE_MS,
                    _ => false,
                };
                let passed = cur.resets_at_ms.is_some_and(|r| obs.captured_at_ms >= r - w.reset_precision_ms);
                let later = matches!((cur.resets_at_ms, w.resets_at_ms), (Some(a), Some(b)) if b > a + SAME_INSTANCE_TOLERANCE_MS);
                let drop = cur.last_used_percent.map(|u| u - w.used_percent).unwrap_or(0.0);
                if moved && passed && later {
                    out.closed.push(cur.instance_key.clone());
                    out.events.push(ProviderEventDraft {
                        event_type: "quota_refreshed_observed",
                        window_key: Some(w.key.clone()),
                        visibility: "user",
                        confidence: "medium",
                        evidence: json!({
                            "previousResetsAtMs": cur.resets_at_ms, "newResetsAtMs": w.resets_at_ms,
                            "usedBefore": cur.last_used_percent, "usedAfter": w.used_percent,
                            "source": obs.source.as_str(), "note": "previous reset time had passed; cause not inferred",
                        }),
                    });
                    open_new("refreshed_after_reset", &mut out)
                } else if moved && drop >= SUBSTANTIAL_DROP_PP {
                    out.closed.push(cur.instance_key.clone());
                    out.events.push(ProviderEventDraft {
                        event_type: "quota_window_changed",
                        window_key: Some(w.key.clone()),
                        visibility: "diagnostic",
                        confidence: "low",
                        evidence: json!({
                            "previousResetsAtMs": cur.resets_at_ms, "newResetsAtMs": w.resets_at_ms,
                            "usedBefore": cur.last_used_percent, "usedAfter": w.used_percent,
                            "source": obs.source.as_str(), "note": "reset moved before it was due and usage dropped; cause unknown",
                        }),
                    });
                    open_new("window_changed", &mut out)
                } else {
                    if moved {
                        // Drift without a drop: same period, diagnostic only.
                        out.events.push(ProviderEventDraft {
                            event_type: "quota_window_changed",
                            window_key: Some(w.key.clone()),
                            visibility: "diagnostic",
                            confidence: "low",
                            evidence: json!({ "previousResetsAtMs": cur.resets_at_ms, "newResetsAtMs": w.resets_at_ms, "usedAfter": w.used_percent, "kept": "same instance" }),
                        });
                    }
                    // A more precise (statusline, seconds) reset time refines the instance.
                    if w.resets_at_ms.is_some()
                        && (w.reset_precision_ms <= 1000 || cur.resets_at_ms.is_none())
                        && w.resets_at_ms != cur.resets_at_ms
                    {
                        out.reset_updates.push((cur.instance_key.clone(), w.resets_at_ms));
                    }
                    out.instance_for.push((w.key.clone(), cur.instance_key.clone()));
                    cur.instance_key.clone()
                }
            }
        };
        let already = current.is_some_and(|c| c.instance_key == key && c.limit_reported);
        if w.used_percent >= 100.0 && !already {
            out.limit_reported.push(key.clone());
            out.events.push(ProviderEventDraft {
                event_type: "limit_reached_observed",
                window_key: Some(w.key.clone()),
                visibility: "diagnostic",
                confidence: "medium",
                evidence: json!({ "usedPercent": w.used_percent, "note": "usage reached 100 %; availability is not inferred" }),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: i64 = 3_600_000;

    fn obs(at: i64, windows: Vec<(&str, f64, Option<i64>, i64)>) -> ClaudeObservation {
        ClaudeObservation {
            source: ClaudeSource::StatusLine,
            captured_at_ms: at,
            claude_version: None,
            billing: Billing::Subscription,
            windows: windows
                .into_iter()
                .map(|(k, u, r, p)| ClaudeWindowObs {
                    key: k.into(),
                    kind: if k == FIVE_HOUR_KEY { WindowKind::FiveHour } else { WindowKind::Weekly },
                    label: None,
                    duration_mins: None,
                    used_percent: u,
                    resets_at_ms: r,
                    reset_precision_ms: p,
                })
                .collect(),
            raw_sanitized: Value::Null,
        }
    }

    fn inst(key: &str, w: &str, r: Option<i64>, used: f64) -> OpenInstance {
        OpenInstance {
            instance_key: key.into(),
            window_key: w.into(),
            resets_at_ms: r,
            last_used_percent: Some(used),
            limit_reported: false,
        }
    }

    #[test]
    fn first_observation_opens_instances_without_events() {
        let o = track(
            &[],
            &obs(0, vec![(FIVE_HOUR_KEY, 6.0, Some(4 * H), 1000), (WEEKLY_KEY, 29.0, Some(90 * H), 1000)]),
            Some("abcdef0123"),
            1,
        );
        assert_eq!(o.opened.len(), 2);
        assert!(o.events.is_empty());
        assert_eq!(o.instance_for[0].1, "claude|abcdef01|five_hour|1");
    }

    #[test]
    fn minute_precision_from_the_cli_stays_in_the_same_instance() {
        let open = [inst("k5", FIVE_HOUR_KEY, Some(4 * H + 59_000), 6.0)];
        let o = track(&open, &obs(H, vec![(FIVE_HOUR_KEY, 7.0, Some(4 * H), 60_000)]), None, 2);
        assert!(o.opened.is_empty() && o.events.is_empty());
        assert!(o.reset_updates.is_empty(), "a minute-precision value never overwrites a second-precision one");
        assert_eq!(o.instance_for, vec![(FIVE_HOUR_KEY.to_string(), "k5".to_string())]);
    }

    #[test]
    fn refresh_after_the_reset_time_is_a_user_visible_event_and_a_new_instance() {
        let open = [inst("k5", FIVE_HOUR_KEY, Some(4 * H), 80.0)];
        let o = track(&open, &obs(4 * H + 10 * 60_000, vec![(FIVE_HOUR_KEY, 2.0, Some(9 * H), 1000)]), None, 9);
        assert_eq!(o.closed, vec!["k5".to_string()]);
        assert_eq!(o.events.len(), 1);
        assert_eq!((o.events[0].event_type, o.events[0].visibility), ("quota_refreshed_observed", "user"));
        assert_eq!(o.opened[0].2, "refreshed_after_reset");
    }

    #[test]
    fn a_reset_time_change_alone_is_not_a_refresh() {
        let open = [inst("kw", WEEKLY_KEY, Some(90 * H), 30.0)];
        let o = track(&open, &obs(H, vec![(WEEKLY_KEY, 30.0, Some(92 * H), 1000)]), None, 3);
        assert!(o.opened.is_empty() && o.closed.is_empty(), "same instance");
        assert_eq!(o.events.len(), 1);
        assert_eq!((o.events[0].event_type, o.events[0].visibility), ("quota_window_changed", "diagnostic"));
    }

    #[test]
    fn early_move_with_a_substantial_drop_opens_a_new_instance_quietly() {
        let open = [inst("kw", WEEKLY_KEY, Some(90 * H), 60.0)];
        let o = track(&open, &obs(H, vec![(WEEKLY_KEY, 5.0, Some(160 * H), 1000)]), None, 4);
        assert_eq!(o.closed, vec!["kw".to_string()]);
        assert_eq!(o.events[0].visibility, "diagnostic", "cause unknown: never a user-visible refresh claim");
    }

    #[test]
    fn limit_reached_is_reported_once_and_never_as_availability() {
        let open = [inst("k5", FIVE_HOUR_KEY, Some(4 * H), 99.0)];
        let o = track(&open, &obs(H, vec![(FIVE_HOUR_KEY, 100.0, Some(4 * H), 1000)]), None, 5);
        assert_eq!(o.limit_reported, vec!["k5".to_string()]);
        assert_eq!(o.events[0].visibility, "diagnostic");
        let mut again = open.clone();
        again[0].limit_reported = true;
        assert!(track(&again, &obs(H, vec![(FIVE_HOUR_KEY, 100.0, Some(4 * H), 1000)]), None, 6).events.is_empty());
    }
}

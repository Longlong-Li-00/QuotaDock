//! Windows V1 pre-soak hardening: health log, watchdog (stuck database), shadow estimator.
//! Uses a temporary data folder only.

use quotadock_core::analytics::burn_rate::{BurnPoint, origin_slope_per_hour, shadow_rates};
use quotadock_core::claude::monitor::ClaudeMonitor;
use quotadock_core::pipeline::{CoreConfig, QuotaDockCore};
use quotadock_core::product::runtime::ProductRuntime;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

fn runtime() -> (tempfile::TempDir, Arc<ProductRuntime>) {
    let dir = tempfile::tempdir().unwrap();
    let core = Arc::new(QuotaDockCore::open(CoreConfig::with_database(dir.path().join("q.sqlite3")), Arc::new(|_| {})).unwrap());
    let rt = ProductRuntime::with_claude(core, ClaudeMonitor::new(dir.path(), &dir.path().join("home"))).unwrap();
    (dir, rt)
}

fn health_lines(rt: &ProductRuntime) -> Vec<Value> {
    std::fs::read_to_string(rt.health().path()).unwrap_or_default().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

fn kinds(rt: &ProductRuntime) -> Vec<String> {
    health_lines(rt).iter().map(|l| l["kind"].as_str().unwrap_or_default().to_string()).collect()
}

#[tokio::test]
async fn lifecycle_pause_and_window_changes_are_logged_next_to_the_database() {
    let (dir, rt) = runtime();
    rt.record_start("hidden", true);
    rt.note_foreground(true);
    rt.note_foreground(true); // unchanged: not logged again
    rt.note_foreground(false);
    rt.pause().await;
    rt.resume();
    rt.record_stop("quit");
    assert!(rt.health().path().starts_with(dir.path()), "logs\\health.jsonl in the data folder");
    assert_eq!(kinds(&rt), vec!["app_start", "foreground", "foreground", "paused", "paused", "app_stop"]);
    let start = &health_lines(&rt)[0];
    assert_eq!((start["launch"].as_str(), start["relaunched"].as_bool()), (Some("hidden"), Some(true)));
}

#[test]
fn the_watchdog_samples_resources_and_reports_a_stuck_database() {
    let (_dir, rt) = runtime();
    rt.watchdog_tick(false);
    let first = health_lines(&rt);
    assert_eq!(first.len(), 1, "one resource sample at the first tick");
    assert_eq!(first[0]["kind"], "resources");
    assert!(first[0]["app"]["ws"].as_u64().unwrap_or(0) > 0 || cfg!(not(windows)));
    // Hold the database the way a deadlock would, and tick three times.
    let core = rt.core.clone();
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let holder = std::thread::spawn(move || {
        let _ = core.with_db(|_| {
            let _ = rx.recv_timeout(Duration::from_secs(10));
            Ok(())
        });
    });
    std::thread::sleep(Duration::from_millis(200));
    for _ in 0..3 {
        rt.watchdog_tick(false);
    }
    assert!(kinds(&rt).contains(&"db_busy".to_string()), "{:?}", kinds(&rt));
    tx.send(()).unwrap();
    holder.join().unwrap();
    rt.watchdog_tick(false);
    assert_eq!(kinds(&rt).last().map(String::as_str), Some("db_busy_end"));
    // Without polling attached there is no expected cadence, so no MONITORING_GAP.
    assert!(!kinds(&rt).contains(&"monitoring_gap".to_string()));
}

/// Real run 2026-10-08: Codex 5h went 0 -> 6 -> 12 -> 13 % within ~11 min of the derived origin
/// (22:44:03), then stayed at 13 % for 75 min. The production fit (through the origin) read ~12 pp/h;
/// the average since the origin was ~8.8 pp/h and the recent slope 0.
#[test]
fn shadow_estimates_compare_the_production_fit_with_simpler_rates() {
    let origin = 0i64;
    let min = 60_000i64;
    let mut points = vec![
        BurnPoint { captured_at_ms: origin + 107 * min / 60, used_percent: 6.0 },
        BurnPoint { captured_at_ms: origin + 408 * min / 60, used_percent: 12.0 },
        BurnPoint { captured_at_ms: origin + 709 * min / 60, used_percent: 13.0 },
    ];
    let mut t = 709 * min / 60;
    while t < 88 * min {
        t += 5 * min;
        points.push(BurnPoint { captured_at_ms: origin + t, used_percent: 13.0 });
    }
    let production = origin_slope_per_hour(origin, &points);
    let s = shadow_rates(origin, &points, production);
    let (p, avg, recent) = (s.production.unwrap(), s.average_since_origin.unwrap(), s.recent_30m.unwrap());
    assert!(p > avg * 1.2, "front-loaded usage: the origin fit reads high ({p:.1} vs average {avg:.1})");
    assert!((avg - 13.0 / (t as f64 / 3_600_000.0)).abs() < 1e-9);
    assert_eq!(recent, 0.0, "flat for the last 30 min");
    // Too little recent data: no recent slope rather than a guess.
    let short = shadow_rates(origin, &points[..2], None);
    assert_eq!(short.recent_30m, None);
}

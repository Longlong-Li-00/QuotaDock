//! Polling controller under tokio's paused clock.

use quotadock_core::pipeline::PollTrigger;
use quotadock_core::polling::controller::{PollFn, backoff_delay};
use quotadock_core::polling::{PollingConfig, RefreshDecision, spawn_polling, validate_interval};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::Instant;

type Log = Arc<Mutex<Vec<(Duration, PollTrigger)>>>;

fn recorder(outcomes: Vec<bool>, delay: Duration) -> (PollFn, Log) {
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let start = Instant::now();
    let outcomes = Arc::new(Mutex::new(outcomes));
    let l = log.clone();
    let f: PollFn = Arc::new(move |trigger| {
        let l = l.clone();
        let outcomes = outcomes.clone();
        Box::pin(async move {
            l.lock().unwrap().push((start.elapsed(), trigger));
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            let mut o = outcomes.lock().unwrap();
            if o.is_empty() { true } else { o.remove(0) }
        })
    });
    (f, log)
}

async fn advance(d: Duration) {
    tokio::time::advance(d).await;
    tokio::task::yield_now().await;
    tokio::task::yield_now().await;
}

fn secs(log: &Log) -> Vec<u64> {
    log.lock().unwrap().iter().map(|(d, _)| d.as_secs()).collect()
}

#[tokio::test(start_paused = true)]
async fn immediate_first_poll_then_five_minutes() {
    let (f, log) = recorder(vec![], Duration::ZERO);
    let h = spawn_polling(f, PollingConfig::default());
    advance(Duration::from_millis(1)).await;
    advance(Duration::from_secs(300)).await;
    advance(Duration::from_secs(300)).await;
    assert_eq!(secs(&log), vec![0, 300, 600]);
    assert_eq!(log.lock().unwrap()[0].1, PollTrigger::Startup);
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn backoff_1_2_5_10_15_then_back_to_normal() {
    let (f, log) = recorder(vec![false, false, false, false, false, false, true], Duration::ZERO);
    let h = spawn_polling(f, PollingConfig::default());
    advance(Duration::from_millis(1)).await;
    for step in [60, 120, 300, 600, 900, 900, 300] {
        advance(Duration::from_secs(step)).await;
    }
    let s = secs(&log);
    let gaps: Vec<u64> = s.windows(2).map(|w| w[1] - w[0]).collect();
    assert_eq!(gaps, vec![60, 120, 300, 600, 900, 900, 300]);
    assert_eq!(h.status().consecutive_failures, 0);
    h.stop().await;
    assert_eq!(backoff_delay(42), Duration::from_secs(900), "15 min cap");
}

#[tokio::test(start_paused = true)]
async fn manual_refresh_is_throttled_and_coalesced() {
    let (f, log) = recorder(vec![], Duration::from_secs(2));
    let h = spawn_polling(f, PollingConfig::default());
    advance(Duration::from_millis(1)).await; // startup poll running (2 s)
    advance(Duration::from_secs(3)).await;
    assert_eq!(h.refresh_now().await, RefreshDecision::Throttled, "within 10 s of the last start");
    advance(Duration::from_secs(10)).await;
    let decision = h.refresh_now().await;
    assert_eq!(decision, RefreshDecision::Started);
    // While that manual read runs, more requests are served by it.
    let h2 = h.clone();
    let pending = tokio::spawn(async move { h2.refresh_now().await });
    advance(Duration::from_secs(3)).await;
    assert_eq!(pending.await.unwrap(), RefreshDecision::Coalesced);
    assert_eq!(log.lock().unwrap().iter().filter(|(_, t)| *t == PollTrigger::Manual).count(), 1);
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn foreground_uses_sixty_seconds() {
    let (f, log) = recorder(vec![], Duration::ZERO);
    let h = spawn_polling(f, PollingConfig::default());
    advance(Duration::from_millis(1)).await;
    h.set_foreground(true);
    advance(Duration::from_secs(60)).await;
    advance(Duration::from_secs(60)).await;
    assert_eq!(secs(&log), vec![0, 60, 120]);
    h.stop().await;
}

/// Real run 2026-10-09: the window was shown and then hidden right after launch, and the
/// "immediate" startup read waited a whole interval (tokio::select! took the command first).
#[tokio::test(start_paused = true)]
async fn window_changes_before_the_first_read_do_not_delay_it() {
    for _ in 0..20 {
        let (f, log) = recorder(vec![], Duration::ZERO);
        let h = spawn_polling(f, PollingConfig::default());
        h.set_foreground(true);
        h.set_foreground(false);
        advance(Duration::from_millis(1)).await;
        assert_eq!(secs(&log), vec![0], "the startup read is immediate");
        assert_eq!(log.lock().unwrap()[0].1, PollTrigger::Startup);
        advance(Duration::from_secs(300)).await;
        assert_eq!(secs(&log), vec![0, 300], "then the background interval");
        h.stop().await;
    }
}

#[tokio::test(start_paused = true)]
async fn pause_stops_scheduling_and_resume_polls_immediately() {
    let (f, log) = recorder(vec![], Duration::ZERO);
    let h = spawn_polling(f, PollingConfig::default());
    advance(Duration::from_millis(1)).await;
    assert_eq!(secs(&log), vec![0]);
    h.pause().await;
    assert!(h.status().paused);
    assert_eq!(h.refresh_now().await, RefreshDecision::Paused, "manual refresh makes no request while paused");
    advance(Duration::from_secs(3600)).await;
    assert_eq!(secs(&log), vec![0], "an hour paused: no reads at all");
    h.resume();
    advance(Duration::from_millis(1)).await;
    assert_eq!(secs(&log), vec![0, 3600], "resume reads immediately");
    advance(Duration::from_secs(300)).await;
    assert_eq!(secs(&log), vec![0, 3600, 3900], "then the normal 5-minute schedule");
    assert!(!h.status().paused);
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_pause_then_resume_during_a_read_ends_running_with_the_resume_read() {
    let (f, log) = recorder(vec![], Duration::from_secs(10));
    let h = spawn_polling(f, PollingConfig::default());
    advance(Duration::from_millis(1)).await;
    assert_eq!(secs(&log), vec![0], "the startup read is running for 10 s");
    let h2 = h.clone();
    let pause = tokio::spawn(async move { h2.pause().await });
    tokio::task::yield_now().await;
    h.resume();
    advance(Duration::from_secs(10)).await;
    pause.await.unwrap();
    advance(Duration::from_millis(1)).await;
    assert!(!h.status().paused, "the Resume after the Pause is not dropped");
    assert_eq!(secs(&log), vec![0, 10], "and it reads at once");
    assert_eq!(log.lock().unwrap()[1].1, PollTrigger::Manual);
    advance(Duration::from_secs(10)).await; // the resume read ends at 20
    advance(Duration::from_secs(300)).await;
    assert_eq!(secs(&log), vec![0, 10, 320], "then the normal schedule");

    // Resume then Pause during a read ends paused.
    assert_eq!(secs(&log).len(), 3, "the read at 320 is running");
    h.resume();
    let h3 = h.clone();
    let pause = tokio::spawn(async move { h3.pause().await });
    advance(Duration::from_secs(10)).await;
    pause.await.unwrap();
    advance(Duration::from_secs(3600)).await;
    assert!(h.status().paused);
    assert_eq!(secs(&log).len(), 3, "paused: no further read");
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn start_paused_makes_no_read_until_resume() {
    let (f, log) = recorder(vec![], Duration::ZERO);
    let h = spawn_polling(f, PollingConfig { start_paused: true, ..Default::default() });
    advance(Duration::from_secs(3600)).await;
    assert!(secs(&log).is_empty(), "not even the startup read");
    assert!(h.status().paused);
    assert_eq!(h.refresh_now().await, RefreshDecision::Paused);
    h.resume();
    advance(Duration::from_millis(1)).await;
    assert_eq!(secs(&log), vec![3600], "resume reads immediately");
    advance(Duration::from_secs(300)).await;
    assert_eq!(secs(&log), vec![3600, 3900], "then the normal 5-minute schedule");
    assert!(!h.status().paused);
    h.stop().await;
}

#[test]
fn interval_validation() {
    assert!(validate_interval(Duration::from_secs(29), false).is_err());
    assert!(validate_interval(Duration::from_secs(30), false).is_ok());
    assert!(validate_interval(Duration::from_secs(10), true).is_ok());
    assert!(validate_interval(Duration::from_secs(5), true).is_err(), "never per-second polling");
}

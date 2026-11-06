use crate::pipeline::PollTrigger;
use serde::Serialize;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Instant, sleep_until};

pub const DEFAULT_BACKGROUND_INTERVAL: Duration = Duration::from_secs(300);
pub const DEFAULT_FOREGROUND_INTERVAL: Duration = Duration::from_secs(60);
/// Lowest interval accepted without the explicit unsafe development flag.
pub const MIN_SAFE_INTERVAL: Duration = Duration::from_secs(30);
/// Absolute floor even with the unsafe development flag.
pub const UNSAFE_FLOOR: Duration = Duration::from_secs(10);
/// Manual refreshes closer together than this are throttled.
pub const MIN_MANUAL_GAP: Duration = Duration::from_secs(10);
/// 1, 2, 5, 10, then 15 minutes (cap).
pub const BACKOFF: [Duration; 5] =
    [Duration::from_secs(60), Duration::from_secs(120), Duration::from_secs(300), Duration::from_secs(600), Duration::from_secs(900)];

pub fn backoff_delay(consecutive_failures: u32) -> Duration {
    if consecutive_failures == 0 {
        return Duration::ZERO;
    }
    BACKOFF[(consecutive_failures as usize - 1).min(BACKOFF.len() - 1)]
}

/// Ok(interval) or an explanation. Anything below 5 min is development/foreground use.
pub fn validate_interval(requested: Duration, allow_unsafe_dev_poll: bool) -> Result<Duration, String> {
    let floor = if allow_unsafe_dev_poll { UNSAFE_FLOOR } else { MIN_SAFE_INTERVAL };
    if requested < floor {
        return Err(format!(
            "interval must be at least {} s{}",
            floor.as_secs(),
            if allow_unsafe_dev_poll { "" } else { " (--unsafe-dev-poll lowers this to 10 s, development only)" }
        ));
    }
    Ok(requested)
}

#[derive(Debug, Clone, Copy)]
pub struct PollingConfig {
    pub background_interval: Duration,
    pub foreground_interval: Duration,
    /// Begin paused (Codex turned off in Settings): the first read waits for `resume`.
    pub start_paused: bool,
}

impl Default for PollingConfig {
    fn default() -> Self {
        Self { background_interval: DEFAULT_BACKGROUND_INTERVAL, foreground_interval: DEFAULT_FOREGROUND_INTERVAL, start_paused: false }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RefreshDecision {
    Started,
    /// A read was already running or queued; it will serve this request.
    Coalesced,
    /// A read started less than MIN_MANUAL_GAP ago.
    Throttled,
    /// Monitoring is paused: no backend request is made.
    Paused,
    Stopped,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PollingStatus {
    pub running: bool,
    pub paused: bool,
    pub foreground: bool,
    pub consecutive_failures: u32,
    pub polls: u64,
    pub last_success: Option<bool>,
    pub next_delay_ms: Option<u64>,
    /// Wall-clock start / end of the latest read (watchdog: a read that never finishes, or no read).
    pub last_started_ms: Option<i64>,
    pub last_finished_ms: Option<i64>,
}

pub type PollFn = Arc<dyn Fn(PollTrigger) -> Pin<Box<dyn Future<Output = bool> + Send>> + Send + Sync>;

enum Command {
    Refresh(oneshot::Sender<RefreshDecision>),
    Foreground(bool),
    /// Acknowledged once no read is in flight; nothing is scheduled until Resume.
    Pause(oneshot::Sender<()>),
    Resume,
    Stop(oneshot::Sender<()>),
}

#[derive(Clone)]
pub struct PollingHandle {
    tx: mpsc::UnboundedSender<Command>,
    status: Arc<Mutex<PollingStatus>>,
}

impl PollingHandle {
    pub async fn refresh_now(&self) -> RefreshDecision {
        let (tx, rx) = oneshot::channel();
        if self.tx.send(Command::Refresh(tx)).is_err() {
            return RefreshDecision::Stopped;
        }
        rx.await.unwrap_or(RefreshDecision::Stopped)
    }

    pub fn set_foreground(&self, foreground: bool) {
        let _ = self.tx.send(Command::Foreground(foreground));
    }

    /// Stops scheduling reads. Returns after any in-flight read has finished, so the caller can
    /// then safely stop the provider (app-server).
    pub async fn pause(&self) {
        let (tx, rx) = oneshot::channel();
        if self.tx.send(Command::Pause(tx)).is_ok() {
            let _ = rx.await;
        }
    }

    /// Resumes with an immediate read, then the normal schedule (or backoff after a failure).
    pub fn resume(&self) {
        let _ = self.tx.send(Command::Resume);
    }

    pub async fn stop(&self) {
        let (tx, rx) = oneshot::channel();
        if self.tx.send(Command::Stop(tx)).is_ok() {
            let _ = rx.await;
        }
    }

    pub fn status(&self) -> PollingStatus {
        self.status.lock().unwrap().clone()
    }
}

/// Starts the loop with an immediate first read (unless `start_paused`). `poll` returns true on success.
pub fn spawn_polling(poll: PollFn, cfg: PollingConfig) -> PollingHandle {
    let (tx, mut rx) = mpsc::unbounded_channel::<Command>();
    let status = Arc::new(Mutex::new(PollingStatus { running: true, paused: cfg.start_paused, ..Default::default() }));
    let st = status.clone();
    tokio::spawn(async move {
        let mut deadline = Instant::now();
        let mut foreground = false;
        let mut failures: u32 = 0;
        let mut last_started: Option<Instant> = None;
        let mut trigger = PollTrigger::Startup;
        let mut paused = cfg.start_paused;
        loop {
            // While paused only commands wake the loop: no read is ever scheduled.
            let (is_paused, until) = (paused, deadline);
            let wait = async move { if is_paused { std::future::pending::<()>().await } else { sleep_until(until).await } };
            tokio::select! {
                _ = wait => {}
                cmd = rx.recv() => {
                    match cmd {
                        None => break,
                        Some(Command::Stop(done)) => { let _ = done.send(()); break; }
                        Some(Command::Foreground(f)) => {
                            foreground = f;
                            st.lock().unwrap().foreground = f;
                            // Before the first read the deadline stays "now": showing or hiding the
                            // window at launch must not postpone the startup read by an interval.
                            if let Some(base) = last_started.filter(|_| failures == 0 && !paused) {
                                let interval = if f { cfg.foreground_interval } else { cfg.background_interval };
                                deadline = (base + interval).max(Instant::now());
                            }
                            continue;
                        }
                        Some(Command::Pause(done)) => {
                            paused = true;
                            st.lock().unwrap().paused = true;
                            st.lock().unwrap().next_delay_ms = None;
                            let _ = done.send(());
                            continue;
                        }
                        Some(Command::Resume) => {
                            if !paused {
                                continue;
                            }
                            paused = false;
                            st.lock().unwrap().paused = false;
                            trigger = PollTrigger::Manual;
                        }
                        Some(Command::Refresh(reply)) => {
                            if paused {
                                let _ = reply.send(RefreshDecision::Paused);
                                continue;
                            }
                            if last_started.is_some_and(|t| t.elapsed() < MIN_MANUAL_GAP) {
                                let _ = reply.send(RefreshDecision::Throttled);
                                continue;
                            }
                            let _ = reply.send(RefreshDecision::Started);
                            trigger = PollTrigger::Manual;
                        }
                    }
                }
            }
            last_started = Some(Instant::now());
            st.lock().unwrap().last_started_ms = Some(crate::time::now_ms());
            let ok = poll(trigger).await;
            trigger = PollTrigger::Scheduled;
            // Requests that arrived during the read are served by it, in their order: a Pause then a
            // Resume (Codex turned off and on again) ends running, with the Resume's immediate read.
            let mut resumed = false;
            while let Ok(cmd) = rx.try_recv() {
                match cmd {
                    Command::Refresh(reply) => {
                        let _ = reply.send(RefreshDecision::Coalesced);
                    }
                    Command::Foreground(f) => foreground = f,
                    Command::Pause(done) => {
                        paused = true;
                        resumed = false;
                        st.lock().unwrap().paused = true;
                        let _ = done.send(());
                    }
                    Command::Resume => {
                        if paused {
                            paused = false;
                            resumed = true;
                            st.lock().unwrap().paused = false;
                        }
                    }
                    Command::Stop(done) => {
                        let _ = done.send(());
                        st.lock().unwrap().running = false;
                        return;
                    }
                }
            }
            failures = if ok { 0 } else { failures + 1 };
            let delay =
                if ok { if foreground { cfg.foreground_interval } else { cfg.background_interval } } else { backoff_delay(failures) };
            deadline = if resumed { Instant::now() } else { Instant::now() + delay };
            if resumed {
                trigger = PollTrigger::Manual;
            }
            let mut s = st.lock().unwrap();
            s.polls += 1;
            s.consecutive_failures = failures;
            s.last_success = Some(ok);
            s.foreground = foreground;
            s.next_delay_ms = if paused { None } else if resumed { Some(0) } else { Some(delay.as_millis() as u64) };
            s.last_finished_ms = Some(crate::time::now_ms());
        }
        st.lock().unwrap().running = false;
    });
    PollingHandle { tx, status }
}

//! Development verification entry point (no GUI).
//!
//!   quotadock-dev [status]            one real read, persisted, then a summary
//!   quotadock-dev watch [--count N] [--interval SECS] [--unsafe-dev-poll]
//!   quotadock-dev summary             summary from SQLite only (no backend read)
//!   quotadock-dev events [--all]      user-visible events (--all: diagnostics too)
//!   quotadock-dev history [--limit N]
//!   quotadock-dev reanalyze           rebuild derived state from raw snapshots
//!   quotadock-dev discover            list codex executables
//!   options: --db <path>  (default %LOCALAPPDATA%\QuotaDock\quotadock.sqlite3)

use quotadock_core::analytics::WindowType;
use quotadock_core::analytics::forecast::{ForecastStatus, UsageForecast};
use quotadock_core::codex::discovery::{DiscoveryOptions, discover};
use quotadock_core::commands::{self, CurrentUsage, WindowStatus};
use quotadock_core::pipeline::{CoreConfig, PollTrigger, QuotaDockCore};
use quotadock_core::polling::validate_interval;
use quotadock_core::platform::data_location::validated_database_path;
use quotadock_core::time::{local_display_ms, now_ms, relative};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

struct Args {
    command: String,
    db: Option<PathBuf>,
    count: u32,
    interval: Duration,
    unsafe_dev_poll: bool,
    all: bool,
    limit: usize,
}

fn parse_args() -> Result<Args, String> {
    let mut it = std::env::args().skip(1);
    let mut a = Args {
        command: "status".into(),
        db: None,
        count: 3,
        interval: Duration::from_secs(60),
        unsafe_dev_poll: false,
        all: false,
        limit: 20,
    };
    let mut first = true;
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--db" => a.db = Some(PathBuf::from(it.next().ok_or("--db needs a path")?)),
            "--count" => a.count = it.next().and_then(|v| v.parse().ok()).ok_or("--count needs a number")?,
            "--interval" => a.interval = Duration::from_secs(it.next().and_then(|v| v.parse().ok()).ok_or("--interval needs seconds")?),
            "--unsafe-dev-poll" => a.unsafe_dev_poll = true,
            "--all" => a.all = true,
            "--limit" => a.limit = it.next().and_then(|v| v.parse().ok()).ok_or("--limit needs a number")?,
            "-h" | "--help" => return Err(String::new()),
            cmd if first && !cmd.starts_with('-') => a.command = cmd.to_string(),
            other => return Err(format!("unknown argument {other}")),
        }
        first = false;
    }
    Ok(a)
}

fn pct(v: Option<f64>) -> String {
    v.map(|x| format!("{x}%")).unwrap_or_else(|| "n/a".into())
}

fn print_window(label: &str, w: &WindowStatus, now: i64) {
    println!("{label}:");
    if !w.present {
        println!("  window not reported in the latest snapshot ({})", w.observation_state.as_deref().unwrap_or("unknown"));
        return;
    }
    println!("  {} used, {} remaining", pct(w.used_percent), pct(w.remaining_percent));
    match (w.confirmed_reset_at_ms, w.reported_reset_at_ms) {
        (Some(r), _) => println!("  Reset: {} local ({}) [confirmed anchor]", local_display_ms(r), relative(r, now)),
        (None, Some(r)) => println!(
            "  Reset: {} local ({}) [{} - not used for countdown/forecast]",
            local_display_ms(r),
            relative(r, now),
            w.reset_schedule
        ),
        (None, None) => println!("  Reset: unknown"),
    }
    println!(
        "  State: {} | candidate: {}{} | quality: {}",
        w.observation_state.as_deref().unwrap_or("-"),
        w.candidate_state.as_deref().unwrap_or("-"),
        if w.sliding_anchor { " | sliding anchor" } else { "" },
        w.snapshot_quality
    );
}

fn print_forecast(label: &str, f: &UsageForecast) {
    let b = &f.burn;
    let rate = |r: Option<f64>| r.map(|x| format!("{x:.2} pp/h")).unwrap_or_else(|| "n/a".into());
    println!(
        "Burn rate ({label}): {:?} | 6h {} | 24h {} | cycle {} | {} normal snapshots over {} min, +{:.1} pp observed",
        b.status,
        rate(b.recent_6h_rate),
        rate(b.recent_24h_rate),
        rate(b.current_cycle_rate),
        b.points,
        b.span_ms / 60_000,
        b.observed_consumption_pp
    );
    let detail = match f.status {
        ForecastStatus::InsufficientData => "insufficient_data (needs >= 3 normal snapshots, >= 30 min, >= 2 pp consumed)".to_string(),
        ForecastStatus::NotProjected => format!(
            "not currently projected (burn {:.2} pp/h; pressure {:?}; risk {:?})",
            f.burn_rate_per_hour.unwrap_or(0.0),
            f.pressure,
            f.risk
        ),
        ForecastStatus::ResetScheduleUnconfirmed => format!(
            "exhaustion ~{} local; reset_schedule_unconfirmed ({:?}) -> no margin/pressure",
            f.estimated_exhaustion_at_ms.map(local_display_ms).unwrap_or_default(),
            f.reset_schedule
        ),
        ForecastStatus::Ok => format!(
            "exhaustion ~{} local, margin {:.1} h vs confirmed reset, pressure {:.2}, risk {:?}, confidence {:?}",
            f.estimated_exhaustion_at_ms.map(local_display_ms).unwrap_or_default(),
            f.margin_hours.unwrap_or(0.0),
            f.pressure.unwrap_or(0.0),
            f.risk,
            f.confidence
        ),
    };
    println!("Forecast ({label}): {detail}  [QuotaDock heuristic]");
}

fn print_summary(core: &QuotaDockCore, usage: &CurrentUsage) -> Result<(), Box<dyn std::error::Error>> {
    let now = usage.captured_at_ms.unwrap_or_else(now_ms);
    println!();
    println!("CODEX   (account {}, classifier {})", usage.account.as_deref().unwrap_or("?"), usage.classifier_version);
    println!();
    let availability = match usage.ordinary_usage_allowed {
        Some(true) => "Allowed",
        Some(false) => "Blocked",
        None => "Unknown",
    };
    println!("Availability: {availability}   (ordinaryUsageAllowed = {:?})", usage.ordinary_usage_allowed);
    if let Some(c) = usage.captured_at_ms {
        println!("Captured:     {} local", local_display_ms(c));
    }
    println!();
    for w in &usage.windows {
        print_window(if w.window_type == WindowType::FiveHour { "5h" } else { "Weekly" }, w, now);
        println!();
    }
    println!(
        "Reset credits: {}   Buckets: {}",
        usage.reset_credit_count.map(|c| c.to_string()).unwrap_or_else(|| "n/a".into()),
        usage.bucket_ids.join(", ")
    );
    println!(
        "History: {} snapshots, {} polls ({} failed{})",
        usage.polls.snapshots,
        usage.polls.poll_runs,
        usage.polls.failed_polls,
        usage.polls.last_error.as_deref().map(|e| format!("; last error: {e}")).unwrap_or_default()
    );
    for w in &usage.windows {
        if let Some(c) = &w.current_cycle {
            println!(
                "Current cycle ({}): observed since {} local ({}, confidence {}), {} -> {}",
                w.window_type.as_str(),
                local_display_ms(c.observed_started_at_ms),
                c.start_reason,
                c.confidence,
                pct(c.initial_used_percent),
                pct(c.final_used_percent)
            );
        }
    }
    let events = commands::get_quota_events(core, true, 1)?;
    match events.first() {
        Some(e) => println!(
            "Last event: {} [{}] {} {} at {} local",
            e.event_type,
            e.visibility,
            e.window_type.as_deref().unwrap_or("account"),
            e.confidence,
            local_display_ms(e.observed_at_ms)
        ),
        None => println!("Last event: none"),
    }
    println!();
    for t in WindowType::ALL {
        let f = commands::get_usage_forecast(core, t)?;
        print_forecast(if t == WindowType::FiveHour { "5h" } else { "weekly" }, &f);
    }
    println!();
    Ok(())
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("{e}");
            }
            eprintln!(
                "usage: quotadock-dev [status|watch|summary|events|history|reanalyze|discover|claude-probe|data-location] [--db PATH] [--count N] [--interval SECS] [--unsafe-dev-poll] [--all] [--limit N]"
            );
            std::process::exit(64);
        }
    };
    if let Err(e) = run(args).await {
        eprintln!("[quotadock] error: {e}");
        std::process::exit(1);
    }
}

async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    if args.command == "discover" {
        let d = discover(&DiscoveryOptions::default()).await;
        println!("{}", serde_json::to_string_pretty(&d)?);
        return Ok(());
    }
    if args.command == "data-location" {
        // Read-only: what the app would decide at startup (opens nothing).
        let ctx = quotadock_core::platform::data_location::PackageContext::current();
        println!("context: {} {}", ctx.as_str(), ctx.family().unwrap_or(""));
        println!("decision: {:?}", quotadock_core::platform::data_location::decide_current(&[]));
        return Ok(());
    }
    // An explicit --db is the caller's choice; the default database only after the package check
    // (inside a packaged shell it would be a second, virtualized copy).
    let db = match args.db.clone() {
        Some(p) => p,
        None => validated_database_path().map_err(|e| format!("{e}; pass --db <path> explicitly or run outside the package"))?,
    };
    let log: quotadock_core::pipeline::LogFn = Arc::new(|line: &str| eprintln!("[quotadock] {line}"));
    let core = QuotaDockCore::open(CoreConfig::with_database(db), log)?;
    let info = core.database_info();
    eprintln!(
        "[quotadock] database {} (schema v{}, journal_mode={}, foreign_keys={}, busy_timeout={} ms)",
        info.path.display(),
        info.schema_version,
        info.journal_mode,
        info.foreign_keys,
        info.busy_timeout_ms
    );

    match args.command.as_str() {
        "status" => {
            let out = core.poll_once(PollTrigger::Manual).await;
            eprintln!("[quotadock] poll: {:?}{}", out.status, out.error.as_deref().map(|e| format!(" ({e})")).unwrap_or_default());
            for e in &out.events {
                eprintln!("[quotadock] event {} ({:?})", e.event_type.as_str(), e.confidence);
            }
            let status = commands::get_provider_status(&core, None).await;
            if let Some(i) = &status.provider.installation {
                eprintln!(
                    "[quotadock] codex-cli {} ({:?}) at {} | app-server pid {:?} version {:?} | plan {:?}",
                    i.version,
                    i.source,
                    i.path.display(),
                    status.provider.app_server_pid,
                    status.provider.app_server_version,
                    status.provider.plan_type
                );
            }
            print_summary(&core, &commands::get_current_codex_usage(&core)?)?;
        }
        "watch" => {
            let interval = validate_interval(args.interval, args.unsafe_dev_poll)?;
            if interval < Duration::from_secs(300) {
                eprintln!(
                    "[quotadock] interval {} s is DEVELOPMENT ONLY (production default 300 s); every read is a live backend request",
                    interval.as_secs()
                );
            }
            for i in 0..args.count {
                if i > 0 {
                    tokio::time::sleep(interval).await;
                }
                let out = core.poll_once(if i == 0 { PollTrigger::Manual } else { PollTrigger::Scheduled }).await;
                let usage = commands::get_current_codex_usage(&core)?;
                let w = |t: WindowType| usage.windows.iter().find(|w| w.window_type == t).cloned();
                let fmt = |w: Option<WindowStatus>| {
                    w.map(|w| {
                        format!(
                            "{} reset={} ({}, {})",
                            pct(w.used_percent),
                            w.reported_reset_at_ms.map(|r| r.to_string()).unwrap_or("null".into()),
                            w.observation_state.unwrap_or_default(),
                            w.reset_schedule
                        )
                    })
                    .unwrap_or_default()
                };
                println!(
                    "[{}] poll {}/{}: {:?} | allowed={:?} | 5h {} | weekly {} | credits {:?} | events: {}",
                    local_display_ms(now_ms()),
                    i + 1,
                    args.count,
                    out.status,
                    usage.ordinary_usage_allowed,
                    fmt(w(WindowType::FiveHour)),
                    fmt(w(WindowType::Weekly)),
                    usage.reset_credit_count,
                    if out.events.is_empty() {
                        "none".to_string()
                    } else {
                        out.events.iter().map(|e| e.event_type.as_str()).collect::<Vec<_>>().join(",")
                    }
                );
            }
            print_summary(&core, &commands::get_current_codex_usage(&core)?)?;
        }
        "summary" => print_summary(&core, &commands::get_current_codex_usage(&core)?)?,
        "events" => {
            for e in commands::get_quota_events(&core, args.all, args.limit)? {
                println!(
                    "{} | {} | {} | {} | {} | confirmed={} | {}",
                    local_display_ms(e.observed_at_ms),
                    e.event_type,
                    e.window_type.as_deref().unwrap_or("account"),
                    e.visibility,
                    e.confidence,
                    e.confirmed,
                    e.evidence
                );
            }
        }
        "history" => {
            for s in commands::get_usage_history(&core, 0, args.limit)? {
                println!(
                    "#{} {} | allowed={:?} | 5h {} @{:?} [{}] | weekly {} @{:?} [{}] | credits {:?} | {}",
                    s.id,
                    local_display_ms(s.captured_at_ms),
                    s.ordinary_usage_allowed,
                    pct(s.five_hour.used_percent),
                    s.five_hour.reset_at_ms,
                    s.five_hour.quality,
                    pct(s.weekly.used_percent),
                    s.weekly.reset_at_ms,
                    s.weekly.quality,
                    s.reset_credit_count,
                    s.analysis_status
                );
            }
        }
        "claude-probe" => {
            // Environment + one guarded `claude -p /usage` (local command, no model call) into --db.
            use quotadock_core::claude::{model::PROVIDER_CLAUDE, monitor::ClaudeMonitor, store};
            let local = std::env::var_os("USERPROFILE").map(std::path::PathBuf::from).ok_or("USERPROFILE not set")?;
            let data_dir = core.database_info().path.parent().map(std::path::PathBuf::from).unwrap_or_default();
            let m = ClaudeMonitor::new(&data_dir, &local);
            let env = m.refresh_environment(now_ms()).await;
            println!(
                "claude: {:?} {:?} | version {:?} | billing {:?} | logged_in {:?} auth_method {:?} subscription {:?} | env overrides {:?}",
                env.installation.as_ref().map(|i| i.kind),
                env.installation.as_ref().map(|i| i.path.display().to_string()),
                env.version,
                env.billing(),
                env.auth.as_ref().map(|a| a.logged_in),
                env.auth.as_ref().and_then(|a| a.auth_method.clone()),
                env.auth.as_ref().and_then(|a| a.subscription_type.clone()),
                env.env_flags
            );
            let t0 = now_ms();
            let ingested = m.run_active(&core, now_ms()).await?;
            println!(
                "active /usage: {} in {} ms; state {:?}",
                if ingested.is_some() { "ingested" } else { "no observation" },
                now_ms() - t0,
                m.active_state()
            );
            if let Some(o) = core.with_db(|c| store::latest(c, PROVIDER_CLAUDE))? {
                for w in &o.windows {
                    println!(
                        "  {:<24} {:>5.1}% used  reset {:?} (precision {} ms)  instance {:?}",
                        w.window_key,
                        w.used_percent,
                        w.resets_at_ms.map(local_display_ms),
                        w.reset_precision_ms,
                        w.instance_key
                    );
                }
            }
            for r in core.with_db(|c| store::recent_runs(c, PROVIDER_CLAUDE, 3))? {
                println!("  run {} {} {:?}", local_display_ms(r.started_at_ms), r.status, r.error);
            }
        }
        "reanalyze" => {
            let n = core.reanalyze_all()?;
            eprintln!("[quotadock] re-analysed {n} snapshots with {}", quotadock_core::analytics::CLASSIFIER_VERSION);
            print_summary(&core, &commands::get_current_codex_usage(&core)?)?;
        }
        other => return Err(format!("unknown command {other}").into()),
    }
    core.shutdown().await;
    Ok(())
}

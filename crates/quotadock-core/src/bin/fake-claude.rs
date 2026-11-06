//! Test double for the Claude Code CLI (never touches the network or ~/.claude).
//!
//! Behaviour comes from `mode.txt` in the working directory (QuotaDock runs claude in its own
//! per-test data folder): ok | apikey | loggedout | authmissing | autherror | ratelimit | inference |
//! unrecognized | slow.
//! Every invocation's arguments are appended to `calls.log` in the same folder.

use chrono::{Duration, Local};
use serde_json::json;
use std::io::Write;

fn usage_text() -> String {
    let fmt = |d: chrono::DateTime<Local>| d.format("%b %-d, %-I:%M%P").to_string();
    let now = Local::now();
    format!(
        "You are currently using your subscription to power your Claude Code usage\n\nCurrent session: 12% used · resets {} (Test/Zone)\nCurrent week (all models): 71% used · resets {} (Test/Zone)\nCurrent week (Fable): 3% used · resets {} (Test/Zone)\n\nWhat's contributing to your limits usage?\n",
        fmt(now + Duration::hours(3)),
        fmt(now + Duration::days(3)),
        fmt(now + Duration::days(3))
    )
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = std::fs::read_to_string("mode.txt").unwrap_or_else(|_| "ok".into()).trim().to_string();
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("calls.log") {
        let _ = writeln!(f, "{}", args.join(" "));
    }
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    match a.as_slice() {
        ["--version"] => println!("2.1.278 (Claude Code)"),
        ["auth", "status", "--json"] => {
            let v = match mode.as_str() {
                "apikey" => json!({ "loggedIn": true, "authMethod": "api_key", "apiProvider": "firstParty" }),
                "loggedout" => json!({ "loggedIn": false }),
                // No usable answer: the field is missing, or the command failed outright.
                "authmissing" => json!({ "authMethod": "claude.ai" }),
                "autherror" => {
                    eprintln!("Error: connect ETIMEDOUT");
                    std::process::exit(1);
                }
                _ => {
                    json!({ "loggedIn": true, "authMethod": "claude.ai", "apiProvider": "firstParty", "email": "someone@example.com", "orgId": "org-test", "subscriptionType": "max" })
                }
            };
            println!("{v}");
        }
        ["-p", "/usage", ..] => {
            if mode == "slow" {
                std::thread::sleep(std::time::Duration::from_secs(120));
            }
            let base = |result: String, is_error: bool| {
                json!({ "type": "result", "subtype": if is_error { "error" } else { "success" }, "is_error": is_error, "duration_ms": 700,
                    "duration_api_ms": 0, "num_turns": 0, "total_cost_usd": 0, "local_command": "usage", "result": result,
                    "usage": { "input_tokens": 0, "output_tokens": 0, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0 }, "modelUsage": {} })
            };
            let v = match mode.as_str() {
                "ratelimit" => base("API Error: 429 Too Many Requests. Please retry after 120 seconds.".into(), true),
                "inference" => {
                    let mut v = base("Your usage looks fine!".into(), false);
                    v["duration_api_ms"] = json!(912);
                    v["usage"]["output_tokens"] = json!(31);
                    v["local_command"] = serde_json::Value::Null;
                    v
                }
                "unrecognized" => base("Usage information is not available right now.".into(), false),
                "badformat" => base("Current session: 140% used\nCurrent week (all models): 3% used\n".into(), false),
                "apikey" => base("You are currently using API key billing for Claude Code.".into(), false),
                _ => base(usage_text(), false),
            };
            println!("{v}");
        }
        _ => {
            eprintln!("fake-claude: unsupported invocation {args:?}");
            std::process::exit(2);
        }
    }
}

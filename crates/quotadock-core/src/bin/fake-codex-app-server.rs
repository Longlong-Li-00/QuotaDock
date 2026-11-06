//! Test double for `codex app-server --listen stdio://` (JSONL, no "jsonrpc" member).
//! Never touches the network or ~/.codex.
//!
//!   fake-codex-app-server --fixture <rate-limits.json> [--unauth] [--crash-after-init]
//!                         [--exit-after-reads <n>] [--log <file>]
//!
//! The fixture may be a raw result or `{ "_meta": ..., "response": <result> }`.

use serde_json::{Value, json};
use std::io::{BufRead, Write};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let flag = |name: &str| args.iter().any(|a| a == name);
    let fixture: Value =
        arg("--fixture").and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(json!({}));
    let rate_limits = fixture.get("response").cloned().unwrap_or(fixture);
    let unauth = flag("--unauth");
    let crash_after_init = flag("--crash-after-init");
    let exit_after_reads: Option<u32> = arg("--exit-after-reads").and_then(|n| n.parse().ok());
    let mut reads = 0u32;
    let mut log = arg("--log").and_then(|p| std::fs::OpenOptions::new().create(true).append(true).open(p).ok());

    let stdout = std::io::stdout();
    let send = |v: Value| {
        let mut out = stdout.lock();
        let _ = writeln!(out, "{v}");
        let _ = out.flush();
    };
    send(Value::String("this line is not JSON".into())); // noise the client must tolerate
    let mut initialized = false;
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if let Some(f) = log.as_mut() {
            let _ = writeln!(f, "{line}");
        }
        let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
        let Some(id) = msg.get("id").cloned() else { continue }; // client notifications are only logged upstream
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        if method.is_empty() {
            continue; // a response (e.g. the client's refusal of our server request)
        }
        if method != "initialize" && !initialized {
            send(json!({ "id": id, "error": { "code": -32600, "message": "Not initialized" } }));
            continue;
        }
        match method {
            "initialize" => {
                initialized = true;
                let name = msg.pointer("/params/clientInfo/name").and_then(Value::as_str).unwrap_or("?");
                send(
                    json!({ "id": id, "result": { "userAgent": format!("{name}/0.162.0-alpha.2 (fake)"), "codexHome": "C:\\fake", "platformFamily": "windows", "platformOs": "windows" } }),
                );
                send(json!({ "method": "remoteControl/status/changed", "params": { "status": "disabled" } }));
                send(json!({ "id": "srv-1", "method": "item/tool/call", "params": {} }));
                if crash_after_init {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    std::process::exit(3);
                }
            }
            "account/read" => {
                if unauth {
                    send(json!({ "id": id, "result": { "account": null, "requiresOpenaiAuth": true } }));
                } else {
                    let acct = rate_limits.get("accountId").cloned().unwrap_or(json!("acct_fake"));
                    send(
                        json!({ "id": id, "result": { "account": { "type": "chatgpt", "email": "someone@example.com", "planType": "plus" }, "requiresOpenaiAuth": true, "workspaceRouting": { "chatgptAccountId": acct } } }),
                    );
                }
            }
            "account/rateLimits/read" => {
                if unauth {
                    send(
                        json!({ "id": id, "error": { "code": -32600, "message": "codex account authentication required to read rate limits" } }),
                    );
                } else {
                    send(json!({ "id": id, "result": rate_limits }));
                    reads += 1;
                    if exit_after_reads.is_some_and(|n| reads >= n) {
                        std::process::exit(0); // the child going away on its own (update, crash)
                    }
                }
            }
            other => {
                send(json!({ "id": id, "error": { "code": -32600, "message": format!("Invalid request: unknown variant `{other}`") } }))
            }
        }
    }
    if flag("--linger") {
        // Ignore stdin EOF (unlike the real app-server) so only the job object can stop us.
        std::thread::sleep(std::time::Duration::from_secs(120));
    }
}

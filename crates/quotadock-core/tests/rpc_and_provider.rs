//! JSON-RPC client (in memory) and the provider against the fake app-server child process.

use quotadock_core::codex::provider::{CodexProvider, PollStatus, ProviderConfig};
use quotadock_core::codex::rpc::{RpcConnection, RpcError};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const FAKE: &str = env!("CARGO_BIN_EXE_fake-codex-app-server");

fn fixture() -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/codex-rate-limits-real-redacted.json")
        .to_string_lossy()
        .into_owned()
}

/// An in-memory "server" that answers line by line with a closure.
fn spawn_memory_server(
    reply: impl Fn(&Value) -> Vec<Value> + Send + 'static,
) -> (RpcConnection, tokio::sync::mpsc::UnboundedReceiver<Value>) {
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let (client_read, client_write) = tokio::io::split(client_io);
    let (server_read, mut server_write) = tokio::io::split(server_io);
    let (seen_tx, seen_rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut lines = BufReader::new(server_read).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let v: Value = serde_json::from_str(&line).unwrap();
            let _ = seen_tx.send(v.clone());
            for out in reply(&v) {
                let text = match out {
                    Value::String(s) => s, // raw noise line
                    other => other.to_string(),
                };
                if server_write.write_all(format!("{text}\n").as_bytes()).await.is_err() {
                    return;
                }
            }
        }
    });
    (RpcConnection::new(client_read, client_write), seen_rx)
}

#[tokio::test]
async fn correlates_responses_and_omits_the_jsonrpc_member() {
    let (rpc, mut seen) = spawn_memory_server(|req| {
        let id = req["id"].clone();
        vec![
            json!({ "method": "account/updated", "params": {} }),
            Value::String("not json".into()),
            json!({ "id": id, "result": { "echo": req["method"] } }),
        ]
    });
    let r = rpc.request("account/read", Some(json!({})), Duration::from_secs(5)).await.unwrap();
    assert_eq!(r["echo"], "account/read");
    let sent = seen.recv().await.unwrap();
    assert!(sent.get("jsonrpc").is_none());
    assert_eq!(sent["id"], 1);
    let stats = rpc.stats();
    assert_eq!(stats.responses, 1);
    assert_eq!(stats.notifications, 1);
    assert_eq!(stats.noise, 1);
}

#[tokio::test]
async fn refuses_methods_outside_the_allowlist_without_writing() {
    let (rpc, mut seen) = spawn_memory_server(|_| vec![]);
    for m in [
        "account/logout",
        "account/login/start",
        "account/rateLimitResetCredit/consume",
        "fs/writeFile",
        "process/spawn",
        "command/exec",
        "config/value/write",
        "thread/start",
    ] {
        assert_eq!(rpc.request(m, None, Duration::from_millis(50)).await, Err(RpcError::MethodNotAllowed(m.into())));
    }
    assert!(matches!(rpc.notify("thread/started").await, Err(RpcError::MethodNotAllowed(_))));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(seen.try_recv().is_err(), "nothing reached the wire");
}

#[tokio::test]
async fn server_requests_are_refused() {
    let (rpc, mut seen) = spawn_memory_server(|req| {
        if req["method"] == "account/read" {
            vec![json!({ "id": "srv-1", "method": "item/tool/call", "params": {} }), json!({ "id": req["id"], "result": {} })]
        } else {
            vec![]
        }
    });
    rpc.request("account/read", Some(json!({})), Duration::from_secs(5)).await.unwrap();
    let _first = seen.recv().await.unwrap();
    let refusal = tokio::time::timeout(Duration::from_secs(2), seen.recv()).await.unwrap().unwrap();
    assert_eq!(refusal["id"], "srv-1");
    assert_eq!(refusal["error"]["code"], -32601);
    assert_eq!(rpc.stats().server_requests, 1);
}

#[tokio::test]
async fn server_errors_timeouts_and_eof() {
    let (rpc, _seen) = spawn_memory_server(|req| match req["method"].as_str() {
        Some("account/rateLimits/read") => vec![
            json!({ "id": req["id"], "error": { "code": -32600, "message": "codex account authentication required to read rate limits" } }),
        ],
        _ => vec![],
    });
    let e = rpc.request("account/rateLimits/read", None, Duration::from_secs(5)).await.unwrap_err();
    assert!(matches!(e, RpcError::Server { code: -32600, .. }));
    let e = rpc.request("account/read", None, Duration::from_millis(50)).await.unwrap_err();
    assert!(matches!(e, RpcError::Timeout { .. }));
}

fn provider(extra: &[&str]) -> CodexProvider {
    let mut args = vec!["--fixture".to_string(), fixture()];
    args.extend(extra.iter().map(|s| s.to_string()));
    CodexProvider::new(ProviderConfig {
        override_command: Some((PathBuf::from(FAKE), args)),
        request_timeout: Some(Duration::from_secs(10)),
        ..Default::default()
    })
}

fn process_exists(pid: u32) -> bool {
    let out = std::process::Command::new("tasklist").args(["/FI", &format!("PID eq {pid}"), "/NH"]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).contains(&pid.to_string())
}

#[tokio::test]
async fn provider_reads_through_one_persistent_child_and_stops_it() {
    let log = tempfile::NamedTempFile::new().unwrap();
    let log_path = log.path().to_string_lossy().into_owned();
    let mut p = provider(&["--log", &log_path]);
    let first = p.read().await.expect("first read");
    assert_eq!(first.result["rateLimitsByLimitId"]["codex"]["primary"]["windowDurationMins"], 300);
    let pid = p.status().app_server_pid.expect("child alive");
    let _second = p.read().await.expect("second read");
    assert_eq!(p.status().app_server_pid, Some(pid), "the same child serves every poll");
    assert_eq!(p.status().app_server_version.as_deref(), Some("0.162.0-alpha.2"));
    p.shutdown().await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!process_exists(pid), "child stopped with the provider");

    let sent = std::fs::read_to_string(&log_path).unwrap();
    let methods: Vec<String> = sent
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter_map(|v| v["method"].as_str().map(str::to_string))
        .collect();
    assert_eq!(methods, ["initialize", "initialized", "account/read", "account/rateLimits/read", "account/rateLimits/read"]);
    assert!(sent.contains("\"excludeResetCreditDetails\":true") && sent.contains("\"supportsLunaReserve\":false"));
    assert!(sent.contains("\"experimentalApi\":false"));
    assert!(sent.contains("-32601"), "the fake's server request was refused");
    assert!(!sent.contains("refreshToken\":true"), "never asks for a token refresh");
}

#[tokio::test]
async fn provider_reports_not_authenticated() {
    let mut p = provider(&["--unauth"]);
    let e = p.read().await.unwrap_err();
    assert_eq!(e.status, PollStatus::AuthenticationError);
    assert!(e.message.contains("not signed in"));
    assert_eq!(p.status().app_server_pid, None, "child stopped so a later sign-in is picked up");
}

#[tokio::test]
async fn provider_reports_each_app_server_start_with_its_reason() {
    let mut p = provider(&["--exit-after-reads", "1"]);
    p.read().await.expect("first read");
    let first = p.take_start().expect("first start recorded");
    assert_eq!(first.reason, "initial");
    assert!(first.pid.is_some());
    assert_eq!(first.cli_version, "0.162.0-alpha.2");
    assert_eq!(p.take_start(), None, "taken once");

    tokio::time::sleep(Duration::from_millis(400)).await; // the child exits after one read
    p.read().await.expect("read after the child exited");
    assert_eq!(p.take_start().map(|s| s.reason), Some("after_exit"), "a provider restart");

    p.shutdown().await; // pause
    p.read().await.expect("read after resume");
    assert_eq!(p.take_start().map(|s| s.reason), Some("after_stop"));
    p.shutdown().await;
}

#[tokio::test]
async fn provider_recovers_from_a_crashed_child() {
    let mut p = provider(&["--crash-after-init"]);
    let e = p.read().await.unwrap_err();
    assert_eq!(e.status, PollStatus::ProviderUnavailable, "{e}");
}

#[cfg(windows)]
#[test]
fn job_object_kills_the_child_when_quotadock_dies() {
    // A helper process (this test binary re-executed) starts the provider and is then killed
    // with TerminateProcess; the app-server child must not survive it.
    if std::env::var("QD_JOB_HELPER").is_ok() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut p = provider(&["--linger"]);
            p.read().await.unwrap();
            println!("CHILD_PID={}", p.status().app_server_pid.unwrap());
            std::io::Write::flush(&mut std::io::stdout()).unwrap();
            tokio::time::sleep(Duration::from_secs(60)).await;
        });
        return;
    }
    let exe = std::env::current_exe().unwrap();
    let mut helper = std::process::Command::new(exe)
        .args(["--exact", "job_object_kills_the_child_when_quotadock_dies", "--nocapture"])
        .env("QD_JOB_HELPER", "1")
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = helper.stdout.take().unwrap();
    let mut child_pid = None;
    for line in std::io::BufRead::lines(std::io::BufReader::new(stdout)) {
        let line = line.unwrap();
        if let Some(p) = line.strip_prefix("CHILD_PID=") {
            child_pid = Some(p.trim().parse::<u32>().unwrap());
            break;
        }
    }
    let child_pid = child_pid.expect("helper reported its app-server pid");
    assert!(process_exists(child_pid));
    helper.kill().unwrap(); // TerminateProcess: no destructors, no cleanup code runs
    let _ = helper.wait();
    std::thread::sleep(Duration::from_millis(500));
    assert!(!process_exists(child_pid), "app-server child outlived a killed QuotaDock");
}

/// A real child that exits only on stdin EOF: closing the connection must end it at once. Before,
/// the pipe stayed open on Windows and every Codex stop waited 3 s, then killed the app-server.
#[cfg(windows)]
#[tokio::test]
async fn close_gives_a_child_process_its_stdin_eof() {
    let mut child = tokio::process::Command::new("sort.exe")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("sort.exe");
    let rpc = RpcConnection::new(child.stdout.take().unwrap(), child.stdin.take().unwrap());
    let started = std::time::Instant::now();
    rpc.close("test").await;
    let status = tokio::time::timeout(Duration::from_secs(2), child.wait()).await.expect("the child saw EOF and exited");
    assert!(status.unwrap().success());
    assert!(started.elapsed() < Duration::from_secs(1));
}

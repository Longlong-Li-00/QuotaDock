//! Read-only JSON-RPC client for the app-server JSONL wire format (one JSON object per line,
//! no "jsonrpc" member). Generic over any AsyncRead/AsyncWrite so it is testable in memory.
//!
//! Inbound lines are separated into: responses (resolve pending requests), server requests
//! (refused with -32601: QuotaDock serves nothing), notifications (counted) and noise.

use super::types::{CLIENT_NOTIFICATION_METHODS, READ_ONLY_REQUEST_METHODS};
use crate::codex::sanitize::sanitize_string;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex as AsyncMutex, oneshot};

const MAX_LINE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub enum RpcError {
    /// The method is not on the read-only allowlist; nothing was sent.
    MethodNotAllowed(String),
    /// JSON-RPC error object from the server.
    Server {
        method: String,
        code: i64,
        message: String,
    },
    Timeout {
        method: String,
        after: Duration,
    },
    Closed(String),
    Io(String),
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MethodNotAllowed(m) => write!(f, "method \"{m}\" is not on QuotaDock's read-only allowlist"),
            Self::Server { method, code, message } => write!(f, "{method} failed ({code}): {message}"),
            Self::Timeout { method, after } => write!(f, "{method} timed out after {} ms", after.as_millis()),
            Self::Closed(r) => write!(f, "JSON-RPC connection closed: {r}"),
            Self::Io(e) => write!(f, "JSON-RPC I/O error: {e}"),
        }
    }
}

impl std::error::Error for RpcError {}

#[derive(Debug, Default)]
pub struct RpcStats {
    pub responses: AtomicU64,
    pub notifications: AtomicU64,
    pub server_requests: AtomicU64,
    pub noise: AtomicU64,
    pub unmatched_responses: AtomicU64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct RpcStatsSnapshot {
    pub responses: u64,
    pub notifications: u64,
    pub server_requests: u64,
    pub noise: u64,
    pub unmatched_responses: u64,
}

type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, (i64, String)>>>>>;

pub struct RpcConnection {
    writer: Arc<AsyncMutex<Box<dyn AsyncWrite + Send + Unpin>>>,
    pending: Pending,
    next_id: AtomicI64,
    closed: Arc<AtomicBool>,
    close_reason: Arc<Mutex<Option<String>>>,
    stats: Arc<RpcStats>,
    reader_task: tokio::task::JoinHandle<()>,
}

pub fn is_allowed_request(method: &str) -> bool {
    READ_ONLY_REQUEST_METHODS.contains(&method)
}

impl RpcConnection {
    pub fn new<R, W>(reader: R, writer: W) -> Self
    where
        R: AsyncRead + Send + Unpin + 'static,
        W: AsyncWrite + Send + Unpin + 'static,
    {
        let writer: Arc<AsyncMutex<Box<dyn AsyncWrite + Send + Unpin>>> = Arc::new(AsyncMutex::new(Box::new(writer)));
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let closed = Arc::new(AtomicBool::new(false));
        let close_reason = Arc::new(Mutex::new(None));
        let stats = Arc::new(RpcStats::default());
        let reader_task = tokio::spawn(read_loop(
            BufReader::new(reader),
            writer.clone(),
            pending.clone(),
            closed.clone(),
            close_reason.clone(),
            stats.clone(),
        ));
        Self { writer, pending, next_id: AtomicI64::new(1), closed, close_reason, stats, reader_task }
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    pub fn stats(&self) -> RpcStatsSnapshot {
        RpcStatsSnapshot {
            responses: self.stats.responses.load(Ordering::Relaxed),
            notifications: self.stats.notifications.load(Ordering::Relaxed),
            server_requests: self.stats.server_requests.load(Ordering::Relaxed),
            noise: self.stats.noise.load(Ordering::Relaxed),
            unmatched_responses: self.stats.unmatched_responses.load(Ordering::Relaxed),
        }
    }

    fn closed_error(&self) -> RpcError {
        RpcError::Closed(self.close_reason.lock().unwrap().clone().unwrap_or_else(|| "closed".into()))
    }

    pub async fn request(&self, method: &str, params: Option<Value>, timeout: Duration) -> Result<Value, RpcError> {
        if !is_allowed_request(method) {
            return Err(RpcError::MethodNotAllowed(method.into()));
        }
        if self.is_closed() {
            return Err(self.closed_error());
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        let mut msg = json!({ "id": id, "method": method });
        if let Some(p) = params {
            msg["params"] = p;
        }
        if let Err(e) = self.write_line(&msg).await {
            self.pending.lock().unwrap().remove(&id);
            return Err(e);
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(Ok(v))) => Ok(v),
            Ok(Ok(Err((code, message)))) => Err(RpcError::Server { method: method.into(), code, message }),
            Ok(Err(_)) => Err(self.closed_error()),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err(RpcError::Timeout { method: method.into(), after: timeout })
            }
        }
    }

    pub async fn notify(&self, method: &str) -> Result<(), RpcError> {
        if !CLIENT_NOTIFICATION_METHODS.contains(&method) {
            return Err(RpcError::MethodNotAllowed(method.into()));
        }
        if self.is_closed() {
            return Err(self.closed_error());
        }
        self.write_line(&json!({ "method": method })).await
    }

    async fn write_line(&self, msg: &Value) -> Result<(), RpcError> {
        write_message(&self.writer, msg).await
    }

    /// Closes our write side (the app-server exits on stdin EOF) and fails pending requests.
    /// tokio's `ChildStdin::shutdown` does nothing on Windows, so the pipe is also dropped: only
    /// that gives the child its EOF, instead of the 3 s wait and kill in `AppServerProcess::stop`.
    pub async fn close(&self, reason: &str) {
        mark_closed(&self.closed, &self.close_reason, &self.pending, reason);
        let mut writer = self.writer.lock().await;
        let _ = writer.shutdown().await;
        *writer = Box::new(tokio::io::sink());
    }
}

impl Drop for RpcConnection {
    fn drop(&mut self) {
        self.reader_task.abort();
    }
}

async fn write_message(writer: &Arc<AsyncMutex<Box<dyn AsyncWrite + Send + Unpin>>>, msg: &Value) -> Result<(), RpcError> {
    let mut line = serde_json::to_vec(msg).map_err(|e| RpcError::Io(e.to_string()))?;
    line.push(b'\n');
    let mut w = writer.lock().await;
    w.write_all(&line).await.map_err(|e| RpcError::Io(e.to_string()))?;
    w.flush().await.map_err(|e| RpcError::Io(e.to_string()))
}

fn mark_closed(closed: &AtomicBool, reason_slot: &Mutex<Option<String>>, pending: &Pending, reason: &str) {
    if closed.swap(true, Ordering::SeqCst) {
        return;
    }
    *reason_slot.lock().unwrap() = Some(reason.to_string());
    // Dropping the senders wakes every waiter with a "closed" error.
    pending.lock().unwrap().clear();
}

async fn read_loop<R: AsyncRead + Unpin>(
    mut reader: BufReader<R>,
    writer: Arc<AsyncMutex<Box<dyn AsyncWrite + Send + Unpin>>>,
    pending: Pending,
    closed: Arc<AtomicBool>,
    close_reason: Arc<Mutex<Option<String>>>,
    stats: Arc<RpcStats>,
) {
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) => {
                mark_closed(&closed, &close_reason, &pending, "peer closed stdout (EOF)");
                return;
            }
            Ok(_) => {}
            Err(e) => {
                mark_closed(&closed, &close_reason, &pending, &format!("read error: {e}"));
                return;
            }
        }
        if buf.len() > MAX_LINE_BYTES {
            stats.noise.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        let line = String::from_utf8_lossy(&buf);
        let line = line.trim_end_matches(['\n', '\r']);
        if line.trim().is_empty() {
            continue;
        }
        let Ok(Value::Object(msg)) = serde_json::from_str::<Value>(line) else {
            stats.noise.fetch_add(1, Ordering::Relaxed);
            let _ = sanitize_string(line); // noise is never logged unsanitized
            continue;
        };
        let id = msg.get("id").filter(|v| v.is_i64() || v.is_u64() || v.is_string()).cloned();
        let method = msg.get("method").and_then(Value::as_str);
        match (id, method) {
            (Some(id), None) if msg.contains_key("result") || msg.contains_key("error") => {
                let Some(num) = id.as_i64() else {
                    stats.unmatched_responses.fetch_add(1, Ordering::Relaxed);
                    continue;
                };
                let Some(tx) = pending.lock().unwrap().remove(&num) else {
                    stats.unmatched_responses.fetch_add(1, Ordering::Relaxed);
                    continue;
                };
                stats.responses.fetch_add(1, Ordering::Relaxed);
                let outcome = match msg.get("error") {
                    Some(Value::Object(e)) => Err((
                        e.get("code").and_then(Value::as_i64).unwrap_or(-32603),
                        e.get("message").and_then(Value::as_str).unwrap_or("unknown error").to_string(),
                    )),
                    _ => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
                };
                let _ = tx.send(outcome);
            }
            (Some(id), Some(_method)) => {
                // Approvals, tool calls, token refresh, attestation...: never served.
                stats.server_requests.fetch_add(1, Ordering::Relaxed);
                let refusal =
                    json!({ "id": id, "error": { "code": -32601, "message": "QuotaDock is a read-only client and serves no requests" } });
                let _ = write_message(&writer, &refusal).await;
            }
            (None, Some(_method)) => {
                stats.notifications.fetch_add(1, Ordering::Relaxed);
            }
            _ => {
                stats.noise.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

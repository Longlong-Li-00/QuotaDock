//! The persistent `codex app-server --listen stdio://` child owned by the Rust backend.
//!
//! Containment:
//! - spawned without a shell and without a console window;
//! - placed in a Windows Job Object with KILL_ON_JOB_CLOSE, so it cannot outlive QuotaDock even
//!   if QuotaDock is killed (in addition to the app-server exiting on stdin EOF, observed in 1B);
//! - `kill_on_drop` as a last resort;
//! - the only bytes it receives are allowlisted JSON-RPC messages (see `rpc.rs`).

use super::rpc::{RpcConnection, RpcError, RpcStatsSnapshot};
use super::sanitize::sanitize_string;
use super::types::{CLIENT_NAME, CLIENT_TITLE, CLIENT_VERSION};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub user_agent: String,
    pub codex_home: String,
    pub platform_family: String,
    pub platform_os: String,
}

#[derive(Debug)]
pub enum ProcessError {
    Spawn(String),
    Handshake(String),
}

impl std::fmt::Display for ProcessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn(e) => write!(f, "failed to spawn app-server: {e}"),
            Self::Handshake(e) => write!(f, "app-server handshake failed: {e}"),
        }
    }
}

impl std::error::Error for ProcessError {}

pub struct AppServerProcess {
    child: Child,
    rpc: RpcConnection,
    init: InitializeResult,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    #[cfg(windows)]
    _job: Option<job::JobHandle>,
}

impl AppServerProcess {
    /// Spawns `<codex> app-server --listen stdio://` (or `args` for tests) and completes the
    /// initialize handshake. Requests before `initialize` are rejected by the server (-32600).
    pub async fn start(program: &Path, args: &[&str], env: &[(String, String)], timeout: Duration) -> Result<Self, ProcessError> {
        let mut cmd = Command::new(program);
        cmd.args(args)
            .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        cmd.creation_flags(super::discovery::CREATE_NO_WINDOW);
        let mut child = cmd.spawn().map_err(|e| ProcessError::Spawn(e.to_string()))?;

        #[cfg(windows)]
        let job = child.raw_handle().and_then(|h| job::JobHandle::contain(h).ok());

        let stdin = child.stdin.take().ok_or_else(|| ProcessError::Spawn("no stdin".into()))?;
        let stdout = child.stdout.take().ok_or_else(|| ProcessError::Spawn("no stdout".into()))?;
        let stderr = child.stderr.take().ok_or_else(|| ProcessError::Spawn("no stderr".into()))?;
        let stderr_tail = Arc::new(Mutex::new(VecDeque::new()));
        let tail = stderr_tail.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let mut t = tail.lock().unwrap();
                t.push_back(sanitize_string(&line).chars().take(500).collect());
                if t.len() > 50 {
                    t.pop_front();
                }
            }
        });

        let rpc = RpcConnection::new(stdout, stdin);
        let params = json!({
            "clientInfo": { "name": CLIENT_NAME, "title": CLIENT_TITLE, "version": CLIENT_VERSION },
            "capabilities": { "experimentalApi": false }
        });
        let init = match rpc.request("initialize", Some(params), timeout).await {
            Ok(v) => {
                serde_json::from_value::<InitializeResult>(v).map_err(|e| ProcessError::Handshake(format!("bad initialize result: {e}")))
            }
            Err(e) => Err(ProcessError::Handshake(e.to_string())),
        };
        let init = match init {
            Ok(i) => i,
            Err(e) => {
                let _ = child.start_kill();
                let tail: Vec<String> = stderr_tail.lock().unwrap().iter().rev().take(3).cloned().collect();
                return Err(match e {
                    ProcessError::Handshake(m) if !tail.is_empty() => {
                        ProcessError::Handshake(format!("{m} (stderr: {})", tail.join(" | ")))
                    }
                    other => other,
                });
            }
        };
        // Not required by the server (client notifications are only logged); sent for hygiene.
        let _ = rpc.notify("initialized").await;
        Ok(Self {
            child,
            rpc,
            init,
            stderr_tail,
            #[cfg(windows)]
            _job: job,
        })
    }

    pub fn initialize_result(&self) -> &InitializeResult {
        &self.init
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    pub fn is_alive(&mut self) -> bool {
        !self.rpc.is_closed() && matches!(self.child.try_wait(), Ok(None))
    }

    pub fn rpc_stats(&self) -> RpcStatsSnapshot {
        self.rpc.stats()
    }

    pub fn stderr_tail(&self) -> Vec<String> {
        self.stderr_tail.lock().unwrap().iter().cloned().collect()
    }

    pub async fn request(&self, method: &str, params: Option<Value>, timeout: Duration) -> Result<Value, RpcError> {
        self.rpc.request(method, params, timeout).await
    }

    /// Closes stdin (the app-server exits on EOF), waits briefly, then kills if needed.
    pub async fn stop(mut self) {
        self.rpc.close("transport stopped").await;
        if tokio::time::timeout(Duration::from_secs(3), self.child.wait()).await.is_err() {
            let _ = self.child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(2), self.child.wait()).await;
        }
    }
}

/// Server version from the initialize userAgent: "quotadock/0.162.0-alpha.2 (...)" -> "0.162.0-alpha.2".
pub fn parse_server_version(user_agent: &str) -> Option<String> {
    let first = user_agent.split_whitespace().next()?;
    let (_, v) = first.split_once('/')?;
    let ok = v.chars().next()?.is_ascii_digit() && v.split('.').count() >= 3;
    ok.then(|| v.to_string())
}

#[cfg(windows)]
pub(crate) mod job {
    use std::os::windows::io::RawHandle;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectExtendedLimitInformation, SetInformationJobObject,
    };

    /// Owns a job object whose closing kills every process in it.
    pub struct JobHandle(HANDLE);

    // The handle is an opaque kernel handle used only through thread-safe Win32 calls.
    unsafe impl Send for JobHandle {}
    unsafe impl Sync for JobHandle {}

    impl JobHandle {
        pub fn contain(process: RawHandle) -> Result<Self, String> {
            unsafe {
                let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if job.is_null() {
                    return Err("CreateJobObjectW failed".into());
                }
                let handle = JobHandle(job);
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let ok = SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if ok == 0 {
                    return Err("SetInformationJobObject failed".into());
                }
                if AssignProcessToJobObject(job, process as HANDLE) == 0 {
                    return Err("AssignProcessToJobObject failed".into());
                }
                Ok(handle)
            }
        }
    }

    impl Drop for JobHandle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

//! Locating and running the user's Claude Code CLI. Only three invocations exist and none of them
//! reaches a model: `--version`, `auth status --json` and the local `/usage` slash command.
//!
//! Every run: no shell, the cleaned environment (`env::claude_child_env`), a fixed working
//! directory inside QuotaDock's data folder, stdin closed, a timeout, and (Windows) a job object so
//! nothing outlives the call.

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallKind {
    /// `npm i -g @anthropic-ai/claude-code`: `%APPDATA%\npm\node_modules\@anthropic-ai\claude-code\bin\claude.exe`
    Npm,
    /// Native installer: `%USERPROFILE%\.local\bin\claude.exe`
    Native,
    /// Found on PATH.
    Path,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeInstallation {
    pub path: PathBuf,
    pub kind: InstallKind,
}

const NPM_REL: &str = r"node_modules\@anthropic-ai\claude-code\bin\claude.exe";

/// Finds claude.exe without running anything.
pub fn locate(env: &[(String, String)]) -> Option<ClaudeInstallation> {
    let var = |name: &str| env.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone());
    if let Some(appdata) = var("APPDATA") {
        let p = Path::new(&appdata).join("npm").join(NPM_REL);
        if p.is_file() {
            return Some(ClaudeInstallation { path: p, kind: InstallKind::Npm });
        }
    }
    if let Some(home) = var("USERPROFILE") {
        let p = Path::new(&home).join(".local").join("bin").join("claude.exe");
        if p.is_file() {
            return Some(ClaudeInstallation { path: p, kind: InstallKind::Native });
        }
    }
    for dir in var("PATH").unwrap_or_default().split(';').filter(|d| !d.trim().is_empty()) {
        let d = Path::new(dir.trim());
        let exe = d.join("claude.exe");
        if exe.is_file() {
            return Some(ClaudeInstallation { path: exe, kind: InstallKind::Path });
        }
        // An npm shim (claude.cmd) in a custom npm prefix: use the real binary next to it.
        if d.join("claude.cmd").is_file() {
            let real = d.join(NPM_REL);
            if real.is_file() {
                return Some(ClaudeInstallation { path: real, kind: InstallKind::Npm });
            }
        }
    }
    None
}

#[derive(Debug, Clone)]
pub struct CliOutput {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliError {
    Spawn(String),
    Timeout,
    Io(String),
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn(e) => write!(f, "could not start claude: {e}"),
            Self::Timeout => write!(f, "claude did not finish in time"),
            Self::Io(e) => write!(f, "claude output error: {e}"),
        }
    }
}

/// Runs claude.exe once (no shell) and captures its output.
pub async fn run(exe: &Path, args: &[&str], cwd: &Path, env: &[(String, String)], timeout: Duration) -> Result<CliOutput, CliError> {
    std::fs::create_dir_all(cwd).map_err(|e| CliError::Spawn(e.to_string()))?;
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .current_dir(cwd)
        .env_clear()
        .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(crate::codex::discovery::CREATE_NO_WINDOW);
    let started = Instant::now();
    let mut child = cmd.spawn().map_err(|e| CliError::Spawn(e.to_string()))?;
    #[cfg(windows)]
    let _job = child.raw_handle().and_then(|h| crate::codex::process::job::JobHandle::contain(h).ok());
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let work = async {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let (a, b) = tokio::join!(stdout.read_to_end(&mut out), stderr.read_to_end(&mut err));
        a.and(b).map_err(|e| CliError::Io(e.to_string()))?;
        let status = child.wait().await.map_err(|e| CliError::Io(e.to_string()))?;
        Ok::<_, CliError>((status.code(), out, err))
    };
    match tokio::time::timeout(timeout, work).await {
        Ok(Ok((code, out, err))) => Ok(CliOutput {
            code,
            stdout: String::from_utf8_lossy(&out).into_owned(),
            stderr: String::from_utf8_lossy(&err).into_owned(),
            duration_ms: started.elapsed().as_millis() as i64,
        }),
        Ok(Err(e)) => Err(e),
        // Dropping `child` (kill_on_drop) and `_job` (KILL_ON_JOB_CLOSE) ends the process tree.
        Err(_) => Err(CliError::Timeout),
    }
}

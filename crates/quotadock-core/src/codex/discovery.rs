//! Locates codex executables without hard-coding versioned install paths.
//! Port of `src/codex/discovery.ts` (daemon probing is not ported: the V1 transport is stdio).

use super::types::{CodexInstallation, InstallationSource, VersionGate};
use super::version::{compare_versions, gate_version, parse_codex_version_output};
use crate::codex::sanitize::sanitize_string;
use serde::Serialize;
use std::cmp::Ordering;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;

#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Clone)]
pub struct CapturedRun {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub error: Option<String>,
}

/// Runs a short-lived command (no shell) with a timeout and captures its output.
pub async fn run_capture(program: &Path, args: &[&str], timeout: Duration) -> CapturedRun {
    let mut cmd = Command::new(program);
    cmd.args(args).stdin(std::process::Stdio::null()).kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    match tokio::time::timeout(timeout, cmd.output()).await {
        Ok(Ok(out)) => CapturedRun {
            code: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            error: None,
        },
        Ok(Err(e)) => CapturedRun { code: None, stdout: String::new(), stderr: String::new(), error: Some(e.to_string()) },
        Err(_) => CapturedRun { code: None, stdout: String::new(), stderr: String::new(), error: Some("timed out".into()) },
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Rejected {
    pub path: PathBuf,
    pub source: InstallationSource,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryResult {
    pub candidates: Vec<CodexInstallation>,
    pub rejected: Vec<Rejected>,
    pub selected: Option<CodexInstallation>,
    pub selection_reason: String,
}

#[derive(Debug, Clone, Default)]
pub struct DiscoveryOptions {
    /// Explicit executable (future Settings field); also read from QUOTADOCK_CODEX_PATH.
    pub manual_path: Option<PathBuf>,
    pub allow_unverified_version: bool,
}

struct CandidatePath {
    path: PathBuf,
    source: InstallationSource,
    detail: String,
}

/// Codex Desktop (Microsoft Store): ask Windows where the package lives.
async fn desktop_package_candidates() -> Vec<CandidatePath> {
    if !cfg!(windows) {
        return vec![];
    }
    let run = run_capture(
        Path::new("powershell.exe"),
        &["-NoProfile", "-NonInteractive", "-Command", "Get-AppxPackage -Name OpenAI.Codex | ForEach-Object { $_.InstallLocation }"],
        Duration::from_secs(20),
    )
    .await;
    run.stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|loc| CandidatePath {
            path: Path::new(loc).join("app").join("resources").join("codex.exe"),
            source: InstallationSource::Desktop,
            detail: "Codex Desktop package (Get-AppxPackage OpenAI.Codex)".into(),
        })
        .collect()
}

fn read_dir_names(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|it| it.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default()
}

/// Copies Codex Desktop stages under %LOCALAPPDATA%\OpenAI\Codex\bin\<hash>\.
fn desktop_runtime_candidates() -> Vec<CandidatePath> {
    let Some(base) = dirs::data_local_dir().map(|d| d.join("OpenAI").join("Codex").join("bin")) else { return vec![] };
    read_dir_names(&base)
        .into_iter()
        .map(|d| base.join(d).join("codex.exe"))
        .filter(|p| p.exists())
        .map(|path| CandidatePath { path, source: InstallationSource::Desktop, detail: "runtime copy staged by Codex Desktop".into() })
        .collect()
}

async fn standalone_candidates() -> Vec<CandidatePath> {
    let mut out = Vec::new();
    let which = if cfg!(windows) {
        run_capture(Path::new("where.exe"), &["codex"], Duration::from_secs(10)).await
    } else {
        run_capture(Path::new("which"), &["-a", "codex"], Duration::from_secs(10)).await
    };
    for line in which.stdout.lines().map(str::trim).filter(|l| !l.is_empty()) {
        // npm shims (codex.cmd / codex.ps1) would need a shell; only native executables are used.
        if !cfg!(windows) || line.to_ascii_lowercase().ends_with(".exe") {
            out.push(CandidatePath { path: PathBuf::from(line), source: InstallationSource::Standalone, detail: "codex on PATH".into() });
        }
    }
    if let Some(appdata) = dirs::data_dir() {
        let vendor = appdata.join("npm").join("node_modules").join("@openai").join("codex").join("vendor");
        for target in read_dir_names(&vendor) {
            let p = vendor.join(&target).join("codex").join("codex.exe");
            if p.exists() {
                out.push(CandidatePath {
                    path: p,
                    source: InstallationSource::Standalone,
                    detail: format!("npm global @openai/codex ({target})"),
                });
            }
        }
    }
    out
}

fn vscode_candidates() -> Vec<CandidatePath> {
    let Some(home) = dirs::home_dir() else { return vec![] };
    let (platform_dir, exe) = if cfg!(windows) { ("windows-x86_64", "codex.exe") } else { ("linux-x86_64", "codex") };
    let mut out = Vec::new();
    for editor in [".vscode", ".vscode-insiders", ".cursor", ".windsurf"] {
        let root = home.join(editor).join("extensions");
        for dir in read_dir_names(&root).into_iter().filter(|d| d.starts_with("openai.chatgpt-")) {
            let p = root.join(&dir).join("bin").join(platform_dir).join(exe);
            if p.exists() {
                out.push(CandidatePath { path: p, source: InstallationSource::Vscode, detail: format!("{editor} extension {dir}") });
            }
        }
    }
    out
}

fn canonical_key(p: &Path) -> String {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()).to_string_lossy().to_ascii_lowercase()
}

/// Manual configuration wins; otherwise the highest acceptable version, ties broken by source.
pub fn select_installation(candidates: &[CodexInstallation], allow_unverified: bool) -> (Option<CodexInstallation>, String) {
    let acceptable = |c: &CodexInstallation| {
        matches!(c.gate, VersionGate::Verified | VersionGate::UnverifiedNewer)
            || (allow_unverified && c.gate == VersionGate::UnsupportedOlder)
    };
    if let Some(manual) = candidates.iter().find(|c| c.source == InstallationSource::Manual) {
        return if acceptable(manual) {
            (Some(manual.clone()), "manually configured executable".into())
        } else {
            (None, format!("manually configured executable has {:?} version {}", manual.gate, manual.version))
        };
    }
    let mut ok: Vec<&CodexInstallation> = candidates.iter().filter(|c| acceptable(c)).collect();
    if ok.is_empty() {
        let reason = if candidates.is_empty() { "no codex executable found" } else { "no executable with a verified or newer version" };
        return (None, reason.into());
    }
    ok.sort_by(|a, b| {
        compare_versions(&b.version, &a.version).unwrap_or(Ordering::Equal).then(a.source.priority().cmp(&b.source.priority()))
    });
    let best = ok[0].clone();
    let reason = format!("highest acceptable version ({}, {:?}) from {:?}", best.version, best.gate, best.source);
    (Some(best), reason)
}

pub async fn discover(options: &DiscoveryOptions) -> DiscoveryResult {
    let manual = options.manual_path.clone().or_else(|| std::env::var_os("QUOTADOCK_CODEX_PATH").map(PathBuf::from));
    let mut all: Vec<CandidatePath> = Vec::new();
    if let Some(p) = manual {
        all.push(CandidatePath { path: p, source: InstallationSource::Manual, detail: "explicitly configured".into() });
    }
    all.extend(desktop_package_candidates().await);
    all.extend(desktop_runtime_candidates());
    all.extend(standalone_candidates().await);
    all.extend(vscode_candidates());

    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    let mut rejected = Vec::new();
    for c in all {
        if !seen.insert(canonical_key(&c.path)) {
            continue;
        }
        if !c.path.exists() {
            rejected.push(Rejected { path: c.path, source: c.source, reason: "file does not exist".into() });
            continue;
        }
        let run = run_capture(&c.path, &["--version"], Duration::from_secs(15)).await;
        match parse_codex_version_output(&run.stdout) {
            Some(version) => {
                let gate = gate_version(&version);
                candidates.push(CodexInstallation { path: c.path, version, source: c.source, detail: c.detail, gate });
            }
            None => {
                let why = run.error.unwrap_or_else(|| run.stderr.lines().next().unwrap_or("").to_string());
                rejected.push(Rejected { path: c.path, source: c.source, reason: format!("--version failed: {}", sanitize_string(&why)) });
            }
        }
    }
    candidates.sort_by(|a, b| {
        a.source.priority().cmp(&b.source.priority()).then(compare_versions(&b.version, &a.version).unwrap_or(Ordering::Equal))
    });
    let (selected, selection_reason) = select_installation(&candidates, options.allow_unverified_version);
    DiscoveryResult { candidates, rejected, selected, selection_reason }
}

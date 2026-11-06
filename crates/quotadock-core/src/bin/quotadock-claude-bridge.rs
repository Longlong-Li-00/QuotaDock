//! QuotaDock status-line bridge, run by Claude Code on status-line updates (opt-in).
//!
//! stdin JSON -> keep only `rate_limits` (+ version, model name, hashed session id) -> atomic write to
//! `%USERPROFILE%\.quotadock\claude\statusline.json` -> print one status line. With
//! `--chain-b64 <command>` the user's previous status-line command runs with the same input and its
//! output is printed instead. No network, no SQLite, never blocks Claude Code for long.

#![cfg_attr(not(debug_assertions), windows_subsystem = "console")]

use quotadock_core::claude::integration::b64_decode;
use quotadock_core::claude::statusline::{self, BridgeRecord};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const CHAIN_TIMEOUT: Duration = Duration::from_secs(5);

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Same shell choice as Claude Code on Windows: Git Bash when installed, otherwise PowerShell.
fn shell_for(command: &str) -> Command {
    let git_bash = std::env::var_os("CLAUDE_CODE_GIT_BASH_PATH")
        .map(PathBuf::from)
        .into_iter()
        .chain(["C:\\Program Files\\Git\\bin\\bash.exe", "C:\\Program Files (x86)\\Git\\bin\\bash.exe"].iter().map(PathBuf::from));
    for bash in git_bash {
        if bash.is_file() {
            let mut c = Command::new(bash);
            c.args(["-c", command]);
            return c;
        }
    }
    let mut c = Command::new("powershell");
    c.args(["-NoProfile", "-NonInteractive", "-Command", command]);
    c
}

fn run_chain(command: &str, input: &[u8]) -> Option<String> {
    let mut cmd = shell_for(command);
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = cmd.spawn().ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input);
    }
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < CHAIN_TIMEOUT => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                break;
            }
        }
    }
    reader.join().ok()
}

fn main() {
    let mut input = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut input);
    let rec: BridgeRecord = statusline::extract(&statusline::parse_input(&input), now_ms());
    // The drop folder is under the user profile (not AppData, see statusline.rs); a failed write
    // never breaks Claude Code.
    let dir = std::env::var_os("USERPROFILE").map(|home| statusline::bridge_dir(&PathBuf::from(home)));
    if let Some(dir) = &dir {
        let _ = statusline::write_record(dir, &rec);
    }
    let args: Vec<String> = std::env::args().collect();
    let chained = args
        .iter()
        .position(|a| a == "--chain-b64")
        .and_then(|i| args.get(i + 1))
        .and_then(|b| b64_decode(b))
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .and_then(|command| run_chain(&command, &input));
    let display = dir.as_deref().map(statusline::read_display).unwrap_or_default();
    let line = chained.unwrap_or_else(|| statusline::status_text(&rec, display));
    let mut out = std::io::stdout();
    let _ = out.write_all(line.trim_end_matches(['\r', '\n']).as_bytes());
    let _ = out.flush();
}

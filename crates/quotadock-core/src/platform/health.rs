//! Health observation for long runs (Windows V1 soak): an append-only JSON-lines log next to the
//! database, independent of the SQLite connection, so it keeps working when the database is stuck.
//!
//! ```text
//! app_start / app_stop          lifecycle (version, pid, launch kind); one app_stop per process,
//!                               "session_end" when Windows logs off or shuts down
//! foreground / paused           window and pause state changes (to explain cadence)
//! monitoring_gap / gap_end      MONITORING_GAP: expected cadence clearly exceeded [QUOTADOCK HEURISTIC]
//! db_busy                       the database mutex stayed held for several watchdog ticks
//! power                         system suspend / resume (PowerRegisterSuspendResumeNotification)
//! resources                     working set / private bytes / handles / CPU of QuotaDock, its
//!                               WebView2 tree, its codex app-server and claude children
//! claude_env / claude_active    Claude Code version or auth mode changes; active source suspended
//! ```
//!
//! Nothing here is a quota event, and nothing here contains prompts, paths of user files, account
//! values or credentials.

use serde_json::{Value, json};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

pub const HEALTH_FILE: &str = "health.jsonl";
const MAX_BYTES: u64 = 4 * 1024 * 1024;

pub struct HealthLog {
    path: PathBuf,
    file: Mutex<Option<File>>,
    stopped: AtomicBool,
}

impl HealthLog {
    /// `<data_dir>\logs\health.jsonl` (rotated to `health.1.jsonl` at 4 MB).
    pub fn new(data_dir: &Path) -> Self {
        Self { path: data_dir.join("logs").join(HEALTH_FILE), file: Mutex::new(None), stopped: AtomicBool::new(false) }
    }

    /// The process's one `app_stop` line: a second quit, or the exit event after a quit, adds none.
    pub fn record_stop(&self, reason: &str) {
        if !self.stopped.swap(true, Ordering::SeqCst) {
            self.record("app_stop", json!({ "reason": reason }));
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Appends `{"t": <ms>, "kind": <kind>, ...fields}`. Failures are ignored: health logging must
    /// never affect monitoring.
    pub fn record(&self, kind: &str, fields: Value) {
        let mut line = json!({ "t": crate::time::now_ms(), "kind": kind });
        if let (Some(obj), Value::Object(extra)) = (line.as_object_mut(), fields) {
            obj.extend(extra);
        }
        let mut guard = match self.file.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if guard.is_none() {
            if let Some(dir) = self.path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            *guard = OpenOptions::new().create(true).append(true).open(&self.path).ok();
        }
        let too_big = guard.as_ref().and_then(|f| f.metadata().ok()).is_some_and(|m| m.len() > MAX_BYTES);
        if too_big {
            *guard = None;
            let _ = std::fs::rename(&self.path, self.path.with_file_name("health.1.jsonl"));
            *guard = OpenOptions::new().create(true).append(true).open(&self.path).ok();
        }
        if let Some(f) = guard.as_mut() {
            let _ = writeln!(f, "{line}");
        }
    }
}

/// Open/close bookkeeping for one MONITORING_GAP (the watchdog calls `observe` every tick).
#[derive(Debug, Default)]
pub struct GapTracker {
    open_since_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GapChange {
    Opened { since_ms: i64, idle_ms: i64 },
    Closed { since_ms: i64, duration_ms: i64 },
}

impl GapTracker {
    /// `last_activity_ms`: the newest sign of life (poll start, Claude update or attempt).
    /// `threshold_ms`: None while no cadence is expected (paused, disabled, backing off).
    pub fn observe(&mut self, last_activity_ms: Option<i64>, threshold_ms: Option<i64>, now_ms: i64) -> Option<GapChange> {
        let last = last_activity_ms?;
        match (self.open_since_ms, threshold_ms) {
            (None, Some(t)) if now_ms - last > t => {
                self.open_since_ms = Some(last);
                Some(GapChange::Opened { since_ms: last, idle_ms: now_ms - last })
            }
            (Some(since), _) if last > since => {
                self.open_since_ms = None;
                Some(GapChange::Closed { since_ms: since, duration_ms: last - since })
            }
            _ => None,
        }
    }
}

/// Expected cadence and MONITORING_GAP threshold [QUOTADOCK HEURISTIC]: Codex 60 s foreground
/// (gap > 3x) / 300 s background (> 2x); Claude active 5 min / 15 min (> 2x). A longer scheduled
/// delay (backoff after failures) raises the expectation instead of counting as a gap.
pub fn codex_gap_threshold_ms(foreground: bool, scheduled_delay_ms: Option<u64>) -> i64 {
    let cadence = if foreground { 60_000 } else { 300_000 };
    let expected = scheduled_delay_ms.map(|d| d as i64).unwrap_or(cadence).max(cadence);
    expected * if foreground { 3 } else { 2 }
}

/// Twice the active Claude interval. `foreground_for_ms`: how long the window has been in front.
/// Until one foreground threshold has passed since the switch, the background one still holds: a
/// read planned on the background cadence is not late just because the window opened (backlog 36).
pub fn claude_gap_threshold_ms(foreground: bool, foreground_for_ms: Option<i64>) -> i64 {
    use crate::claude::monitor::{ACTIVE_BACKGROUND_MS, ACTIVE_FOREGROUND_MS};
    let just_switched = foreground_for_ms.is_some_and(|d| d < 2 * ACTIVE_FOREGROUND_MS);
    2 * if foreground && !just_switched { ACTIVE_FOREGROUND_MS } else { ACTIVE_BACKGROUND_MS }
}

/// One process in a resource sample.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProcStat {
    pub pid: u32,
    pub working_set: u64,
    pub private_bytes: u64,
    pub handles: u32,
    pub cpu_ms: u64,
}

#[derive(Debug, Clone, Default)]
pub struct ProcessEntry {
    pub pid: u32,
    pub parent: u32,
    pub name: String,
}

/// Descendants of `root` (not including it) in a process list.
pub fn descendants(list: &[ProcessEntry], root: u32) -> Vec<&ProcessEntry> {
    let mut out: Vec<&ProcessEntry> = Vec::new();
    let mut frontier = vec![root];
    while let Some(p) = frontier.pop() {
        for e in list.iter().filter(|e| e.parent == p && e.pid != root && e.pid != 0) {
            if !out.iter().any(|o| o.pid == e.pid) {
                out.push(e);
                frontier.push(e.pid);
            }
        }
    }
    out
}

/// The resource sample written as `resources` (sizes in bytes, CPU in ms since process start).
pub fn resource_sample(foreground: bool) -> Value {
    let me = std::process::id();
    let list = process_list();
    let tree = descendants(&list, me);
    let sum = |name: &str| -> (usize, ProcStat) {
        let stats: Vec<ProcStat> = tree.iter().filter(|e| e.name.eq_ignore_ascii_case(name)).filter_map(|e| proc_stat(e.pid)).collect();
        let total = stats.iter().fold(ProcStat::default(), |a, s| ProcStat {
            pid: 0,
            working_set: a.working_set + s.working_set,
            private_bytes: a.private_bytes + s.private_bytes,
            handles: a.handles + s.handles,
            cpu_ms: a.cpu_ms + s.cpu_ms,
        });
        (stats.len(), total)
    };
    let own = proc_stat(me).unwrap_or_default();
    let (webviews, webview) = sum("msedgewebview2.exe");
    let (codex_n, codex) = sum("codex.exe");
    let (claude_n, claude) = sum("claude.exe");
    // codex.exe / claude.exe whose parent no longer exists: candidates for orphans (any origin).
    let alive: std::collections::HashSet<u32> = list.iter().map(|e| e.pid).collect();
    let orphan = |name: &str| list.iter().filter(|e| e.name.eq_ignore_ascii_case(name) && !alive.contains(&e.parent)).count();
    json!({
        "foreground": foreground,
        "app": { "ws": own.working_set, "private": own.private_bytes, "handles": own.handles, "cpuMs": own.cpu_ms },
        "webview": { "count": webviews, "ws": webview.working_set, "private": webview.private_bytes, "cpuMs": webview.cpu_ms },
        "codex": { "count": codex_n, "ws": codex.working_set, "private": codex.private_bytes, "cpuMs": codex.cpu_ms },
        "claude": { "count": claude_n, "ws": claude.working_set, "cpuMs": claude.cpu_ms },
        "orphanCandidates": { "codex": orphan("codex.exe"), "claude": orphan("claude.exe") },
    })
}

#[cfg(windows)]
pub fn process_list() -> Vec<ProcessEntry> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS};
    let mut out = Vec::new();
    // SAFETY: standard Toolhelp enumeration; the snapshot handle is closed below.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE || snap.is_null() {
            return out;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut e) != 0;
        while ok {
            let end = e.szExeFile.iter().position(|c| *c == 0).unwrap_or(e.szExeFile.len());
            out.push(ProcessEntry { pid: e.th32ProcessID, parent: e.th32ParentProcessID, name: String::from_utf16_lossy(&e.szExeFile[..end]) });
            ok = Process32NextW(snap, &mut e) != 0;
        }
        CloseHandle(snap);
    }
    out
}

#[cfg(not(windows))]
pub fn process_list() -> Vec<ProcessEntry> {
    Vec::new()
}

#[cfg(windows)]
pub fn proc_stat(pid: u32) -> Option<ProcStat> {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::{
        GetProcessHandleCount, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
    };
    // SAFETY: read-only queries on a handle opened (and closed) here.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut mem: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        mem.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        let have_mem = K32GetProcessMemoryInfo(h, &mut mem, mem.cb) != 0;
        let mut handles = 0u32;
        GetProcessHandleCount(h, &mut handles);
        let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut c, mut x, mut k, mut u) = (zero, zero, zero, zero);
        let have_times = GetProcessTimes(h, &mut c, &mut x, &mut k, &mut u) != 0;
        CloseHandle(h);
        let ft = |f: FILETIME| ((f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64) / 10_000;
        Some(ProcStat {
            pid,
            working_set: if have_mem { mem.WorkingSetSize as u64 } else { 0 },
            private_bytes: if have_mem { mem.PagefileUsage as u64 } else { 0 },
            handles,
            cpu_ms: if have_times { ft(k) + ft(u) } else { 0 },
        })
    }
}

#[cfg(not(windows))]
pub fn proc_stat(_pid: u32) -> Option<ProcStat> {
    None
}

static POWER_LOG: std::sync::OnceLock<std::sync::Arc<HealthLog>> = std::sync::OnceLock::new();

/// Records system suspend / resume into `log` for the rest of the process lifetime.
#[cfg(windows)]
pub fn watch_power_events(log: std::sync::Arc<HealthLog>) -> bool {
    use windows_sys::Win32::System::Power::{DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS, PowerRegisterSuspendResumeNotification};
    use windows_sys::Win32::UI::WindowsAndMessaging::DEVICE_NOTIFY_CALLBACK;
    unsafe extern "system" fn on_power(_ctx: *const core::ffi::c_void, kind: u32, _setting: *const core::ffi::c_void) -> u32 {
        // PBT_APMSUSPEND = 4, PBT_APMRESUMESUSPEND = 7, PBT_APMRESUMEAUTOMATIC = 18
        let event = match kind {
            4 => "suspend",
            7 => "resume_user",
            18 => "resume_automatic",
            _ => return 0,
        };
        if let Some(log) = POWER_LOG.get() {
            log.record("power", json!({ "event": event }));
        }
        0
    }
    if POWER_LOG.set(log).is_err() {
        return true; // already registered
    }
    // The registration lives as long as the process; the parameters must stay valid, so leak them.
    let params = Box::leak(Box::new(DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS { Callback: Some(on_power), Context: std::ptr::null_mut() }));
    let mut handle: *mut core::ffi::c_void = std::ptr::null_mut();
    // SAFETY: `params` is 'static; the callback only reads a OnceLock.
    let rc = unsafe { PowerRegisterSuspendResumeNotification(DEVICE_NOTIFY_CALLBACK, params as *mut _ as *mut _, &mut handle) };
    rc == 0
}

#[cfg(not(windows))]
pub fn watch_power_events(log: std::sync::Arc<HealthLog>) -> bool {
    let _ = POWER_LOG.set(log);
    false
}

static SESSION_LOG: std::sync::OnceLock<std::sync::Arc<HealthLog>> = std::sync::OnceLock::new();

/// Records `app_stop` with reason "session_end" when Windows ends the session (log off, restart,
/// shut down): the app is then ended without its own quit path. Uses a hidden top-level window on
/// its own thread, the only kind that receives WM_ENDSESSION; it is never shown.
#[cfg(windows)]
pub fn watch_session_end(log: std::sync::Arc<HealthLog>) -> bool {
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, MSG, RegisterClassW, TranslateMessage, WM_ENDSESSION, WNDCLASSW, WS_OVERLAPPED,
    };
    unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if msg == WM_ENDSESSION && wparam != 0
            && let Some(log) = SESSION_LOG.get()
        {
            // ENDSESSION_CLOSEAPP (1): Restart Manager closing it for an installer, not the session.
            log.record_stop(if lparam & 1 != 0 { "restart_manager" } else { "session_end" });
        }
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }
    if SESSION_LOG.set(log).is_err() {
        return true; // already watching
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new().name("session-end".into()).spawn(move || unsafe {
        let class: Vec<u16> = "QuotaDockSessionEnd\0".encode_utf16().collect();
        let instance = GetModuleHandleW(std::ptr::null());
        let wc = WNDCLASSW { lpfnWndProc: Some(proc), hInstance: instance, lpszClassName: class.as_ptr(), ..std::mem::zeroed() };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(0, class.as_ptr(), class.as_ptr(), WS_OVERLAPPED, 0, 0, 0, 0, std::ptr::null_mut(), std::ptr::null_mut(), instance, std::ptr::null());
        let _ = tx.send(!hwnd.is_null());
        if hwnd.is_null() {
            return;
        }
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    });
    spawned.is_ok() && rx.recv().unwrap_or(false)
}

#[cfg(not(windows))]
pub fn watch_session_end(log: std::sync::Arc<HealthLog>) -> bool {
    let _ = SESSION_LOG.set(log);
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_are_json_lines_with_time_and_kind() {
        let dir = tempfile::tempdir().unwrap();
        let log = HealthLog::new(dir.path());
        log.record("app_start", json!({ "version": "0.1.0" }));
        log.record("foreground", json!({ "value": true }));
        let text = std::fs::read_to_string(log.path()).unwrap();
        let lines: Vec<Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["kind"], "app_start");
        assert_eq!(lines[0]["version"], "0.1.0");
        assert!(lines[1]["t"].as_i64().unwrap() > 0);
    }

    #[test]
    fn claude_keeps_the_background_threshold_right_after_the_window_opens() {
        use crate::claude::monitor::{ACTIVE_BACKGROUND_MS, ACTIVE_FOREGROUND_MS};
        assert_eq!(claude_gap_threshold_ms(false, None), 2 * ACTIVE_BACKGROUND_MS);
        assert_eq!(claude_gap_threshold_ms(true, Some(60_000)), 2 * ACTIVE_BACKGROUND_MS, "just opened: no gap yet");
        assert_eq!(claude_gap_threshold_ms(true, Some(2 * ACTIVE_FOREGROUND_MS)), 2 * ACTIVE_FOREGROUND_MS, "in front long enough");
        assert_eq!(claude_gap_threshold_ms(true, None), 2 * ACTIVE_FOREGROUND_MS);
    }

    #[test]
    fn one_app_stop_per_process_whichever_path_writes_it_first() {
        let dir = tempfile::tempdir().unwrap();
        let log = HealthLog::new(dir.path());
        log.record_stop("quit");
        log.record_stop("quit");
        log.record_stop("exit");
        let text = std::fs::read_to_string(log.path()).unwrap();
        assert_eq!(text.lines().count(), 1);
        assert!(text.contains(r#""reason":"quit""#));
    }

    #[test]
    fn a_gap_opens_once_and_closes_when_activity_resumes() {
        let mut g = GapTracker::default();
        let t = codex_gap_threshold_ms(true, Some(60_000));
        assert_eq!(t, 180_000, "foreground: 3 x 60 s");
        assert_eq!(codex_gap_threshold_ms(false, Some(300_000)), 600_000, "background: 2 x 300 s");
        assert_eq!(codex_gap_threshold_ms(true, Some(900_000)), 2_700_000, "backoff raises the expectation");
        assert_eq!(g.observe(Some(0), Some(t), 100_000), None);
        assert_eq!(g.observe(Some(0), Some(t), 200_000), Some(GapChange::Opened { since_ms: 0, idle_ms: 200_000 }));
        assert_eq!(g.observe(Some(0), Some(t), 400_000), None, "reported once");
        assert_eq!(g.observe(Some(450_000), Some(t), 451_000), Some(GapChange::Closed { since_ms: 0, duration_ms: 450_000 }));
        assert_eq!(g.observe(Some(450_000), None, 9_000_000), None, "no expectation while paused");
    }

    #[test]
    fn process_trees_are_followed_through_children() {
        let p = |pid, parent, name: &str| ProcessEntry { pid, parent, name: name.into() };
        let list = vec![p(10, 1, "quotadock-app.exe"), p(11, 10, "msedgewebview2.exe"), p(12, 11, "msedgewebview2.exe"), p(13, 10, "codex.exe"), p(14, 2, "codex.exe")];
        let d: Vec<u32> = descendants(&list, 10).iter().map(|e| e.pid).collect();
        assert_eq!(d.len(), 3);
        assert!(d.contains(&12) && !d.contains(&14));
        // A real sample of this test process does not fail.
        let s = resource_sample(false);
        assert!(s["app"]["ws"].as_u64().unwrap_or(0) > 0 || cfg!(not(windows)));
    }
}

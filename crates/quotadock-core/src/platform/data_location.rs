//! Where QuotaDock's data lives, and the check that must pass before any SQLite database opens.
//!
//! Windows redirects `%LOCALAPPDATA%` writes of processes running inside an MSIX package (e.g. a
//! shell in Claude Desktop's Code tab) to that package's private store [LOCAL TEST, Phase 3.5]. A
//! QuotaDock started there would silently create a second database. So, before anything is
//! opened:
//!
//! ```text
//! unpackaged                      -> %LOCALAPPDATA%\QuotaDock (canonical; QUOTADOCK_DATA_DIR overrides)
//! packaged, no relaunch marker    -> relaunch once through the Explorer shell (outside any package)
//! packaged, relaunch marker       -> fail closed: message, exit, no SQLite
//! ```
//!
//! Older databases left in a package store are never merged, copied, moved or deleted here.

use std::path::{Path, PathBuf};

pub const APP_DIR: &str = "QuotaDock";
pub const DATABASE_FILE: &str = crate::storage::database::DATABASE_FILE;
/// Internal marker added to the one relaunch; never accepted from anywhere else as a command.
pub const RELAUNCH_MARKER: &str = "--quotadock-unpackaged-relaunch";
/// The only arguments carried over by the relaunch. Anything else is dropped.
pub const FORWARDED_ARGS: [&str; 2] = ["--hidden", "--autostart"];
pub const CLAUDE_PACKAGE_PREFIX: &str = "Claude_";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageContext {
    Unpackaged,
    /// This process has the package identity of Claude Desktop (`Claude_<publisher id>`).
    ClaudeDesktop(String),
    /// This process has another package identity.
    Other(String),
    /// No package identity, but new files under %LOCALAPPDATA% land in this package's private
    /// store. Observed for shells started by Claude Desktop's Code tab [LOCAL TEST, Phase 3.6]:
    /// GetCurrentPackageFamilyName reports no package there, so identity alone is not enough.
    Redirected(String),
}

impl PackageContext {
    /// From the package family name of the current process (`None` = no package identity).
    pub fn from_family_name(family: Option<&str>) -> Self {
        match family {
            None => Self::Unpackaged,
            Some(f) if f.starts_with(CLAUDE_PACKAGE_PREFIX) => Self::ClaudeDesktop(f.to_string()),
            Some(f) => Self::Other(f.to_string()),
        }
    }

    /// Package identity first (GetCurrentPackageFamilyName); without one, a write probe under
    /// %LOCALAPPDATA% shows whether file-system virtualization applies anyway.
    pub fn current() -> Self {
        let by_identity = Self::from_family_name(current_package_family().as_deref());
        if by_identity.is_packaged() {
            return by_identity;
        }
        match dirs::data_local_dir().and_then(|d| redirect_probe(&d)) {
            Some(family) => Self::Redirected(family),
            None => Self::Unpackaged,
        }
    }

    pub fn is_packaged(&self) -> bool {
        !matches!(self, Self::Unpackaged)
    }

    pub fn family(&self) -> Option<&str> {
        match self {
            Self::Unpackaged => None,
            Self::ClaudeDesktop(f) | Self::Other(f) | Self::Redirected(f) => Some(f),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Unpackaged => "unpackaged",
            Self::ClaudeDesktop(_) => "claude_desktop_package",
            Self::Other(_) => "other_package",
            Self::Redirected(f) if f.starts_with(CLAUDE_PACKAGE_PREFIX) => "redirected_by_claude_desktop",
            Self::Redirected(_) => "redirected_by_package",
        }
    }
}

/// Creates a uniquely named empty file directly under `local_app_data`, looks for it in every
/// `Packages\<family>\LocalCache\Local`, then removes it. Returns the family whose store received
/// it (i.e. this process's new AppData files are virtualized), or None.
pub fn redirect_probe(local_app_data: &Path) -> Option<String> {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let name = format!(".quotadock-redirect-probe-{}-{nanos}.tmp", std::process::id());
    let probe = local_app_data.join(&name);
    std::fs::write(&probe, b"").ok()?;
    let found = find_in_package_stores(&local_app_data.join("Packages"), &name);
    let _ = std::fs::remove_file(&probe);
    found
}

/// The package family whose `LocalCache\Local` holds `file_name`.
pub fn find_in_package_stores(packages_dir: &Path, file_name: &str) -> Option<String> {
    std::fs::read_dir(packages_dir).ok()?.flatten().find_map(|entry| {
        let hit = entry.path().join("LocalCache").join("Local").join(file_name).is_file();
        hit.then(|| entry.file_name().to_string_lossy().into_owned())
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocationError {
    /// Running with package identity: the canonical folder would be virtualized.
    Packaged(PackageContext),
    NoLocalAppData,
    /// The resolved folder itself is inside a package store.
    Virtualized(PathBuf),
}

impl std::fmt::Display for LocationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Packaged(ctx) => write!(f, "running inside a packaged application ({})", ctx.as_str()),
            Self::NoLocalAppData => write!(f, "cannot resolve the local app data directory"),
            Self::Virtualized(p) => write!(f, "data directory is inside a package store: {}", p.display()),
        }
    }
}

impl std::error::Error for LocationError {}

/// A package's private store: `...\AppData\Local\Packages\<family>\LocalCache\...`.
fn inside_package_store(path: &Path) -> bool {
    let s = path.to_string_lossy().to_ascii_lowercase().replace('/', "\\");
    s.contains("\\packages\\") && s.contains("\\localcache\\")
}

/// The data directory, only if the package check passes. `override_dir` is `QUOTADOCK_DATA_DIR`.
pub fn data_dir_for(ctx: &PackageContext, local_app_data: Option<&Path>, override_dir: Option<&Path>) -> Result<PathBuf, LocationError> {
    if ctx.is_packaged() {
        return Err(LocationError::Packaged(ctx.clone()));
    }
    let dir = match override_dir {
        Some(d) => d.to_path_buf(),
        None => local_app_data.ok_or(LocationError::NoLocalAppData)?.join(APP_DIR),
    };
    if inside_package_store(&dir) {
        return Err(LocationError::Virtualized(dir));
    }
    Ok(dir)
}

/// The validated data directory of this process (nothing is created).
pub fn validated_data_dir() -> Result<PathBuf, LocationError> {
    let override_dir = std::env::var_os("QUOTADOCK_DATA_DIR").map(PathBuf::from);
    data_dir_for(&PackageContext::current(), dirs::data_local_dir().as_deref(), override_dir.as_deref())
}

/// The validated canonical database path (nothing is created or opened).
pub fn validated_database_path() -> Result<PathBuf, LocationError> {
    validated_data_dir().map(|d| d.join(DATABASE_FILE))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupDecision {
    /// Safe to open the database in this folder.
    Proceed { data_dir: PathBuf, relaunched: bool },
    /// Start again outside the package with these arguments, then exit.
    Relaunch { args: Vec<String> },
    /// Already relaunched once and still packaged (or no usable folder): show the message, exit.
    FailClosed(LocationError),
}

/// Decides what a starting QuotaDock does with `args` (without the program name). Pure.
pub fn startup_decision(
    ctx: &PackageContext,
    args: &[String],
    local_app_data: Option<&Path>,
    override_dir: Option<&Path>,
) -> StartupDecision {
    let relaunched = args.iter().any(|a| a == RELAUNCH_MARKER);
    match data_dir_for(ctx, local_app_data, override_dir) {
        Ok(data_dir) => StartupDecision::Proceed { data_dir, relaunched },
        Err(LocationError::Packaged(_)) if !relaunched => {
            let mut forwarded: Vec<String> = Vec::new();
            for a in args.iter().filter(|a| FORWARDED_ARGS.contains(&a.as_str())) {
                if !forwarded.contains(a) {
                    forwarded.push(a.clone());
                }
            }
            forwarded.push(RELAUNCH_MARKER.to_string());
            StartupDecision::Relaunch { args: forwarded }
        }
        Err(e) => StartupDecision::FailClosed(e),
    }
}

/// `startup_decision` for this process: its package identity, the Known Folder `LocalAppData`
/// and `QUOTADOCK_DATA_DIR`. Must run before anything opens SQLite.
pub fn decide_current(args: &[String]) -> StartupDecision {
    let override_dir = std::env::var_os("QUOTADOCK_DATA_DIR").map(PathBuf::from);
    startup_decision(&PackageContext::current(), args, dirs::data_local_dir().as_deref(), override_dir.as_deref())
}

/// Arguments the app should act on: the internal marker is not a command.
pub fn strip_marker(args: &[String]) -> Vec<String> {
    args.iter().filter(|a| *a != RELAUNCH_MARKER).cloned().collect()
}

pub const FAIL_CLOSED_TITLE: &str = "QuotaDock";
pub const FAIL_CLOSED_TEXT: &str = "QuotaDock was started inside another packaged application and cannot safely access its normal data store.\n\nPlease launch QuotaDock directly from Windows.\n\nQuotaDock 当前从其他打包应用内部启动，无法安全访问正常的数据目录。\n\n请直接从 Windows 启动 QuotaDock。";

#[cfg(windows)]
fn current_package_family() -> Option<String> {
    use windows_sys::Win32::Foundation::{APPMODEL_ERROR_NO_PACKAGE, ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS};
    use windows_sys::Win32::Storage::Packaging::Appx::GetCurrentPackageFamilyName;
    let mut len: u32 = 0;
    // SAFETY: a zero-length query returns the required length without writing.
    let rc = unsafe { GetCurrentPackageFamilyName(&mut len, std::ptr::null_mut()) };
    if rc == APPMODEL_ERROR_NO_PACKAGE {
        return None;
    }
    if rc != ERROR_INSUFFICIENT_BUFFER || len == 0 {
        // Unexpected: treat as packaged-unknown so the database is not opened blindly.
        return Some(format!("unknown-package-error-{rc}"));
    }
    let mut buf = vec![0u16; len as usize];
    // SAFETY: `buf` holds `len` UTF-16 units, as requested by the first call.
    let rc = unsafe { GetCurrentPackageFamilyName(&mut len, buf.as_mut_ptr()) };
    if rc != ERROR_SUCCESS {
        return Some(format!("unknown-package-error-{rc}"));
    }
    let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..end]))
}

#[cfg(not(windows))]
fn current_package_family() -> Option<String> {
    None
}

/// Starts `exe` with `args` through the Explorer shell's own process (Shell.Application of the
/// desktop folder view), so the new process has no package identity. `args` must come from
/// `startup_decision` (allow-listed); nothing from the command line is passed through unchecked.
#[cfg(windows)]
pub fn relaunch_via_shell(exe: &Path, args: &[String]) -> Result<(), String> {
    use windows::Win32::System::Com::{
        CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize, IDispatch, IServiceProvider,
    };
    use windows::Win32::UI::Shell::{
        IShellBrowser, IShellDispatch2, IShellFolderViewDual, IShellWindows, SID_STopLevelBrowser, SVGIO_BACKGROUND, SWC_DESKTOP,
        SWFO_NEEDDISPATCH, ShellWindows,
    };
    use windows::Win32::System::Variant::{VARIANT, VT_BSTR, VT_I4, VariantClear};
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::{BSTR, Interface};

    // The Win32 VARIANT is a plain union in windows 0.62: set the tag and the value by hand.
    fn variant_i4(v: i32) -> VARIANT {
        let mut var = VARIANT::default();
        // SAFETY: writing the VT_I4 member of a zeroed VARIANT.
        unsafe {
            let inner = &mut *var.Anonymous.Anonymous;
            inner.vt = VT_I4;
            inner.Anonymous.lVal = v;
        }
        var
    }
    fn variant_bstr(s: &str) -> VARIANT {
        let mut var = VARIANT::default();
        // SAFETY: writing the VT_BSTR member; VariantClear frees the string afterwards.
        unsafe {
            let inner = &mut *var.Anonymous.Anonymous;
            inner.vt = VT_BSTR;
            inner.Anonymous.bstrVal = std::mem::ManuallyDrop::new(BSTR::from(s));
        }
        var
    }

    for a in args {
        if a != RELAUNCH_MARKER && !FORWARDED_ARGS.contains(&a.as_str()) {
            return Err(format!("refusing to forward argument {a:?}"));
        }
    }
    let dir = exe.parent().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
    let joined = args.join(" ");
    // SAFETY: plain COM calls on this thread; every interface is released when dropped.
    unsafe {
        let init = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let result = (|| -> windows::core::Result<()> {
            let shell_windows: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_LOCAL_SERVER)?;
            let mut hwnd = 0i32;
            let desktop = variant_i4(0); // CSIDL_DESKTOP
            let disp = shell_windows.FindWindowSW(&desktop, &VARIANT::default(), SWC_DESKTOP, &mut hwnd, SWFO_NEEDDISPATCH)?;
            let provider: IServiceProvider = disp.cast()?;
            let browser: IShellBrowser = provider.QueryService(&SID_STopLevelBrowser)?;
            let view = browser.QueryActiveShellView()?;
            let background: IDispatch = view.GetItemObject(SVGIO_BACKGROUND)?;
            let folder_view: IShellFolderViewDual = background.cast()?;
            let shell: IShellDispatch2 = folder_view.Application()?.cast()?;
            let mut strings = [variant_bstr(&joined), variant_bstr(&dir), variant_bstr("open")];
            let r = shell.ShellExecute(
                &BSTR::from(exe.to_string_lossy().as_ref()),
                &strings[0],
                &strings[1],
                &strings[2],
                &variant_i4(SW_SHOWNORMAL.0),
            );
            for s in &mut strings {
                let _ = VariantClear(s);
            }
            r
        })();
        if init.is_ok() {
            CoUninitialize();
        }
        result.map_err(|e| e.to_string())
    }
}

#[cfg(not(windows))]
pub fn relaunch_via_shell(_exe: &Path, _args: &[String]) -> Result<(), String> {
    Err("not supported on this platform".into())
}

/// The fail-closed message (both languages: the UI language is not known before settings load).
#[cfg(windows)]
pub fn show_fail_closed_message() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONWARNING, MB_OK, MessageBoxW};
    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let (text, title) = (wide(FAIL_CLOSED_TEXT), wide(FAIL_CLOSED_TITLE));
    // SAFETY: both buffers are NUL-terminated UTF-16 and outlive the call.
    unsafe {
        MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_OK | MB_ICONWARNING);
    }
}

#[cfg(not(windows))]
pub fn show_fail_closed_message() {
    eprintln!("{FAIL_CLOSED_TEXT}");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn package_identity_is_classified_from_the_family_name() {
        assert_eq!(PackageContext::from_family_name(None), PackageContext::Unpackaged);
        assert!(matches!(PackageContext::from_family_name(Some("Claude_pzs8sxrjxfjjc")), PackageContext::ClaudeDesktop(_)));
        assert!(matches!(PackageContext::from_family_name(Some("Microsoft.WindowsTerminal_8wekyb3d8bbwe")), PackageContext::Other(_)));
        // Whatever this test process is, asking Windows must not fail.
        let _ = PackageContext::current();
    }

    #[test]
    fn a_probe_found_in_a_package_store_names_that_package() {
        let tmp = tempfile::tempdir().unwrap();
        let packages = tmp.path().join("Packages");
        let store = packages.join("Claude_pzs8sxrjxfjjc").join("LocalCache").join("Local");
        std::fs::create_dir_all(&store).unwrap();
        std::fs::create_dir_all(packages.join("Microsoft.Other_8wekyb3d8bbwe").join("LocalCache").join("Local")).unwrap();
        assert_eq!(find_in_package_stores(&packages, "probe.tmp"), None);
        std::fs::write(store.join("probe.tmp"), b"").unwrap();
        assert_eq!(find_in_package_stores(&packages, "probe.tmp").as_deref(), Some("Claude_pzs8sxrjxfjjc"));
        let ctx = PackageContext::Redirected("Claude_pzs8sxrjxfjjc".into());
        assert!(ctx.is_packaged());
        assert_eq!(ctx.as_str(), "redirected_by_claude_desktop");
        assert!(matches!(data_dir_for(&ctx, Some(tmp.path()), None), Err(LocationError::Packaged(_))));
        // A real probe in a plain folder finds nothing and leaves nothing behind.
        assert_eq!(redirect_probe(tmp.path()), None);
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 1, "only the Packages folder of this test");
    }

    #[test]
    fn unpackaged_uses_the_canonical_folder_and_creates_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let d = data_dir_for(&PackageContext::Unpackaged, Some(tmp.path()), None).unwrap();
        assert_eq!(d, tmp.path().join("QuotaDock"));
        assert!(!d.exists(), "validation only resolves, it never creates");
        let o = data_dir_for(&PackageContext::Unpackaged, Some(tmp.path()), Some(&tmp.path().join("custom"))).unwrap();
        assert_eq!(o, tmp.path().join("custom"));
    }

    #[test]
    fn packaged_or_virtualized_folders_are_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let claude = PackageContext::from_family_name(Some("Claude_pzs8sxrjxfjjc"));
        assert!(matches!(data_dir_for(&claude, Some(tmp.path()), None), Err(LocationError::Packaged(_))));
        assert!(matches!(data_dir_for(&claude, Some(tmp.path()), Some(tmp.path())), Err(LocationError::Packaged(_))), "no override escapes it");
        let store = Path::new(r"C:\Users\u\AppData\Local\Packages\Claude_pzs8sxrjxfjjc\LocalCache\Local");
        assert!(matches!(data_dir_for(&PackageContext::Unpackaged, Some(store), None), Err(LocationError::Virtualized(_))));
        assert!(matches!(data_dir_for(&PackageContext::Unpackaged, None, None), Err(LocationError::NoLocalAppData)));
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0, "nothing was created");
    }

    #[test]
    fn a_packaged_start_relaunches_once_with_allow_listed_arguments() {
        let ctx = PackageContext::from_family_name(Some("Claude_pzs8sxrjxfjjc"));
        let tmp = tempfile::tempdir().unwrap();
        let d = startup_decision(&ctx, &args(&["--hidden", "--quit", r"C:\evil.exe", "--autostart", "--hidden"]), Some(tmp.path()), None);
        assert_eq!(d, StartupDecision::Relaunch { args: args(&["--hidden", "--autostart", RELAUNCH_MARKER]) });
        // The relaunched process is still packaged: fail closed, never loop.
        let again = startup_decision(&ctx, &args(&["--hidden", RELAUNCH_MARKER]), Some(tmp.path()), None);
        assert!(matches!(again, StartupDecision::FailClosed(LocationError::Packaged(_))));
        // Outside the package the marker is harmless and is not a command.
        let ok = startup_decision(&PackageContext::Unpackaged, &args(&[RELAUNCH_MARKER, "--hidden"]), Some(tmp.path()), None);
        assert_eq!(ok, StartupDecision::Proceed { data_dir: tmp.path().join("QuotaDock"), relaunched: true });
        assert_eq!(strip_marker(&args(&[RELAUNCH_MARKER, "--hidden"])), args(&["--hidden"]));
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0, "no database or folder before Proceed");
    }

    #[cfg(windows)]
    #[test]
    fn the_shell_relaunch_refuses_unknown_arguments() {
        assert!(relaunch_via_shell(Path::new(r"C:\nowhere\quotadock-app.exe"), &args(&["--quit"])).is_err());
    }
}

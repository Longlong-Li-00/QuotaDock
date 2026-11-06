//! Windows logon start. The autostart plugin writes the HKCU Run value as `path --autostart`
//! without quotes; QuotaDock rewrites it in the installer's quoted form, so a profile path with a
//! space is never split into another program's name.

use std::path::Path;

pub const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// The Run value QuotaDock wants: the program path in quotes, then `--autostart`.
pub fn run_command(exe: &Path) -> String {
    format!("\"{}\" --autostart", exe.display())
}

/// Writes `run_command(exe)` as the HKCU Run value `name`. Only called right after the plugin has
/// written its own form of the same value; it never creates autostart on its own.
#[cfg(windows)]
pub fn write_run_value(name: &str, exe: &Path) -> Result<(), String> {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ, RegCloseKey, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW};
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let (key_name, value_name, data) = (wide(RUN_KEY), wide(name), wide(&run_command(exe)));
    unsafe {
        let mut key: HKEY = std::ptr::null_mut();
        let r = RegOpenKeyExW(HKEY_CURRENT_USER, key_name.as_ptr(), 0, KEY_SET_VALUE | KEY_QUERY_VALUE, &mut key);
        if r != ERROR_SUCCESS {
            return Err(format!("open Run key: {r}"));
        }
        // Only an existing value is rewritten.
        let exists = RegQueryValueExW(key, value_name.as_ptr(), std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) == ERROR_SUCCESS;
        let r = if exists {
            RegSetValueExW(key, value_name.as_ptr(), 0, REG_SZ, data.as_ptr().cast(), (data.len() * 2) as u32)
        } else {
            ERROR_SUCCESS
        };
        RegCloseKey(key);
        if r != ERROR_SUCCESS { Err(format!("write Run value: {r}")) } else { Ok(()) }
    }
}

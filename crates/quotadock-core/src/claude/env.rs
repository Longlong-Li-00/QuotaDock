//! The environment a Claude Code child process gets: what the user's own terminal would have.
//!
//! QuotaDock may itself be started from inside a Claude session (Claude Desktop injects
//! `CLAUDE_CODE_*`, `ANTHROPIC_BASE_URL`, a messaging socket/token, ...). Passing those on would make
//! the child an embedded Desktop session instead of the user's Claude Code, so every
//! `CLAUDE*` / `ANTHROPIC*` / `MCP_*` / `OTEL_*` variable is dropped and only the values the user or
//! machine actually defines (Windows user/system environment) are put back.
//!
//! Secret values are never logged or returned: `AuthEnvFlags` only says whether a variable exists.

use serde::Serialize;

const STRIPPED_PREFIXES: [&str; 4] = ["CLAUDE", "ANTHROPIC", "MCP_", "OTEL_"];

fn stripped(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    STRIPPED_PREFIXES.iter().any(|p| upper.starts_with(p))
}

/// Presence of variables that change Claude Code's billing/auth mode. Values are never read out.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthEnvFlags {
    pub api_key: bool,
    pub auth_token: bool,
    pub oauth_token: bool,
    pub bedrock: bool,
    pub vertex: bool,
    pub foundry: bool,
    pub custom_base_url: bool,
}

impl AuthEnvFlags {
    /// A variable that makes Claude Code bill something other than the claude.ai subscription.
    pub fn non_subscription_override(&self) -> bool {
        self.api_key || self.auth_token || self.bedrock || self.vertex || self.foundry
    }
}

/// Builds the child environment from the current process environment and the user's registry
/// environment (user values override machine values).
pub fn claude_child_env() -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = std::env::vars().filter(|(k, _)| !stripped(k)).collect();
    let mut defined: Vec<(String, String)> = Vec::new();
    for (k, v) in registry::machine_env().into_iter().chain(registry::user_env()) {
        if stripped(&k) {
            defined.retain(|(name, _)| !name.eq_ignore_ascii_case(&k));
            defined.push((k, v));
        }
    }
    env.extend(defined);
    env
}

pub fn auth_env_flags(env: &[(String, String)]) -> AuthEnvFlags {
    let has = |name: &str| env.iter().any(|(k, v)| k.eq_ignore_ascii_case(name) && !v.trim().is_empty());
    let truthy = |name: &str| {
        env.iter().any(|(k, v)| k.eq_ignore_ascii_case(name) && matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
    };
    AuthEnvFlags {
        api_key: has("ANTHROPIC_API_KEY"),
        auth_token: has("ANTHROPIC_AUTH_TOKEN"),
        oauth_token: has("CLAUDE_CODE_OAUTH_TOKEN"),
        bedrock: truthy("CLAUDE_CODE_USE_BEDROCK"),
        vertex: truthy("CLAUDE_CODE_USE_VERTEX"),
        foundry: truthy("CLAUDE_CODE_USE_FOUNDRY"),
        custom_base_url: has("ANTHROPIC_BASE_URL"),
    }
}

#[cfg(windows)]
mod registry {
    use windows_sys::Win32::Foundation::{ERROR_NO_MORE_ITEMS, ERROR_SUCCESS};
    use windows_sys::Win32::System::Environment::ExpandEnvironmentStringsW;
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, REG_EXPAND_SZ, REG_SZ, RegCloseKey, RegEnumValueW, RegOpenKeyExW,
    };

    pub fn user_env() -> Vec<(String, String)> {
        values(HKEY_CURRENT_USER, "Environment")
    }

    pub fn machine_env() -> Vec<(String, String)> {
        values(HKEY_LOCAL_MACHINE, r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment")
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    fn expand(s: &str) -> String {
        let src = wide(s);
        unsafe {
            let needed = ExpandEnvironmentStringsW(src.as_ptr(), std::ptr::null_mut(), 0);
            if needed == 0 {
                return s.to_string();
            }
            let mut buf = vec![0u16; needed as usize];
            let written = ExpandEnvironmentStringsW(src.as_ptr(), buf.as_mut_ptr(), needed);
            if written == 0 || written > needed {
                return s.to_string();
            }
            String::from_utf16_lossy(&buf[..(written as usize).saturating_sub(1)])
        }
    }

    fn values(root: HKEY, subkey: &str) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let sub = wide(subkey);
        unsafe {
            let mut key: HKEY = std::ptr::null_mut();
            if RegOpenKeyExW(root, sub.as_ptr(), 0, KEY_READ, &mut key) != ERROR_SUCCESS {
                return out;
            }
            let mut index = 0u32;
            loop {
                let mut name = vec![0u16; 16_384];
                let mut name_len = name.len() as u32;
                let mut kind = 0u32;
                let mut data = vec![0u8; 65_536];
                let mut data_len = data.len() as u32;
                let r = RegEnumValueW(
                    key,
                    index,
                    name.as_mut_ptr(),
                    &mut name_len,
                    std::ptr::null(),
                    &mut kind,
                    data.as_mut_ptr(),
                    &mut data_len,
                );
                index += 1;
                if r == ERROR_NO_MORE_ITEMS {
                    break;
                }
                if r != ERROR_SUCCESS || (kind != REG_SZ && kind != REG_EXPAND_SZ) {
                    continue;
                }
                let n = String::from_utf16_lossy(&name[..name_len as usize]);
                let units: Vec<u16> = data[..data_len as usize].as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
                let raw = String::from_utf16_lossy(&units);
                let v = raw.trim_end_matches('\0').to_string();
                out.push((n, if kind == REG_EXPAND_SZ { expand(&v) } else { v }));
            }
            RegCloseKey(key);
        }
        out
    }
}

#[cfg(not(windows))]
mod registry {
    pub fn user_env() -> Vec<(String, String)> {
        Vec::new()
    }
    pub fn machine_env() -> Vec<(String, String)> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_injected_session_variables_but_reports_presence_only() {
        assert!(stripped("CLAUDECODE") && stripped("CLAUDE_CODE_SESSION_ID") && stripped("ANTHROPIC_BASE_URL") && stripped("MCP_X"));
        assert!(!stripped("PATH") && !stripped("USERPROFILE"));
        let env =
            vec![("ANTHROPIC_API_KEY".to_string(), "sk-secret".to_string()), ("CLAUDE_CODE_USE_BEDROCK".to_string(), "0".to_string())];
        let f = auth_env_flags(&env);
        assert!(f.api_key && !f.bedrock && f.non_subscription_override());
        assert!(!format!("{f:?}").contains("sk-secret"));
    }
}

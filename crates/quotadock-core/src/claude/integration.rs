//! Opt-in status-line bridge installation into `~/.claude/settings.json`.
//!
//! Nothing here runs unless the user clicks "Enable Claude integration" after reviewing the exact
//! change. Enabling backs the file up, keeps every other key, and chains an existing status line
//! (the bridge runs it with the same input and prints its output). Disabling restores the previous
//! `statusLine` only if the current one is still exactly what QuotaDock wrote.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub const BRIDGE_EXE: &str = "quotadock-claude-bridge.exe";

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationPlan {
    pub settings_path: PathBuf,
    pub settings_exists: bool,
    pub current_status_line: Option<Value>,
    pub new_status_line: Value,
    pub chains_existing: bool,
    pub bridge_install_path: PathBuf,
    pub backup_path: PathBuf,
    /// When the preview was made. Enabling re-plans at this time, so the backup file is the one shown.
    pub planned_at_ms: i64,
    /// Fingerprint of what the user reviewed: the whole settings file, both statusLines and paths.
    pub plan_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationState {
    pub enabled: bool,
    pub installed_at_ms: i64,
    pub settings_path: PathBuf,
    pub previous_status_line: Option<Value>,
    pub installed_status_line: Value,
    pub backup_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DisableOutcome {
    Restored,
    /// The user changed statusLine after QuotaDock installed the bridge: left untouched.
    ChangedByUser,
    /// settings.json is gone or unreadable: nothing to restore.
    NothingToRestore,
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn b64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(B64[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let vals: Vec<u8> =
        s.bytes().filter(|c| *c != b'=').map(|c| B64.iter().position(|x| *x == c).map(|p| p as u8)).collect::<Option<_>>()?;
    for chunk in vals.chunks(4) {
        let mut n = 0u32;
        for (i, v) in chunk.iter().enumerate() {
            n |= (*v as u32) << (18 - 6 * i);
        }
        for i in 0..chunk.len().saturating_sub(1) {
            out.push((n >> (16 - 8 * i)) as u8);
        }
    }
    Some(out)
}

/// The command Claude Code will run. Windows: Git Bash or PowerShell [ANTHROPIC DOCS], so forward
/// slashes, `~` for the home directory, and a base64 argument for a chained command (no quoting).
pub fn bridge_command(bridge: &Path, home: &Path, previous: Option<&Value>) -> String {
    let path = match bridge.strip_prefix(home) {
        Ok(rel) => format!("~/{}", rel.to_string_lossy().replace('\\', "/")),
        Err(_) => bridge.to_string_lossy().replace('\\', "/"),
    };
    let path = if path.contains(' ') { format!("\"{path}\"") } else { path };
    match previous.and_then(|p| p.get("command")).and_then(Value::as_str).filter(|c| !c.trim().is_empty()) {
        Some(cmd) => format!("{path} --chain-b64 {}", b64_encode(cmd.as_bytes())),
        None => path,
    }
}

fn read_settings(path: &Path) -> Result<Option<Map<String, Value>>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(m)) => Ok(Some(m)),
            Ok(_) => Err("settings.json is not a JSON object".into()),
            Err(e) => Err(format!("settings.json is not valid JSON ({e}); not modifying it")),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

fn write_settings(path: &Path, map: &Map<String, Value>) -> Result<(), String> {
    let text = serde_json::to_string_pretty(&Value::Object(map.clone())).map_err(|e| e.to_string())? + "\n";
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("json.quotadock-tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

pub fn plan(
    settings_path: &Path,
    bridge_install_path: &Path,
    home: &Path,
    backup_dir: &Path,
    now_ms: i64,
) -> Result<IntegrationPlan, String> {
    let current = read_settings(settings_path)?;
    let current_status_line = current.as_ref().and_then(|m| m.get("statusLine").cloned());
    let mut new_status_line =
        json!({ "type": "command", "command": bridge_command(bridge_install_path, home, current_status_line.as_ref()) });
    // Keep the user's display options for the chained status line.
    if let Some(prev) = current_status_line.as_ref().and_then(Value::as_object) {
        for k in ["padding", "refreshInterval"] {
            if let Some(v) = prev.get(k) {
                new_status_line[k] = v.clone();
            }
        }
    }
    let chains_existing =
        current_status_line.as_ref().and_then(|p| p.get("command")).and_then(Value::as_str).is_some_and(|c| !c.trim().is_empty());
    let backup_path = backup_dir.join(format!("claude-settings.backup-{now_ms}.json"));
    let file_digest = std::fs::read(settings_path).map(|b| hex(&Sha256::digest(&b))).unwrap_or_else(|_| "absent".into());
    let reviewed = json!({
        "file": file_digest, "settings": settings_path, "current": current_status_line, "new": new_status_line,
        "bridge": bridge_install_path, "backup": backup_path,
    });
    let plan_id = hex(&Sha256::digest(reviewed.to_string().as_bytes())[..16]);
    Ok(IntegrationPlan {
        settings_path: settings_path.to_path_buf(),
        settings_exists: current.is_some(),
        current_status_line,
        new_status_line,
        chains_existing,
        bridge_install_path: bridge_install_path.to_path_buf(),
        backup_path,
        planned_at_ms: now_ms,
        plan_id,
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The plan about to be applied must be the one the user reviewed: any change to settings.json
/// since the preview (not only to statusLine) means showing the change again.
pub fn check_reviewed(plan: &IntegrationPlan, reviewed_plan_id: &str) -> Result<(), String> {
    if plan.plan_id == reviewed_plan_id {
        Ok(())
    } else {
        Err("Claude settings changed since the preview; review the change again".into())
    }
}

/// Backs up settings.json, then writes the planned statusLine. Refuses if the file changed since
/// the plan was shown.
pub fn enable(plan: &IntegrationPlan, now_ms: i64) -> Result<IntegrationState, String> {
    let current = read_settings(&plan.settings_path)?;
    if current.as_ref().and_then(|m| m.get("statusLine").cloned()) != plan.current_status_line {
        return Err("Claude settings changed since the preview; review the change again".into());
    }
    let backup_path = if current.is_some() {
        if let Some(dir) = plan.backup_path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::copy(&plan.settings_path, &plan.backup_path).map_err(|e| format!("backup failed: {e}"))?;
        Some(plan.backup_path.clone())
    } else {
        None
    };
    let mut map = current.unwrap_or_default();
    map.insert("statusLine".into(), plan.new_status_line.clone());
    write_settings(&plan.settings_path, &map)?;
    Ok(IntegrationState {
        enabled: true,
        installed_at_ms: now_ms,
        settings_path: plan.settings_path.clone(),
        previous_status_line: plan.current_status_line.clone(),
        installed_status_line: plan.new_status_line.clone(),
        backup_path,
    })
}

pub fn disable(state: &IntegrationState) -> Result<DisableOutcome, String> {
    let Some(mut map) = read_settings(&state.settings_path)? else { return Ok(DisableOutcome::NothingToRestore) };
    if map.get("statusLine") != Some(&state.installed_status_line) {
        return Ok(DisableOutcome::ChangedByUser);
    }
    match &state.previous_status_line {
        Some(prev) => {
            map.insert("statusLine".into(), prev.clone());
        }
        None => {
            map.remove("statusLine");
        }
    }
    write_settings(&state.settings_path, &map)?;
    Ok(DisableOutcome::Restored)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(content: Option<&str>) -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let settings = home.join(".claude").join("settings.json");
        std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
        if let Some(c) = content {
            std::fs::write(&settings, c).unwrap();
        }
        let bridge = home.join(".quotadock").join("bin").join(BRIDGE_EXE);
        (dir, home, settings, bridge)
    }

    #[test]
    fn base64_round_trip() {
        for s in ["", "a", "ab", "abc", "jq -r '.model.display_name' && echo \"$(date)\"", "powershell -NoProfile -File C:/x y/s.ps1"] {
            assert_eq!(b64_decode(&b64_encode(s.as_bytes())).unwrap(), s.as_bytes());
        }
    }

    #[test]
    fn no_existing_status_line_install_and_remove() {
        let (dir, home, settings, bridge) = setup(Some(r#"{"theme":"dark","autoUpdatesChannel":"latest"}"#));
        let p = plan(&settings, &bridge, &home, dir.path(), 7).unwrap();
        assert!(!p.chains_existing && p.current_status_line.is_none());
        assert_eq!(p.new_status_line["command"], "~/.quotadock/bin/quotadock-claude-bridge.exe");
        let st = enable(&p, 8).unwrap();
        let written: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(written["theme"], "dark", "other keys kept");
        assert_eq!(
            written.as_object().unwrap().keys().collect::<Vec<_>>(),
            vec!["theme", "autoUpdatesChannel", "statusLine"],
            "order kept"
        );
        assert!(st.backup_path.as_ref().unwrap().is_file());
        assert_eq!(disable(&st).unwrap(), DisableOutcome::Restored);
        let restored: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        assert!(restored.get("statusLine").is_none());
        assert_eq!(restored["theme"], "dark");
    }

    #[test]
    fn existing_simple_status_line_is_chained_and_restored() {
        let (dir, home, settings, bridge) = setup(Some(r#"{"statusLine":{"type":"command","command":"echo hi","padding":1}}"#));
        let p = plan(&settings, &bridge, &home, dir.path(), 7).unwrap();
        assert!(p.chains_existing);
        let cmd = p.new_status_line["command"].as_str().unwrap();
        let b64 = cmd.split("--chain-b64 ").nth(1).unwrap();
        assert_eq!(b64_decode(b64).unwrap(), b"echo hi");
        assert_eq!(p.new_status_line["padding"], 1, "display options kept");
        let st = enable(&p, 8).unwrap();
        assert_eq!(disable(&st).unwrap(), DisableOutcome::Restored);
        let restored: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(restored["statusLine"], json!({"type":"command","command":"echo hi","padding":1}));
    }

    #[test]
    fn existing_complex_status_line_survives_and_user_edits_are_respected() {
        let complex = r#"{"statusLine":{"type":"command","command":"bash -c 'input=$(cat); echo \"$input\" | jq -r \".model.display_name\" | sed \"s/ /_/g\"'","refreshInterval":5},"permissions":{"allow":["Bash(git status)"]}}"#;
        let (dir, home, settings, bridge) = setup(Some(complex));
        let original: Value = serde_json::from_str(complex).unwrap();
        let p = plan(&settings, &bridge, &home, dir.path(), 7).unwrap();
        let b64 = p.new_status_line["command"].as_str().unwrap().split("--chain-b64 ").nth(1).unwrap().to_string();
        assert_eq!(String::from_utf8(b64_decode(&b64).unwrap()).unwrap(), original["statusLine"]["command"].as_str().unwrap());
        assert_eq!(p.new_status_line["refreshInterval"], 5);
        let st = enable(&p, 8).unwrap();
        // The user edits statusLine afterwards: disabling must not clobber it.
        let mut edited: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        edited["statusLine"] = json!({"type":"command","command":"my-new-line"});
        std::fs::write(&settings, edited.to_string()).unwrap();
        assert_eq!(disable(&st).unwrap(), DisableOutcome::ChangedByUser);
        let after: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(after["statusLine"]["command"], "my-new-line");
        assert_eq!(after["permissions"], original["permissions"]);
    }

    #[test]
    fn missing_file_is_created_and_invalid_json_is_refused() {
        let (dir, home, settings, bridge) = setup(None);
        let p = plan(&settings, &bridge, &home, dir.path(), 7).unwrap();
        assert!(!p.settings_exists);
        let st = enable(&p, 8).unwrap();
        assert!(st.backup_path.is_none());
        assert!(settings.is_file());
        std::fs::write(&settings, "{ not json").unwrap();
        assert!(plan(&settings, &bridge, &home, dir.path(), 9).is_err());
    }

    #[test]
    fn a_stale_preview_is_refused() {
        let (dir, home, settings, bridge) = setup(Some(r#"{}"#));
        let p = plan(&settings, &bridge, &home, dir.path(), 7).unwrap();
        std::fs::write(&settings, r#"{"statusLine":{"type":"command","command":"x"}}"#).unwrap();
        assert!(enable(&p, 8).is_err());
    }

    #[test]
    fn enabling_applies_exactly_the_reviewed_plan() {
        let (dir, home, settings, bridge) = setup(Some(r#"{"theme":"dark"}"#));
        let shown = plan(&settings, &bridge, &home, dir.path(), 7).unwrap();
        let again = plan(&settings, &bridge, &home, dir.path(), shown.planned_at_ms).unwrap();
        assert_eq!(again.backup_path, shown.backup_path, "the backup file is the one in the preview");
        assert!(check_reviewed(&again, &shown.plan_id).is_ok());
        // Any edit after the preview invalidates it, not only one to statusLine.
        std::fs::write(&settings, r#"{"theme":"light"}"#).unwrap();
        let changed = plan(&settings, &bridge, &home, dir.path(), shown.planned_at_ms).unwrap();
        assert!(check_reviewed(&changed, &shown.plan_id).is_err());
    }
}

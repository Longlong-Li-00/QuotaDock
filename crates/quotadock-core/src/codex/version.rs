//! Semver-style comparison for codex-cli versions ("0.162.0-alpha.2", "0.154.0-alpha.6.2").

use super::types::{VERIFIED_CODEX_VERSION, VersionGate};
use std::cmp::Ordering;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedVersion {
    core: [u64; 3],
    prerelease: Vec<String>,
}

pub fn parse_version(text: &str) -> Option<ParsedVersion> {
    let text = text.trim();
    let text = text.split('+').next()?;
    let (core, pre) = match text.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (text, None),
    };
    let nums: Vec<u64> = core.split('.').map(|p| p.parse().ok()).collect::<Option<Vec<_>>>()?;
    if nums.len() != 3 {
        return None;
    }
    let prerelease = match pre {
        Some(p) if !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') => {
            p.split('.').map(str::to_string).collect()
        }
        Some(_) => return None,
        None => Vec::new(),
    };
    Some(ParsedVersion { core: [nums[0], nums[1], nums[2]], prerelease })
}

/// "codex-cli 0.162.0-alpha.2" -> "0.162.0-alpha.2"
pub fn parse_codex_version_output(output: &str) -> Option<String> {
    let idx = output.find("codex-cli")?;
    let v = output[idx + "codex-cli".len()..].split_whitespace().next()?;
    parse_version(v).map(|_| v.to_string())
}

fn compare_identifiers(a: &str, b: &str) -> Ordering {
    match (a.parse::<u64>(), b.parse::<u64>()) {
        (Ok(x), Ok(y)) => x.cmp(&y),
        (Ok(_), Err(_)) => Ordering::Less,
        (Err(_), Ok(_)) => Ordering::Greater,
        _ => a.cmp(b),
    }
}

pub fn compare_versions(a: &str, b: &str) -> Option<Ordering> {
    let (pa, pb) = (parse_version(a)?, parse_version(b)?);
    let core = pa.core.cmp(&pb.core);
    if core != Ordering::Equal {
        return Some(core);
    }
    Some(match (pa.prerelease.is_empty(), pb.prerelease.is_empty()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) => {
            for i in 0..pa.prerelease.len().max(pb.prerelease.len()) {
                match (pa.prerelease.get(i), pb.prerelease.get(i)) {
                    (None, _) => return Some(Ordering::Less),
                    (_, None) => return Some(Ordering::Greater),
                    (Some(x), Some(y)) => {
                        let o = compare_identifiers(x, y);
                        if o != Ordering::Equal {
                            return Some(o);
                        }
                    }
                }
            }
            Ordering::Equal
        }
    })
}

pub fn gate_version(version: &str) -> VersionGate {
    match compare_versions(version, VERIFIED_CODEX_VERSION) {
        None => VersionGate::Unparseable,
        Some(Ordering::Equal) => VersionGate::Verified,
        Some(Ordering::Greater) => VersionGate::UnverifiedNewer,
        Some(Ordering::Less) => VersionGate::UnsupportedOlder,
    }
}

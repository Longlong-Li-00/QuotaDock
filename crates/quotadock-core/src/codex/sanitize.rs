//! Single entry point for scrubbing anything that may reach a log, the database or a fixture.
//! Port of `src/codex/sanitize.ts`.

use regex::Regex;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::sync::OnceLock;

const REDACTED: &str = "[REDACTED]";

const IDENTITY_KEYS: &[&str] = &[
    "email",
    "accountid",
    "account_id",
    "chatgptaccountid",
    "chatgpt_account_id",
    "userid",
    "user_id",
    "orgid",
    "org_id",
    "organizationid",
    "workspaceid",
    "installationid",
    "installation_id",
    "servername",
];

struct Patterns {
    credential_key: Regex,
    jwt: Regex,
    bearer: Regex,
    sk_key: Regex,
    email: Regex,
}

fn patterns() -> &'static Patterns {
    static P: OnceLock<Patterns> = OnceLock::new();
    P.get_or_init(|| Patterns {
        credential_key: Regex::new(r"(?i)token|secret|password|passwd|cookie|authorization|api[-_]?key|session[-_]?id|bearer").unwrap(),
        jwt: Regex::new(r"\beyJ[\w-]{5,}\.[\w-]{5,}\.[\w-]*").unwrap(),
        bearer: Regex::new(r"(?i)\bBearer\s+[\w.~+/=-]+").unwrap(),
        sk_key: Regex::new(r"\bsk-[\w-]{10,}").unwrap(),
        email: Regex::new(r"[\w.+-]+@[\w-]+(\.[\w-]+)+").unwrap(),
    })
}

pub fn is_sensitive_key(key: &str) -> bool {
    patterns().credential_key.is_match(key) || IDENTITY_KEYS.contains(&key.to_ascii_lowercase().as_str())
}

pub fn sanitize_string(s: &str) -> String {
    let p = patterns();
    let s = p.jwt.replace_all(s, "[REDACTED_JWT]");
    let s = p.bearer.replace_all(&s, "Bearer [REDACTED]");
    let s = p.sk_key.replace_all(&s, "[REDACTED_KEY]");
    p.email.replace_all(&s, "[REDACTED_EMAIL]").into_owned()
}

/// Deep copy with credentials and identifiers removed (null values stay null).
pub fn sanitize_value(v: &Value) -> Value {
    sanitize_depth(v, 0)
}

fn sanitize_depth(v: &Value, depth: usize) -> Value {
    if depth > 32 {
        return Value::String("[TRUNCATED]".into());
    }
    match v {
        Value::String(s) => Value::String(sanitize_string(s)),
        Value::Array(a) => Value::Array(a.iter().map(|x| sanitize_depth(x, depth + 1)).collect()),
        Value::Object(m) => {
            let mut out = Map::new();
            for (k, x) in m {
                let value = if is_sensitive_key(k) && !x.is_null() { Value::String(REDACTED.into()) } else { sanitize_depth(x, depth + 1) };
                out.insert(k.clone(), value);
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

/// Stable local identity for an account without storing the account id.
/// SHA-256 over a domain-separated id, truncated to 128 bits (32 hex chars).
pub fn account_fingerprint(account_id: &str) -> String {
    let digest = Sha256::digest(format!("quotadock:account:v1:{account_id}").as_bytes());
    digest.iter().take(16).map(|b| format!("{b:02x}")).collect()
}

/// Short form for logs and UI.
pub fn short_fingerprint(fp: &str) -> String {
    fp.chars().take(8).collect()
}

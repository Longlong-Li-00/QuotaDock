//! Claude authentication-loss notice (Windows V1). Real run 2026-10-20: Claude Code reported
//! `loggedIn: false`; QuotaDock stopped its active reads as it should, but nobody was told, and
//! Claude monitoring stayed silent for days.
//!
//! One notice per sign-out episode: QuotaDock must have seen Claude Code signed in before, and the
//! current `claude auth status --json` result must say `loggedIn: false` explicitly. A failed or
//! unparseable check, a missing field, network errors, timeouts, rate limits and /usage problems are
//! never a sign-out. The episode state is persisted, so a restart while still signed out does not
//! notify again; signing in re-arms it. Nothing beyond that command's output is read and nothing is
//! changed: the user signs in themselves.

use super::i18n;
use super::notifications::{Candidate, CandidateKind, RuleKey, priority};
use super::settings::Lang;
use crate::claude::model::PROVIDER_CLAUDE;
use crate::claude::usage::AuthStatus;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

const SETTINGS_KEY: &str = "claude_auth_watch";

/// What one authentication check said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthObservation {
    SignedIn,
    SignedOut,
    /// No usable answer (check failed, output unparseable, `loggedIn` missing or not a boolean).
    Unknown,
}

impl AuthObservation {
    pub fn of(auth: Option<&AuthStatus>) -> Self {
        match auth.and_then(|a| a.logged_in_reported) {
            Some(true) => Self::SignedIn,
            Some(false) => Self::SignedOut,
            None => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthWatch {
    /// No explicit signed-in result seen yet: a sign-out now is not "just signed out".
    #[default]
    NeverSignedIn,
    SignedIn,
    /// Signed out after a confirmed sign-in; this episode's notice was issued.
    SignedOut,
}

/// Next state, and whether this observation starts a new sign-out episode (notify once).
pub fn step(state: AuthWatch, obs: AuthObservation) -> (AuthWatch, bool) {
    match (state, obs) {
        (_, AuthObservation::Unknown) => (state, false),
        (_, AuthObservation::SignedIn) => (AuthWatch::SignedIn, false),
        (AuthWatch::SignedIn, AuthObservation::SignedOut) => (AuthWatch::SignedOut, true),
        (s, AuthObservation::SignedOut) => (s, false),
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthWatchRecord {
    pub state: AuthWatch,
    pub changed_at_ms: Option<i64>,
}

pub fn load(conn: &Connection) -> rusqlite::Result<AuthWatchRecord> {
    let v: Option<String> =
        conn.query_row("SELECT value_json FROM settings WHERE key = ?1", params![SETTINGS_KEY], |r| r.get(0)).optional()?;
    Ok(v.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default())
}

fn save(conn: &Connection, rec: &AuthWatchRecord, now_ms: i64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO settings (key, value_json, updated_at_ms) VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at_ms = excluded.updated_at_ms",
        params![SETTINGS_KEY, serde_json::to_string(rec).unwrap_or_default(), now_ms],
    )?;
    Ok(())
}

/// A sign-out episode whose notice is due. Its state is saved by `commit` only once the notice has
/// been recorded, so a failure in between leaves the episode for the next check instead of losing it.
#[derive(Debug, Clone, PartialEq)]
pub struct DueNotice {
    pub episode_ms: i64,
    pub record: AuthWatchRecord,
}

/// Applies one check result: other state changes are saved at once; a due notice is returned.
pub fn observe(conn: &Connection, obs: AuthObservation, now_ms: i64) -> rusqlite::Result<Option<DueNotice>> {
    let rec = load(conn)?;
    let (state, notify) = step(rec.state, obs);
    let next = AuthWatchRecord { state, changed_at_ms: Some(now_ms) };
    if notify {
        return Ok(Some(DueNotice { episode_ms: now_ms, record: next }));
    }
    if state != rec.state {
        save(conn, &next, now_ms)?;
    }
    Ok(None)
}

/// The notice of `due` was recorded: this episode is now notified.
pub fn commit(conn: &Connection, due: &DueNotice, now_ms: i64) -> rusqlite::Result<()> {
    save(conn, &due.record, now_ms)
}

pub fn candidate(lang: Lang, episode_ms: i64) -> Candidate {
    let title = i18n::claude_signed_out_title(lang).to_string();
    Candidate {
        // Ahead of every reminder: when several fire together only two more are listed, and monitoring
        // having stopped must never be one of the folded ones.
        priority: priority::MONITORING_STOPPED,
        kind: CandidateKind::SignedOut,
        provider: super::claude_view::DISPLAY_NAME,
        window: None,
        remaining: None,
        keys: vec![RuleKey::new("claude_signed_out", PROVIDER_CLAUDE, "account", &format!("episode:{episode_ms}"))],
        title,
        body: i18n::claude_signed_out_body(lang).into(),
        short: i18n::claude_signed_out_line(lang).into(),
    }
}

#[cfg(test)]
mod tests {
    use super::AuthObservation::{SignedIn, SignedOut, Unknown};
    use super::*;

    fn run(obs: &[AuthObservation]) -> (AuthWatch, usize) {
        obs.iter().fold((AuthWatch::default(), 0), |(s, n), o| {
            let (next, notify) = step(s, *o);
            (next, n + usize::from(notify))
        })
    }

    #[test]
    fn one_notice_per_sign_out_episode() {
        assert_eq!(run(&[SignedIn, SignedOut]), (AuthWatch::SignedOut, 1));
        assert_eq!(run(&[SignedIn, SignedOut, SignedOut, Unknown, SignedOut]).1, 1, "same episode");
        assert_eq!(run(&[SignedIn, SignedOut, SignedIn, SignedOut]).1, 2, "signing in re-arms");
    }

    #[test]
    fn never_signed_in_is_not_a_sign_out() {
        assert_eq!(run(&[SignedOut, SignedOut]), (AuthWatch::NeverSignedIn, 0));
        assert_eq!(run(&[SignedOut, SignedIn, SignedOut]).1, 1);
    }

    #[test]
    fn unknown_results_change_nothing() {
        assert_eq!(run(&[SignedIn, Unknown, Unknown]), (AuthWatch::SignedIn, 0));
        assert_eq!(run(&[Unknown, SignedOut]).1, 0);
    }

    #[test]
    fn only_an_explicit_false_is_a_sign_out() {
        let parse = |s: &str| crate::claude::usage::parse_auth_status(s);
        assert_eq!(AuthObservation::of(parse(r#"{"loggedIn": false}"#).as_ref()), SignedOut);
        assert_eq!(AuthObservation::of(parse(r#"{"loggedIn": true, "authMethod": "claude.ai"}"#).as_ref()), SignedIn);
        assert_eq!(AuthObservation::of(parse(r#"{"authMethod": "claude.ai"}"#).as_ref()), Unknown, "missing field");
        assert_eq!(AuthObservation::of(parse(r#"{"loggedIn": "false"}"#).as_ref()), Unknown, "not a boolean");
        assert_eq!(AuthObservation::of(parse("Error: connect ETIMEDOUT").as_ref()), Unknown, "not JSON");
        assert_eq!(AuthObservation::of(None), Unknown, "the check itself failed");
    }

    #[test]
    fn the_episode_survives_a_restart() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE settings (key TEXT PRIMARY KEY, value_json TEXT NOT NULL, updated_at_ms INTEGER NOT NULL);")
            .unwrap();
        assert_eq!(observe(&conn, SignedIn, 1).unwrap(), None);
        let due = observe(&conn, SignedOut, 2).unwrap().expect("notice due");
        assert_eq!(due.episode_ms, 2);
        // Not recorded yet (e.g. the notification write failed): the next check is still due.
        assert_eq!(observe(&conn, SignedOut, 3).unwrap().map(|d| d.episode_ms), Some(3));
        commit(&conn, &due, 4).unwrap();
        // A restart reads the persisted episode: still signed out, nothing new.
        assert_eq!(observe(&conn, SignedOut, 5).unwrap(), None);
        assert_eq!(load(&conn).unwrap(), AuthWatchRecord { state: AuthWatch::SignedOut, changed_at_ms: Some(2) });
    }

    #[test]
    fn notice_text() {
        let zh = candidate(Lang::ZhCn, 7);
        assert_eq!(zh.short, "Claude 监控已中断：Claude Code 已退出登录，请重新登录。");
        assert_eq!(format!("{}：{}", zh.title, zh.body), zh.short);
        let en = candidate(Lang::En, 7);
        assert_eq!(en.short, "Claude monitoring stopped: Claude Code is signed out. Sign in again to resume monitoring.");
        assert_eq!(format!("{}: {}", en.title, en.body), en.short);
        assert_eq!(en.keys[0].scope_key, "episode:7");
    }
}

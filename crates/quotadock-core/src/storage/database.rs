//! Database location and connection setup.

use super::migrations::migrate;
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const DATABASE_FILE: &str = "quotadock.sqlite3";

// The canonical path is only available through `platform::data_location::validated_database_path`,
// which refuses to resolve it inside a packaged (virtualized) context. No SQLite database may be
// opened before that check succeeds.

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseInfo {
    pub path: PathBuf,
    pub schema_version: i64,
    pub journal_mode: String,
    pub foreign_keys: bool,
    pub busy_timeout_ms: i64,
}

/// Exclusive per-database lock: one QuotaDock engine per database, so no snapshot is ever
/// analysed by two processes. Held for the lifetime of the returned guard.
pub struct InstanceLock {
    _file: std::fs::File,
}

pub fn acquire_instance_lock(db_path: &Path) -> Result<InstanceLock, String> {
    let lock_path = db_path.with_extension("lock");
    if let Some(dir) = lock_path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).truncate(false).write(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        opts.share_mode(0); // no other handle may open the file while we hold it
    }
    opts.open(&lock_path)
        .map(|f| InstanceLock { _file: f })
        .map_err(|e| format!("another QuotaDock instance is using {} ({e})", db_path.display()))
}

pub fn open_database(path: &Path) -> rusqlite::Result<(Connection, DatabaseInfo)> {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_millis(5_000))?;
    let journal_mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
    conn.pragma_update(None, "foreign_keys", true)?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    let schema_version = migrate(&mut conn)?;
    let foreign_keys: bool = conn.query_row("PRAGMA foreign_keys", [], |r| r.get(0))?;
    let busy_timeout_ms: i64 = conn.query_row("PRAGMA busy_timeout", [], |r| r.get(0))?;
    Ok((conn, DatabaseInfo { path: path.to_path_buf(), schema_version, journal_mode, foreign_keys, busy_timeout_ms }))
}

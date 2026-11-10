//! SQLite schema and migrations. Mirrors `auto_dm/db.py`, so both versions share one DB file.

use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use rusqlite::Connection;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS instagram_accounts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    instagram_user_id TEXT NOT NULL UNIQUE,
    username TEXT NOT NULL,
    access_token TEXT NOT NULL,
    page_id TEXT NOT NULL DEFAULT '',
    page_access_token TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS oauth_states (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    state_hash TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS instagram_media (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    instagram_media_id TEXT NOT NULL UNIQUE,
    caption TEXT NOT NULL DEFAULT '',
    media_type TEXT NOT NULL DEFAULT '',
    permalink TEXT NOT NULL DEFAULT '',
    media_url TEXT NOT NULL DEFAULT '',
    thumbnail_url TEXT NOT NULL DEFAULT '',
    timestamp TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS automations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    instagram_media_id TEXT NOT NULL UNIQUE,
    title TEXT NOT NULL,
    keyword TEXT NOT NULL,
    dm_message TEXT NOT NULL,
    follow_required INTEGER NOT NULL DEFAULT 1,
    active INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS comments (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    automation_id INTEGER,
    instagram_comment_id TEXT NOT NULL UNIQUE,
    instagram_media_id TEXT NOT NULL,
    commenter_id TEXT NOT NULL,
    commenter_username TEXT NOT NULL DEFAULT '',
    recipient_id TEXT NOT NULL DEFAULT '',
    text TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'queued',
    error TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (automation_id) REFERENCES automations(id) ON DELETE SET NULL
);

CREATE TABLE IF NOT EXISTS dm_jobs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    comment_id INTEGER NOT NULL UNIQUE,
    status TEXT NOT NULL DEFAULT 'queued',
    step TEXT NOT NULL DEFAULT 'first',
    attempts INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    claim_token TEXT NOT NULL DEFAULT '',
    last_error TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (comment_id) REFERENCES comments(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_comments_status ON comments(status);
CREATE INDEX IF NOT EXISTS idx_comments_commenter ON comments(commenter_id);
CREATE INDEX IF NOT EXISTS idx_jobs_status_next ON dm_jobs(status, next_attempt_at);
CREATE INDEX IF NOT EXISTS idx_jobs_claim_token ON dm_jobs(claim_token);
CREATE INDEX IF NOT EXISTS idx_automations_active_media ON automations(active, instagram_media_id);
CREATE INDEX IF NOT EXISTS idx_media_timestamp ON instagram_media(timestamp);
CREATE INDEX IF NOT EXISTS idx_oauth_states_created ON oauth_states(created_at);
";

/// Column additions for DB files created by older versions. Duplicate columns are skipped.
const MIGRATIONS: &[&str] = &[
    "ALTER TABLE dm_jobs ADD COLUMN claim_token TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE instagram_accounts ADD COLUMN page_id TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE instagram_accounts ADD COLUMN page_access_token TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE instagram_media ADD COLUMN media_url TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE automations ADD COLUMN follow_required INTEGER NOT NULL DEFAULT 1",
    "ALTER TABLE comments ADD COLUMN recipient_id TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE dm_jobs ADD COLUMN step TEXT NOT NULL DEFAULT 'first'",
];

/// Jobs left in pre-API follow statuses already used their one private reply on the comment.
/// Park them as 'waiting' so the commenter's next DM triggers the API follow check.
/// Only legacy job statuses match, so re-running init is a no-op.
const LEGACY_STATUS_MIGRATIONS: &[&str] = &[
    "UPDATE comments
     SET status = 'waiting_follow', error = '', updated_at = CURRENT_TIMESTAMP
     WHERE id IN (
         SELECT comment_id FROM dm_jobs WHERE status IN ('waiting_follow', 'ready_to_send')
     )",
    "UPDATE dm_jobs
     SET status = 'waiting', claim_token = '', updated_at = CURRENT_TIMESTAMP
     WHERE status IN ('waiting_follow', 'ready_to_send')",
];

/// Opens a connection with the same settings as Python `sqlite3.connect`.
/// rusqlite commits each statement on its own unless a transaction is opened.
pub fn open(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.execute_batch("PRAGMA foreign_keys=ON")?;
    Ok(conn)
}

/// Creates tables and applies migrations on an open connection.
pub fn init(conn: &Connection) -> Result<()> {
    // journal_mode returns a row, so it is read instead of executed.
    let _mode: String = conn.query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))?;
    conn.execute_batch(SCHEMA)?;
    for sql in MIGRATIONS {
        if let Err(err) = conn.execute(sql, [])
            && !err
                .to_string()
                .to_lowercase()
                .contains("duplicate column name")
        {
            return Err(err.into());
        }
    }
    for sql in LEGACY_STATUS_MIGRATIONS {
        conn.execute(sql, [])?;
    }
    Ok(())
}

pub fn init_db(path: &Path) -> Result<()> {
    init(&open(path)?)
}

#[cfg(test)]
pub fn test_conn() -> Connection {
    let conn = Connection::open_in_memory().expect("in-memory db");
    conn.execute_batch("PRAGMA foreign_keys=ON")
        .expect("pragma");
    init(&conn).expect("init");
    conn
}

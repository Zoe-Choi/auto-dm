from __future__ import annotations

import sqlite3
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path

from .config import settings

SCHEMA = """
PRAGMA journal_mode=WAL;
PRAGMA foreign_keys=ON;

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
    attempts INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    claim_token TEXT NOT NULL DEFAULT '',
    last_error TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (comment_id) REFERENCES comments(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_comments_status ON comments(status);
CREATE INDEX IF NOT EXISTS idx_jobs_status_next ON dm_jobs(status, next_attempt_at);
CREATE INDEX IF NOT EXISTS idx_jobs_claim_token ON dm_jobs(claim_token);
CREATE INDEX IF NOT EXISTS idx_automations_active_media ON automations(active, instagram_media_id);
CREATE INDEX IF NOT EXISTS idx_media_timestamp ON instagram_media(timestamp);
CREATE INDEX IF NOT EXISTS idx_oauth_states_created ON oauth_states(created_at);
"""

MIGRATIONS = [
    "ALTER TABLE dm_jobs ADD COLUMN claim_token TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE instagram_accounts ADD COLUMN page_id TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE instagram_accounts ADD COLUMN page_access_token TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE instagram_media ADD COLUMN media_url TEXT NOT NULL DEFAULT ''",
]


@contextmanager
def connect(db_path: Path | None = None) -> Iterator[sqlite3.Connection]:
    path = db_path or settings.db_file
    conn = sqlite3.connect(path)
    conn.row_factory = sqlite3.Row
    try:
        conn.execute("PRAGMA foreign_keys=ON")
        yield conn
        conn.commit()
    finally:
        conn.close()


def init_db(db_path: Path | None = None) -> None:
    with connect(db_path) as conn:
        conn.executescript(SCHEMA)
        for sql in MIGRATIONS:
            try:
                conn.execute(sql)
            except sqlite3.OperationalError as exc:
                if "duplicate column name" not in str(exc).lower():
                    raise

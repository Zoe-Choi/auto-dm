//! Database operations for automations, comments, and the DM job queue.
//! Mirrors `auto_dm/services.py`; function names and SQL match the Python version.

use std::collections::BTreeMap;

use anyhow::Result;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rusqlite::{Connection, ErrorCode, OptionalExtension, Row, named_params, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const MAX_ATTEMPTS: i64 = 5;
pub const FOLLOW_CHECK_PAYLOAD: &str = "AUTO_DM_CHECK:";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingComment {
    pub comment_id: String,
    pub media_id: String,
    pub commenter_id: String,
    pub commenter_username: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Automation {
    pub id: i64,
    pub instagram_media_id: String,
    pub title: String,
    pub keyword: String,
    pub dm_message: String,
    pub follow_required: bool,
    pub active: bool,
}

impl Automation {
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            instagram_media_id: row.get("instagram_media_id")?,
            title: row.get("title")?,
            keyword: row.get("keyword")?,
            dm_message: row.get("dm_message")?,
            follow_required: row.get("follow_required")?,
            active: row.get("active")?,
        })
    }
}

/// One media item from the Graph API `/{ig-user-id}/media` response.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct MediaItem {
    #[serde(default)]
    pub id: String,
    pub caption: Option<String>,
    pub media_type: Option<String>,
    pub permalink: Option<String>,
    pub media_url: Option<String>,
    pub thumbnail_url: Option<String>,
    pub timestamp: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Media {
    pub instagram_media_id: String,
    pub caption: String,
    pub media_type: String,
    pub permalink: String,
    pub media_url: String,
    pub thumbnail_url: String,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Comment {
    pub id: i64,
    pub automation_id: Option<i64>,
    pub instagram_comment_id: String,
    pub commenter_id: String,
    pub commenter_username: String,
    pub text: String,
    pub status: String,
    pub error: String,
}

impl Comment {
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            automation_id: row.get("automation_id")?,
            instagram_comment_id: row.get("instagram_comment_id")?,
            commenter_id: row.get("commenter_id")?,
            commenter_username: row.get("commenter_username")?,
            text: row.get("text")?,
            status: row.get("status")?,
            error: row.get("error")?,
        })
    }
}

/// A comment row joined with its automation title for the admin page.
#[derive(Debug, Clone, Serialize)]
pub struct RecentComment {
    #[serde(flatten)]
    pub comment: Comment,
    pub automation_title: Option<String>,
}

/// A claimed job joined with the comment and automation it needs.
#[derive(Debug, Clone)]
pub struct Job {
    pub id: i64,
    pub comment_id: i64,
    pub step: String,
    pub attempts: i64,
    pub instagram_comment_id: String,
    pub recipient_id: String,
    pub dm_message: String,
    pub follow_required: bool,
}

impl Job {
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            comment_id: row.get("comment_id")?,
            step: row.get("step")?,
            attempts: row.get("attempts")?,
            instagram_comment_id: row.get("instagram_comment_id")?,
            recipient_id: row.get("recipient_id")?,
            dm_message: row.get("dm_message")?,
            follow_required: row.get("follow_required")?,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Account {
    pub instagram_user_id: String,
    pub username: String,
    #[serde(skip)]
    pub access_token: String,
    pub page_id: String,
    #[serde(skip)]
    pub page_access_token: String,
}

impl Account {
    /// The page token is preferred because Instagram messaging runs through the linked Page.
    pub fn stored_token(&self) -> &str {
        if self.page_access_token.is_empty() {
            &self.access_token
        } else {
            &self.page_access_token
        }
    }
}

pub fn normalize_keyword(value: &str) -> String {
    value.trim().to_lowercase()
}

pub fn split_keywords(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(normalize_keyword)
        .filter(|k| !k.is_empty())
        .collect()
}

pub fn keyword_matches(keyword: &str, text: &str) -> bool {
    let lowered = text.to_lowercase();
    split_keywords(keyword)
        .iter()
        .any(|k| lowered.contains(k.as_str()))
}

fn oauth_state_hash(state: &str) -> String {
    hex::encode(Sha256::digest(state.as_bytes()))
}

/// Same shape as Python `secrets.token_urlsafe(24)`.
fn random_token() -> String {
    let bytes: [u8; 24] = rand::random();
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn create_oauth_state(conn: &Connection) -> Result<String> {
    let state = random_token();
    conn.execute(
        "INSERT INTO oauth_states (state_hash) VALUES (?1)",
        params![oauth_state_hash(&state)],
    )?;
    Ok(state)
}

pub fn consume_oauth_state(conn: &Connection, state: &str) -> Result<bool> {
    let deleted = conn.execute(
        "DELETE FROM oauth_states WHERE state_hash = ?1",
        params![oauth_state_hash(state)],
    )?;
    Ok(deleted == 1)
}

pub struct AutomationInput<'a> {
    pub instagram_media_id: &'a str,
    pub title: &'a str,
    pub keyword: &'a str,
    pub dm_message: &'a str,
    pub follow_required: bool,
    pub active: bool,
}

/// Updates by id when given, otherwise inserts or updates by media id. Returns the row id.
pub fn upsert_automation(
    conn: &Connection,
    input: &AutomationInput,
    automation_id: Option<i64>,
) -> Result<i64> {
    let media_id = input.instagram_media_id.trim();
    let title = match input.title.trim() {
        "" => media_id,
        title => title,
    };
    let keyword = split_keywords(input.keyword).join(", ");
    let dm_message = input.dm_message.trim();

    if let Some(id) = automation_id {
        conn.execute(
            "UPDATE automations
             SET instagram_media_id = :media_id, title = :title, keyword = :keyword,
                 dm_message = :dm_message, follow_required = :follow_required,
                 active = :active, updated_at = CURRENT_TIMESTAMP
             WHERE id = :id",
            named_params! {
                ":media_id": media_id, ":title": title, ":keyword": keyword,
                ":dm_message": dm_message, ":follow_required": input.follow_required,
                ":active": input.active, ":id": id,
            },
        )?;
        return Ok(id);
    }

    conn.execute(
        "INSERT INTO automations (
             instagram_media_id, title, keyword, dm_message, follow_required, active
         ) VALUES (:media_id, :title, :keyword, :dm_message, :follow_required, :active)
         ON CONFLICT(instagram_media_id) DO UPDATE SET
             title = excluded.title,
             keyword = excluded.keyword,
             dm_message = excluded.dm_message,
             follow_required = excluded.follow_required,
             active = excluded.active,
             updated_at = CURRENT_TIMESTAMP",
        named_params! {
            ":media_id": media_id, ":title": title, ":keyword": keyword,
            ":dm_message": dm_message, ":follow_required": input.follow_required,
            ":active": input.active,
        },
    )?;
    Ok(conn.query_row(
        "SELECT id FROM automations WHERE instagram_media_id = ?1",
        params![media_id],
        |row| row.get(0),
    )?)
}

pub fn delete_automation(conn: &Connection, automation_id: i64) -> Result<()> {
    conn.execute(
        "DELETE FROM automations WHERE id = ?1",
        params![automation_id],
    )?;
    Ok(())
}

pub fn upsert_instagram_media(conn: &Connection, items: &[MediaItem]) -> Result<usize> {
    let mut saved = 0;
    for item in items {
        let media_id = item.id.trim();
        if media_id.is_empty() {
            continue;
        }
        let text = |value: &Option<String>| value.clone().unwrap_or_default();
        conn.execute(
            "INSERT INTO instagram_media (
                 instagram_media_id, caption, media_type, permalink, media_url, thumbnail_url,
                 timestamp
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(instagram_media_id) DO UPDATE SET
                 caption = excluded.caption,
                 media_type = excluded.media_type,
                 permalink = excluded.permalink,
                 media_url = excluded.media_url,
                 thumbnail_url = excluded.thumbnail_url,
                 timestamp = excluded.timestamp,
                 updated_at = CURRENT_TIMESTAMP",
            params![
                media_id,
                text(&item.caption),
                text(&item.media_type),
                text(&item.permalink),
                text(&item.media_url),
                text(&item.thumbnail_url),
                text(&item.timestamp),
            ],
        )?;
        saved += 1;
    }
    Ok(saved)
}

pub fn list_instagram_media(conn: &Connection, limit: i64) -> Result<Vec<Media>> {
    let mut stmt = conn.prepare(
        "SELECT instagram_media_id, caption, media_type, permalink, media_url, thumbnail_url,
                timestamp
         FROM instagram_media
         ORDER BY timestamp DESC, updated_at DESC, id DESC
         LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit], |row| {
        Ok(Media {
            instagram_media_id: row.get(0)?,
            caption: row.get(1)?,
            media_type: row.get(2)?,
            permalink: row.get(3)?,
            media_url: row.get(4)?,
            thumbnail_url: row.get(5)?,
            timestamp: row.get(6)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn list_automations(conn: &Connection) -> Result<Vec<Automation>> {
    let mut stmt = conn.prepare("SELECT * FROM automations ORDER BY created_at DESC, id DESC")?;
    let rows = stmt.query_map([], Automation::from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn find_matching_automation(
    conn: &Connection,
    media_id: &str,
    text: &str,
) -> Result<Option<Automation>> {
    let mut stmt = conn.prepare(
        "SELECT * FROM automations WHERE active = 1 AND instagram_media_id = ?1 ORDER BY id DESC",
    )?;
    let rows = stmt.query_map(params![media_id], Automation::from_row)?;
    for row in rows {
        let automation = row?;
        if keyword_matches(&automation.keyword, text) {
            return Ok(Some(automation));
        }
    }
    Ok(None)
}

fn comment_by(conn: &Connection, column: &str, value: &dyn rusqlite::ToSql) -> Result<Comment> {
    let sql = format!("SELECT * FROM comments WHERE {column} = ?1");
    Ok(conn.query_row(&sql, [value], Comment::from_row)?)
}

/// Stores a comment once per Instagram comment id and queues a job when a keyword matches.
pub fn record_comment(conn: &Connection, incoming: &IncomingComment) -> Result<Comment> {
    let automation = find_matching_automation(conn, &incoming.media_id, &incoming.text)?;
    let status = if automation.is_some() {
        "queued"
    } else {
        "ignored"
    };
    let inserted = conn.execute(
        "INSERT INTO comments (
             automation_id, instagram_comment_id, instagram_media_id, commenter_id,
             commenter_username, text, status
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            automation.as_ref().map(|a| a.id),
            incoming.comment_id,
            incoming.media_id,
            incoming.commenter_id,
            incoming.commenter_username,
            incoming.text,
            status,
        ],
    );
    match inserted {
        Ok(_) => {}
        // Meta retries webhooks, so a duplicate comment id returns the stored row.
        Err(rusqlite::Error::SqliteFailure(err, _))
            if err.code == ErrorCode::ConstraintViolation =>
        {
            return comment_by(conn, "instagram_comment_id", &incoming.comment_id);
        }
        Err(err) => return Err(err.into()),
    }
    let comment_id = conn.last_insert_rowid();
    if automation.is_some() {
        conn.execute(
            "INSERT INTO dm_jobs (comment_id) VALUES (?1)",
            params![comment_id],
        )?;
    }
    comment_by(conn, "id", &comment_id)
}

pub fn list_recent_comments(conn: &Connection, limit: i64) -> Result<Vec<RecentComment>> {
    let mut stmt = conn.prepare(
        "SELECT c.*, a.title AS automation_title
         FROM comments c
         LEFT JOIN automations a ON a.id = c.automation_id
         ORDER BY c.created_at DESC, c.id DESC
         LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit], |row| {
        Ok(RecentComment {
            comment: Comment::from_row(row)?,
            automation_title: row.get("automation_title")?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

const JOB_SELECT: &str = "
    SELECT j.*, c.instagram_comment_id, c.recipient_id, a.dm_message, a.follow_required
    FROM dm_jobs j
    JOIN comments c ON c.id = j.comment_id
    JOIN automations a ON a.id = c.automation_id";

#[cfg(test)]
pub fn next_jobs(conn: &Connection, limit: i64) -> Result<Vec<Job>> {
    let sql = format!(
        "{JOB_SELECT}
         WHERE j.status = 'queued' AND j.next_attempt_at <= CURRENT_TIMESTAMP
         ORDER BY j.created_at ASC, j.id ASC
         LIMIT ?1"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![limit], Job::from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Requeues jobs whose worker died mid-batch.
pub fn reset_stale_processing_jobs(conn: &Connection, stale_minutes: i64) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE dm_jobs
         SET status = 'queued', claim_token = '', updated_at = CURRENT_TIMESTAMP
         WHERE status = 'processing'
           AND updated_at <= datetime(CURRENT_TIMESTAMP, ?1)",
        params![format!("-{stale_minutes} minutes")],
    )?)
}

/// Marks a batch as processing under one random token so two workers never share a job.
pub fn claim_jobs(conn: &Connection, limit: i64) -> Result<Vec<Job>> {
    let token = hex::encode(rand::random::<[u8; 16]>());
    conn.execute(
        "UPDATE dm_jobs
         SET status = 'processing', claim_token = ?1, updated_at = CURRENT_TIMESTAMP
         WHERE id IN (
             SELECT id FROM dm_jobs
             WHERE status = 'queued' AND next_attempt_at <= CURRENT_TIMESTAMP
             ORDER BY created_at ASC, id ASC
             LIMIT ?2
         )",
        params![token, limit],
    )?;
    let sql = format!(
        "{JOB_SELECT}
         WHERE j.claim_token = ?1 AND j.status = 'processing'
         ORDER BY j.created_at ASC, j.id ASC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![token], Job::from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn mark_job_sent(conn: &Connection, job_id: i64, comment_id: i64) -> Result<()> {
    conn.execute(
        "UPDATE dm_jobs SET status = 'sent', claim_token = '', updated_at = CURRENT_TIMESTAMP
         WHERE id = ?1",
        params![job_id],
    )?;
    conn.execute(
        "UPDATE comments SET status = 'sent', error = '', updated_at = CURRENT_TIMESTAMP
         WHERE id = ?1",
        params![comment_id],
    )?;
    Ok(())
}

pub fn note_comment_error(conn: &Connection, comment_id: i64, error: &str) -> Result<()> {
    conn.execute(
        "UPDATE comments SET error = ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2",
        params![truncate(error, 500), comment_id],
    )?;
    Ok(())
}

/// Parks a job until the commenter taps a DM button. status: waiting_click | waiting_follow.
pub fn mark_job_waiting(
    conn: &Connection,
    job_id: i64,
    comment_id: i64,
    status: &str,
) -> Result<()> {
    conn.execute(
        "UPDATE dm_jobs
         SET status = 'waiting', claim_token = '', last_error = '', updated_at = CURRENT_TIMESTAMP
         WHERE id = ?1",
        params![job_id],
    )?;
    conn.execute(
        "UPDATE comments SET status = ?1, error = '', updated_at = CURRENT_TIMESTAMP WHERE id = ?2",
        params![status, comment_id],
    )?;
    Ok(())
}

pub fn follow_check_payload(comment_id: i64) -> String {
    format!("{FOLLOW_CHECK_PAYLOAD}{comment_id}")
}

/// Requeues a waiting job so the worker checks whether the commenter follows the account.
pub fn request_follow_check(
    conn: &Connection,
    comment_id: i64,
    recipient_id: &str,
) -> Result<bool> {
    let updated = conn.execute(
        "UPDATE dm_jobs
         SET status = 'queued', step = 'check_follow', attempts = 0,
             next_attempt_at = CURRENT_TIMESTAMP, claim_token = '', last_error = '',
             updated_at = CURRENT_TIMESTAMP
         WHERE comment_id = ?1 AND status = 'waiting'",
        params![comment_id],
    )?;
    if updated == 0 {
        return Ok(false);
    }
    conn.execute(
        "UPDATE comments SET recipient_id = ?1, status = 'queued', updated_at = CURRENT_TIMESTAMP
         WHERE id = ?2",
        params![recipient_id, comment_id],
    )?;
    Ok(true)
}

/// Handles one Instagram `messaging` webhook event. Returns true when a follow check is queued.
pub fn handle_message_event(conn: &Connection, event: &Value, business_id: &str) -> Result<bool> {
    let sender_id = event
        .pointer("/sender/id")
        .and_then(Value::as_str)
        .unwrap_or("");
    let is_echo = event
        .pointer("/message/is_echo")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if sender_id.is_empty() || is_echo || (!business_id.is_empty() && sender_id == business_id) {
        return Ok(false);
    }

    let payload = event
        .pointer("/message/quick_reply/payload")
        .or_else(|| event.pointer("/postback/payload"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if let Some(suffix) = payload.strip_prefix(FOLLOW_CHECK_PAYLOAD) {
        return match suffix.parse::<i64>() {
            Ok(comment_id) => request_follow_check(conn, comment_id, sender_id),
            Err(_) => Ok(false),
        };
    }

    // Fallback for a typed reply: the latest waiting comment from the same sender.
    let waiting: Option<i64> = conn
        .query_row(
            "SELECT j.comment_id
             FROM dm_jobs j
             JOIN comments c ON c.id = j.comment_id
             WHERE j.status = 'waiting' AND c.commenter_id = ?1
             ORDER BY j.id DESC
             LIMIT 1",
            params![sender_id],
            |row| row.get(0),
        )
        .optional()?;
    match waiting {
        Some(comment_id) => request_follow_check(conn, comment_id, sender_id),
        None => Ok(false),
    }
}

pub fn requeue_failed_jobs(conn: &Connection) -> Result<usize> {
    let count = conn.execute(
        "UPDATE dm_jobs
         SET status = 'queued', attempts = 0, next_attempt_at = CURRENT_TIMESTAMP,
             claim_token = '', last_error = '', updated_at = CURRENT_TIMESTAMP
         WHERE status = 'failed'",
        [],
    )?;
    conn.execute(
        "UPDATE comments
         SET status = 'queued', error = '', updated_at = CURRENT_TIMESTAMP
         WHERE id IN (SELECT comment_id FROM dm_jobs WHERE status = 'queued')
           AND status = 'failed'",
        [],
    )?;
    Ok(count)
}

/// Backs off 1, 2, 4, 8, 16 minutes (capped at 30) and fails after MAX_ATTEMPTS.
pub fn retry_job(
    conn: &Connection,
    job_id: i64,
    comment_id: i64,
    attempts: i64,
    error: &str,
) -> Result<()> {
    let next_status = if attempts + 1 >= MAX_ATTEMPTS {
        "failed"
    } else {
        "queued"
    };
    let delay_minutes = 2_i64.pow(attempts.clamp(0, 5) as u32).min(30);
    let error = truncate(error, 500);
    conn.execute(
        "UPDATE dm_jobs
         SET status = ?1, attempts = attempts + 1,
             next_attempt_at = datetime(CURRENT_TIMESTAMP, ?2), claim_token = '',
             last_error = ?3, updated_at = CURRENT_TIMESTAMP
         WHERE id = ?4",
        params![
            next_status,
            format!("+{delay_minutes} minutes"),
            error,
            job_id
        ],
    )?;
    conn.execute(
        "UPDATE comments SET status = ?1, error = ?2, updated_at = CURRENT_TIMESTAMP WHERE id = ?3",
        params![next_status, error, comment_id],
    )?;
    Ok(())
}

pub fn dashboard_counts(conn: &Connection) -> Result<BTreeMap<String, i64>> {
    let tables = [
        ("automations", "SELECT COUNT(*) FROM automations"),
        (
            "active_automations",
            "SELECT COUNT(*) FROM automations WHERE active = 1",
        ),
        ("media", "SELECT COUNT(*) FROM instagram_media"),
        ("comments", "SELECT COUNT(*) FROM comments"),
    ];
    let mut counts = BTreeMap::new();
    for (name, sql) in tables {
        counts.insert(name.to_string(), conn.query_row(sql, [], |row| row.get(0))?);
    }
    let mut stmt = conn.prepare("SELECT status, COUNT(*) FROM dm_jobs GROUP BY status")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    for row in rows {
        let (status, count) = row?;
        counts.insert(format!("jobs_{status}"), count);
    }
    Ok(counts)
}

pub fn latest_account(conn: &Connection) -> Result<Option<Account>> {
    Ok(conn
        .query_row(
            "SELECT instagram_user_id, username, access_token, page_id, page_access_token
             FROM instagram_accounts ORDER BY updated_at DESC, id DESC LIMIT 1",
            [],
            |row| {
                Ok(Account {
                    instagram_user_id: row.get(0)?,
                    username: row.get(1)?,
                    access_token: row.get(2)?,
                    page_id: row.get(3)?,
                    page_access_token: row.get(4)?,
                })
            },
        )
        .optional()?)
}

/// Tokens must already be protected with `security::protect_secret`.
pub fn upsert_account(conn: &Connection, account: &Account) -> Result<()> {
    conn.execute(
        "INSERT INTO instagram_accounts (
             instagram_user_id, username, access_token, page_id, page_access_token
         ) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(instagram_user_id) DO UPDATE SET
             username = excluded.username,
             access_token = excluded.access_token,
             page_id = excluded.page_id,
             page_access_token = excluded.page_access_token,
             updated_at = CURRENT_TIMESTAMP",
        params![
            account.instagram_user_id,
            account.username,
            account.access_token,
            account.page_id,
            account.page_access_token,
        ],
    )?;
    Ok(())
}

/// Cuts at a char boundary; slicing a String by byte index could split a Korean character.
fn truncate(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::db::test_conn;

    pub fn add_automation(conn: &Connection, follow_required: bool) -> i64 {
        upsert_automation(
            conn,
            &AutomationInput {
                instagram_media_id: "media-1",
                title: "reel",
                keyword: "info",
                dm_message: "here is the link",
                follow_required,
                active: true,
            },
            None,
        )
        .expect("add automation")
    }

    pub fn incoming(comment_id: &str, commenter_id: &str, text: &str) -> IncomingComment {
        IncomingComment {
            comment_id: comment_id.into(),
            media_id: "media-1".into(),
            commenter_id: commenter_id.into(),
            commenter_username: "tester".into(),
            text: text.into(),
        }
    }

    #[test]
    fn keyword_matches_case_insensitive() {
        assert!(keyword_matches("info", "send INFO please"));
        assert!(!keyword_matches("info", "price please"));
    }

    #[test]
    fn keyword_matches_any_comma_separated_keyword() {
        assert!(keyword_matches("정보, 링크", "링크 주세요"));
        assert!(keyword_matches("정보,링크", "정보요"));
        assert!(!keyword_matches("정보, 링크", "가격 문의"));
        assert!(!keyword_matches(" , ", "anything"));
    }

    #[test]
    fn record_comment_creates_one_job_for_matching_comment() {
        let conn = test_conn();
        let automation_id = add_automation(&conn, false);
        let comment =
            record_comment(&conn, &incoming("comment-1", "user-1", "info please")).unwrap();
        let duplicate =
            record_comment(&conn, &incoming("comment-1", "user-1", "info please")).unwrap();
        let jobs = next_jobs(&conn, 20).unwrap();

        assert_eq!(comment.automation_id, Some(automation_id));
        assert_eq!(duplicate.id, comment.id);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].dm_message, "here is the link");
    }

    #[test]
    fn non_matching_comment_is_ignored() {
        let conn = test_conn();
        add_automation(&conn, false);
        let comment =
            record_comment(&conn, &incoming("comment-2", "user-2", "price please")).unwrap();
        assert_eq!(comment.status, "ignored");
        assert!(next_jobs(&conn, 20).unwrap().is_empty());
    }

    #[test]
    fn delete_automation_removes_setting() {
        let conn = test_conn();
        let id = add_automation(&conn, true);
        delete_automation(&conn, id).unwrap();
        assert!(list_automations(&conn).unwrap().is_empty());
    }

    #[test]
    fn update_automation_changes_fields() {
        let conn = test_conn();
        let id = add_automation(&conn, true);
        let input = AutomationInput {
            instagram_media_id: "media-2",
            title: "photo",
            keyword: "price",
            dm_message: "price list",
            follow_required: false,
            active: false,
        };
        upsert_automation(&conn, &input, Some(id)).unwrap();
        let item = &list_automations(&conn).unwrap()[0];
        assert_eq!(item.instagram_media_id, "media-2");
        assert_eq!(item.title, "photo");
        assert_eq!(item.keyword, "price");
        assert_eq!(item.dm_message, "price list");
        assert!(!item.follow_required);
        assert!(!item.active);
    }

    #[test]
    fn create_automation_with_same_media_updates_existing_row() {
        let conn = test_conn();
        let first_id = add_automation(&conn, true);
        let input = AutomationInput {
            instagram_media_id: "media-1",
            title: "updated reel",
            keyword: "price",
            dm_message: "updated message",
            follow_required: false,
            active: false,
        };
        let second_id = upsert_automation(&conn, &input, None).unwrap();
        let rows = list_automations(&conn).unwrap();
        assert_eq!(first_id, second_id);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "updated reel");
        assert!(!rows[0].follow_required);
    }

    #[test]
    fn upsert_automation_normalizes_keywords_and_default_title() {
        let conn = test_conn();
        let input = AutomationInput {
            instagram_media_id: "media-9",
            title: "",
            keyword: " 정보 ,, 링크 ",
            dm_message: "msg",
            follow_required: true,
            active: true,
        };
        upsert_automation(&conn, &input, None).unwrap();
        let item = &list_automations(&conn).unwrap()[0];
        assert_eq!(item.keyword, "정보, 링크");
        assert_eq!(item.title, "media-9");
    }

    #[test]
    fn claim_jobs_prevents_reclaiming_same_batch() {
        let conn = test_conn();
        add_automation(&conn, false);
        for i in 0..2000 {
            record_comment(
                &conn,
                &incoming(&format!("comment-{i}"), &format!("user-{i}"), "info"),
            )
            .unwrap();
        }
        let first: Vec<i64> = claim_jobs(&conn, 500)
            .unwrap()
            .iter()
            .map(|j| j.id)
            .collect();
        let second: Vec<i64> = claim_jobs(&conn, 500)
            .unwrap()
            .iter()
            .map(|j| j.id)
            .collect();
        assert_eq!(first.len(), 500);
        assert_eq!(second.len(), 500);
        assert!(first.iter().all(|id| !second.contains(id)));
    }

    #[test]
    fn upsert_instagram_media_keeps_latest_values() {
        let conn = test_conn();
        let item = |id: &str, caption: &str, ts: &str| MediaItem {
            id: id.into(),
            caption: Some(caption.into()),
            timestamp: Some(ts.into()),
            ..Default::default()
        };
        let saved = upsert_instagram_media(
            &conn,
            &[
                item("media-1", "first caption", "2026-06-30T00:00:00+0000"),
                item("media-2", "second caption", "2026-06-29T00:00:00+0000"),
            ],
        )
        .unwrap();
        upsert_instagram_media(
            &conn,
            &[item(
                "media-1",
                "updated caption",
                "2026-06-30T00:00:00+0000",
            )],
        )
        .unwrap();
        let media = list_instagram_media(&conn, 20).unwrap();
        assert_eq!(saved, 2);
        assert_eq!(media.len(), 2);
        assert_eq!(media[0].instagram_media_id, "media-1");
        assert_eq!(media[0].caption, "updated caption");
    }

    #[test]
    fn oauth_state_is_consumed_once() {
        let conn = test_conn();
        let state = create_oauth_state(&conn).unwrap();
        let stored: String = conn
            .query_row("SELECT state_hash FROM oauth_states", [], |r| r.get(0))
            .unwrap();
        assert_ne!(stored, state);
        assert!(consume_oauth_state(&conn, &state).unwrap());
        assert!(!consume_oauth_state(&conn, &state).unwrap());
    }

    #[test]
    fn dashboard_counts_reports_automation_media_and_jobs() {
        let conn = test_conn();
        add_automation(&conn, false);
        upsert_instagram_media(
            &conn,
            &[MediaItem {
                id: "media-1".into(),
                timestamp: Some("2026-06-30".into()),
                ..Default::default()
            }],
        )
        .unwrap();
        record_comment(
            &conn,
            &incoming("comment-dashboard", "user-d", "info please"),
        )
        .unwrap();
        let counts = dashboard_counts(&conn).unwrap();
        assert_eq!(counts["automations"], 1);
        assert_eq!(counts["active_automations"], 1);
        assert_eq!(counts["media"], 1);
        assert_eq!(counts["comments"], 1);
        assert_eq!(counts["jobs_queued"], 1);
    }

    #[test]
    fn reset_stale_processing_jobs_requeues_only_old_claims() {
        let conn = test_conn();
        add_automation(&conn, false);
        for i in 0..2 {
            record_comment(
                &conn,
                &incoming(&format!("stale-{i}"), &format!("u-{i}"), "info"),
            )
            .unwrap();
        }
        let claimed = claim_jobs(&conn, 2).unwrap();
        let (old_id, new_id) = (claimed[0].id, claimed[1].id);
        conn.execute(
            "UPDATE dm_jobs SET updated_at = datetime(CURRENT_TIMESTAMP, '-20 minutes') WHERE id = ?1",
            params![old_id],
        )
        .unwrap();
        assert_eq!(reset_stale_processing_jobs(&conn, 15).unwrap(), 1);
        let status = |id: i64| -> String {
            conn.query_row(
                "SELECT status FROM dm_jobs WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(status(old_id), "queued");
        assert_eq!(status(new_id), "processing");
    }

    #[test]
    fn requeue_failed_jobs_resets_attempts_and_errors() {
        let conn = test_conn();
        add_automation(&conn, false);
        let comment = record_comment(&conn, &incoming("failed-comment", "u-f", "info")).unwrap();
        let job = &next_jobs(&conn, 20).unwrap()[0];
        retry_job(&conn, job.id, comment.id, 4, "token expired").unwrap();
        assert_eq!(requeue_failed_jobs(&conn).unwrap(), 1);
        let (status, attempts, last_error, comment_status, error): (
            String,
            i64,
            String,
            String,
            String,
        ) = conn
            .query_row(
                "SELECT j.status, j.attempts, j.last_error, c.status, c.error
                 FROM dm_jobs j JOIN comments c ON c.id = j.comment_id WHERE j.id = ?1",
                params![job.id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!(
            (status.as_str(), attempts, last_error.as_str()),
            ("queued", 0, "")
        );
        assert_eq!((comment_status.as_str(), error.as_str()), ("queued", ""));
    }

    #[test]
    fn truncate_keeps_korean_characters_whole() {
        assert_eq!(truncate("가나다", 2), "가나");
    }
}

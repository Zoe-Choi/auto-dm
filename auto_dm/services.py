from __future__ import annotations

import hashlib
import secrets
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from sqlite3 import Connection, IntegrityError, Row
from uuid import uuid4

MAX_ATTEMPTS = 5
FOLLOW_CHECK_PAYLOAD = "AUTO_DM_CHECK:"


@dataclass(frozen=True)
class IncomingComment:
    comment_id: str
    media_id: str
    commenter_id: str
    commenter_username: str
    text: str


def normalize_keyword(value: str) -> str:
    return value.strip().casefold()


def split_keywords(value: str) -> list[str]:
    return [k for k in (normalize_keyword(part) for part in value.split(",")) if k]


def keyword_matches(keyword: str, text: str) -> bool:
    lowered = text.casefold()
    return any(k in lowered for k in split_keywords(keyword))


def _oauth_state_hash(state: str) -> str:
    return hashlib.sha256(state.encode("utf-8")).hexdigest()


def create_oauth_state(conn: Connection) -> str:
    state = secrets.token_urlsafe(24)
    conn.execute("INSERT INTO oauth_states (state_hash) VALUES (?)", (_oauth_state_hash(state),))
    return state


def consume_oauth_state(conn: Connection, state: str) -> bool:
    cursor = conn.execute(
        "DELETE FROM oauth_states WHERE state_hash = ?",
        (_oauth_state_hash(state),),
    )
    return cursor.rowcount == 1


def upsert_automation(
    conn: Connection,
    *,
    instagram_media_id: str,
    title: str,
    keyword: str,
    dm_message: str,
    active: bool,
    follow_required: bool = True,
    automation_id: int | None = None,
) -> int:
    media_id = instagram_media_id.strip()
    values = {
        "instagram_media_id": media_id,
        "title": title.strip() or media_id,
        "keyword": ", ".join(split_keywords(keyword)),
        "dm_message": dm_message.strip(),
        "follow_required": int(follow_required),
        "active": int(active),
    }
    if automation_id:
        conn.execute(
            """
            UPDATE automations
            SET instagram_media_id = :instagram_media_id,
                title = :title,
                keyword = :keyword,
                dm_message = :dm_message,
                follow_required = :follow_required,
                active = :active,
                updated_at = CURRENT_TIMESTAMP
            WHERE id = :id
            """,
            {**values, "id": automation_id},
        )
        return automation_id

    conn.execute(
        """
        INSERT INTO automations (
            instagram_media_id, title, keyword, dm_message, follow_required, active
        ) VALUES (
            :instagram_media_id, :title, :keyword, :dm_message, :follow_required, :active
        )
        ON CONFLICT(instagram_media_id) DO UPDATE SET
            title = excluded.title,
            keyword = excluded.keyword,
            dm_message = excluded.dm_message,
            follow_required = excluded.follow_required,
            active = excluded.active,
            updated_at = CURRENT_TIMESTAMP
        """,
        values,
    )
    row = conn.execute(
        "SELECT id FROM automations WHERE instagram_media_id = ?",
        (values["instagram_media_id"],),
    ).fetchone()
    return int(row["id"])


def delete_automation(conn: Connection, automation_id: int) -> None:
    conn.execute("DELETE FROM automations WHERE id = ?", (automation_id,))

def upsert_instagram_media(conn: Connection, media_items: list[dict]) -> int:
    saved = 0
    for item in media_items:
        media_id = str(item.get("id") or "").strip()
        if not media_id:
            continue
        conn.execute(
            """
            INSERT INTO instagram_media (
                instagram_media_id, caption, media_type, permalink, media_url, thumbnail_url,
                timestamp
            ) VALUES (?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(instagram_media_id) DO UPDATE SET
                caption = excluded.caption,
                media_type = excluded.media_type,
                permalink = excluded.permalink,
                media_url = excluded.media_url,
                thumbnail_url = excluded.thumbnail_url,
                timestamp = excluded.timestamp,
                updated_at = CURRENT_TIMESTAMP
            """,
            (
                media_id,
                str(item.get("caption") or ""),
                str(item.get("media_type") or ""),
                str(item.get("permalink") or ""),
                str(item.get("media_url") or ""),
                str(item.get("thumbnail_url") or ""),
                str(item.get("timestamp") or ""),
            ),
        )
        saved += 1
    return saved


def list_instagram_media(conn: Connection, limit: int = 20) -> list[Row]:
    return list(
        conn.execute(
            """
            SELECT *
            FROM instagram_media
            ORDER BY timestamp DESC, updated_at DESC, id DESC
            LIMIT ?
            """,
            (limit,),
        )
    )

def list_automations(conn: Connection) -> list[Row]:
    return list(conn.execute("SELECT * FROM automations ORDER BY created_at DESC, id DESC"))


def find_matching_automation(conn: Connection, media_id: str, text: str) -> Row | None:
    rows = conn.execute(
        "SELECT * FROM automations WHERE active = 1 AND instagram_media_id = ? ORDER BY id DESC",
        (media_id,),
    ).fetchall()
    for row in rows:
        if keyword_matches(row["keyword"], text):
            return row
    return None


def record_comment(conn: Connection, incoming: IncomingComment) -> Row | None:
    automation = find_matching_automation(conn, incoming.media_id, incoming.text)
    status = "queued" if automation else "ignored"
    automation_id = automation["id"] if automation else None
    try:
        cursor = conn.execute(
            """
            INSERT INTO comments (
                automation_id, instagram_comment_id, instagram_media_id, commenter_id,
                commenter_username, text, status
            ) VALUES (?, ?, ?, ?, ?, ?, ?)
            """,
            (
                automation_id,
                incoming.comment_id,
                incoming.media_id,
                incoming.commenter_id,
                incoming.commenter_username,
                incoming.text,
                status,
            ),
        )
        comment_id = int(cursor.lastrowid)
    except IntegrityError:
        return conn.execute(
            "SELECT * FROM comments WHERE instagram_comment_id = ?",
            (incoming.comment_id,),
        ).fetchone()

    if automation:
        conn.execute("INSERT INTO dm_jobs (comment_id) VALUES (?)", (comment_id,))
    return conn.execute("SELECT * FROM comments WHERE id = ?", (comment_id,)).fetchone()


def list_recent_comments(conn: Connection, limit: int = 50) -> list[Row]:
    return list(
        conn.execute(
            """
            SELECT c.*, a.title AS automation_title
            FROM comments c
            LEFT JOIN automations a ON a.id = c.automation_id
            ORDER BY c.created_at DESC, c.id DESC
            LIMIT ?
            """,
            (limit,),
        )
    )


def next_jobs(conn: Connection, limit: int = 20) -> list[Row]:
    return list(
        conn.execute(
            """
            SELECT j.*, c.instagram_comment_id, c.commenter_id, c.commenter_username,
                   c.recipient_id, c.status AS comment_status, a.dm_message, a.follow_required
            FROM dm_jobs j
            JOIN comments c ON c.id = j.comment_id
            JOIN automations a ON a.id = c.automation_id
            WHERE j.status = 'queued'
              AND j.next_attempt_at <= CURRENT_TIMESTAMP
            ORDER BY j.created_at ASC, j.id ASC
            LIMIT ?
            """,
            (limit,),
        )
    )




def reset_stale_processing_jobs(conn: Connection, *, stale_minutes: int = 15) -> int:
    cursor = conn.execute(
        """
        UPDATE dm_jobs
        SET status = 'queued', claim_token = '', updated_at = CURRENT_TIMESTAMP
        WHERE status = 'processing'
          AND updated_at <= datetime(CURRENT_TIMESTAMP, ?)
        """,
        (f"-{stale_minutes} minutes",),
    )
    return int(cursor.rowcount)
def claim_jobs(conn: Connection, limit: int = 20) -> list[Row]:
    token = uuid4().hex
    conn.execute(
        """
        UPDATE dm_jobs
        SET status = 'processing', claim_token = ?, updated_at = CURRENT_TIMESTAMP
        WHERE id IN (
            SELECT id
            FROM dm_jobs
            WHERE status = 'queued'
              AND next_attempt_at <= CURRENT_TIMESTAMP
            ORDER BY created_at ASC, id ASC
            LIMIT ?
        )
        """,
        (token, limit),
    )
    return list(
        conn.execute(
            """
            SELECT j.*, c.instagram_comment_id, c.commenter_id, c.commenter_username,
                   c.recipient_id, c.status AS comment_status, a.dm_message, a.follow_required
            FROM dm_jobs j
            JOIN comments c ON c.id = j.comment_id
            JOIN automations a ON a.id = c.automation_id
            WHERE j.claim_token = ? AND j.status = 'processing'
            ORDER BY j.created_at ASC, j.id ASC
            """,
            (token,),
        )
    )


def mark_job_sent(conn: Connection, job_id: int, comment_id: int) -> None:
    conn.execute(
        """
        UPDATE dm_jobs
        SET status = 'sent', claim_token = '', updated_at = CURRENT_TIMESTAMP
        WHERE id = ?
        """,
        (job_id,),
    )
    conn.execute(
        "UPDATE comments SET status = 'sent', error = '', updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        (comment_id,),
    )


def note_comment_error(conn: Connection, comment_id: int, error: str) -> None:
    conn.execute(
        "UPDATE comments SET error = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        (error[:500], comment_id),
    )


def mark_job_waiting(conn: Connection, job_id: int, comment_id: int, status: str) -> None:
    """Park a job until the commenter taps a DM button. status: waiting_click | waiting_follow."""
    conn.execute(
        """
        UPDATE dm_jobs
        SET status = 'waiting', claim_token = '', last_error = '', updated_at = CURRENT_TIMESTAMP
        WHERE id = ?
        """,
        (job_id,),
    )
    conn.execute(
        "UPDATE comments SET status = ?, error = '', updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        (status, comment_id),
    )


def follow_check_payload(comment_id: int) -> str:
    return f"{FOLLOW_CHECK_PAYLOAD}{comment_id}"


def request_follow_check(conn: Connection, comment_id: int, recipient_id: str) -> bool:
    """Requeue a waiting job so the worker checks whether the commenter follows the account."""
    cursor = conn.execute(
        """
        UPDATE dm_jobs
        SET status = 'queued', step = 'check_follow', attempts = 0,
            next_attempt_at = CURRENT_TIMESTAMP, claim_token = '', last_error = '',
            updated_at = CURRENT_TIMESTAMP
        WHERE comment_id = ? AND status = 'waiting'
        """,
        (comment_id,),
    )
    if cursor.rowcount == 0:
        return False
    conn.execute(
        """
        UPDATE comments
        SET recipient_id = ?, status = 'queued', updated_at = CURRENT_TIMESTAMP
        WHERE id = ?
        """,
        (recipient_id, comment_id),
    )
    return True


def handle_message_event(conn: Connection, event: dict, business_id: str = "") -> bool:
    """Handle one Instagram `messaging` webhook event. Returns True when a follow check is queued."""
    sender_id = str((event.get("sender") or {}).get("id") or "")
    message = event.get("message") or {}
    postback = event.get("postback") or {}
    if not sender_id or message.get("is_echo") or (business_id and sender_id == business_id):
        return False

    payload = str((message.get("quick_reply") or {}).get("payload") or postback.get("payload") or "")
    if payload.startswith(FOLLOW_CHECK_PAYLOAD):
        suffix = payload[len(FOLLOW_CHECK_PAYLOAD):]
        if suffix.isdigit():
            return request_follow_check(conn, int(suffix), sender_id)
        return False

    # Fallback for a typed reply: the latest waiting comment from the same sender.
    row = conn.execute(
        """
        SELECT j.comment_id
        FROM dm_jobs j
        JOIN comments c ON c.id = j.comment_id
        WHERE j.status = 'waiting' AND c.commenter_id = ?
        ORDER BY j.id DESC
        LIMIT 1
        """,
        (sender_id,),
    ).fetchone()
    if not row:
        return False
    return request_follow_check(conn, int(row["comment_id"]), sender_id)


def requeue_failed_jobs(conn: Connection) -> int:
    cursor = conn.execute(
        """
        UPDATE dm_jobs
        SET status = 'queued', attempts = 0, next_attempt_at = CURRENT_TIMESTAMP,
            claim_token = '', last_error = '', updated_at = CURRENT_TIMESTAMP
        WHERE status = 'failed'
        """
    )
    conn.execute(
        """
        UPDATE comments
        SET status = 'queued', error = '', updated_at = CURRENT_TIMESTAMP
        WHERE id IN (SELECT comment_id FROM dm_jobs WHERE status = 'queued')
          AND status = 'failed'
        """
    )
    return int(cursor.rowcount)
def retry_job(conn: Connection, job_id: int, comment_id: int, attempts: int, error: str) -> None:
    next_status = "failed" if attempts + 1 >= MAX_ATTEMPTS else "queued"
    next_time = datetime.now(timezone.utc) + timedelta(minutes=min(30, 2 ** attempts))
    conn.execute(
        """
        UPDATE dm_jobs
        SET status = ?, attempts = attempts + 1, next_attempt_at = ?, claim_token = '',
            last_error = ?, updated_at = CURRENT_TIMESTAMP
        WHERE id = ?
        """,
        (next_status, next_time.strftime("%Y-%m-%d %H:%M:%S"), error[:500], job_id),
    )
    conn.execute(
        "UPDATE comments SET status = ?, error = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        (next_status, error[:500], comment_id),
    )

def dashboard_counts(conn: Connection) -> dict[str, int]:
    tables = {
        "automations": "SELECT COUNT(*) FROM automations",
        "active_automations": "SELECT COUNT(*) FROM automations WHERE active = 1",
        "media": "SELECT COUNT(*) FROM instagram_media",
        "comments": "SELECT COUNT(*) FROM comments",
    }
    counts = {name: int(conn.execute(sql).fetchone()[0]) for name, sql in tables.items()}
    for row in conn.execute("SELECT status, COUNT(*) AS count FROM dm_jobs GROUP BY status"):
        counts[f"jobs_{row['status']}"] = int(row["count"])
    return counts

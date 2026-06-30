from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from sqlite3 import Connection, IntegrityError, Row

MAX_ATTEMPTS = 5


@dataclass(frozen=True)
class IncomingComment:
    comment_id: str
    media_id: str
    commenter_id: str
    commenter_username: str
    text: str


def normalize_keyword(value: str) -> str:
    return value.strip().casefold()


def keyword_matches(keyword: str, text: str) -> bool:
    normalized = normalize_keyword(keyword)
    if not normalized:
        return False
    return normalized in text.casefold()


def upsert_automation(
    conn: Connection,
    *,
    instagram_media_id: str,
    title: str,
    keyword: str,
    dm_message: str,
    follow_required: bool,
    active: bool,
    automation_id: int | None = None,
) -> int:
    values = {
        "instagram_media_id": instagram_media_id.strip(),
        "title": title.strip(),
        "keyword": keyword.strip(),
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

    cursor = conn.execute(
        """
        INSERT INTO automations (
            instagram_media_id, title, keyword, dm_message, follow_required, active
        ) VALUES (
            :instagram_media_id, :title, :keyword, :dm_message, :follow_required, :active
        )
        """,
        values,
    )
    return int(cursor.lastrowid)


def delete_automation(conn: Connection, automation_id: int) -> None:
    conn.execute("DELETE FROM automations WHERE id = ?", (automation_id,))


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
            SELECT j.*, c.instagram_comment_id, c.commenter_username, a.dm_message, a.follow_required
            FROM dm_jobs j
            JOIN comments c ON c.id = j.comment_id
            JOIN automations a ON a.id = c.automation_id
            WHERE j.status IN ('queued', 'ready_to_send')
              AND j.next_attempt_at <= CURRENT_TIMESTAMP
            ORDER BY j.created_at ASC, j.id ASC
            LIMIT ?
            """,
            (limit,),
        )
    )


def mark_job_sent(conn: Connection, job_id: int, comment_id: int) -> None:
    conn.execute(
        "UPDATE dm_jobs SET status = 'sent', updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        (job_id,),
    )
    conn.execute(
        "UPDATE comments SET status = 'sent', error = '', updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        (comment_id,),
    )


def mark_job_waiting_follow(conn: Connection, job_id: int, comment_id: int) -> None:
    conn.execute(
        "UPDATE dm_jobs SET status = 'waiting_follow', updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        (job_id,),
    )
    conn.execute(
        """
        UPDATE comments
        SET status = 'waiting_follow', error = 'Follower verification requires manual confirmation',
            updated_at = CURRENT_TIMESTAMP
        WHERE id = ?
        """,
        (comment_id,),
    )


def mark_follow_confirmed(conn: Connection, comment_id: int) -> bool:
    cursor = conn.execute(
        """
        UPDATE dm_jobs
        SET status = 'ready_to_send', next_attempt_at = CURRENT_TIMESTAMP,
            last_error = '', updated_at = CURRENT_TIMESTAMP
        WHERE comment_id = ? AND status = 'waiting_follow'
        """,
        (comment_id,),
    )
    if cursor.rowcount == 0:
        return False
    conn.execute(
        """
        UPDATE comments
        SET status = 'ready_to_send', error = '', updated_at = CURRENT_TIMESTAMP
        WHERE id = ?
        """,
        (comment_id,),
    )
    return True


def retry_job(conn: Connection, job_id: int, comment_id: int, attempts: int, error: str) -> None:
    next_status = "failed" if attempts + 1 >= MAX_ATTEMPTS else "queued"
    next_time = datetime.now(timezone.utc) + timedelta(minutes=min(30, 2 ** attempts))
    conn.execute(
        """
        UPDATE dm_jobs
        SET status = ?, attempts = attempts + 1, next_attempt_at = ?, last_error = ?,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = ?
        """,
        (next_status, next_time.strftime("%Y-%m-%d %H:%M:%S"), error[:500], job_id),
    )
    conn.execute(
        "UPDATE comments SET status = ?, error = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        (next_status, error[:500], comment_id),
    )

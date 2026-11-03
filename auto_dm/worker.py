from __future__ import annotations

import random
from sqlite3 import Connection, Row

from .db import connect
from .instagram import InstagramClient
from .security import unprotect_secret
from .services import (
    claim_jobs,
    follow_check_payload,
    mark_job_sent,
    mark_job_waiting,
    note_comment_error,
    reset_stale_processing_jobs,
    retry_job,
)

COMMENT_REPLIES = (
    "DM 확인해 주세요!",
    "DM 전송 완료!",
)
GET_DM_BUTTON = "DM 받기"
FOLLOWED_BUTTON = "팔로우했어요"
GET_DM_PROMPT = "댓글 감사합니다! 아래 'DM 받기' 버튼을 눌러주세요."
FOLLOW_PROMPT = (
    "팔로워분들께만 보내드리고 있어요.\n"
    "아래 프로필에서 팔로우한 뒤 '팔로우했어요' 버튼을 눌러주세요.\n"
    "{profile_url}"
)


def quick_reply(title: str, comment_id: int) -> list[dict[str, str]]:
    return [{"content_type": "text", "title": title, "payload": follow_check_payload(comment_id)}]


def profile_url(username: str) -> str:
    return f"https://www.instagram.com/{username}/"


async def _send_first_message(
    conn: Connection, client: InstagramClient, job: Row, token: str
) -> None:
    if job["follow_required"]:
        await client.send_private_reply(
            job["instagram_comment_id"],
            GET_DM_PROMPT,
            token,
            quick_replies=quick_reply(GET_DM_BUTTON, job["comment_id"]),
        )
        mark_job_waiting(conn, job["id"], job["comment_id"], "waiting_click")
    else:
        await client.send_private_reply(job["instagram_comment_id"], job["dm_message"], token)
        mark_job_sent(conn, job["id"], job["comment_id"])


async def _check_follow_and_send(
    conn: Connection, client: InstagramClient, job: Row, token: str, username: str
) -> None:
    recipient_id = job["recipient_id"]
    if await client.is_user_following(recipient_id, token):
        await client.send_message(recipient_id, job["dm_message"], token)
        mark_job_sent(conn, job["id"], job["comment_id"])
        return
    await client.send_message(
        recipient_id,
        FOLLOW_PROMPT.format(profile_url=profile_url(username)),
        token,
        quick_replies=quick_reply(FOLLOWED_BUTTON, job["comment_id"]),
    )
    mark_job_waiting(conn, job["id"], job["comment_id"], "waiting_follow")


async def process_jobs(*, limit: int = 20, client: InstagramClient | None = None) -> int:
    client = client or InstagramClient()
    processed = 0
    with connect() as conn:
        account = conn.execute(
            "SELECT * FROM instagram_accounts ORDER BY updated_at DESC, id DESC LIMIT 1"
        ).fetchone()
        if not account:
            return 0
        token = account["page_access_token"] or account["access_token"]
        access_token = unprotect_secret(token)
        reset_stale_processing_jobs(conn)
        jobs = claim_jobs(conn, limit=limit)
        for job in jobs:
            is_first = job["step"] != "check_follow"
            try:
                if is_first:
                    await _send_first_message(conn, client, job, access_token)
                else:
                    await _check_follow_and_send(
                        conn, client, job, access_token, account["username"]
                    )
            except Exception as exc:  # noqa: BLE001
                retry_job(conn, job["id"], job["comment_id"], job["attempts"], str(exc))
                continue
            processed += 1
            if not is_first:
                continue
            # The DM is already sent, so a failed public reply must not requeue the job.
            try:
                await client.reply_to_comment(
                    job["instagram_comment_id"], random.choice(COMMENT_REPLIES), access_token
                )
            except Exception as exc:  # noqa: BLE001
                note_comment_error(conn, job["comment_id"], f"댓글 답글 실패: {exc}")
    return processed

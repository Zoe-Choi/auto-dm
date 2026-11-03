from __future__ import annotations

import random

from .db import connect
from .instagram import InstagramClient
from .security import unprotect_secret
from .services import (
    claim_jobs,
    mark_job_sent,
    note_comment_error,
    reset_stale_processing_jobs,
    retry_job,
)

COMMENT_REPLIES = (
    "DM 확인해 주세요!",
    "DM 전송 완료!",
)


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
            try:
                await client.send_private_reply(
                    job["instagram_comment_id"], job["dm_message"], access_token
                )
            except Exception as exc:  # noqa: BLE001
                retry_job(conn, job["id"], job["comment_id"], job["attempts"], str(exc))
                continue
            mark_job_sent(conn, job["id"], job["comment_id"])
            processed += 1
            # The DM is already sent, so a failed public reply must not requeue the job.
            try:
                await client.reply_to_comment(
                    job["instagram_comment_id"], random.choice(COMMENT_REPLIES), access_token
                )
            except Exception as exc:  # noqa: BLE001
                note_comment_error(conn, job["comment_id"], f"댓글 답글 실패: {exc}")
    return processed

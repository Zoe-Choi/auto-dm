from __future__ import annotations

from .db import connect
from .instagram import InstagramClient
from .security import unprotect_secret
from .services import mark_job_sent, mark_job_waiting_follow, next_jobs, retry_job

FOLLOW_PROMPT = "팔로우 후 다시 댓글을 남겨주세요. 확인되면 안내 DM을 보내드릴게요."


async def process_jobs(*, limit: int = 20, client: InstagramClient | None = None) -> int:
    client = client or InstagramClient()
    processed = 0
    with connect() as conn:
        account = conn.execute(
            "SELECT * FROM instagram_accounts ORDER BY updated_at DESC, id DESC LIMIT 1"
        ).fetchone()
        if not account:
            return 0
        access_token = unprotect_secret(account["access_token"])
        jobs = next_jobs(conn, limit=limit)
        for job in jobs:
            try:
                if job["follow_required"]:
                    await client.send_private_reply(
                        job["instagram_comment_id"], FOLLOW_PROMPT, access_token
                    )
                    mark_job_waiting_follow(conn, job["id"], job["comment_id"])
                else:
                    await client.send_private_reply(
                        job["instagram_comment_id"], job["dm_message"], access_token
                    )
                    mark_job_sent(conn, job["id"], job["comment_id"])
                processed += 1
            except Exception as exc:  # noqa: BLE001
                retry_job(conn, job["id"], job["comment_id"], job["attempts"], str(exc))
    return processed
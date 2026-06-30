from __future__ import annotations

from .db import connect
from .instagram import InstagramClient
from .security import unprotect_secret
from .services import claim_jobs, mark_job_sent, mark_job_waiting_follow, reset_stale_processing_jobs, retry_job

FOLLOW_PROMPT = "Please follow this account and leave the keyword comment again. We will send the DM after confirmation."


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
                if job["comment_status"] == "ready_to_send" or not job["follow_required"]:
                    await client.send_private_reply(
                        job["instagram_comment_id"], job["dm_message"], access_token
                    )
                    mark_job_sent(conn, job["id"], job["comment_id"])
                else:
                    await client.send_private_reply(
                        job["instagram_comment_id"], FOLLOW_PROMPT, access_token
                    )
                    mark_job_waiting_follow(conn, job["id"], job["comment_id"])
                processed += 1
            except Exception as exc:  # noqa: BLE001
                retry_job(conn, job["id"], job["comment_id"], job["attempts"], str(exc))
    return processed

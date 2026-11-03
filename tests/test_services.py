import asyncio
import tempfile
import unittest
from functools import partial
from pathlib import Path
from unittest.mock import patch
from urllib.parse import parse_qs, urlparse

from auto_dm.admin import hash_admin_password, valid_admin_credentials
from auto_dm.config import Settings
from auto_dm.db import connect, init_db
from auto_dm.instagram import InstagramClient
from auto_dm.services import (
    IncomingComment,
    claim_jobs,
    consume_oauth_state,
    create_oauth_state,
    dashboard_counts,
    delete_automation,
    follow_check_payload,
    handle_message_event,
    keyword_matches,
    list_automations,
    list_instagram_media,
    next_jobs,
    record_comment,
    requeue_failed_jobs,
    reset_stale_processing_jobs,
    retry_job,
    upsert_automation,
    upsert_instagram_media,
)
from auto_dm.webhook import meta_signature, valid_meta_signature
from auto_dm.worker import (
    COMMENT_REPLIES,
    FOLLOWED_BUTTON,
    GET_DM_BUTTON,
    GET_DM_PROMPT,
    process_jobs,
)


class ServiceTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.db_path = Path(self.tmp.name) / "test.sqlite3"
        init_db(self.db_path)

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def add_automation(self, conn, *, follow_required=False):
        return upsert_automation(
            conn,
            instagram_media_id="media-1",
            title="reel",
            keyword="info",
            dm_message="here is the link",
            follow_required=follow_required,
            active=True,
        )

    def test_keyword_matches_case_insensitive(self) -> None:
        self.assertTrue(keyword_matches("info", "send INFO please"))
        self.assertFalse(keyword_matches("info", "price please"))

    def test_record_comment_creates_one_job_for_matching_comment(self) -> None:
        with connect(self.db_path) as conn:
            automation_id = self.add_automation(conn)
            comment = record_comment(
                conn,
                IncomingComment(
                    comment_id="comment-1",
                    media_id="media-1",
                    commenter_id="user-1",
                    commenter_username="tester",
                    text="info please",
                ),
            )
            duplicate = record_comment(
                conn,
                IncomingComment(
                    comment_id="comment-1",
                    media_id="media-1",
                    commenter_id="user-1",
                    commenter_username="tester",
                    text="info please",
                ),
            )
            jobs = next_jobs(conn)

        self.assertIsNotNone(comment)
        self.assertIsNotNone(duplicate)
        self.assertEqual(comment["automation_id"], automation_id)
        self.assertEqual(duplicate["id"], comment["id"])
        self.assertEqual(len(jobs), 1)
        self.assertEqual(jobs[0]["dm_message"], "here is the link")

    def test_non_matching_comment_is_ignored(self) -> None:
        with connect(self.db_path) as conn:
            self.add_automation(conn)
            comment = record_comment(
                conn,
                IncomingComment(
                    comment_id="comment-2",
                    media_id="media-1",
                    commenter_id="user-2",
                    commenter_username="tester",
                    text="price please",
                ),
            )
            jobs = next_jobs(conn)

        self.assertIsNotNone(comment)
        self.assertEqual(comment["status"], "ignored")
        self.assertEqual(jobs, [])

    def test_delete_automation_removes_setting(self) -> None:
        with connect(self.db_path) as conn:
            automation_id = self.add_automation(conn)
            delete_automation(conn, automation_id)
            self.assertEqual(list_automations(conn), [])

    def test_update_automation_changes_keyword_and_message(self) -> None:
        with connect(self.db_path) as conn:
            automation_id = self.add_automation(conn)
            upsert_automation(
                conn,
                automation_id=automation_id,
                instagram_media_id="media-2",
                title="photo",
                keyword="price",
                dm_message="price list",
                active=False,
            )
            item = list_automations(conn)[0]

        self.assertEqual(item["instagram_media_id"], "media-2")
        self.assertEqual(item["title"], "photo")
        self.assertEqual(item["keyword"], "price")
        self.assertEqual(item["dm_message"], "price list")
        self.assertEqual(item["active"], 0)

    def test_claim_jobs_prevents_reclaiming_same_batch(self) -> None:
        with connect(self.db_path) as conn:
            self.add_automation(conn)
            for i in range(2000):
                record_comment(
                    conn,
                    IncomingComment(
                        comment_id=f"comment-{i}",
                        media_id="media-1",
                        commenter_id=f"user-{i}",
                        commenter_username="tester",
                        text="info please",
                    ),
                )
            first_batch = claim_jobs(conn, limit=500)
            second_batch = claim_jobs(conn, limit=500)

        first_ids = {row["id"] for row in first_batch}
        second_ids = {row["id"] for row in second_batch}
        self.assertEqual(len(first_batch), 500)
        self.assertEqual(len(second_batch), 500)
        self.assertEqual(len(first_ids & second_ids), 0)

    def test_upsert_instagram_media_keeps_latest_values(self) -> None:
        with connect(self.db_path) as conn:
            saved = upsert_instagram_media(
                conn,
                [
                    {
                        "id": "media-1",
                        "caption": "first caption",
                        "media_type": "IMAGE",
                        "permalink": "https://example.com/1",
                        "timestamp": "2026-06-30T00:00:00+0000",
                    },
                    {
                        "id": "media-2",
                        "caption": "second caption",
                        "media_type": "VIDEO",
                        "permalink": "https://example.com/2",
                        "timestamp": "2026-06-29T00:00:00+0000",
                    },
                ],
            )
            upsert_instagram_media(
                conn,
                [
                    {
                        "id": "media-1",
                        "caption": "updated caption",
                        "media_type": "IMAGE",
                        "permalink": "https://example.com/updated",
                        "timestamp": "2026-06-30T00:00:00+0000",
                    }
                ],
            )
            media = list_instagram_media(conn)

        self.assertEqual(saved, 2)
        self.assertEqual(len(media), 2)
        self.assertEqual(media[0]["instagram_media_id"], "media-1")
        self.assertEqual(media[0]["caption"], "updated caption")



    def test_admin_credentials_check_hash(self) -> None:
        password_hash = hash_admin_password("secret")

        self.assertTrue(
            valid_admin_credentials(
                "admin",
                "secret",
                expected_username="admin",
                expected_password_hash=password_hash,
            )
        )
        self.assertFalse(
            valid_admin_credentials(
                "admin",
                "wrong",
                expected_username="admin",
                expected_password_hash=password_hash,
            )
        )
        self.assertFalse(
            valid_admin_credentials(
                "other",
                "secret",
                expected_username="admin",
                expected_password_hash=password_hash,
            )
        )

    def test_meta_webhook_signature_validation(self) -> None:
        payload = b'{"entry": []}'
        signature = meta_signature(payload, "app-secret")

        self.assertTrue(valid_meta_signature(payload, signature, "app-secret"))
        self.assertFalse(valid_meta_signature(payload, signature, "other-secret"))
        self.assertFalse(valid_meta_signature(payload, None, "app-secret"))
        self.assertTrue(valid_meta_signature(payload, None, ""))

    def test_oauth_state_is_consumed_once(self) -> None:
        with connect(self.db_path) as conn:
            state = create_oauth_state(conn)
            stored = conn.execute("SELECT state_hash FROM oauth_states").fetchone()
            first = consume_oauth_state(conn, state)
            second = consume_oauth_state(conn, state)

        self.assertIsNotNone(stored)
        self.assertNotEqual(stored["state_hash"], state)
        self.assertTrue(first)
        self.assertFalse(second)

    def test_instagram_auth_url_includes_oauth_state(self) -> None:
        client = InstagramClient(
            Settings(
                meta_app_id="app-id",
                meta_redirect_uri="https://example.com/auth/instagram/callback",
            )
        )
        query = parse_qs(urlparse(client.auth_url(state="state-1")).query)

        self.assertEqual(query["client_id"], ["app-id"])
        self.assertEqual(query["redirect_uri"], ["https://example.com/auth/instagram/callback"])
        self.assertEqual(query["state"], ["state-1"])

    def test_dashboard_counts_reports_automation_media_and_jobs(self) -> None:
        with connect(self.db_path) as conn:
            self.add_automation(conn)
            upsert_instagram_media(conn, [{"id": "media-1", "timestamp": "2026-06-30"}])
            record_comment(
                conn,
                IncomingComment(
                    comment_id="comment-dashboard",
                    media_id="media-1",
                    commenter_id="user-dashboard",
                    commenter_username="tester",
                    text="info please",
                ),
            )
            counts = dashboard_counts(conn)

        self.assertEqual(counts["automations"], 1)
        self.assertEqual(counts["active_automations"], 1)
        self.assertEqual(counts["media"], 1)
        self.assertEqual(counts["comments"], 1)
        self.assertEqual(counts["jobs_queued"], 1)

    def test_reset_stale_processing_jobs_requeues_only_old_claims(self) -> None:
        with connect(self.db_path) as conn:
            self.add_automation(conn)
            for i in range(2):
                record_comment(
                    conn,
                    IncomingComment(
                        comment_id=f"stale-comment-{i}",
                        media_id="media-1",
                        commenter_id=f"stale-user-{i}",
                        commenter_username="tester",
                        text="info please",
                    ),
                )
            claimed = claim_jobs(conn, limit=2)
            old_job_id = claimed[0]["id"]
            new_job_id = claimed[1]["id"]
            conn.execute(
                "UPDATE dm_jobs SET updated_at = datetime(CURRENT_TIMESTAMP, '-20 minutes') WHERE id = ?",
                (old_job_id,),
            )
            reset_count = reset_stale_processing_jobs(conn, stale_minutes=15)
            rows = conn.execute(
                "SELECT id, status FROM dm_jobs WHERE id IN (?, ?) ORDER BY id",
                (old_job_id, new_job_id),
            ).fetchall()

        self.assertEqual(reset_count, 1)
        self.assertEqual(rows[0]["status"], "queued")
        self.assertEqual(rows[1]["status"], "processing")

    def test_requeue_failed_jobs_resets_attempts_and_errors(self) -> None:
        with connect(self.db_path) as conn:
            self.add_automation(conn)
            comment = record_comment(
                conn,
                IncomingComment(
                    comment_id="failed-comment",
                    media_id="media-1",
                    commenter_id="failed-user",
                    commenter_username="tester",
                    text="info please",
                ),
            )
            job = next_jobs(conn)[0]
            retry_job(conn, job["id"], comment["id"], 4, "token expired")
            count = requeue_failed_jobs(conn)
            row = conn.execute(
                "SELECT j.status, j.attempts, j.last_error, c.status AS comment_status, c.error "
                "FROM dm_jobs j JOIN comments c ON c.id = j.comment_id WHERE j.id = ?",
                (job["id"],),
            ).fetchone()

        self.assertEqual(count, 1)
        self.assertEqual(row["status"], "queued")
        self.assertEqual(row["attempts"], 0)
        self.assertEqual(row["last_error"], "")
        self.assertEqual(row["comment_status"], "queued")
        self.assertEqual(row["error"], "")

    def test_create_automation_with_same_media_updates_existing_row(self) -> None:
        with connect(self.db_path) as conn:
            first_id = self.add_automation(conn)
            second_id = upsert_automation(
                conn,
                instagram_media_id="media-1",
                title="updated reel",
                keyword="price",
                dm_message="updated message",
                active=False,
            )
            rows = list_automations(conn)

        self.assertEqual(first_id, second_id)
        self.assertEqual(len(rows), 1)
        self.assertEqual(rows[0]["title"], "updated reel")
        self.assertEqual(rows[0]["keyword"], "price")
        self.assertEqual(rows[0]["dm_message"], "updated message")
        self.assertEqual(rows[0]["active"], 0)

    def test_keyword_matches_any_comma_separated_keyword(self) -> None:
        self.assertTrue(keyword_matches("정보, 링크", "링크 주세요"))
        self.assertTrue(keyword_matches("정보,링크", "정보요"))
        self.assertFalse(keyword_matches("정보, 링크", "가격 문의"))
        self.assertFalse(keyword_matches(" , ", "anything"))

    def test_upsert_automation_normalizes_keywords_and_default_title(self) -> None:
        with connect(self.db_path) as conn:
            upsert_automation(
                conn,
                instagram_media_id="media-9",
                title="",
                keyword=" 정보 ,, 링크 ",
                dm_message="msg",
                active=True,
            )
            item = list_automations(conn)[0]

        self.assertEqual(item["keyword"], "정보, 링크")
        self.assertEqual(item["title"], "media-9")

    def run_worker(self, client) -> int:
        with patch("auto_dm.worker.connect", partial(connect, self.db_path)):
            return asyncio.run(process_jobs(client=client))

    def seed_job(self, conn, comment_id="worker-comment", follow_required=False):
        conn.execute(
            "INSERT INTO instagram_accounts (instagram_user_id, username, access_token) "
            "VALUES ('ig-1', 'shop', 'token-1')"
        )
        self.add_automation(conn, follow_required=follow_required)
        return record_comment(
            conn,
            IncomingComment(
                comment_id=comment_id,
                media_id="media-1",
                commenter_id="user-w",
                commenter_username="tester",
                text="info please",
            ),
        )

    def test_worker_sends_dm_then_replies_to_comment(self) -> None:
        client = FakeClient()
        with connect(self.db_path) as conn:
            comment = self.seed_job(conn)

        processed = self.run_worker(client)

        with connect(self.db_path) as conn:
            row = conn.execute(
                "SELECT status, error FROM comments WHERE id = ?", (comment["id"],)
            ).fetchone()
        self.assertEqual(processed, 1)
        self.assertEqual(client.dms, [("worker-comment", "here is the link", "token-1")])
        self.assertEqual(len(client.replies), 1)
        self.assertIn(client.replies[0][1], COMMENT_REPLIES)
        self.assertEqual(row["status"], "sent")
        self.assertEqual(row["error"], "")

    def test_worker_keeps_dm_sent_when_comment_reply_fails(self) -> None:
        client = FakeClient(fail_reply=True)
        with connect(self.db_path) as conn:
            comment = self.seed_job(conn)

        self.run_worker(client)

        with connect(self.db_path) as conn:
            row = conn.execute(
                "SELECT j.status, c.status AS comment_status, c.error "
                "FROM dm_jobs j JOIN comments c ON c.id = j.comment_id WHERE c.id = ?",
                (comment["id"],),
            ).fetchone()
        self.assertEqual(row["status"], "sent")
        self.assertEqual(row["comment_status"], "sent")
        self.assertIn("댓글 답글 실패", row["error"])

    def test_worker_retries_when_dm_fails_without_reply(self) -> None:
        client = FakeClient(fail_dm=True)
        with connect(self.db_path) as conn:
            comment = self.seed_job(conn)

        self.run_worker(client)

        with connect(self.db_path) as conn:
            row = conn.execute(
                "SELECT status, attempts FROM dm_jobs WHERE comment_id = ?", (comment["id"],)
            ).fetchone()
        self.assertEqual(row["status"], "queued")
        self.assertEqual(row["attempts"], 1)
        self.assertEqual(client.replies, [])

    def comment_state(self, comment_id):
        with connect(self.db_path) as conn:
            return conn.execute(
                "SELECT j.status, j.step, c.status AS comment_status, c.recipient_id "
                "FROM dm_jobs j JOIN comments c ON c.id = j.comment_id WHERE c.id = ?",
                (comment_id,),
            ).fetchone()

    def click(self, comment_id, sender="user-w"):
        event = {
            "sender": {"id": sender},
            "message": {"mid": "m1", "text": "x", "quick_reply": {
                "payload": follow_check_payload(comment_id),
            }},
        }
        with connect(self.db_path) as conn:
            return handle_message_event(conn, event, "ig-1")

    def test_follow_required_flow_checks_follow_before_sending_dm(self) -> None:
        with connect(self.db_path) as conn:
            comment = self.seed_job(conn, follow_required=True)
        cid = comment["id"]

        client = FakeClient(follows=False)
        self.run_worker(client)
        self.assertEqual(client.dms[0][1], GET_DM_PROMPT)
        self.assertEqual(client.dm_buttons[0][0]["title"], GET_DM_BUTTON)
        self.assertEqual(client.dm_buttons[0][0]["payload"], follow_check_payload(cid))
        self.assertEqual(len(client.replies), 1)
        self.assertEqual(self.comment_state(cid)["comment_status"], "waiting_click")
        self.assertEqual(self.run_worker(client), 0)

        self.assertTrue(self.click(cid))
        self.assertEqual(self.comment_state(cid)["step"], "check_follow")
        self.run_worker(client)
        prompt_to, prompt_text, buttons = client.messages[-1]
        self.assertEqual(prompt_to, "user-w")
        self.assertIn("https://www.instagram.com/shop/", prompt_text)
        self.assertEqual(buttons[0]["title"], FOLLOWED_BUTTON)
        self.assertEqual(self.comment_state(cid)["comment_status"], "waiting_follow")

        client.follows = True
        self.assertTrue(self.click(cid))
        self.run_worker(client)
        self.assertEqual(client.messages[-1][:2], ("user-w", "here is the link"))
        state = self.comment_state(cid)
        self.assertEqual(state["status"], "sent")
        self.assertEqual(state["comment_status"], "sent")
        self.assertEqual(len(client.replies), 1)
        self.assertFalse(self.click(cid))

    def test_message_event_ignores_echo_and_unknown_sender(self) -> None:
        with connect(self.db_path) as conn:
            comment = self.seed_job(conn, follow_required=True)
        self.run_worker(FakeClient())

        with connect(self.db_path) as conn:
            echo = {"sender": {"id": "user-w"}, "message": {"is_echo": True, "text": "hi"}}
            own = {"sender": {"id": "ig-1"}, "message": {"text": "hi"}}
            stranger = {"sender": {"id": "someone"}, "message": {"text": "hi"}}
            self.assertFalse(handle_message_event(conn, echo, "ig-1"))
            self.assertFalse(handle_message_event(conn, own, "ig-1"))
            self.assertFalse(handle_message_event(conn, stranger, "ig-1"))
            typed = {"sender": {"id": "user-w"}, "message": {"text": "링크 주세요"}}
            self.assertTrue(handle_message_event(conn, typed, "ig-1"))

        self.assertEqual(self.comment_state(comment["id"])["step"], "check_follow")

    def test_follow_check_failure_keeps_step_for_retry(self) -> None:
        with connect(self.db_path) as conn:
            comment = self.seed_job(conn, follow_required=True)
        self.run_worker(FakeClient())
        self.click(comment["id"])

        self.run_worker(FakeClient(fail_dm=True))

        state = self.comment_state(comment["id"])
        self.assertEqual(state["status"], "queued")
        self.assertEqual(state["step"], "check_follow")


class FakeClient:
    def __init__(
        self, *, fail_dm: bool = False, fail_reply: bool = False, follows: bool = False
    ) -> None:
        self.fail_dm = fail_dm
        self.fail_reply = fail_reply
        self.follows = follows
        self.dms: list[tuple[str, str, str]] = []
        self.dm_buttons: list = []
        self.messages: list[tuple[str, str, list]] = []
        self.replies: list[tuple[str, str, str]] = []

    async def send_private_reply(
        self, comment_id: str, message: str, token: str, quick_replies=None
    ) -> dict:
        if self.fail_dm:
            raise RuntimeError("dm failed")
        self.dms.append((comment_id, message, token))
        self.dm_buttons.append(quick_replies)
        return {}

    async def send_message(self, recipient_id: str, message: str, token: str, quick_replies=None):
        if self.fail_dm:
            raise RuntimeError("dm failed")
        self.messages.append((recipient_id, message, quick_replies))
        return {}

    async def is_user_following(self, recipient_id: str, token: str) -> bool:
        if self.fail_dm:
            raise RuntimeError("profile failed")
        return self.follows

    async def reply_to_comment(self, comment_id: str, message: str, token: str) -> dict:
        if self.fail_reply:
            raise RuntimeError("reply failed")
        self.replies.append((comment_id, message, token))
        return {}


if __name__ == "__main__":
    unittest.main()

import tempfile
import unittest
from pathlib import Path

from auto_dm.admin import hash_admin_password, valid_admin_credentials
from auto_dm.db import connect, init_db
from auto_dm.services import (
    IncomingComment,
    claim_jobs,
    dashboard_counts,
    delete_automation,
    keyword_matches,
    list_automations,
    list_instagram_media,
    mark_follow_confirmed,
    mark_job_waiting_follow,
    next_jobs,
    record_comment,
    upsert_automation,
    upsert_instagram_media,
)
from auto_dm.webhook import meta_signature, valid_meta_signature


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
            automation_id = self.add_automation(conn, follow_required=True)
            delete_automation(conn, automation_id)
            self.assertEqual(list_automations(conn), [])

    def test_update_automation_changes_keyword_and_message(self) -> None:
        with connect(self.db_path) as conn:
            automation_id = self.add_automation(conn, follow_required=True)
            upsert_automation(
                conn,
                automation_id=automation_id,
                instagram_media_id="media-2",
                title="photo",
                keyword="price",
                dm_message="price list",
                follow_required=False,
                active=False,
            )
            item = list_automations(conn)[0]

        self.assertEqual(item["instagram_media_id"], "media-2")
        self.assertEqual(item["title"], "photo")
        self.assertEqual(item["keyword"], "price")
        self.assertEqual(item["dm_message"], "price list")
        self.assertEqual(item["follow_required"], 0)
        self.assertEqual(item["active"], 0)

    def test_mark_follow_confirmed_requeues_final_dm(self) -> None:
        with connect(self.db_path) as conn:
            self.add_automation(conn, follow_required=True)
            comment = record_comment(
                conn,
                IncomingComment(
                    comment_id="comment-3",
                    media_id="media-1",
                    commenter_id="user-3",
                    commenter_username="tester",
                    text="info please",
                ),
            )
            job = next_jobs(conn)[0]
            mark_job_waiting_follow(conn, job["id"], comment["id"])
            changed = mark_follow_confirmed(conn, comment["id"])
            ready_job = next_jobs(conn)[0]

        self.assertTrue(changed)
        self.assertEqual(ready_job["status"], "ready_to_send")
        self.assertEqual(ready_job["dm_message"], "here is the link")

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
if __name__ == "__main__":
    unittest.main()

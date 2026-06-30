import tempfile
import unittest
from pathlib import Path

from auto_dm.db import connect, init_db
from auto_dm.services import (
    IncomingComment,
    delete_automation,
    keyword_matches,
    list_automations,
    next_jobs,
    record_comment,
    upsert_automation,
)


class ServiceTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.db_path = Path(self.tmp.name) / "test.sqlite3"
        init_db(self.db_path)

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def test_keyword_matches_case_insensitive(self) -> None:
        self.assertTrue(keyword_matches("정보", "정보 주세요"))
        self.assertTrue(keyword_matches("Info", "send INFO please"))
        self.assertFalse(keyword_matches("정보", "가격 문의"))

    def test_record_comment_creates_one_job_for_matching_comment(self) -> None:
        with connect(self.db_path) as conn:
            automation_id = upsert_automation(
                conn,
                instagram_media_id="media-1",
                title="릴스",
                keyword="정보",
                dm_message="링크입니다",
                follow_required=False,
                active=True,
            )
            comment = record_comment(
                conn,
                IncomingComment(
                    comment_id="comment-1",
                    media_id="media-1",
                    commenter_id="user-1",
                    commenter_username="tester",
                    text="정보 부탁해요",
                ),
            )
            duplicate = record_comment(
                conn,
                IncomingComment(
                    comment_id="comment-1",
                    media_id="media-1",
                    commenter_id="user-1",
                    commenter_username="tester",
                    text="정보 부탁해요",
                ),
            )
            jobs = next_jobs(conn)

        self.assertIsNotNone(comment)
        self.assertIsNotNone(duplicate)
        self.assertEqual(comment["automation_id"], automation_id)
        self.assertEqual(duplicate["id"], comment["id"])
        self.assertEqual(len(jobs), 1)
        self.assertEqual(jobs[0]["dm_message"], "링크입니다")

    def test_non_matching_comment_is_ignored(self) -> None:
        with connect(self.db_path) as conn:
            upsert_automation(
                conn,
                instagram_media_id="media-1",
                title="릴스",
                keyword="정보",
                dm_message="링크입니다",
                follow_required=False,
                active=True,
            )
            comment = record_comment(
                conn,
                IncomingComment(
                    comment_id="comment-2",
                    media_id="media-1",
                    commenter_id="user-2",
                    commenter_username="tester",
                    text="가격 궁금해요",
                ),
            )
            jobs = next_jobs(conn)

        self.assertIsNotNone(comment)
        self.assertEqual(comment["status"], "ignored")
        self.assertEqual(jobs, [])

    def test_delete_automation_removes_setting(self) -> None:
        with connect(self.db_path) as conn:
            automation_id = upsert_automation(
                conn,
                instagram_media_id="media-1",
                title="릴스",
                keyword="정보",
                dm_message="링크입니다",
                follow_required=True,
                active=True,
            )
            delete_automation(conn, automation_id)
            self.assertEqual(list_automations(conn), [])


if __name__ == "__main__":
    unittest.main()

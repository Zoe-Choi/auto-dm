//! Sends queued DMs. Mirrors `auto_dm/worker.py`.
//!
//! Job steps:
//! - `first`: the one private reply allowed per comment. With follow_required it carries a
//!   `DM 받기` button and parks the job as waiting; otherwise it is the DM itself.
//! - `check_follow`: queued again when the user taps a button or replies. Sends the DM to
//!   followers, or a follow prompt with a `팔로우했어요` button to everyone else.

use anyhow::Result;
use rusqlite::Connection;

use crate::instagram::{InstagramApi, QuickReply};
use crate::security::unprotect_secret;
use crate::services::{self, Job};

pub const COMMENT_REPLIES: [&str; 2] = ["DM 확인해 주세요!", "DM 전송 완료!"];
pub const GET_DM_BUTTON: &str = "DM 받기";
pub const FOLLOWED_BUTTON: &str = "팔로우했어요";
pub const GET_DM_PROMPT: &str = "댓글 감사합니다! 아래 'DM 받기' 버튼을 눌러주세요.";

pub fn follow_prompt(username: &str) -> String {
    format!(
        "팔로워분들께만 보내드리고 있어요.\n\
         아래 프로필에서 팔로우한 뒤 '팔로우했어요' 버튼을 눌러주세요.\n\
         https://www.instagram.com/{username}/"
    )
}

fn button(title: &str, comment_id: i64) -> [QuickReply; 1] {
    [QuickReply::text(
        title,
        services::follow_check_payload(comment_id),
    )]
}

fn random_comment_reply() -> &'static str {
    COMMENT_REPLIES[rand::random_range(0..COMMENT_REPLIES.len())]
}

async fn send_first_message(
    conn: &Connection,
    client: &impl InstagramApi,
    job: &Job,
    token: &str,
) -> Result<()> {
    if job.follow_required {
        let buttons = button(GET_DM_BUTTON, job.comment_id);
        client
            .send_private_reply(&job.instagram_comment_id, GET_DM_PROMPT, token, &buttons)
            .await?;
        services::mark_job_waiting(conn, job.id, job.comment_id, "waiting_click")
    } else {
        client
            .send_private_reply(&job.instagram_comment_id, &job.dm_message, token, &[])
            .await?;
        services::mark_job_sent(conn, job.id, job.comment_id)
    }
}

async fn check_follow_and_send(
    conn: &Connection,
    client: &impl InstagramApi,
    job: &Job,
    token: &str,
    username: &str,
) -> Result<()> {
    if client.is_user_following(&job.recipient_id, token).await? {
        client
            .send_message(&job.recipient_id, &job.dm_message, token, &[])
            .await?;
        return services::mark_job_sent(conn, job.id, job.comment_id);
    }
    let buttons = button(FOLLOWED_BUTTON, job.comment_id);
    client
        .send_message(&job.recipient_id, &follow_prompt(username), token, &buttons)
        .await?;
    services::mark_job_waiting(conn, job.id, job.comment_id, "waiting_follow")
}

/// Processes one batch and returns how many jobs succeeded.
pub async fn process_jobs(
    conn: &Connection,
    client: &impl InstagramApi,
    limit: i64,
) -> Result<usize> {
    let Some(account) = services::latest_account(conn)? else {
        return Ok(0);
    };
    let token = unprotect_secret(account.stored_token())?;
    services::reset_stale_processing_jobs(conn, 15)?;
    let jobs = services::claim_jobs(conn, limit)?;

    let mut processed = 0;
    for job in &jobs {
        let is_first = job.step != "check_follow";
        let result = if is_first {
            send_first_message(conn, client, job, &token).await
        } else {
            check_follow_and_send(conn, client, job, &token, &account.username).await
        };
        if let Err(err) = result {
            services::retry_job(
                conn,
                job.id,
                job.comment_id,
                job.attempts,
                &format!("{err:#}"),
            )?;
            continue;
        }
        processed += 1;
        if !is_first {
            continue;
        }
        // The DM is already sent, so a failed public reply must not requeue the job.
        let reply = random_comment_reply();
        if let Err(err) = client
            .reply_to_comment(&job.instagram_comment_id, reply, &token)
            .await
        {
            let note = format!("댓글 답글 실패: {err:#}");
            if let Err(db_err) = services::note_comment_error(conn, job.comment_id, &note) {
                // Keep the batch going so later jobs still run.
                tracing::error!(
                    comment_id = job.comment_id,
                    "failed to record reply error: {db_err:#}"
                );
            }
        }
    }
    Ok(processed)
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use anyhow::bail;
    use rusqlite::params;
    use serde_json::json;

    use super::*;
    use crate::db::{init, test_conn};
    use crate::services::tests::{add_automation, incoming};
    use crate::services::{follow_check_payload, handle_message_event, record_comment};

    /// Records every call. The trait takes `&self`, so recording needs interior mutability.
    #[derive(Default)]
    struct FakeClient {
        fail_dm: bool,
        fail_reply: bool,
        follows: Cell<bool>,
        private_replies: RefCell<Vec<(String, String, Vec<QuickReply>)>>,
        messages: RefCell<Vec<(String, String, Vec<QuickReply>)>>,
        replies: RefCell<Vec<(String, String)>>,
    }

    impl InstagramApi for FakeClient {
        async fn send_private_reply(
            &self,
            comment_id: &str,
            message: &str,
            _token: &str,
            quick_replies: &[QuickReply],
        ) -> Result<()> {
            if self.fail_dm {
                bail!("dm failed");
            }
            self.private_replies.borrow_mut().push((
                comment_id.into(),
                message.into(),
                quick_replies.to_vec(),
            ));
            Ok(())
        }

        async fn send_message(
            &self,
            recipient_id: &str,
            message: &str,
            _token: &str,
            quick_replies: &[QuickReply],
        ) -> Result<()> {
            if self.fail_dm {
                bail!("dm failed");
            }
            self.messages.borrow_mut().push((
                recipient_id.into(),
                message.into(),
                quick_replies.to_vec(),
            ));
            Ok(())
        }

        async fn is_user_following(&self, _recipient_id: &str, _token: &str) -> Result<bool> {
            if self.fail_dm {
                bail!("profile failed");
            }
            Ok(self.follows.get())
        }

        async fn reply_to_comment(
            &self,
            comment_id: &str,
            message: &str,
            _token: &str,
        ) -> Result<()> {
            if self.fail_reply {
                bail!("reply failed");
            }
            self.replies
                .borrow_mut()
                .push((comment_id.into(), message.into()));
            Ok(())
        }
    }

    fn seed_job(conn: &Connection, follow_required: bool) -> i64 {
        conn.execute(
            "INSERT INTO instagram_accounts (instagram_user_id, username, access_token)
             VALUES ('ig-1', 'shop', 'token-1')",
            [],
        )
        .unwrap();
        add_automation(conn, follow_required);
        record_comment(conn, &incoming("worker-comment", "user-w", "info please"))
            .unwrap()
            .id
    }

    struct State {
        job_status: String,
        step: String,
        comment_status: String,
        error: String,
    }

    fn state(conn: &Connection, comment_id: i64) -> State {
        conn.query_row(
            "SELECT j.status, j.step, c.status, c.error
             FROM dm_jobs j JOIN comments c ON c.id = j.comment_id WHERE c.id = ?1",
            params![comment_id],
            |r| {
                Ok(State {
                    job_status: r.get(0)?,
                    step: r.get(1)?,
                    comment_status: r.get(2)?,
                    error: r.get(3)?,
                })
            },
        )
        .unwrap()
    }

    fn click(conn: &Connection, comment_id: i64, sender: &str) -> bool {
        let event = json!({
            "sender": {"id": sender},
            "message": {"mid": "m1", "text": "x",
                        "quick_reply": {"payload": follow_check_payload(comment_id)}}
        });
        handle_message_event(conn, &event, "ig-1").unwrap()
    }

    #[tokio::test]
    async fn sends_dm_then_replies_to_comment() {
        let conn = test_conn();
        let comment_id = seed_job(&conn, false);
        let client = FakeClient::default();

        assert_eq!(process_jobs(&conn, &client, 20).await.unwrap(), 1);

        let dms = client.private_replies.borrow();
        assert_eq!(dms[0].0, "worker-comment");
        assert_eq!(dms[0].1, "here is the link");
        assert!(dms[0].2.is_empty());
        assert!(COMMENT_REPLIES.contains(&client.replies.borrow()[0].1.as_str()));
        let s = state(&conn, comment_id);
        assert_eq!((s.comment_status.as_str(), s.error.as_str()), ("sent", ""));
    }

    #[tokio::test]
    async fn keeps_dm_sent_when_comment_reply_fails() {
        let conn = test_conn();
        let comment_id = seed_job(&conn, false);
        let client = FakeClient {
            fail_reply: true,
            ..Default::default()
        };

        process_jobs(&conn, &client, 20).await.unwrap();

        let s = state(&conn, comment_id);
        assert_eq!(s.job_status, "sent");
        assert_eq!(s.comment_status, "sent");
        assert!(s.error.contains("댓글 답글 실패"));
    }

    #[tokio::test]
    async fn retries_when_dm_fails_without_reply() {
        let conn = test_conn();
        let comment_id = seed_job(&conn, false);
        let client = FakeClient {
            fail_dm: true,
            ..Default::default()
        };

        process_jobs(&conn, &client, 20).await.unwrap();

        let attempts: i64 = conn
            .query_row(
                "SELECT attempts FROM dm_jobs WHERE comment_id = ?1",
                params![comment_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(state(&conn, comment_id).job_status, "queued");
        assert_eq!(attempts, 1);
        assert!(client.replies.borrow().is_empty());
    }

    #[tokio::test]
    async fn continues_when_recording_reply_error_fails() {
        let conn = test_conn();
        let first = seed_job(&conn, false);
        let second = record_comment(&conn, &incoming("worker-comment-2", "user-x", "info"))
            .unwrap()
            .id;
        // Make note_comment_error fail the way a locked database would.
        conn.execute_batch(
            "CREATE TRIGGER fail_reply_note BEFORE UPDATE OF error ON comments
             WHEN NEW.error LIKE '댓글 답글 실패%'
             BEGIN SELECT RAISE(ABORT, 'database is locked'); END;",
        )
        .unwrap();
        let client = FakeClient {
            fail_reply: true,
            ..Default::default()
        };

        assert_eq!(process_jobs(&conn, &client, 20).await.unwrap(), 2);
        assert_eq!(state(&conn, first).job_status, "sent");
        assert_eq!(state(&conn, second).job_status, "sent");
    }

    #[tokio::test]
    async fn follow_required_flow_checks_follow_before_sending_dm() {
        let conn = test_conn();
        let cid = seed_job(&conn, true);
        let client = FakeClient::default();

        process_jobs(&conn, &client, 20).await.unwrap();
        {
            let dms = client.private_replies.borrow();
            assert_eq!(dms[0].1, GET_DM_PROMPT);
            assert_eq!(dms[0].2[0].title, GET_DM_BUTTON);
            assert_eq!(dms[0].2[0].payload, follow_check_payload(cid));
        }
        assert_eq!(client.replies.borrow().len(), 1);
        assert_eq!(state(&conn, cid).comment_status, "waiting_click");
        assert_eq!(process_jobs(&conn, &client, 20).await.unwrap(), 0);

        assert!(click(&conn, cid, "user-w"));
        assert_eq!(state(&conn, cid).step, "check_follow");
        process_jobs(&conn, &client, 20).await.unwrap();
        {
            let messages = client.messages.borrow();
            let (to, text, buttons) = messages.last().unwrap();
            assert_eq!(to, "user-w");
            assert!(text.contains("https://www.instagram.com/shop/"));
            assert_eq!(buttons[0].title, FOLLOWED_BUTTON);
        }
        assert_eq!(state(&conn, cid).comment_status, "waiting_follow");

        client.follows.set(true);
        assert!(click(&conn, cid, "user-w"));
        process_jobs(&conn, &client, 20).await.unwrap();
        {
            let messages = client.messages.borrow();
            let (to, text, _) = messages.last().unwrap();
            assert_eq!((to.as_str(), text.as_str()), ("user-w", "here is the link"));
        }
        let s = state(&conn, cid);
        assert_eq!(
            (s.job_status.as_str(), s.comment_status.as_str()),
            ("sent", "sent")
        );
        assert_eq!(client.replies.borrow().len(), 1);
        assert!(!click(&conn, cid, "user-w"));
    }

    #[tokio::test]
    async fn message_event_ignores_echo_and_unknown_sender() {
        let conn = test_conn();
        let cid = seed_job(&conn, true);
        process_jobs(&conn, &FakeClient::default(), 20)
            .await
            .unwrap();

        let echo = json!({"sender": {"id": "user-w"}, "message": {"is_echo": true, "text": "hi"}});
        let own = json!({"sender": {"id": "ig-1"}, "message": {"text": "hi"}});
        let stranger = json!({"sender": {"id": "someone"}, "message": {"text": "hi"}});
        assert!(!handle_message_event(&conn, &echo, "ig-1").unwrap());
        assert!(!handle_message_event(&conn, &own, "ig-1").unwrap());
        assert!(!handle_message_event(&conn, &stranger, "ig-1").unwrap());
        let typed = json!({"sender": {"id": "user-w"}, "message": {"text": "링크 주세요"}});
        assert!(handle_message_event(&conn, &typed, "ig-1").unwrap());
        assert_eq!(state(&conn, cid).step, "check_follow");
    }

    #[tokio::test]
    async fn follow_check_failure_keeps_step_for_retry() {
        let conn = test_conn();
        let cid = seed_job(&conn, true);
        process_jobs(&conn, &FakeClient::default(), 20)
            .await
            .unwrap();
        click(&conn, cid, "user-w");

        process_jobs(
            &conn,
            &FakeClient {
                fail_dm: true,
                ..Default::default()
            },
            20,
        )
        .await
        .unwrap();

        let s = state(&conn, cid);
        assert_eq!(
            (s.job_status.as_str(), s.step.as_str()),
            ("queued", "check_follow")
        );
    }

    #[tokio::test]
    async fn init_parks_legacy_follow_jobs_for_api_check() {
        let conn = test_conn();
        let waiting = seed_job(&conn, true);
        let ready = record_comment(&conn, &incoming("legacy-ready", "user-r", "info"))
            .unwrap()
            .id;
        for (comment_id, status) in [(waiting, "waiting_follow"), (ready, "ready_to_send")] {
            conn.execute(
                "UPDATE dm_jobs SET status = ?1 WHERE comment_id = ?2",
                params![status, comment_id],
            )
            .unwrap();
            conn.execute(
                "UPDATE comments SET status = ?1 WHERE id = ?2",
                params![status, comment_id],
            )
            .unwrap();
        }

        init(&conn).unwrap();
        init(&conn).unwrap();

        for comment_id in [waiting, ready] {
            let s = state(&conn, comment_id);
            assert_eq!(
                (s.job_status.as_str(), s.comment_status.as_str()),
                ("waiting", "waiting_follow")
            );
        }
        let client = FakeClient {
            follows: Cell::new(true),
            ..Default::default()
        };
        assert_eq!(process_jobs(&conn, &client, 20).await.unwrap(), 0);
        assert!(click(&conn, ready, "user-r"));
        process_jobs(&conn, &client, 20).await.unwrap();
        let messages = client.messages.borrow();
        let (to, text, _) = messages.last().unwrap();
        assert_eq!((to.as_str(), text.as_str()), ("user-r", "here is the link"));
        assert!(client.private_replies.borrow().is_empty());
        assert_eq!(state(&conn, ready).job_status, "sent");
    }
}

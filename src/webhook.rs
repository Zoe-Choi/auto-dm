//! Meta webhook signature checks and payload parsing.
//! Mirrors `auto_dm/webhook.py` plus the payload loop from the Python webhook route.

use anyhow::Result;
use hmac::{Hmac, KeyInit, Mac};
use rusqlite::Connection;
use serde_json::Value;
use sha2::Sha256;

use crate::admin::constant_time_eq;
use crate::services::{self, IncomingComment};

pub fn meta_signature(payload: &[u8], app_secret: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(app_secret.as_bytes())
        .expect("HMAC accepts keys of any length");
    mac.update(payload);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

/// An empty app secret disables the check, matching the Python behavior for local testing.
pub fn valid_meta_signature(payload: &[u8], header_value: Option<&str>, app_secret: &str) -> bool {
    if app_secret.is_empty() {
        return true;
    }
    match header_value {
        Some(value) if value.starts_with("sha256=") => constant_time_eq(
            meta_signature(payload, app_secret).as_bytes(),
            value.as_bytes(),
        ),
        _ => false,
    }
}

fn str_field<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

/// Picks the first non-empty string, like Python `a or b`.
fn first_non_empty<'a>(candidates: &[&'a str]) -> &'a str {
    candidates
        .iter()
        .copied()
        .find(|s| !s.is_empty())
        .unwrap_or("")
}

/// Turns one `changes[].value` object into a comment, or None when ids are missing.
pub fn incoming_comment(value: &Value) -> Option<IncomingComment> {
    let null = Value::Null;
    let media = value.get("media").unwrap_or(&null);
    let commenter = value.get("from").unwrap_or(&null);
    let comment_id = first_non_empty(&[str_field(value, "id"), str_field(value, "comment_id")]);
    let media_id = first_non_empty(&[str_field(media, "id"), str_field(value, "media_id")]);
    if comment_id.is_empty() || media_id.is_empty() {
        return None;
    }
    Some(IncomingComment {
        comment_id: comment_id.to_string(),
        media_id: media_id.to_string(),
        commenter_id: first_non_empty(&[str_field(commenter, "id"), str_field(value, "from_id")])
            .to_string(),
        commenter_username: first_non_empty(&[
            str_field(commenter, "username"),
            str_field(commenter, "name"),
        ])
        .to_string(),
        text: first_non_empty(&[str_field(value, "text"), str_field(value, "message")]).to_string(),
    })
}

/// Records comments and DM button clicks from one webhook POST body.
pub fn handle_payload(conn: &Connection, payload: &Value) -> Result<()> {
    let entries = payload.get("entry").and_then(Value::as_array);
    for entry in entries.into_iter().flatten() {
        let business_id = str_field(entry, "id");
        let events = entry.get("messaging").and_then(Value::as_array);
        for event in events.into_iter().flatten() {
            services::handle_message_event(conn, event, business_id)?;
        }
        let changes = entry.get("changes").and_then(Value::as_array);
        for change in changes.into_iter().flatten() {
            if let Some(incoming) = change.get("value").and_then(incoming_comment) {
                services::record_comment(conn, &incoming)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_webhook_signature_validation() {
        let payload = br#"{"entry": []}"#;
        let signature = meta_signature(payload, "app-secret");
        assert!(valid_meta_signature(
            payload,
            Some(&signature),
            "app-secret"
        ));
        assert!(!valid_meta_signature(
            payload,
            Some(&signature),
            "other-secret"
        ));
        assert!(!valid_meta_signature(payload, None, "app-secret"));
        assert!(valid_meta_signature(payload, None, ""));
    }

    #[test]
    fn incoming_comment_reads_meta_fields() {
        let value = serde_json::json!({
            "id": "c1",
            "media": {"id": "m1"},
            "from": {"id": "u1", "username": "tester"},
            "text": "info please"
        });
        let comment = incoming_comment(&value).expect("comment");
        assert_eq!(comment.comment_id, "c1");
        assert_eq!(comment.media_id, "m1");
        assert_eq!(comment.commenter_id, "u1");
        assert_eq!(comment.commenter_username, "tester");
        assert_eq!(comment.text, "info please");
        assert!(incoming_comment(&serde_json::json!({"id": "c1"})).is_none());
    }
}

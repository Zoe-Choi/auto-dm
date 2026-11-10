//! Meta Graph API client. Mirrors `auto_dm/instagram.py`.
//!
//! The worker only depends on the `InstagramApi` trait, so tests swap in a fake client.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::{RequestBuilder, Url};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::Settings;
use crate::services::MediaItem;

pub const AUTH_SCOPES: [&str; 5] = [
    "instagram_basic",
    "instagram_manage_comments",
    "instagram_manage_messages",
    "pages_show_list",
    "pages_read_engagement",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QuickReply {
    pub content_type: String,
    pub title: String,
    pub payload: String,
}

impl QuickReply {
    pub fn text(title: &str, payload: String) -> Self {
        Self {
            content_type: "text".into(),
            title: title.into(),
            payload,
        }
    }
}

/// Calls the worker makes. `async fn` in a trait works since Rust 1.75.
#[allow(async_fn_in_trait)] // Used only with concrete types inside this crate.
pub trait InstagramApi {
    /// The one DM allowed per comment; the user must respond before follow-up messages.
    async fn send_private_reply(
        &self,
        comment_id: &str,
        message: &str,
        token: &str,
        quick_replies: &[QuickReply],
    ) -> Result<()>;
    async fn send_message(
        &self,
        recipient_id: &str,
        message: &str,
        token: &str,
        quick_replies: &[QuickReply],
    ) -> Result<()>;
    /// Needs the user's consent, which is set once they message or tap a button.
    async fn is_user_following(&self, recipient_id: &str, token: &str) -> Result<bool>;
    async fn reply_to_comment(&self, comment_id: &str, message: &str, token: &str) -> Result<()>;
}

#[derive(Debug, Deserialize)]
pub struct InstagramBusinessAccount {
    pub id: String,
    pub username: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Page {
    pub id: String,
    pub name: Option<String>,
    pub access_token: Option<String>,
    pub instagram_business_account: Option<InstagramBusinessAccount>,
}

#[derive(Deserialize)]
struct DataList<T> {
    #[serde(default = "Vec::new")]
    data: Vec<T>,
}

#[derive(Clone)]
pub struct GraphClient {
    settings: Settings,
    base_url: String,
    http: reqwest::Client,
}

impl GraphClient {
    pub fn new(settings: Settings) -> Result<Self> {
        let base_url = format!("https://graph.facebook.com/{}", settings.meta_graph_version);
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()?;
        Ok(Self {
            settings,
            base_url,
            http,
        })
    }

    pub fn auth_url(&self, state: &str) -> Result<String> {
        if self.settings.meta_app_id.is_empty() {
            bail!("META_APP_ID is required");
        }
        let scope = AUTH_SCOPES.join(",");
        let url = Url::parse_with_params(
            "https://www.facebook.com/dialog/oauth",
            &[
                ("client_id", self.settings.meta_app_id.as_str()),
                ("redirect_uri", self.settings.meta_redirect_uri.as_str()),
                ("scope", scope.as_str()),
                ("response_type", "code"),
                ("state", state),
            ],
        )?;
        Ok(url.into())
    }

    /// Sends a request and returns the JSON body.
    /// Errors never include the URL, because the access token travels in its query string.
    async fn send(request: RequestBuilder) -> Result<Value> {
        let response = request.send().await.map_err(|err| err.without_url())?;
        let status = response.status();
        let body = response.text().await.map_err(|err| err.without_url())?;
        if !status.is_success() {
            bail!("Graph API {status}: {body}");
        }
        serde_json::from_str(&body).context("Graph API returned invalid JSON")
    }

    pub async fn exchange_code(&self, code: &str) -> Result<String> {
        if self.settings.meta_app_id.is_empty() || self.settings.meta_app_secret.is_empty() {
            bail!("META_APP_ID and META_APP_SECRET are required");
        }
        let request = self
            .http
            .get(format!("{}/oauth/access_token", self.base_url))
            .query(&[
                ("client_id", self.settings.meta_app_id.as_str()),
                ("client_secret", self.settings.meta_app_secret.as_str()),
                ("redirect_uri", self.settings.meta_redirect_uri.as_str()),
                ("code", code),
            ]);
        let body = Self::send(request).await?;
        body["access_token"]
            .as_str()
            .map(str::to_string)
            .context("access_token missing from OAuth response")
    }

    pub async fn get_pages_with_instagram(&self, token: &str) -> Result<Vec<Page>> {
        let request = self
            .http
            .get(format!("{}/me/accounts", self.base_url))
            .query(&[
                ("access_token", token),
                (
                    "fields",
                    "id,name,access_token,instagram_business_account{id,username}",
                ),
            ]);
        let list: DataList<Page> = serde_json::from_value(Self::send(request).await?)?;
        Ok(list.data)
    }

    pub async fn list_media(&self, instagram_user_id: &str, token: &str) -> Result<Vec<MediaItem>> {
        let request = self
            .http
            .get(format!("{}/{instagram_user_id}/media", self.base_url))
            .query(&[
                ("access_token", token),
                (
                    "fields",
                    "id,caption,media_type,media_url,permalink,thumbnail_url,timestamp",
                ),
                ("limit", "50"),
            ]);
        let list: DataList<MediaItem> = serde_json::from_value(Self::send(request).await?)?;
        Ok(list.data)
    }

    async fn post_message(
        &self,
        recipient: Value,
        text: &str,
        token: &str,
        quick_replies: &[QuickReply],
    ) -> Result<()> {
        let mut message = json!({ "text": text });
        if !quick_replies.is_empty() {
            message["quick_replies"] = serde_json::to_value(quick_replies)?;
        }
        let request = self
            .http
            .post(format!("{}/me/messages", self.base_url))
            .query(&[("access_token", token)])
            .json(&json!({ "recipient": recipient, "message": message }));
        Self::send(request).await?;
        Ok(())
    }
}

impl InstagramApi for GraphClient {
    async fn send_private_reply(
        &self,
        comment_id: &str,
        message: &str,
        token: &str,
        quick_replies: &[QuickReply],
    ) -> Result<()> {
        self.post_message(
            json!({ "comment_id": comment_id }),
            message,
            token,
            quick_replies,
        )
        .await
    }

    async fn send_message(
        &self,
        recipient_id: &str,
        message: &str,
        token: &str,
        quick_replies: &[QuickReply],
    ) -> Result<()> {
        self.post_message(json!({ "id": recipient_id }), message, token, quick_replies)
            .await
    }

    async fn is_user_following(&self, recipient_id: &str, token: &str) -> Result<bool> {
        let request = self
            .http
            .get(format!("{}/{recipient_id}", self.base_url))
            .query(&[
                ("access_token", token),
                ("fields", "is_user_follow_business"),
            ]);
        let body = Self::send(request).await?;
        Ok(body["is_user_follow_business"].as_bool().unwrap_or(false))
    }

    async fn reply_to_comment(&self, comment_id: &str, message: &str, token: &str) -> Result<()> {
        let request = self
            .http
            .post(format!("{}/{comment_id}/replies", self.base_url))
            .query(&[("access_token", token), ("message", message)]);
        Self::send(request).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instagram_auth_url_includes_oauth_state() {
        let settings = Settings {
            meta_app_id: "app-id".into(),
            meta_redirect_uri: "https://example.com/auth/instagram/callback".into(),
            ..Settings::default()
        };
        let url = Url::parse(
            &GraphClient::new(settings)
                .unwrap()
                .auth_url("state-1")
                .unwrap(),
        )
        .unwrap();
        let query: Vec<(String, String)> = url.query_pairs().into_owned().collect();
        let get = |key: &str| {
            query
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("client_id"), Some("app-id"));
        assert_eq!(
            get("redirect_uri"),
            Some("https://example.com/auth/instagram/callback")
        );
        assert_eq!(get("state"), Some("state-1"));
    }

    #[test]
    fn auth_url_requires_app_id() {
        assert!(
            GraphClient::new(Settings::default())
                .unwrap()
                .auth_url("s")
                .is_err()
        );
    }
}

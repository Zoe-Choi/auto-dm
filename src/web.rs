//! Admin pages, OAuth, and the Meta webhook. Mirrors `auto_dm/app.py`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Context, anyhow};
use axum::Router;
use axum::body::Bytes;
use axum::extract::{Form, Path, Query, Request, State};
use axum::http::header::{AUTHORIZATION, WWW_AUTHENTICATE};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use minijinja::value::Serde;
use minijinja::{Environment, context, path_loader};
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::Value;
use tower_http::services::ServeDir;

use crate::admin::valid_admin_credentials;
use crate::config::Settings;
use crate::instagram::GraphClient;
use crate::security::{protect_secret, unprotect_secret};
use crate::services::{self, Account, AutomationInput};
use crate::worker::COMMENT_REPLIES;
use crate::{db, webhook};

pub struct AppState {
    settings: Settings,
    templates: Environment<'static>,
    client: GraphClient,
}

type SharedState = Arc<AppState>;

/// Error type for handlers: a status code plus a message shown to the browser.
pub struct AppError {
    status: StatusCode,
    message: String,
}

impl AppError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

/// Lets handlers use `?` on any anyhow-compatible error; those become 500 responses.
impl<E: Into<anyhow::Error>> From<E> for AppError {
    fn from(err: E) -> Self {
        let err = err.into();
        tracing::error!("request failed: {err:#}");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, format!("{err:#}"))
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (self.status, self.message).into_response()
    }
}

type HandlerResult<T> = Result<T, AppError>;

/// Runs blocking SQLite work on a worker thread so the async runtime stays responsive.
/// The closure must own everything it uses (`'static`), because it moves to another thread.
async fn with_db<T, F>(state: &AppState, work: F) -> HandlerResult<T>
where
    F: FnOnce(&Connection) -> anyhow::Result<T> + Send + 'static,
    T: Send + 'static,
{
    let path = state.settings.database_path.clone();
    let result = tokio::task::spawn_blocking(move || work(&db::open(&path)?)).await?;
    Ok(result?)
}

pub fn build_state(settings: Settings) -> anyhow::Result<SharedState> {
    let mut templates = Environment::new();
    templates.set_loader(path_loader("templates"));
    // Load once at startup so a missing or broken template fails fast.
    templates
        .get_template("index.html")
        .context("templates/index.html failed to load; run from the project root")?;
    let client = GraphClient::new(settings.clone())?;
    Ok(Arc::new(AppState {
        settings,
        templates,
        client,
    }))
}

pub fn router(state: SharedState) -> Router {
    let admin = Router::new()
        .route("/", get(index))
        .route("/automations", post(create_automation))
        .route("/automations/{automation_id}", post(update_automation))
        .route(
            "/automations/{automation_id}/delete",
            post(remove_automation),
        )
        .route("/jobs/requeue-failed", post(retry_failed_jobs))
        .route("/auth/instagram", get(instagram_auth))
        .route("/auth/instagram/callback", get(instagram_callback))
        .route("/media/sync", post(sync_media))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_admin));

    Router::new()
        .merge(admin)
        .route(
            "/webhooks/instagram",
            get(verify_webhook).post(instagram_webhook),
        )
        .nest_service("/static", ServeDir::new("static"))
        .with_state(state)
}

async fn require_admin(State(state): State<SharedState>, request: Request, next: Next) -> Response {
    let settings = &state.settings;
    if settings.admin_password_hash.is_empty() {
        return next.run(request).await;
    }
    // Authorization: Basic base64(username:password)
    let credentials = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Basic "))
        .and_then(|encoded| STANDARD.decode(encoded).ok())
        .and_then(|decoded| String::from_utf8(decoded).ok());
    let authorized = credentials
        .as_deref()
        .and_then(|text| text.split_once(':'))
        .is_some_and(|(username, password)| {
            valid_admin_credentials(
                username,
                password,
                &settings.admin_username,
                &settings.admin_password_hash,
            )
        });
    if authorized {
        next.run(request).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            [(WWW_AUTHENTICATE, "Basic")],
            "Admin authentication required",
        )
            .into_response()
    }
}

/// Redirects home with a short status code. The template maps codes to fixed text,
/// so the page never echoes arbitrary query text.
fn back(notice: &str) -> Redirect {
    Redirect::to(&format!("/?notice={notice}"))
}

/// Everything the admin page shows, read in one DB round trip.
struct IndexData {
    automations: Vec<services::Automation>,
    media_items: Vec<services::Media>,
    comments: Vec<services::RecentComment>,
    account: Option<Account>,
    counts: BTreeMap<String, i64>,
}

fn load_index_data(conn: &Connection) -> anyhow::Result<IndexData> {
    Ok(IndexData {
        automations: services::list_automations(conn)?,
        media_items: services::list_instagram_media(conn, 20)?,
        comments: services::list_recent_comments(conn, 20)?,
        account: services::latest_account(conn)?,
        counts: services::dashboard_counts(conn)?,
    })
}

#[derive(Deserialize)]
struct IndexQuery {
    notice: Option<String>,
}

async fn index(
    State(state): State<SharedState>,
    Query(query): Query<IndexQuery>,
) -> HandlerResult<Html<String>> {
    let data = with_db(&state, load_index_data).await?;
    Ok(Html(render_index(&state, &data, query.notice.as_deref())?))
}

impl Settings {
    /// OAuth needs both values; without them the connect button cannot work.
    fn meta_ready(&self) -> bool {
        !self.meta_app_id.is_empty() && !self.meta_app_secret.is_empty()
    }
}

fn render_index(
    state: &AppState,
    data: &IndexData,
    notice: Option<&str>,
) -> anyhow::Result<String> {
    let IndexData {
        automations,
        media_items,
        comments,
        account,
        counts,
    } = data;
    let media_ids: HashSet<&str> = media_items
        .iter()
        .map(|m| m.instagram_media_id.as_str())
        .collect();
    let other_automations: Vec<_> = automations
        .iter()
        .filter(|a| !media_ids.contains(a.instagram_media_id.as_str()))
        .collect();
    let automation_by_media: BTreeMap<&str, _> = automations
        .iter()
        .map(|a| (a.instagram_media_id.as_str(), a))
        .collect();
    let settings = &state.settings;
    let missing_settings: Vec<&str> = [
        ("META_APP_ID", &settings.meta_app_id),
        ("META_APP_SECRET", &settings.meta_app_secret),
        ("META_VERIFY_TOKEN", &settings.meta_verify_token),
    ]
    .into_iter()
    .filter(|(_, value)| value.is_empty())
    .map(|(name, _)| name)
    .collect();

    // Serde(...) converts any serde::Serialize value into a template value.
    let html = state
        .templates
        .get_template("index.html")?
        .render(context! {
            automation_by_media => Serde(&automation_by_media),
            other_automations => Serde(&other_automations),
            comment_replies => Serde(COMMENT_REPLIES),
            media_items => Serde(media_items),
            comments => Serde(comments),
            account => Serde(account),
            failed_count => counts.get("jobs_failed").copied().unwrap_or(0),
            meta_ready => settings.meta_ready(),
            missing_settings => Serde(&missing_settings),
            redirect_uri => settings.meta_redirect_uri.as_str(),
            notice => notice,
        })?;
    Ok(html)
}

/// HTML checkboxes are sent only when checked, so missing fields mean false.
#[derive(Deserialize)]
struct AutomationForm {
    instagram_media_id: String,
    #[serde(default)]
    title: String,
    keyword: String,
    dm_message: String,
    follow_required: Option<String>,
    active: Option<String>,
}

impl AutomationForm {
    fn input(&self) -> AutomationInput<'_> {
        AutomationInput {
            instagram_media_id: &self.instagram_media_id,
            title: &self.title,
            keyword: &self.keyword,
            dm_message: &self.dm_message,
            follow_required: self.follow_required.as_deref() == Some("on"),
            active: self.active.as_deref() == Some("on"),
        }
    }
}

async fn create_automation(
    State(state): State<SharedState>,
    Form(form): Form<AutomationForm>,
) -> HandlerResult<Redirect> {
    with_db(&state, move |conn| {
        services::upsert_automation(conn, &form.input(), None)
    })
    .await?;
    Ok(back("saved"))
}

async fn update_automation(
    State(state): State<SharedState>,
    Path(automation_id): Path<i64>,
    Form(form): Form<AutomationForm>,
) -> HandlerResult<Redirect> {
    with_db(&state, move |conn| {
        services::upsert_automation(conn, &form.input(), Some(automation_id))
    })
    .await?;
    Ok(back("saved"))
}

async fn remove_automation(
    State(state): State<SharedState>,
    Path(automation_id): Path<i64>,
) -> HandlerResult<Redirect> {
    with_db(&state, move |conn| {
        services::delete_automation(conn, automation_id)
    })
    .await?;
    Ok(back("deleted"))
}

async fn retry_failed_jobs(State(state): State<SharedState>) -> HandlerResult<Redirect> {
    with_db(&state, services::requeue_failed_jobs).await?;
    Ok(back("requeued"))
}

async fn instagram_auth(State(state): State<SharedState>) -> HandlerResult<Redirect> {
    if !state.settings.meta_ready() {
        return Ok(back("meta_config"));
    }
    let oauth_state = with_db(&state, services::create_oauth_state).await?;
    Ok(Redirect::to(&state.client.auth_url(&oauth_state)?))
}

/// Meta sends `code` and `state` on success, or `error` fields when the user cancels.
#[derive(Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
}

async fn instagram_callback(
    State(state): State<SharedState>,
    Query(query): Query<CallbackQuery>,
) -> HandlerResult<Redirect> {
    let (Some(code), Some(oauth_state)) = (query.code, query.state) else {
        return Ok(back("oauth_denied"));
    };
    if !with_db(&state, move |conn| {
        services::consume_oauth_state(conn, &oauth_state)
    })
    .await?
    {
        return Ok(back("oauth_state"));
    }
    match connect_account(&state, &code).await {
        Ok(true) => Ok(back("connected")),
        Ok(false) => Ok(back("no_business_account")),
        Err(err) => {
            tracing::error!("instagram connect failed: {err:#}");
            Ok(back("oauth_failed"))
        }
    }
}

/// Returns false when no Facebook Page has an Instagram business account attached.
async fn connect_account(state: &AppState, code: &str) -> anyhow::Result<bool> {
    let access_token = state.client.exchange_code(code).await?;
    let pages = state.client.get_pages_with_instagram(&access_token).await?;
    let Some(page) = pages
        .into_iter()
        .find(|page| page.instagram_business_account.is_some())
    else {
        return Ok(false);
    };
    let instagram = page
        .instagram_business_account
        .ok_or_else(|| anyhow!("checked above"))?;
    let page_token = page.access_token.unwrap_or_else(|| access_token.clone());
    let account = Account {
        instagram_user_id: instagram.id,
        username: instagram
            .username
            .or(page.name)
            .unwrap_or_else(|| "Instagram".into()),
        access_token: protect_secret(&access_token)?,
        page_id: page.id,
        page_access_token: protect_secret(&page_token)?,
    };
    let path = state.settings.database_path.clone();
    tokio::task::spawn_blocking(move || services::upsert_account(&db::open(&path)?, &account))
        .await??;
    Ok(true)
}

async fn sync_media(State(state): State<SharedState>) -> HandlerResult<Redirect> {
    let Some(account) = with_db(&state, services::latest_account).await? else {
        return Ok(back("not_connected"));
    };
    let fetched = async {
        let token = unprotect_secret(account.stored_token())?;
        state
            .client
            .list_media(&account.instagram_user_id, &token)
            .await
    }
    .await;
    let media = match fetched {
        Ok(media) => media,
        Err(err) => {
            tracing::error!("media sync failed: {err:#}");
            return Ok(back("sync_failed"));
        }
    };
    with_db(&state, move |conn| {
        services::upsert_instagram_media(conn, &media)
    })
    .await?;
    Ok(back("synced"))
}

async fn verify_webhook(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, String>>,
) -> HandlerResult<String> {
    let param = |key: &str| params.get(key).map(String::as_str);
    if param("hub.mode") == Some("subscribe")
        && param("hub.verify_token") == Some(state.settings.meta_verify_token.as_str())
    {
        return Ok(param("hub.challenge").unwrap_or("").to_string());
    }
    Err(AppError::new(StatusCode::FORBIDDEN, "Invalid verify token"))
}

async fn instagram_webhook(
    State(state): State<SharedState>,
    headers: HeaderMap,
    body: Bytes,
) -> HandlerResult<axum::Json<Value>> {
    let signature = headers
        .get("x-hub-signature-256")
        .and_then(|value| value.to_str().ok());
    if !webhook::valid_meta_signature(&body, signature, &state.settings.meta_app_secret) {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "Invalid webhook signature",
        ));
    }
    let payload: Value = if body.is_empty() {
        Value::Object(Default::default())
    } else {
        serde_json::from_slice(&body)
            .map_err(|err| AppError::new(StatusCode::BAD_REQUEST, err.to_string()))?
    };
    with_db(&state, move |conn| webhook::handle_payload(conn, &payload)).await?;
    Ok(axum::Json(serde_json::json!({ "status": "queued" })))
}

pub async fn serve(settings: Settings, addr: &str) -> anyhow::Result<()> {
    db::init_db(&settings.database_path)?;
    let app = router(build_state(settings)?);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("listening on http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use tower::ServiceExt;

    use super::*;
    use crate::admin::hash_admin_password;
    use crate::db::test_conn;
    use crate::services::tests::{add_automation, incoming};
    use crate::services::{MediaItem, record_comment, upsert_instagram_media};

    fn state_with(settings: Settings) -> SharedState {
        build_state(settings).expect("state")
    }

    #[test]
    fn index_renders_cards_statuses_and_follow_option() {
        let conn = test_conn();
        add_automation(&conn, true);
        upsert_instagram_media(
            &conn,
            &[MediaItem {
                id: "media-1".into(),
                caption: Some("event reel".into()),
                ..Default::default()
            }],
        )
        .unwrap();
        record_comment(&conn, &incoming("c1", "u1", "info please")).unwrap();
        record_comment(&conn, &incoming("c2", "u2", "price?")).unwrap();

        let data = load_index_data(&conn).unwrap();
        let html = render_index(&state_with(Settings::default()), &data, None).unwrap();

        assert!(html.contains("event reel"));
        assert!(html.contains("사용 중"));
        assert!(html.contains("팔로워에게만"));
        assert!(html.contains(">대기<"));
        assert!(html.contains(">무시<"));
        assert!(html.contains("DM 확인해 주세요!"));
        // No Meta app configured: show setup steps instead of a dead connect button.
        assert!(html.contains("Instagram 연결 준비"));
        assert!(html.contains("<code>META_APP_ID</code>"));
        assert!(!html.contains("href=\"/auth/instagram\""));
    }

    #[test]
    fn index_shows_connect_button_and_known_notices_only() {
        let conn = test_conn();
        let data = load_index_data(&conn).unwrap();
        let settings = Settings {
            meta_app_id: "app".into(),
            meta_app_secret: "secret".into(),
            ..Settings::default()
        };
        let state = state_with(settings);

        let html = render_index(&state, &data, Some("meta_config")).unwrap();
        assert!(html.contains("href=\"/auth/instagram\""));
        assert!(!html.contains("Instagram 연결 준비"));
        assert!(html.contains("Meta 앱 설정이 없어"));

        let html = render_index(&state, &data, Some("<script>x</script>")).unwrap();
        assert!(!html.contains("<script>x"));
        assert!(!html.contains("class=\"notice"));
    }

    #[tokio::test]
    async fn connect_redirects_with_notice_instead_of_error_page() {
        let settings = temp_db_settings("connect");
        let db_path = settings.database_path.clone();
        let app = router(state_with(settings));
        let location =
            |response: Response| response.headers()["location"].to_str().unwrap().to_string();
        let get = |uri: &str| HttpRequest::get(uri).body(Body::empty()).unwrap();

        let response = app.clone().oneshot(get("/auth/instagram")).await.unwrap();
        assert_eq!(location(response), "/?notice=meta_config");
        let states: i64 = db::open(&db_path)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM oauth_states", [], |r| r.get(0))
            .unwrap();
        assert_eq!(states, 0);

        let cancelled = "/auth/instagram/callback?error=access_denied&error_reason=user_denied";
        let response = app.clone().oneshot(get(cancelled)).await.unwrap();
        assert_eq!(location(response), "/?notice=oauth_denied");

        let stale = "/auth/instagram/callback?code=c&state=unknown";
        let response = app.oneshot(get(stale)).await.unwrap();
        assert_eq!(location(response), "/?notice=oauth_state");
    }

    fn temp_db_settings(name: &str) -> Settings {
        let path =
            std::env::temp_dir().join(format!("auto-dm-{name}-{}.sqlite3", std::process::id()));
        let _ = std::fs::remove_file(&path);
        db::init_db(&path).unwrap();
        Settings {
            database_path: path,
            ..Settings::default()
        }
    }

    async fn status_of(app: Router, request: HttpRequest<Body>) -> StatusCode {
        app.oneshot(request).await.unwrap().status()
    }

    #[tokio::test]
    async fn admin_pages_require_basic_auth_when_hash_is_set() {
        let settings = Settings {
            admin_password_hash: hash_admin_password("pw"),
            meta_verify_token: "verify".into(),
            ..temp_db_settings("auth")
        };
        let app = router(state_with(settings));
        let get = |uri: &str| HttpRequest::get(uri).body(Body::empty()).unwrap();

        assert_eq!(
            status_of(app.clone(), get("/")).await,
            StatusCode::UNAUTHORIZED
        );
        let authed = HttpRequest::get("/")
            .header(
                AUTHORIZATION,
                format!("Basic {}", STANDARD.encode("admin:pw")),
            )
            .body(Body::empty())
            .unwrap();
        assert_eq!(status_of(app.clone(), authed).await, StatusCode::OK);
        let wrong = HttpRequest::get("/")
            .header(
                AUTHORIZATION,
                format!("Basic {}", STANDARD.encode("admin:nope")),
            )
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            status_of(app.clone(), wrong).await,
            StatusCode::UNAUTHORIZED
        );

        // The webhook stays public so Meta can reach it.
        let verify =
            "/webhooks/instagram?hub.mode=subscribe&hub.verify_token=verify&hub.challenge=42";
        assert_eq!(status_of(app.clone(), get(verify)).await, StatusCode::OK);
        let bad = "/webhooks/instagram?hub.mode=subscribe&hub.verify_token=x&hub.challenge=42";
        assert_eq!(status_of(app, get(bad)).await, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn webhook_rejects_bad_signature_and_records_comment() {
        let settings = Settings {
            meta_app_secret: "secret".into(),
            ..temp_db_settings("hook")
        };
        let db_path = settings.database_path.clone();
        add_automation(&db::open(&db_path).unwrap(), false);
        let app = router(state_with(settings));
        let body = r#"{"entry":[{"id":"ig-1","changes":[{"value":{
            "id":"c1","media":{"id":"media-1"},"from":{"id":"u1"},"text":"info"}}]}]}"#;
        let post = |signature: String| {
            HttpRequest::post("/webhooks/instagram")
                .header("x-hub-signature-256", signature)
                .body(Body::from(body))
                .unwrap()
        };

        assert_eq!(
            status_of(app.clone(), post("sha256=bad".into())).await,
            StatusCode::FORBIDDEN
        );
        let signature = webhook::meta_signature(body.as_bytes(), "secret");
        assert_eq!(status_of(app, post(signature)).await, StatusCode::OK);

        let count: i64 = db::open(&db_path)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM dm_jobs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }
}

//! Runtime settings read from environment variables and `.env`. Mirrors `auto_dm/config.py`.

use std::env;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Settings {
    pub app_base_url: String,
    pub database_path: PathBuf,
    pub admin_username: String,
    pub admin_password_hash: String,
    pub meta_app_id: String,
    pub meta_app_secret: String,
    pub meta_redirect_uri: String,
    pub meta_verify_token: String,
    pub meta_graph_version: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            app_base_url: "http://localhost:8000".into(),
            database_path: PathBuf::from("auto_dm.sqlite3"),
            admin_username: "admin".into(),
            admin_password_hash: String::new(),
            meta_app_id: String::new(),
            meta_app_secret: String::new(),
            meta_redirect_uri: "http://localhost:8000/auth/instagram/callback".into(),
            meta_verify_token: String::new(),
            meta_graph_version: "v20.0".into(),
        }
    }
}

impl Settings {
    /// Loads `.env` when present, then overrides each default with its environment variable.
    pub fn from_env() -> Self {
        // A missing .env file is normal; real environment variables still apply.
        let _ = dotenvy::dotenv();
        let d = Self::default();
        let var = |name: &str, default: String| env::var(name).unwrap_or(default);
        Self {
            app_base_url: var("APP_BASE_URL", d.app_base_url),
            database_path: PathBuf::from(var(
                "DATABASE_PATH",
                d.database_path.to_string_lossy().into_owned(),
            )),
            admin_username: var("ADMIN_USERNAME", d.admin_username),
            admin_password_hash: var("ADMIN_PASSWORD_HASH", d.admin_password_hash),
            meta_app_id: var("META_APP_ID", d.meta_app_id),
            meta_app_secret: var("META_APP_SECRET", d.meta_app_secret),
            meta_redirect_uri: var("META_REDIRECT_URI", d.meta_redirect_uri),
            meta_verify_token: var("META_VERIFY_TOKEN", d.meta_verify_token),
            meta_graph_version: var("META_GRAPH_VERSION", d.meta_graph_version),
        }
    }
}

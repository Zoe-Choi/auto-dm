from __future__ import annotations

import os
from dataclasses import dataclass
from pathlib import Path

try:
    from dotenv import load_dotenv
except ModuleNotFoundError:
    load_dotenv = None

if load_dotenv:
    load_dotenv()


@dataclass(frozen=True)
class Settings:
    app_base_url: str = os.getenv("APP_BASE_URL", "http://localhost:8000")
    database_path: str = os.getenv("DATABASE_PATH", "auto_dm.sqlite3")
    session_secret: str = os.getenv("SESSION_SECRET", "dev-secret")
    admin_username: str = os.getenv("ADMIN_USERNAME", "admin")
    admin_password_hash: str = os.getenv("ADMIN_PASSWORD_HASH", "")
    meta_app_id: str = os.getenv("META_APP_ID", "")
    meta_app_secret: str = os.getenv("META_APP_SECRET", "")
    meta_redirect_uri: str = os.getenv(
        "META_REDIRECT_URI", "http://localhost:8000/auth/instagram/callback"
    )
    meta_verify_token: str = os.getenv("META_VERIFY_TOKEN", "")
    meta_graph_version: str = os.getenv("META_GRAPH_VERSION", "v20.0")

    @property
    def db_file(self) -> Path:
        return Path(self.database_path)


settings = Settings()
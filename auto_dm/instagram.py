from __future__ import annotations

import secrets
from typing import Any
from urllib.parse import urlencode

import httpx

from .config import Settings, settings

AUTH_SCOPES = [
    "instagram_basic",
    "instagram_manage_comments",
    "instagram_manage_messages",
    "pages_show_list",
    "pages_read_engagement",
]


class MetaConfigError(RuntimeError):
    pass


class InstagramClient:
    def __init__(self, config: Settings = settings) -> None:
        self.config = config
        self.base_url = f"https://graph.facebook.com/{config.meta_graph_version}"

    def auth_url(self, state: str | None = None) -> str:
        if not self.config.meta_app_id:
            raise MetaConfigError("META_APP_ID is required")
        params = {
            "client_id": self.config.meta_app_id,
            "redirect_uri": self.config.meta_redirect_uri,
            "scope": ",".join(AUTH_SCOPES),
            "response_type": "code",
            "state": state or secrets.token_urlsafe(24),
        }
        return "https://www.facebook.com/dialog/oauth?" + urlencode(params)

    async def exchange_code(self, code: str) -> dict[str, Any]:
        if not self.config.meta_app_id or not self.config.meta_app_secret:
            raise MetaConfigError("META_APP_ID and META_APP_SECRET are required")
        async with httpx.AsyncClient(timeout=20) as client:
            response = await client.get(
                f"{self.base_url}/oauth/access_token",
                params={
                    "client_id": self.config.meta_app_id,
                    "client_secret": self.config.meta_app_secret,
                    "redirect_uri": self.config.meta_redirect_uri,
                    "code": code,
                },
            )
            response.raise_for_status()
            return response.json()

    async def get_pages_with_instagram(self, access_token: str) -> list[dict[str, Any]]:
        async with httpx.AsyncClient(timeout=20) as client:
            response = await client.get(
                f"{self.base_url}/me/accounts",
                params={
                    "access_token": access_token,
                    "fields": "id,name,access_token,instagram_business_account{id,username}",
                },
            )
            response.raise_for_status()
            return response.json().get("data", [])

    async def list_media(self, instagram_user_id: str, access_token: str) -> list[dict[str, Any]]:
        async with httpx.AsyncClient(timeout=20) as client:
            response = await client.get(
                f"{self.base_url}/{instagram_user_id}/media",
                params={
                    "access_token": access_token,
                    "fields": "id,caption,media_type,permalink,thumbnail_url,timestamp",
                    "limit": 50,
                },
            )
            response.raise_for_status()
            return response.json().get("data", [])

    async def send_private_reply(
        self, comment_id: str, message: str, access_token: str
    ) -> dict[str, Any]:
        async with httpx.AsyncClient(timeout=20) as client:
            response = await client.post(
                f"{self.base_url}/{comment_id}/private_replies",
                params={"access_token": access_token},
                json={"message": message},
            )
            response.raise_for_status()
            return response.json()
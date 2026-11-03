from __future__ import annotations

import json

from fastapi import Depends, FastAPI, Form, HTTPException, Query, Request
from fastapi.responses import HTMLResponse, RedirectResponse, Response
from fastapi.security import HTTPBasic, HTTPBasicCredentials
from fastapi.staticfiles import StaticFiles
from fastapi.templating import Jinja2Templates

from .admin import valid_admin_credentials
from .config import settings
from .db import connect, init_db
from .instagram import InstagramClient, MetaConfigError
from .security import protect_secret, unprotect_secret
from .services import (
    IncomingComment,
    consume_oauth_state,
    create_oauth_state,
    dashboard_counts,
    delete_automation,
    list_automations,
    list_instagram_media,
    list_recent_comments,
    mark_follow_confirmed,
    record_comment,
    requeue_failed_jobs,
    upsert_automation,
    upsert_instagram_media,
)
from .webhook import valid_meta_signature

app = FastAPI(title="Auto DM")
app.mount("/static", StaticFiles(directory="static"), name="static")
templates = Jinja2Templates(directory="templates")
basic_auth = HTTPBasic(auto_error=False)


def require_admin(credentials: HTTPBasicCredentials | None = Depends(basic_auth)) -> None:
    if not settings.admin_password_hash:
        return
    if not credentials or not valid_admin_credentials(
        credentials.username,
        credentials.password,
        expected_username=settings.admin_username,
        expected_password_hash=settings.admin_password_hash,
    ):
        raise HTTPException(
            status_code=401,
            detail="Admin authentication required",
            headers={"WWW-Authenticate": "Basic"},
        )


admin_dependencies = [Depends(require_admin)]


@app.on_event("startup")
def startup() -> None:
    init_db()


@app.get("/", response_class=HTMLResponse, dependencies=admin_dependencies)
def index(request: Request) -> HTMLResponse:
    with connect() as conn:
        automations = list_automations(conn)
        media_items = list_instagram_media(conn)
        comments = list_recent_comments(conn)
        account = conn.execute(
            "SELECT * FROM instagram_accounts ORDER BY updated_at DESC, id DESC LIMIT 1"
        ).fetchone()
        counts = dashboard_counts(conn)
    return templates.TemplateResponse(
        "index.html",
        {
            "request": request,
            "automations": automations,
            "media_items": media_items,
            "comments": comments,
            "account": account,
            "counts": counts,
            "config_status": {
                "app_base_url": bool(settings.app_base_url),
                "admin_auth": bool(settings.admin_password_hash),
                "meta_app_id": bool(settings.meta_app_id),
                "meta_app_secret": bool(settings.meta_app_secret),
                "meta_verify_token": bool(settings.meta_verify_token),
            },
        },
    )


@app.post("/automations", dependencies=admin_dependencies)
def create_automation(
    instagram_media_id: str = Form(...),
    title: str = Form(...),
    keyword: str = Form(...),
    dm_message: str = Form(...),
    follow_required: str | None = Form(None),
    active: str | None = Form(None),
) -> RedirectResponse:
    with connect() as conn:
        upsert_automation(
            conn,
            instagram_media_id=instagram_media_id,
            title=title,
            keyword=keyword,
            dm_message=dm_message,
            follow_required=follow_required == "on",
            active=active == "on",
        )
    return RedirectResponse("/", status_code=303)


@app.post("/automations/{automation_id}", dependencies=admin_dependencies)
def update_automation(
    automation_id: int,
    instagram_media_id: str = Form(...),
    title: str = Form(...),
    keyword: str = Form(...),
    dm_message: str = Form(...),
    follow_required: str | None = Form(None),
    active: str | None = Form(None),
) -> RedirectResponse:
    with connect() as conn:
        upsert_automation(
            conn,
            automation_id=automation_id,
            instagram_media_id=instagram_media_id,
            title=title,
            keyword=keyword,
            dm_message=dm_message,
            follow_required=follow_required == "on",
            active=active == "on",
        )
    return RedirectResponse("/", status_code=303)


@app.post("/automations/{automation_id}/delete", dependencies=admin_dependencies)
def remove_automation(automation_id: int) -> RedirectResponse:
    with connect() as conn:
        delete_automation(conn, automation_id)
    return RedirectResponse("/", status_code=303)


@app.post("/automations/{automation_id}/toggle", dependencies=admin_dependencies)
def toggle_automation(automation_id: int) -> RedirectResponse:
    with connect() as conn:
        conn.execute(
            """
            UPDATE automations
            SET active = CASE active WHEN 1 THEN 0 ELSE 1 END, updated_at = CURRENT_TIMESTAMP
            WHERE id = ?
            """,
            (automation_id,),
        )
    return RedirectResponse("/", status_code=303)


@app.post("/comments/{comment_id}/confirm-follow", dependencies=admin_dependencies)
def confirm_follow(comment_id: int) -> RedirectResponse:
    with connect() as conn:
        mark_follow_confirmed(conn, comment_id)
    return RedirectResponse("/", status_code=303)



@app.post("/jobs/requeue-failed", dependencies=admin_dependencies)
def retry_failed_jobs() -> RedirectResponse:
    with connect() as conn:
        requeue_failed_jobs(conn)
    return RedirectResponse("/", status_code=303)
@app.get("/auth/instagram", dependencies=admin_dependencies)
def instagram_auth() -> RedirectResponse:
    try:
        with connect() as conn:
            state = create_oauth_state(conn)
        return RedirectResponse(InstagramClient().auth_url(state=state))
    except MetaConfigError as exc:
        raise HTTPException(status_code=400, detail=str(exc)) from exc


@app.get("/auth/instagram/callback", dependencies=admin_dependencies)
async def instagram_callback(code: str, state: str) -> RedirectResponse:
    with connect() as conn:
        if not consume_oauth_state(conn, state):
            raise HTTPException(status_code=400, detail="Invalid OAuth state")

    client = InstagramClient()
    token = await client.exchange_code(code)
    access_token = token["access_token"]
    pages = await client.get_pages_with_instagram(access_token)
    selected_page = next((page for page in pages if page.get("instagram_business_account")), None)
    if not selected_page:
        raise HTTPException(status_code=400, detail="No Instagram business account connected")

    instagram_account = selected_page["instagram_business_account"]
    page_access_token = str(selected_page.get("access_token") or access_token)
    with connect() as conn:
        conn.execute(
            """
            INSERT INTO instagram_accounts (
                instagram_user_id, username, access_token, page_id, page_access_token
            ) VALUES (?, ?, ?, ?, ?)
            ON CONFLICT(instagram_user_id) DO UPDATE SET
                username = excluded.username,
                access_token = excluded.access_token,
                page_id = excluded.page_id,
                page_access_token = excluded.page_access_token,
                updated_at = CURRENT_TIMESTAMP
            """,
            (
                str(instagram_account.get("id") or ""),
                str(instagram_account.get("username") or selected_page.get("name") or "Instagram"),
                protect_secret(access_token),
                str(selected_page.get("id") or ""),
                protect_secret(page_access_token),
            ),
        )
    return RedirectResponse("/", status_code=303)


@app.post("/media/sync", dependencies=admin_dependencies)
async def sync_media() -> RedirectResponse:
    with connect() as conn:
        account = conn.execute(
            "SELECT * FROM instagram_accounts ORDER BY updated_at DESC, id DESC LIMIT 1"
        ).fetchone()
    if not account:
        return RedirectResponse("/", status_code=303)

    token = account["page_access_token"] or account["access_token"]
    media = await InstagramClient().list_media(
        account["instagram_user_id"], unprotect_secret(token)
    )
    with connect() as conn:
        upsert_instagram_media(conn, media)
    return RedirectResponse("/", status_code=303)


@app.get("/webhooks/instagram")
def verify_webhook(
    hub_mode: str | None = Query(default=None, alias="hub.mode"),
    hub_challenge: str | None = Query(default=None, alias="hub.challenge"),
    hub_verify_token: str | None = Query(default=None, alias="hub.verify_token"),
) -> Response:
    if hub_mode == "subscribe" and hub_verify_token == settings.meta_verify_token:
        return Response(content=hub_challenge or "", media_type="text/plain")
    raise HTTPException(status_code=403, detail="Invalid verify token")


@app.post("/webhooks/instagram")
async def instagram_webhook(request: Request) -> dict[str, str]:
    body = await request.body()
    signature = request.headers.get("x-hub-signature-256")
    if not valid_meta_signature(body, signature, settings.meta_app_secret):
        raise HTTPException(status_code=403, detail="Invalid webhook signature")

    payload = json.loads(body or b"{}")
    for entry in payload.get("entry", []):
        for change in entry.get("changes", []):
            value = change.get("value", {})
            comment_id = str(value.get("id") or value.get("comment_id") or "")
            media_id = str(value.get("media", {}).get("id") or value.get("media_id") or "")
            text = str(value.get("text") or value.get("message") or "")
            commenter = value.get("from") or {}
            if not comment_id or not media_id:
                continue
            incoming = IncomingComment(
                comment_id=comment_id,
                media_id=media_id,
                commenter_id=str(commenter.get("id") or value.get("from_id") or ""),
                commenter_username=str(commenter.get("username") or commenter.get("name") or ""),
                text=text,
            )
            with connect() as conn:
                record_comment(conn, incoming)
    return {"status": "queued"}

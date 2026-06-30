from __future__ import annotations

from fastapi import FastAPI, Form, HTTPException, Query, Request
from fastapi.responses import HTMLResponse, RedirectResponse, Response
from fastapi.staticfiles import StaticFiles
from fastapi.templating import Jinja2Templates

from .config import settings
from .db import connect, init_db
from .instagram import InstagramClient, MetaConfigError
from .security import protect_secret
from .services import (
    IncomingComment,
    delete_automation,
    list_automations,
    list_recent_comments,
    mark_follow_confirmed,
    record_comment,
    upsert_automation,
)

app = FastAPI(title="Auto DM")
app.mount("/static", StaticFiles(directory="static"), name="static")
templates = Jinja2Templates(directory="templates")


@app.on_event("startup")
def startup() -> None:
    init_db()


@app.get("/", response_class=HTMLResponse)
def index(request: Request) -> HTMLResponse:
    with connect() as conn:
        automations = list_automations(conn)
        comments = list_recent_comments(conn)
        account = conn.execute(
            "SELECT * FROM instagram_accounts ORDER BY updated_at DESC, id DESC LIMIT 1"
        ).fetchone()
    return templates.TemplateResponse(
        "index.html",
        {
            "request": request,
            "automations": automations,
            "comments": comments,
            "account": account,
        },
    )


@app.post("/automations")
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


@app.post("/automations/{automation_id}")
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


@app.post("/automations/{automation_id}/delete")
def remove_automation(automation_id: int) -> RedirectResponse:
    with connect() as conn:
        delete_automation(conn, automation_id)
    return RedirectResponse("/", status_code=303)


@app.post("/automations/{automation_id}/toggle")
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


@app.post("/comments/{comment_id}/confirm-follow")
def confirm_follow(comment_id: int) -> RedirectResponse:
    with connect() as conn:
        mark_follow_confirmed(conn, comment_id)
    return RedirectResponse("/", status_code=303)


@app.get("/auth/instagram")
def instagram_auth() -> RedirectResponse:
    try:
        return RedirectResponse(InstagramClient().auth_url())
    except MetaConfigError as exc:
        raise HTTPException(status_code=400, detail=str(exc)) from exc


@app.get("/auth/instagram/callback")
async def instagram_callback(code: str) -> RedirectResponse:
    client = InstagramClient()
    token = await client.exchange_code(code)
    access_token = token["access_token"]
    profile = await client.get_me(access_token)
    first = (profile.get("data") or [{}])[0]
    instagram_user_id = str(first.get("id") or "connected")
    username = str(first.get("name") or first.get("username") or "Instagram")
    with connect() as conn:
        conn.execute(
            """
            INSERT INTO instagram_accounts (instagram_user_id, username, access_token)
            VALUES (?, ?, ?)
            ON CONFLICT(instagram_user_id) DO UPDATE SET
                username = excluded.username,
                access_token = excluded.access_token,
                updated_at = CURRENT_TIMESTAMP
            """,
            (instagram_user_id, username, protect_secret(access_token)),
        )
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
async def instagram_webhook(payload: dict) -> dict[str, str]:
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

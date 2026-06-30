from __future__ import annotations

import hmac
from hashlib import sha256


def meta_signature(payload: bytes, app_secret: str) -> str:
    digest = hmac.new(app_secret.encode("utf-8"), payload, sha256).hexdigest()
    return f"sha256={digest}"


def valid_meta_signature(payload: bytes, header_value: str | None, app_secret: str) -> bool:
    if not app_secret:
        return True
    if not header_value or not header_value.startswith("sha256="):
        return False
    return hmac.compare_digest(meta_signature(payload, app_secret), header_value)
from __future__ import annotations

import hashlib
import secrets


def hash_admin_password(password: str) -> str:
    return hashlib.sha256(password.encode("utf-8")).hexdigest()


def valid_admin_credentials(
    username: str,
    password: str,
    *,
    expected_username: str,
    expected_password_hash: str,
) -> bool:
    if not expected_password_hash:
        return True
    username_ok = secrets.compare_digest(username, expected_username)
    password_ok = secrets.compare_digest(hash_admin_password(password), expected_password_hash)
    return username_ok and password_ok
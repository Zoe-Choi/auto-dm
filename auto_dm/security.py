from __future__ import annotations

import base64
import ctypes
import sys
from ctypes import wintypes

CRYPTPROTECT_UI_FORBIDDEN = 0x01


class DATA_BLOB(ctypes.Structure):
    _fields_ = [("cbData", wintypes.DWORD), ("pbData", ctypes.POINTER(ctypes.c_char))]


def _blob_from_bytes(data: bytes) -> DATA_BLOB:
    buffer = ctypes.create_string_buffer(data)
    return DATA_BLOB(len(data), ctypes.cast(buffer, ctypes.POINTER(ctypes.c_char)))


def _bytes_from_blob(blob: DATA_BLOB) -> bytes:
    try:
        return ctypes.string_at(blob.pbData, blob.cbData)
    finally:
        ctypes.windll.kernel32.LocalFree(blob.pbData)


def protect_secret(value: str) -> str:
    raw = value.encode("utf-8")
    if sys.platform != "win32":
        return "plain:" + base64.urlsafe_b64encode(raw).decode("ascii")

    input_blob = _blob_from_bytes(raw)
    output_blob = DATA_BLOB()
    ok = ctypes.windll.crypt32.CryptProtectData(
        ctypes.byref(input_blob),
        None,
        None,
        None,
        None,
        CRYPTPROTECT_UI_FORBIDDEN,
        ctypes.byref(output_blob),
    )
    if not ok:
        raise ctypes.WinError()
    return "dpapi:" + base64.urlsafe_b64encode(_bytes_from_blob(output_blob)).decode("ascii")


def unprotect_secret(value: str) -> str:
    if value.startswith("plain:"):
        return base64.urlsafe_b64decode(value[6:].encode("ascii")).decode("utf-8")
    if not value.startswith("dpapi:"):
        return value
    if sys.platform != "win32":
        raise RuntimeError("DPAPI secret can only be opened on the Windows account that saved it")

    encrypted = base64.urlsafe_b64decode(value[6:].encode("ascii"))
    input_blob = _blob_from_bytes(encrypted)
    output_blob = DATA_BLOB()
    ok = ctypes.windll.crypt32.CryptUnprotectData(
        ctypes.byref(input_blob),
        None,
        None,
        None,
        None,
        CRYPTPROTECT_UI_FORBIDDEN,
        ctypes.byref(output_blob),
    )
    if not ok:
        raise ctypes.WinError()
    return _bytes_from_blob(output_blob).decode("utf-8")

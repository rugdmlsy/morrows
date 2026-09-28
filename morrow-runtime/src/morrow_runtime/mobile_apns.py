from __future__ import annotations

import time
from pathlib import Path
from typing import Any

import httpx
import jwt

from .settings import get_settings

_APNS_SANDBOX = "https://api.sandbox.push.apple.com"
_APNS_PRODUCTION = "https://api.push.apple.com"


class APNsWakeError(RuntimeError):
    pass


def apns_configured() -> bool:
    settings = get_settings()
    return bool(
        settings.remote_mobile_apns_enabled
        and settings.remote_mobile_apns_team_id
        and settings.remote_mobile_apns_key_id
        and settings.remote_mobile_apns_key_path
        and settings.remote_mobile_apns_topic
    )


def _provider_token() -> str:
    settings = get_settings()
    key_path = Path(str(settings.remote_mobile_apns_key_path or "")).expanduser()
    if not key_path.is_file():
        raise APNsWakeError("APNs private key file is missing")
    key = key_path.read_text(encoding="utf-8")
    return jwt.encode(
        {"iss": settings.remote_mobile_apns_team_id, "iat": int(time.time())},
        key,
        algorithm="ES256",
        headers={"kid": settings.remote_mobile_apns_key_id},
    )


async def send_background_wake(
    device_token: str,
    *,
    environment: str,
    reason: str = "job",
) -> dict[str, Any]:
    """Send a coalescible low-priority APNs background wake.

    APNs delivery is best effort. A successful HTTP response means APNs accepted
    the notification, not that iOS launched the app.
    """

    if not apns_configured():
        raise APNsWakeError("APNs background wake is not configured")
    token = device_token.strip().lower()
    if not token or len(token) > 256 or any(ch not in "0123456789abcdef" for ch in token):
        raise APNsWakeError("invalid APNs device token")
    if environment not in {"development", "production"}:
        raise APNsWakeError("invalid APNs environment")

    settings = get_settings()
    endpoint = _APNS_SANDBOX if environment == "development" else _APNS_PRODUCTION
    url = f"{endpoint}/3/device/{token}"
    headers = {
        "authorization": f"bearer {_provider_token()}",
        "apns-topic": str(settings.remote_mobile_apns_topic),
        "apns-push-type": "background",
        "apns-priority": "5",
        "apns-collapse-id": "lsm-worker-wake",
        "apns-expiration": str(int(time.time()) + 60),
    }
    payload = {"aps": {"content-available": 1}, "lsm": {"reason": reason}}
    try:
        async with httpx.AsyncClient(http2=True, timeout=10.0) as client:
            response = await client.post(url, headers=headers, json=payload)
    except Exception as exc:  # noqa: BLE001 - preserve provider/network detail for diagnostics.
        raise APNsWakeError(f"APNs request failed: {type(exc).__name__}: {exc}") from exc

    reason_text = ""
    try:
        body = response.json()
        if isinstance(body, dict):
            reason_text = str(body.get("reason") or "")
    except ValueError:
        body = None
    if response.status_code != 200:
        detail = reason_text or f"HTTP {response.status_code}"
        raise APNsWakeError(f"APNs rejected background wake: {detail}")
    return {
        "accepted": True,
        "environment": environment,
        "apns_id": response.headers.get("apns-id"),
    }

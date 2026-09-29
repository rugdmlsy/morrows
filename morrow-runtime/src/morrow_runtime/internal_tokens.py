"""Private runtime credentials for invite-bound container/transport clients.

Public OAuth lives exclusively in morrows-server. No registration, authorization,
metadata or token HTTP endpoints exist in the runtime.
"""
from __future__ import annotations

import time
from typing import Any

import jwt
from starlette.requests import Request

from .settings import get_settings

RUNTIME_SCOPES = (
    "shell:read",
    "shell:write",
    "shell:execute",
    "browser:use",
    "file:share",
    "remote:use",
)

def public_base_url(request: Request | None = None) -> str:
    settings = get_settings()
    if settings.public_base_url:
        return settings.public_base_url.rstrip("/")
    if request is not None:
        proto = request.headers.get("x-forwarded-proto") or request.url.scheme
        if proto == "ws":
            proto = "http"
        elif proto == "wss":
            proto = "https"
        host = (
            request.headers.get("x-forwarded-host")
            or request.headers.get("host")
            or request.url.netloc
        )
        return f"{proto}://{host}".rstrip("/")
    return "http://127.0.0.1:8765"


def issuer_url(request: Request | None = None) -> str:
    settings = get_settings()
    return (settings.runtime_token_issuer or public_base_url(request)).rstrip("/")


def resource_url(request: Request | None = None) -> str:
    settings = get_settings()
    return (settings.runtime_token_resource or public_base_url(request)).rstrip("/")



def issue_access_token(
    *,
    client_id: str,
    scope: str,
    resource: str,
    subject: str = "local-user",
    issuer: str | None = None,
    expires_in_s: int | None = None,
    additional_claims: dict[str, Any] | None = None,
) -> str:
    """Issue a bearer while keeping identity and audience claims authoritative.

    Specialized first-party clients may add revocation metadata and request a
    shorter lifetime than the internal runtime default.  Callers cannot replace
    the issuer, subject, audience, client, scope, or issuance timestamp.
    """
    settings = get_settings()
    now = int(time.time())
    payload = dict(additional_claims or {})
    payload.update({
        "iss": (issuer or issuer_url()).rstrip("/"),
        "sub": subject,
        "aud": resource,
        "iat": now,
        "client_id": client_id,
        "scope": scope,
    })
    lifetime = settings.runtime_token_access_token_ttl_s if expires_in_s is None else int(expires_in_s)
    if lifetime <= 0:
        raise ValueError("Internal credentials must have a bounded lifetime")
    payload["exp"] = now + lifetime
    return jwt.encode(payload, settings.runtime_token_jwt_secret, algorithm="HS256")



def validate_bearer_token(token: str, request: Request | None = None) -> dict[str, Any]:
    settings = get_settings()
    return jwt.decode(
        token,
        settings.runtime_token_jwt_secret,
        algorithms=["HS256"],
        audience=resource_url(request),
        issuer=issuer_url(request),
        options={"require": ["iat", "aud", "iss", "exp"]},
    )

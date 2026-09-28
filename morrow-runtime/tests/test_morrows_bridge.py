from __future__ import annotations

import time

import httpx
import jwt
from starlette.applications import Starlette
from starlette.testclient import TestClient

from morrow_runtime import morrows_bridge
from morrow_runtime.auth import AuthMiddleware, Principal
from morrow_runtime.settings import get_settings


class _FakeAsyncClient:
    calls: list[dict] = []

    def __init__(self, *args, **kwargs):  # noqa: ANN002, ANN003
        self.args = args
        self.kwargs = kwargs

    async def __aenter__(self):
        return self

    async def __aexit__(self, exc_type, exc, tb):  # noqa: ANN001
        return False

    async def request(self, method, url, headers=None, content=None):  # noqa: ANN001
        self.calls.append(
            {
                "method": method,
                "url": url,
                "headers": dict(headers or {}),
                "content": content,
            }
        )
        return httpx.Response(
            200,
            headers={
                "Content-Type": "application/json",
                "Mcp-Session-Id": "morrows-session-1",
            },
            content=b'{"jsonrpc":"2.0","id":1,"result":{"ok":true}}',
            request=httpx.Request(method, url),
        )


def test_morrows_proxy_strips_public_identity_and_injects_verified_marker(monkeypatch) -> None:
    _FakeAsyncClient.calls = []
    monkeypatch.setattr(morrows_bridge.httpx, "AsyncClient", _FakeAsyncClient)
    monkeypatch.setattr(
        morrows_bridge,
        "current_principal",
        lambda: Principal(
            email=None,
            subject="local-user",
            claims={"client_id": "oauth-client-real"},
        ),
    )
    monkeypatch.setattr(
        morrows_bridge,
        "oauth_client_name",
        lambda client_id: "ChatGPT" if client_id == "oauth-client-real" else None,
    )

    app = Starlette(routes=morrows_bridge.morrows_bridge_routes())
    with TestClient(app) as client:
        response = client.post(
            "/morrows",
            headers={
                "Authorization": "Bearer lsm-oauth-token",
                "X-Agent-Instance-Id": "00000000-0000-0000-0000-000000000000",
                "X-Morrows-LSM-OAuth-Verified": "spoofed",
                "X-Morrows-LSM-OAuth-Client-Id": "spoofed-client",
                "X-Morrows-LSM-OAuth-Client-Name": "spoofed-name",
                "Mcp-Protocol-Version": "2025-06-18",
                "Accept": "application/json, text/event-stream",
            },
            content=b'{"jsonrpc":"2.0","id":1,"method":"tools/list"}',
        )

    assert response.status_code == 200
    assert response.headers["mcp-session-id"] == "morrows-session-1"
    assert len(_FakeAsyncClient.calls) == 1
    call = _FakeAsyncClient.calls[0]
    assert call["url"] == "http://127.0.0.1:8787/mcp"
    assert "authorization" not in {name.lower() for name in call["headers"]}
    assert "x-agent-instance-id" not in {name.lower() for name in call["headers"]}
    assert call["headers"]["X-Morrows-LSM-OAuth-Verified"] == "1"
    assert call["headers"]["X-Morrows-LSM-OAuth-Client-Id"] == "oauth-client-real"
    assert call["headers"]["X-Morrows-LSM-OAuth-Client-Name"] == "ChatGPT"
    assert "spoofed-client" not in repr(call)
    assert "spoofed-name" not in repr(call)
    assert "lsm-oauth-token" not in repr(call)
    assert "00000000-0000-0000-0000-000000000000" not in repr(call)
    assert call["headers"]["mcp-protocol-version"] == "2025-06-18"


def test_morrows_route_is_protected_by_lsm_oauth_before_proxy(tmp_path, monkeypatch) -> None:
    secret = "oauth-test-secret-that-is-more-than-32-bytes"
    monkeypatch.setenv("LOCAL_SHELL_MCP_WORKSPACE_ROOT", str(tmp_path))
    monkeypatch.setenv("LOCAL_SHELL_MCP_STATE_DIR", str(tmp_path / ".state"))
    monkeypatch.setenv("LOCAL_SHELL_MCP_AUTH_MODE", "oauth")
    monkeypatch.setenv("LOCAL_SHELL_MCP_OAUTH_JWT_SECRET", secret)
    monkeypatch.setenv("LOCAL_SHELL_MCP_PUBLIC_BASE_URL", "http://testserver")
    get_settings.cache_clear()
    _FakeAsyncClient.calls = []
    monkeypatch.setattr(morrows_bridge.httpx, "AsyncClient", _FakeAsyncClient)

    app = Starlette(routes=morrows_bridge.morrows_bridge_routes())
    app.add_middleware(AuthMiddleware)
    with TestClient(app, base_url="http://testserver") as client:
        unauthenticated = client.post("/morrows", content=b"{}")
        assert unauthenticated.status_code == 401
        challenge = unauthenticated.headers["www-authenticate"]
        assert 'resource_metadata="http://testserver/.well-known/oauth-protected-resource"' in challenge
        assert _FakeAsyncClient.calls == []

        now = int(time.time())
        token = jwt.encode(
            {
                "iat": now,
                "aud": "http://testserver",
                "iss": "http://testserver",
                "sub": "chatgpt-test",
                "client_id": "oauth-client-authenticated",
                "scope": "shell:read",
            },
            secret,
            algorithm="HS256",
        )
        authenticated = client.post(
            "/morrows",
            headers={"Authorization": f"Bearer {token}"},
            content=b'{"jsonrpc":"2.0","id":1,"method":"tools/list"}',
        )
        assert authenticated.status_code == 200
        assert len(_FakeAsyncClient.calls) == 1

    get_settings.cache_clear()


def test_morrows_control_proxy_accepts_only_first_party_browser_oauth(monkeypatch) -> None:
    _FakeAsyncClient.calls = []
    monkeypatch.setattr(morrows_bridge.httpx, "AsyncClient", _FakeAsyncClient)
    monkeypatch.setattr(
        morrows_bridge,
        "current_principal",
        lambda: Principal(
            email=None,
            subject="local-user",
            claims={"client_id": "browser-client"},
        ),
    )
    monkeypatch.setattr(morrows_bridge, "oauth_client_name", lambda _client_id: "Morrows WebUI")
    monkeypatch.setattr(morrows_bridge, "issuer_url", lambda _request: "http://testserver/morrows/auth")
    monkeypatch.setattr(
        morrows_bridge,
        "oauth_client_redirect_uris",
        lambda _client_id: ("http://testserver/morrows/ui/",),
    )

    app = Starlette(routes=morrows_bridge.morrows_bridge_routes())
    with TestClient(app, base_url="http://testserver") as client:
        response = client.get(
            "/morrows/api/tasks?limit=5",
            headers={
                "Authorization": "Bearer lsm-oauth-token",
                "X-Morrows-LSM-OAuth-Control": "spoofed",
            },
        )

    assert response.status_code == 200
    assert len(_FakeAsyncClient.calls) == 1
    call = _FakeAsyncClient.calls[0]
    assert call["url"] == "http://127.0.0.1:8787/api/tasks?limit=5"
    assert "authorization" not in {name.lower() for name in call["headers"]}
    assert call["headers"]["X-Morrows-LSM-OAuth-Verified"] == "1"
    assert call["headers"]["X-Morrows-LSM-OAuth-Client-Id"] == "browser-client"
    assert call["headers"]["X-Morrows-LSM-OAuth-Control"] == "1"
    assert "spoofed" not in repr(call)


def test_morrows_control_proxy_rejects_non_browser_oauth_client(monkeypatch) -> None:
    _FakeAsyncClient.calls = []
    monkeypatch.setattr(morrows_bridge.httpx, "AsyncClient", _FakeAsyncClient)
    monkeypatch.setattr(
        morrows_bridge,
        "current_principal",
        lambda: Principal(
            email=None,
            subject="agent-user",
            claims={"client_id": "chatgpt-client"},
        ),
    )
    monkeypatch.setattr(morrows_bridge, "oauth_client_name", lambda _client_id: "ChatGPT")
    monkeypatch.setattr(
        morrows_bridge,
        "oauth_client_redirect_uris",
        lambda _client_id: ("https://chat.openai.com/aip/callback",),
    )
    monkeypatch.setattr(morrows_bridge, "issuer_url", lambda _request: "http://testserver/morrows/auth")

    app = Starlette(routes=morrows_bridge.morrows_bridge_routes())
    with TestClient(app, base_url="http://testserver") as client:
        response = client.get("/morrows/api/tasks")

    assert response.status_code == 403
    assert _FakeAsyncClient.calls == []


def test_morrows_bridge_accepts_legacy_lsm_token_on_morrows_path(tmp_path, monkeypatch) -> None:
    new_secret = "morrows-new-oauth-secret-that-is-over-32-bytes"
    legacy_secret = "standalone-lsm-legacy-secret-that-is-over-32-bytes"
    monkeypatch.setenv("LOCAL_SHELL_MCP_WORKSPACE_ROOT", str(tmp_path))
    monkeypatch.setenv("LOCAL_SHELL_MCP_STATE_DIR", str(tmp_path / ".state"))
    monkeypatch.setenv("LOCAL_SHELL_MCP_AUTH_MODE", "oauth")
    monkeypatch.setenv("LOCAL_SHELL_MCP_OAUTH_JWT_SECRET", new_secret)
    monkeypatch.setenv("LOCAL_SHELL_MCP_OAUTH_ISSUER", "http://testserver/morrows/auth")
    monkeypatch.setenv("LOCAL_SHELL_MCP_OAUTH_RESOURCE", "http://testserver/morrows/auth")
    monkeypatch.setenv("LOCAL_SHELL_MCP_OAUTH_LEGACY_JWT_SECRET", legacy_secret)
    monkeypatch.setenv("LOCAL_SHELL_MCP_OAUTH_LEGACY_ISSUER", "http://testserver")
    monkeypatch.setenv("LOCAL_SHELL_MCP_OAUTH_LEGACY_RESOURCE", "http://testserver")
    get_settings.cache_clear()
    _FakeAsyncClient.calls = []
    monkeypatch.setattr(morrows_bridge.httpx, "AsyncClient", _FakeAsyncClient)

    now = int(time.time())
    legacy_token = jwt.encode(
        {
            "iat": now,
            "aud": "http://testserver",
            "iss": "http://testserver",
            "sub": "chatgpt-existing",
            "client_id": "existing-chatgpt-client",
            "scope": "shell:read",
        },
        legacy_secret,
        algorithm="HS256",
    )

    app = Starlette(routes=morrows_bridge.morrows_bridge_routes())
    app.add_middleware(AuthMiddleware)
    with TestClient(app, base_url="http://testserver") as client:
        response = client.post(
            "/morrows",
            headers={"Authorization": f"Bearer {legacy_token}"},
            content=b'{"jsonrpc":"2.0","id":1,"method":"tools/list"}',
        )
        assert response.status_code == 200
        assert len(_FakeAsyncClient.calls) == 1

    get_settings.cache_clear()

def test_morrows_proxy_rejects_when_authenticated_principal_has_no_client_id(monkeypatch) -> None:
    _FakeAsyncClient.calls = []
    monkeypatch.setattr(morrows_bridge.httpx, "AsyncClient", _FakeAsyncClient)
    monkeypatch.setattr(
        morrows_bridge,
        "current_principal",
        lambda: Principal(email=None, subject="local-user", claims={"scope": "shell:read"}),
    )

    app = Starlette(routes=morrows_bridge.morrows_bridge_routes())
    with TestClient(app) as client:
        response = client.post(
            "/morrows",
            content=b'{"jsonrpc":"2.0","id":1,"method":"tools/list"}',
        )

    assert response.status_code == 401
    assert _FakeAsyncClient.calls == []

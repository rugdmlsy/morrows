from __future__ import annotations

import asyncio
import json
import os
import time
from pathlib import Path
from types import SimpleNamespace

import jwt
import pytest
from starlette.exceptions import HTTPException
from starlette.requests import Request

import morrow_runtime.container_client as container_client
import morrow_runtime.human_ui as human_ui
from morrow_runtime.auth import Principal, _is_public_path
from morrow_runtime.container_client import (
    ContainerClientManager,
    container_client_routes,
    reset_container_client_manager,
)
from morrow_runtime.internal_tokens import RUNTIME_SCOPES, issue_access_token
from morrow_runtime.settings import get_settings
from morrow_runtime.tools import build_mcp


def _configure(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("LOCAL_SHELL_MCP_WORKSPACE_ROOT", str(tmp_path / "workspace"))
    monkeypatch.setenv("LOCAL_SHELL_MCP_STATE_DIR", str(tmp_path / "state"))
    monkeypatch.setenv("LOCAL_SHELL_MCP_AUDIT_LOG_PATH", str(tmp_path / "audit.jsonl"))
    monkeypatch.setenv("LOCAL_SHELL_MCP_AUTH_MODE", "internal")
    monkeypatch.setenv("LOCAL_SHELL_MCP_AUTH_BYPASS_LOCALHOST", "false")
    monkeypatch.setenv("LOCAL_SHELL_MCP_PUBLIC_BASE_URL", "http://testserver")
    monkeypatch.setenv(
        "LOCAL_SHELL_MCP_RUNTIME_TOKEN_JWT_SECRET",
        "container-client-test-secret-that-is-at-least-32-bytes",
    )
    monkeypatch.setenv("LOCAL_SHELL_MCP_REMOTE_ENABLED", "false")
    (tmp_path / "workspace").mkdir()
    get_settings.cache_clear()
    reset_container_client_manager()


@pytest.mark.asyncio
async def test_invite_is_single_use_and_registry_never_stores_bearer(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _configure(tmp_path, monkeypatch)
    manager = ContainerClientManager()
    invitation = await manager.create_invite(base_url="http://testserver")

    async def consume():  # noqa: ANN202
        return await manager.register(
            invitation["invite"],
            base_url="http://testserver",
            client_version="1.0.0",
        )

    results = await asyncio.gather(consume(), consume(), return_exceptions=True)
    successful = [item for item in results if isinstance(item, tuple)]
    assert len(successful) == 1
    session, token = successful[0]
    claims = jwt.decode(token, options={"verify_signature": False})
    assert claims["sub"] == f"container-client:{session.session_id}"
    assert claims["scope"] == " ".join(RUNTIME_SCOPES)
    assert claims["token_kind"] == "container-client"
    assert 86_395 <= claims["exp"] - claims["iat"] <= 86_405

    registry_text = manager.registry_path.read_text(encoding="utf-8")
    assert token not in registry_text
    assert json.loads(registry_text)["sessions"][0]["jti"] == session.jti
    if os.name != "nt":
        assert manager.registry_path.stat().st_mode & 0o777 == 0o600


@pytest.mark.asyncio
async def test_revoked_session_is_rejected_after_manager_restart(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _configure(tmp_path, monkeypatch)
    manager = ContainerClientManager()
    invitation = await manager.create_invite(base_url="http://testserver")
    session, token = await manager.register(
        invitation["invite"], base_url="http://testserver", client_version="1.0.0"
    )
    claims = jwt.decode(token, options={"verify_signature": False})
    principal = Principal(email=None, subject=claims["sub"], claims=claims)
    manager.begin_call(principal)
    manager.end_call(session.session_id)
    manager.revoke(session.session_id)

    recovered = ContainerClientManager(manager.state_dir)
    with pytest.raises(HTTPException, match="revoked"):
        recovered.begin_call(principal)


def test_client_bootstrap_routes_are_public_but_call_route_is_protected(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _configure(tmp_path, monkeypatch)
    mcp = build_mcp()
    paths = {route.path for route in container_client_routes(mcp)}
    assert paths == {
        "/client/install.sh",
        "/client/v1/manifest.json",
        "/client/v1/lsm",
        "/client/v1/register",
        "/client/v1/call",
    }
    assert _is_public_path("/client/install.sh")
    assert _is_public_path("/client/v1/register")
    assert not _is_public_path("/client/v1/call")


def test_standard_oauth_clients_share_the_single_user_subject(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _configure(tmp_path, monkeypatch)
    tokens = [
        issue_access_token(
            client_id=client_id,
            scope=" ".join(RUNTIME_SCOPES),
            resource="http://testserver",
            issuer="http://testserver",
        )
        for client_id in ("chatgpt", "morrow-chat-bridge")
    ]

    claims = [jwt.decode(token, options={"verify_signature": False}) for token in tokens]
    assert {item["sub"] for item in claims} == {"local-user"}
    assert {item["client_id"] for item in claims} == {"chatgpt", "morrow-chat-bridge"}


def test_expired_invite_is_rejected(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    _configure(tmp_path, monkeypatch)
    manager = ContainerClientManager()
    invitation = asyncio.run(manager.create_invite(base_url="http://testserver"))
    manager.invites[invitation["invite"]].expires_at = int(time.time()) - 1
    with pytest.raises(ValueError, match="expired"):
        asyncio.run(
            manager.register(
                invitation["invite"],
                base_url="http://testserver",
                client_version="1.0.0",
            )
        )


def _ui_request(method: str, *, path_params: dict[str, str] | None = None) -> Request:
    async def receive():  # noqa: ANN202
        return {"type": "http.request", "body": b"{}", "more_body": False}

    return Request(
        {
            "type": "http",
            "http_version": "1.1",
            "method": method,
            "scheme": "http",
            "path": "/api/ui/container-clients",
            "raw_path": b"/api/ui/container-clients",
            "query_string": b"",
            "headers": [],
            "client": ("127.0.0.1", 1),
            "server": ("testserver", 80),
            "path_params": path_params or {},
        },
        receive,
    )


def _client_request(
    body: bytes,
    *,
    path: str,
    app: object | None = None,
) -> Request:
    async def receive():  # noqa: ANN202
        return {"type": "http.request", "body": body, "more_body": False}

    return Request(
        {
            "type": "http",
            "http_version": "1.1",
            "method": "POST",
            "scheme": "http",
            "path": path,
            "raw_path": path.encode(),
            "query_string": b"",
            "headers": [(b"host", b"testserver")],
            "client": ("127.0.0.1", 1),
            "server": ("testserver", 80),
            "app": app or SimpleNamespace(state=SimpleNamespace()),
        },
        receive,
    )


@pytest.mark.asyncio
async def test_authenticated_ui_can_create_list_and_revoke_container_clients(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _configure(tmp_path, monkeypatch)
    manager = ContainerClientManager()
    monkeypatch.setattr(human_ui, "container_client_manager", lambda: manager)
    monkeypatch.setattr(human_ui, "_require_ui_scopes", lambda *args, **kwargs: None)

    invitation_response = await human_ui.api_container_clients(_ui_request("POST"))
    invitation = json.loads(invitation_response.body)["data"]
    session, _ = await manager.register(
        invitation["invite"],
        base_url="http://testserver",
        client_version="1.0.0",
    )
    listed_response = await human_ui.api_container_clients(_ui_request("GET"))
    listed = json.loads(listed_response.body)["data"]["sessions"]
    assert listed[0]["client_id"] == session.session_id
    assert listed[0]["active"] is True

    revoked_response = await human_ui.api_container_client_revoke(
        _ui_request("POST", path_params={"client_id": session.session_id})
    )
    revoked = json.loads(revoked_response.body)["data"]
    assert revoked["active"] is False


@pytest.mark.asyncio
async def test_registration_route_validates_and_returns_private_curl_config(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _configure(tmp_path, monkeypatch)
    manager = ContainerClientManager()
    monkeypatch.setattr(container_client, "container_client_manager", lambda: manager)

    invalid_json = await container_client.client_register(
        _client_request(b"{", path="/client/v1/register")
    )
    assert invalid_json.status_code == 400
    unknown = await container_client.client_register(
        _client_request(b'{"invite":"x","extra":true}', path="/client/v1/register")
    )
    assert unknown.status_code == 422
    invalid_invite = await container_client.client_register(
        _client_request(b'{"invite":"x"}', path="/client/v1/register")
    )
    assert invalid_invite.status_code == 422
    invalid_version = await container_client.client_register(
        _client_request(
            b'{"invite":"lsmcp_cli_inv_unused","client_version":1}',
            path="/client/v1/register",
        )
    )
    assert invalid_version.status_code == 422

    invitation = await manager.create_invite(base_url="http://testserver")
    response = await container_client.client_register(
        _client_request(
            json.dumps({"invite": invitation["invite"], "client_version": "1.2.3"}).encode(),
            path="/client/v1/register",
        )
    )
    assert response.status_code == 200
    config = response.body.decode()
    assert 'url = "http://testserver/client/v1/call"' in config
    assert 'Authorization: Bearer ' in config
    assert response.headers["cache-control"] == "no-store"


@pytest.mark.asyncio
async def test_client_call_reserved_and_real_tool_paths(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _configure(tmp_path, monkeypatch)
    manager = ContainerClientManager()
    invitation = await manager.create_invite(base_url="http://testserver")
    session, token = await manager.register(
        invitation["invite"], base_url="http://testserver", client_version="1.2.3"
    )
    claims = jwt.decode(token, options={"verify_signature": False})
    principal = Principal(email=None, subject=claims["sub"], claims=claims)
    monkeypatch.setattr(container_client, "container_client_manager", lambda: manager)
    monkeypatch.setattr(container_client, "current_principal", lambda: principal)

    class FakeMcp:
        def __init__(self):
            tool = SimpleNamespace(description="Echo", parameters={"type": "object"}, annotations=None)
            self._tool_manager = SimpleNamespace(_tools={"echo": tool, "broken": tool})

        async def call_tool(self, name, arguments):  # noqa: ANN001, ANN202
            if name == "broken":
                raise RuntimeError("boom")
            return ([{"type": "text", "text": json.dumps({"ok": True})}], arguments)

    app = SimpleNamespace(state=SimpleNamespace(container_client_mcp=FakeMcp()))

    async def call(payload):  # noqa: ANN001, ANN202
        body = payload if isinstance(payload, bytes) else json.dumps(payload).encode()
        response = await container_client.client_call(
            _client_request(body, path="/client/v1/call", app=app)
        )
        return response.status_code, json.loads(response.body)

    status, payload = await call({"tool": "lsm.session_info", "arguments": {}})
    assert status == 200
    assert payload["structured_content"]["client_id"] == session.session_id
    status, payload = await call({"tool": "lsm.tools_list", "arguments": {}})
    assert status == 200
    assert [tool["name"] for tool in payload["structured_content"]["tools"]] == [
        "broken",
        "echo",
    ]
    assert (await call({"tool": "missing", "arguments": {}}))[0] == 404
    assert (await call({"tool": "lsm.tools_list", "arguments": {"bad": True}}))[0] == 422
    assert (await call({"tool": "lsm.session_info", "arguments": {"bad": True}}))[0] == 422
    assert (await call({"tool": "echo", "arguments": {"value": 1}}))[1]["ok"] is True
    broken = (await call({"tool": "broken", "arguments": {}}))[1]
    assert broken["ok"] is False
    assert broken["structured_content"]["message"] == "boom"
    assert (await call(b"{"))[0] == 400
    assert (await call({"tool": "echo"}))[0] == 422
    assert manager._active_calls == {}  # noqa: SLF001


def test_client_helpers_and_session_limits(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    _configure(tmp_path, monkeypatch)
    assert container_client._validate_call_envelope(  # noqa: SLF001
        {"tool": "echo", "arguments": {}}
    ) == ("echo", {})
    for invalid in (None, {"tool": "", "arguments": {}}, {"tool": "echo", "arguments": []}):
        with pytest.raises(HTTPException):
            container_client._validate_call_envelope(invalid)  # noqa: SLF001

    assert container_client._content_dict("text") == {"type": "text", "text": "text"}  # noqa: SLF001
    assert container_client._content_dict({"type": "image"}) == {"type": "image"}  # noqa: SLF001
    assert container_client._tool_is_error(  # noqa: SLF001
        {}, [{"type": "text", "text": '{"ok":false}'}]
    )
    assert not container_client._tool_is_error({}, [{"type": "text", "text": "not-json"}])  # noqa: SLF001

    manager = ContainerClientManager()
    with pytest.raises(HTTPException, match="required"):
        manager.begin_call(None)
    with pytest.raises(KeyError):
        manager.revoke("missing")


@pytest.mark.asyncio
async def test_bootstrap_handlers_and_registration_failures(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _configure(tmp_path, monkeypatch)
    request = _client_request(b"{}", path="/client/install.sh")
    install = await container_client.client_install(request)
    manifest = await container_client.client_manifest(request)
    download = await container_client.client_download(request)
    assert b"install.sh --invite INVITE" in install.body
    manifest_payload = json.loads(manifest.body)
    assert manifest_payload["download_url"] == "http://testserver/client/v1/lsm"
    assert len(manifest_payload["sha256"]) == 64
    assert download.body == container_client.CLIENT_SCRIPT.encode()

    class FailingManager:
        async def register(self, *args, **kwargs):  # noqa: ANN002, ANN003, ANN202
            raise ValueError("invite code has expired")

    monkeypatch.setattr(container_client, "container_client_manager", FailingManager)
    expired = await container_client.client_register(
        _client_request(
            b'{"invite":"lsmcp_cli_inv_expired"}', path="/client/v1/register"
        )
    )
    assert expired.status_code == 410

    async def at_capacity(*args, **kwargs):  # noqa: ANN002, ANN003
        raise RuntimeError("session limit")

    monkeypatch.setattr(
        container_client,
        "container_client_manager",
        lambda: SimpleNamespace(register=at_capacity),
    )
    limited = await container_client.client_register(
        _client_request(b'{"invite":"lsmcp_cli_inv_full"}', path="/client/v1/register")
    )
    assert limited.status_code == 429


@pytest.mark.asyncio
async def test_manager_rejects_invalid_principals_and_concurrent_calls(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _configure(tmp_path, monkeypatch)
    monkeypatch.setenv("LOCAL_SHELL_MCP_CONTAINER_CLIENT_MAX_CONCURRENT_CALLS", "1")
    get_settings.cache_clear()
    manager = ContainerClientManager()
    invitation = await manager.create_invite(base_url="http://testserver")
    session, token = await manager.register(
        invitation["invite"], base_url="http://testserver", client_version="1"
    )
    claims = jwt.decode(token, options={"verify_signature": False})
    wrong_kind = Principal(email=None, subject="x", claims={"token_kind": "internal"})
    with pytest.raises(HTTPException, match="required"):
        manager.begin_call(wrong_kind)
    bad_claims = dict(claims, jti="wrong")
    with pytest.raises(HTTPException, match="not active"):
        manager.begin_call(Principal(email=None, subject=claims["sub"], claims=bad_claims))

    principal = Principal(email=None, subject=claims["sub"], claims=claims)
    manager.begin_call(principal)
    with pytest.raises(HTTPException, match="concurrent"):
        manager.begin_call(principal)
    manager.end_call(session.session_id)


def test_registry_recovers_from_backup_and_ignores_malformed_rows(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _configure(tmp_path, monkeypatch)
    state = tmp_path / "state"
    state.mkdir()
    (state / "container-clients.json").write_text("bad", encoding="utf-8")
    (state / "container-clients.json.bak").write_text(
        json.dumps(
            {
                "sessions": [
                    {"bad": True},
                    {
                        "session_id": "ccs_valid",
                        "jti": "jti",
                        "created_at": 1,
                        "expires_at": int(time.time()) + 100,
                    },
                ]
            }
        ),
        encoding="utf-8",
    )
    manager = ContainerClientManager(state)
    assert list(manager.sessions) == ["ccs_valid"]


def test_manager_singleton_tracks_configured_state_and_call_requires_auth(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _configure(tmp_path, monkeypatch)
    first = container_client.container_client_manager()
    assert container_client.container_client_manager() is first
    monkeypatch.setenv("LOCAL_SHELL_MCP_STATE_DIR", str(tmp_path / "other-state"))
    get_settings.cache_clear()
    assert container_client.container_client_manager() is not first


@pytest.mark.asyncio
async def test_client_call_rejects_missing_container_principal(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _configure(tmp_path, monkeypatch)
    monkeypatch.setattr(container_client, "current_principal", lambda: None)
    response = await container_client.client_call(
        _client_request(b'{"tool":"lsm.session_info","arguments":{}}', path="/client/v1/call")
    )
    assert response.status_code == 403
    assert json.loads(response.body)["error"] == "client_auth_failed"

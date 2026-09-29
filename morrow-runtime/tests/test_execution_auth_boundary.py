"""The runtime must not accidentally regain public OAuth/server ownership."""
from pathlib import Path

import pytest
from fastapi import HTTPException
from starlette.applications import Starlette
from starlette.requests import Request
from starlette.testclient import TestClient

from morrow_runtime.auth import verify_request
from morrow_runtime.main import _with_runtime_routes
from morrow_runtime.settings import get_settings


def configure(tmp_path, monkeypatch):
    monkeypatch.setenv("LOCAL_SHELL_MCP_WORKSPACE_ROOT", str(tmp_path))
    monkeypatch.setenv("LOCAL_SHELL_MCP_STATE_DIR", str(tmp_path / "state"))
    monkeypatch.setenv("LOCAL_SHELL_MCP_AUTH_MODE", "internal")
    monkeypatch.setenv("LOCAL_SHELL_MCP_UI_ENABLED", "false")
    monkeypatch.setenv("LOCAL_SHELL_MCP_REMOTE_ENABLED", "false")
    monkeypatch.setenv("LOCAL_SHELL_MCP_REQUIRE_SESSION_CAPABILITY", "true")
    get_settings.cache_clear()


def test_runtime_has_no_public_oauth_or_morrows_bridge(tmp_path, monkeypatch):
    configure(tmp_path, monkeypatch)
    app = _with_runtime_routes(Starlette())
    with TestClient(app) as client:
        assert client.get("/healthz").status_code == 200
        for path in (
            "/morrows", "/morrows/api/tasks", "/oauth/register", "/oauth/authorize",
            "/oauth/token", "/.well-known/oauth-protected-resource",
            "/.well-known/oauth-protected-resource/morrows",
            "/.well-known/oauth-authorization-server",
        ):
            assert client.get(path).status_code == 404, path
            assert client.post(path).status_code == 404, path
    source = Path(__file__).parents[1] / "src/morrow_runtime"
    assert not (source / "oauth.py").exists()
    assert not (source / "morrows_bridge.py").exists()


def test_public_bearers_cannot_replace_execution_capability(tmp_path, monkeypatch):
    configure(tmp_path, monkeypatch)
    for bearer in ("mrw_agent_example", "public-oauth-token", ""):
        request = Request({
            "type": "http", "method": "POST", "path": "/mcp", "scheme": "https",
            "headers": [(b"authorization", f"Bearer {bearer}".encode())],
            "server": ("runtime.example", 443), "client": ("127.0.0.1", 12345),
            "query_string": b"",
        })
        with pytest.raises(HTTPException, match="Session capability"):
            verify_request(request)


def test_public_edge_and_service_do_not_depend_on_runtime_or_lsm():
    root = Path(__file__).parents[2]
    edge = (root / "deploy/morrows-edge.caddy").read_text()
    assert "127.0.0.1:8790" not in edge
    assert "uri strip_prefix /morrows/auth" not in edge
    assert "@morrows_metadata" in edge
    assert "@retired_mcp" in edge
    unit = (root / "deploy/morrows.service").read_text()
    assert "local-shell-mcp.service" not in unit
    assert "morrow-runtime.service" not in unit
    launcher = (root / "scripts/run-vps.sh").read_text()
    assert "local-shell-mcp/service.env" not in launcher

from __future__ import annotations

from types import SimpleNamespace

import pytest

from morrow_runtime import mobile_apns
from morrow_runtime.settings import get_settings


@pytest.fixture(autouse=True)
def _clear_settings():
    get_settings.cache_clear()
    yield
    get_settings.cache_clear()


def test_apns_is_disabled_without_provider_credentials(tmp_path, monkeypatch):
    monkeypatch.setenv("LOCAL_SHELL_MCP_WORKSPACE_ROOT", str(tmp_path))
    monkeypatch.setenv("LOCAL_SHELL_MCP_REMOTE_MOBILE_APNS_ENABLED", "true")
    assert mobile_apns.apns_configured() is False


@pytest.mark.asyncio
async def test_apns_background_wake_uses_sandbox_http2_headers(tmp_path, monkeypatch):
    key = tmp_path / "AuthKey_test.p8"
    key.write_text("private-key", encoding="utf-8")
    monkeypatch.setenv("LOCAL_SHELL_MCP_WORKSPACE_ROOT", str(tmp_path))
    monkeypatch.setenv("LOCAL_SHELL_MCP_REMOTE_MOBILE_APNS_ENABLED", "true")
    monkeypatch.setenv("LOCAL_SHELL_MCP_REMOTE_MOBILE_APNS_TEAM_ID", "TEAMTEST01")
    monkeypatch.setenv("LOCAL_SHELL_MCP_REMOTE_MOBILE_APNS_KEY_ID", "KEYTEST001")
    monkeypatch.setenv("LOCAL_SHELL_MCP_REMOTE_MOBILE_APNS_KEY_PATH", str(key))
    monkeypatch.setenv("LOCAL_SHELL_MCP_REMOTE_MOBILE_APNS_TOPIC", "com.example.worker")
    get_settings.cache_clear()
    monkeypatch.setattr(mobile_apns, "_provider_token", lambda: "provider-jwt")
    captured = {}

    class FakeClient:
        def __init__(self, *, http2, timeout):
            captured["http2"] = http2
            captured["timeout"] = timeout

        async def __aenter__(self):
            return self

        async def __aexit__(self, exc_type, exc, tb):  # noqa: ANN001
            return False

        async def post(self, url, *, headers, json):  # noqa: A002, ANN001
            captured.update(url=url, headers=headers, json=json)
            return SimpleNamespace(status_code=200, headers={"apns-id": "wake-id"}, json=lambda: {})

    monkeypatch.setattr(mobile_apns.httpx, "AsyncClient", FakeClient)

    result = await mobile_apns.send_background_wake("ab" * 32, environment="development")

    assert result["accepted"] is True
    assert captured["http2"] is True
    assert captured["url"].startswith("https://api.sandbox.push.apple.com/3/device/")
    assert captured["headers"]["authorization"] == "bearer provider-jwt"
    assert captured["headers"]["apns-topic"] == "com.example.worker"
    assert captured["headers"]["apns-push-type"] == "background"
    assert captured["headers"]["apns-priority"] == "5"
    assert captured["json"]["aps"] == {"content-available": 1}


@pytest.mark.asyncio
async def test_apns_rejects_invalid_device_token(tmp_path, monkeypatch):
    monkeypatch.setenv("LOCAL_SHELL_MCP_WORKSPACE_ROOT", str(tmp_path))
    monkeypatch.setenv("LOCAL_SHELL_MCP_REMOTE_MOBILE_APNS_ENABLED", "true")
    monkeypatch.setenv("LOCAL_SHELL_MCP_REMOTE_MOBILE_APNS_TEAM_ID", "TEAMTEST01")
    monkeypatch.setenv("LOCAL_SHELL_MCP_REMOTE_MOBILE_APNS_KEY_ID", "KEYTEST001")
    monkeypatch.setenv("LOCAL_SHELL_MCP_REMOTE_MOBILE_APNS_KEY_PATH", str(tmp_path / "unused.p8"))
    monkeypatch.setenv("LOCAL_SHELL_MCP_REMOTE_MOBILE_APNS_TOPIC", "com.example.worker")
    get_settings.cache_clear()

    with pytest.raises(mobile_apns.APNsWakeError, match="invalid APNs device token"):
        await mobile_apns.send_background_wake("not-a-token", environment="development")

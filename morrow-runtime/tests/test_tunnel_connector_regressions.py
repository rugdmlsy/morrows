from __future__ import annotations

import pytest

import morrow_runtime.live_channel as live_channel_module
import morrow_runtime.session_runtime as session_runtime_module
import morrow_runtime.tools as tools
from morrow_runtime.live_channel import LiveChannelManager
from morrow_runtime.models import CommandResult
from morrow_runtime.session_runtime import SessionRuntimeManager
from morrow_runtime.settings import get_settings


def _configure(tmp_path, monkeypatch, *, auth_mode: str = "none") -> None:
    monkeypatch.setenv("LOCAL_SHELL_MCP_WORKSPACE_ROOT", str(tmp_path))
    monkeypatch.setenv("LOCAL_SHELL_MCP_STATE_DIR", str(tmp_path / ".state"))
    monkeypatch.setenv("LOCAL_SHELL_MCP_AUDIT_LOG_PATH", str(tmp_path / "audit.jsonl"))
    monkeypatch.setenv("LOCAL_SHELL_MCP_AUTH_MODE", auth_mode)
    monkeypatch.setenv("LOCAL_SHELL_MCP_REMOTE_ENABLED", "false")
    get_settings.cache_clear()


@pytest.mark.asyncio
async def test_noauth_mode_advertises_noauth_security_scheme(tmp_path, monkeypatch):
    _configure(tmp_path, monkeypatch, auth_mode="none")

    advertised = await tools.build_mcp().list_tools()

    assert advertised
    for tool in advertised:
        schemes = (tool.meta or {}).get("securitySchemes", [])
        assert {scheme.get("type") for scheme in schemes} == {"noauth"}, tool.name


@pytest.mark.asyncio
async def test_run_python_uses_executable_quoting_for_powershell(tmp_path, monkeypatch):
    _configure(tmp_path, monkeypatch)
    monkeypatch.setenv("LOCAL_SHELL_MCP_SHELL_EXECUTABLE", "powershell.exe")
    monkeypatch.setenv("LOCAL_SHELL_MCP_PYTHON_BIN", r"C:\Program Files\Python\python.exe")
    get_settings.cache_clear()
    commands: list[str] = []

    async def fake_run_shell(command: str, **_kwargs) -> CommandResult:
        commands.append(command)
        return CommandResult(
            ok=True,
            exit_code=0,
            timed_out=False,
            duration_ms=1,
            cwd=".",
            command=command,
            stdout="ok",
            stderr="",
            truncated=False,
        )

    monkeypatch.setattr(tools, "run_shell", fake_run_shell)

    result = await tools._run_python("print(1)")

    assert result["ok"] is True
    assert len(commands) == 1
    assert commands[0].startswith("& 'C:\\Program Files\\Python\\python.exe' '")


@pytest.mark.asyncio
async def test_multiplexed_stdio_keeps_explicit_logical_sessions_isolated(tmp_path, monkeypatch):
    _configure(tmp_path, monkeypatch)
    monkeypatch.setenv("LOCAL_SHELL_MCP_MODE", "http")
    monkeypatch.setenv("LOCAL_SHELL_MCP_UI_ENABLED", "true")
    get_settings.cache_clear()

    sessions = SessionRuntimeManager(tmp_path / ".state")
    live = LiveChannelManager()
    monkeypatch.setattr(session_runtime_module, "_MANAGER", sessions)
    monkeypatch.setattr(live_channel_module, "_MANAGER", live)
    # There is deliberately no transport-to-Session routing hook anymore. Both
    # logical tasks share this MCP server instance and remain isolated solely by
    # their explicit session_id values.
    assert not hasattr(tools, "mcp_session_key")

    mcp = tools.build_mcp()
    raw = lambda name: mcp._tool_manager._tools[name].fn  # noqa: E731, SLF001

    started_a = await raw("session_manage")(action="start", objective="A")
    started_b = await raw("session_manage")(action="start", objective="B")
    session_a = started_a["data"]["session_id"]
    session_b = started_b["data"]["session_id"]
    assert session_a != session_b

    workspace_a = await raw("workspace_open")(session_id=session_a)
    workspace_b = await raw("workspace_open")(session_id=session_b)
    live_a = workspace_a.structuredContent["live_id"]
    live_b = workspace_b.structuredContent["live_id"]
    assert live_a != live_b

    result_a = await raw("run_shell")(command="printf A", logical_session_id=session_a)
    result_b = await raw("run_shell")(command="printf B", logical_session_id=session_b)
    assert result_a["ok"] is True
    assert result_b["ok"] is True

    channel_a = live.by_id(live_a)
    channel_b = live.by_id(live_b)
    assert channel_a is not None and channel_b is not None
    commands_a = [
        event.get("data", {}).get("command")
        for event in channel_a.events
        if event["type"] == "tool.started"
    ]
    commands_b = [
        event.get("data", {}).get("command")
        for event in channel_b.events
        if event["type"] == "tool.started"
    ]
    assert "printf A" in commands_a
    assert "printf B" not in commands_a
    assert "printf B" in commands_b
    assert "printf A" not in commands_b

    state_a = sessions.get(session_a, subject="local-mcp-client")
    state_b = sessions.get(session_b, subject="local-mcp-client")
    assert any(
        event.get("data", {}).get("command") == "printf A" for event in state_a["recent_activity"]
    )
    assert not any(
        event.get("data", {}).get("command") == "printf B" for event in state_a["recent_activity"]
    )
    assert any(
        event.get("data", {}).get("command") == "printf B" for event in state_b["recent_activity"]
    )

    # A new agent turn that explicitly resumes the same Logical Session must keep
    # both the durable Activity history and the existing Session workspace.
    await raw("session_manage")(action="resume", session_id=session_a)
    reopened_a = await raw("workspace_open")(session_id=session_a)
    assert reopened_a.structuredContent["live_id"] == live_a
    resumed_a = sessions.get(session_a, subject="local-mcp-client")
    assert any(
        event.get("data", {}).get("command") == "printf A" for event in resumed_a["recent_activity"]
    )
    assert resumed_a["recent_activity"][-1]["type"] == "session.resumed"


def test_resume_uses_only_explicit_session_id(tmp_path):
    manager = SessionRuntimeManager(tmp_path / ".state")
    started = manager.manage("user", action="start", objective="Task")
    session_id = started["session_id"]

    resumed = manager.manage("user", action="resume", session_id=session_id)

    assert resumed["session_id"] == session_id
    assert "active_run" not in resumed
    assert resumed["recent_activity"][-1]["type"] == "session.resumed"
    with pytest.raises(ValueError, match="session_id is required"):
        manager.manage("user", action="resume")
    with pytest.raises(ValueError, match="action must be one of"):
        manager.manage("user", action="list")

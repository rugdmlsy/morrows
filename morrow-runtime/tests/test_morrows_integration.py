from __future__ import annotations

import json
from concurrent.futures import ThreadPoolExecutor
from threading import Barrier, Event

import pytest
from starlette.testclient import TestClient

from morrow_runtime.audit import audit, query_audit
from morrow_runtime.auth import _CURRENT_PRINCIPAL, Principal
from morrow_runtime.capabilities import (
    issue_capability,
    resolve_capability,
    revoke_capability,
    revoke_session_capabilities,
)
from morrow_runtime.execution_scope import execution_session
from morrow_runtime.jobs import list_jobs, start_managed_job, tail_job
from morrow_runtime.main import _build_mcp_http_app
from morrow_runtime.session_runtime import SessionRuntimeManager, get_session_runtime_manager
from morrow_runtime.settings import get_settings
from morrow_runtime.shell_ops import read_shell, shell_owner, start_shell
from morrow_runtime.state_store import get_state_store
from morrow_runtime.tools import build_mcp


def _settings(tmp_path, monkeypatch):
    monkeypatch.setenv("LOCAL_SHELL_MCP_WORKSPACE_ROOT", str(tmp_path))
    monkeypatch.setenv("LOCAL_SHELL_MCP_STATE_DIR", str(tmp_path / ".state"))
    monkeypatch.setenv("LOCAL_SHELL_MCP_AUDIT_LOG_PATH", str(tmp_path / "audit.jsonl"))
    monkeypatch.setenv("LOCAL_SHELL_MCP_AUTH_MODE", "none")
    monkeypatch.setenv("LOCAL_SHELL_MCP_PUBLIC_BASE_URL", "http://testserver")
    monkeypatch.setenv("LOCAL_SHELL_MCP_REMOTE_ENABLED", "false")
    get_settings.cache_clear()


def test_keyed_session_is_concurrent_durable_and_replayable_after_terminal_delete(tmp_path):
    state = tmp_path / ".state"
    first = SessionRuntimeManager(state)
    second = SessionRuntimeManager(state)
    with ThreadPoolExecutor(max_workers=2) as pool:
        one = pool.submit(first.manage, "shared", action="start", idempotency_key="external:42")
        two = pool.submit(second.manage, "shared", action="start", idempotency_key="external:42")
        ids = {one.result()["session_id"], two.result()["session_id"]}
    assert len(ids) == 1
    session_id = ids.pop()
    first.manage("shared", action="finish", session_id=session_id)
    assert second.manage("shared", action="start", idempotency_key="external:42")["session_id"] == session_id
    assert second.manage("other", action="start", idempotency_key="external:42")["session_id"] != session_id
    first.manage("shared", action="delete", session_id=session_id)
    assert second.manage("shared", action="start", idempotency_key="external:42")["session_id"] != session_id


def test_concurrent_capability_rotation_leaves_only_one_usable_token(tmp_path, monkeypatch):
    _settings(tmp_path, monkeypatch)
    session_id = get_session_runtime_manager().manage("shared", action="start")["session_id"]
    start = Barrier(3)

    def issue():
        start.wait()
        return issue_capability(session_id, "shared")

    with ThreadPoolExecutor(max_workers=2) as pool:
        first = pool.submit(issue)
        second = pool.submit(issue)
        start.wait()
        tokens = [first.result()["capability"], second.result()["capability"]]
    assert sum(resolve_capability(token) is not None for token in tokens) == 1


def test_cleanup_fence_invalidates_an_issue_that_was_already_in_progress(tmp_path, monkeypatch):
    _settings(tmp_path, monkeypatch)
    session_id = get_session_runtime_manager().manage("shared", action="start")["session_id"]
    checked = Event()
    continue_issue = Event()

    def verify_active():
        assert get_session_runtime_manager().get(session_id, subject="shared")["status"] == "active"
        checked.set()
        assert continue_issue.wait(5)

    with ThreadPoolExecutor(max_workers=2) as pool:
        pending = pool.submit(issue_capability, session_id, "shared", verify_active)
        assert checked.wait(5)
        cleanup = pool.submit(revoke_session_capabilities, session_id, block=True)
        continue_issue.set()
        issued = pending.result()
        cleanup.result()
    assert resolve_capability(issued["capability"]) is None
    with pytest.raises(ValueError, match="blocked"):
        issue_capability(session_id, "shared")


def test_capability_resolve_rechecks_under_the_revoke_lock(tmp_path, monkeypatch):
    _settings(tmp_path, monkeypatch)
    session_id = get_session_runtime_manager().manage("shared", action="start")["session_id"]
    issued = issue_capability(session_id, "shared")
    capability_key = f"session-capabilities/{issued['capability_id']}.json"
    store = get_state_store()
    original_read = store.read_bytes
    first_read = Event()
    continue_resolve = Event()

    def pause_after_old_record(key):
        value = original_read(key)
        if key == capability_key and not first_read.is_set():
            first_read.set()
            assert continue_resolve.wait(5)
        return value

    monkeypatch.setattr(store, "read_bytes", pause_after_old_record)
    with ThreadPoolExecutor(max_workers=2) as pool:
        pending = pool.submit(resolve_capability, issued["capability"])
        try:
            assert first_read.wait(5)
            revoked = pool.submit(revoke_capability, issued["capability_id"])
            assert revoked.result(timeout=5) == session_id
        finally:
            continue_resolve.set()
        assert pending.result(timeout=5) is None


def test_resolved_local_and_transfer_nodes_are_only_a_session_index(tmp_path):
    manager = SessionRuntimeManager(tmp_path / ".state")
    session_id = manager.manage("shared", action="start")["session_id"]
    manager.begin_tool_call(session_id, "call-1", subject="shared", execution_machines=["local"])
    manager.begin_tool_call(session_id, "call-2", subject="shared",
                            execution_machines=["local", "node-01", "node-02"])
    state = SessionRuntimeManager(tmp_path / ".state").get(session_id, subject="shared")
    assert state["machines_touched"] == ["local", "node-01", "node-02"]
    assert state["in_flight_calls"] == 2


def test_audit_query_filters_logical_session_not_shell_session(tmp_path, monkeypatch):
    _settings(tmp_path, monkeypatch)
    audit("mcp_tool_call_start", call_id="morrows-a", tool="run_shell",
          logical_session="s_one", session="shell_one")
    audit("mcp_tool_call_end", call_id="morrows-a", tool="run_shell",
          logical_session="s_one", session="shell_one", ok=True)
    audit("mcp_tool_call_start", call_id="morrows-b", tool="run_shell",
          logical_session="s_two", session="shell_two")
    found = query_audit(logical_session_id="s_one")
    assert len(found["entries"]) == 1
    assert found["entries"][0]["logical_session_id"] == "s_one"
    assert found["entries"][0]["session"] == "shell_one"


@pytest.mark.asyncio
async def test_independent_shell_owner_persists_and_rejects_other_session(tmp_path, monkeypatch):
    _settings(tmp_path, monkeypatch)

    async def fake_start(_cwd, _name, _command):
        return {"session_id": "shell-independent", "backend": "fake"}

    monkeypatch.setattr("morrow_runtime.shell_ops._start_shell_unlocked", fake_start)
    with execution_session("s_one"):
        shell = await start_shell()
    assert shell["logical_session_id"] == "s_one"
    assert shell_owner("shell-independent")["logical_session_id"] == "s_one"
    with execution_session("s_two"), pytest.raises(PermissionError, match="different Logical Session"):
        await read_shell("shell-independent")


@pytest.mark.asyncio
async def test_managed_job_owner_is_persisted_without_manual_binding(tmp_path, monkeypatch):
    _settings(tmp_path, monkeypatch)
    monkeypatch.setattr("morrow_runtime.jobs._launch_managed_job", lambda *_args: None)
    with execution_session("s_job"):
        job = await start_managed_job("transfer", {"source_path":"a", "destination_path":"b"})
        visible = await list_jobs()
    assert job["logical_session_id"] == "s_job"
    assert job["machine"] == "local"
    assert [row["job_id"] for row in visible["jobs"]] == [job["job_id"]]
    with execution_session("s_other"):
        assert (await list_jobs())["jobs"] == []
        with pytest.raises(PermissionError, match="different Logical Session"):
            await tail_job(job["job_id"])


@pytest.mark.asyncio
async def test_remote_resource_query_reports_partial_results(tmp_path, monkeypatch):
    _settings(tmp_path, monkeypatch)
    from morrow_runtime import control_plane

    session_id = get_session_runtime_manager().manage("shared", action="start")["session_id"]
    get_session_runtime_manager().begin_tool_call(
        session_id, "remote-call", subject="shared", execution_machines=["local", "node-offline"]
    )

    class Offline:
        async def call(self, *_args, **_kwargs):
            raise RuntimeError("offline")

    monkeypatch.setattr(control_plane, "remote_manager", lambda: Offline())
    result = await control_plane._enumerate(session_id, "jobs")
    assert result["complete"] is False
    assert result["unreachable_machines"] == ["node-offline"]


@pytest.mark.asyncio
async def test_control_resource_mutations_carry_actor_and_session_on_local_and_remote(tmp_path, monkeypatch):
    _settings(tmp_path, monkeypatch)
    from morrow_runtime import control_plane, remote

    async def local_stop(job_id):
        audit("job_stop", job_id=job_id, session="shell-local")
        return {"job_id": job_id, "stopped": True}

    async def remote_stop(_tool, args):
        audit("job_stop", job_id=args["job_id"], session="shell-remote")
        return {"job_id": args["job_id"], "stopped": True}

    monkeypatch.setattr(control_plane, "stop_job", local_stop)
    monkeypatch.setattr(remote, "_execute_worker_tool_inner", remote_stop)
    await control_plane._node_call("local", "job_stop", {"job_id": "j_local"}, "s_control")
    await remote.execute_worker_tool("job_stop", {
        "job_id": "j_remote", "_logical_session_id": "s_control",
        "_execution_machine": "node-01", "_control_actor": True,
    })
    records = [json.loads(line) for line in get_settings().audit_log_path.read_text().splitlines()]
    stops = [record for record in records if record.get("event") == "job_stop"]
    assert {record["job_id"] for record in stops} == {"j_local", "j_remote"}
    assert all(record["actor"] == "control" and record["ingress"] == "control"
               and record["logical_session"] == "s_control" for record in stops)


@pytest.mark.asyncio
async def test_explicitly_offline_node_is_rejected_before_dispatch_and_not_indexed(tmp_path, monkeypatch):
    _settings(tmp_path, monkeypatch)
    monkeypatch.setenv("LOCAL_SHELL_MCP_REMOTE_ENABLED", "true")
    get_settings.cache_clear()
    from morrow_runtime import tools as tool_module

    class OfflineRemote:
        calls = 0

        def list_machines(self):
            return {"machines": [{"name":"node-offline", "status":"offline",
                                  "wake":{"provider_configured":False}}]}

        async def call(self, *_args, **_kwargs):
            self.calls += 1
            raise AssertionError("an offline node must not receive a call")

    remote = OfflineRemote()
    monkeypatch.setattr(tool_module, "remote_manager", lambda: remote)
    session_id = get_session_runtime_manager().manage("shared", action="start")["session_id"]
    token = _CURRENT_PRINCIPAL.set(Principal(
        email=None, subject="shared", claims={"auth":"none","bound_session":session_id}))
    try:
        tool = build_mcp()._tool_manager._tools["run_shell"]
        result = await tool.fn(command="true", machine="node-offline",
                               logical_session_id=session_id)
    finally:
        _CURRENT_PRINCIPAL.reset(token)
    assert "remote machine is offline" in str(result)
    assert remote.calls == 0
    assert get_session_runtime_manager().get(session_id, subject="shared")["machines_touched"] == []


@pytest.mark.asyncio
async def test_run_bound_agent_can_get_report_but_cannot_end_session(tmp_path, monkeypatch):
    _settings(tmp_path, monkeypatch)
    session_id = get_session_runtime_manager().manage("shared", action="start")["session_id"]
    principal = Principal(email=None, subject="shared", claims={"auth":"none","bound_session":session_id})
    token = _CURRENT_PRINCIPAL.set(principal)
    try:
        tool = build_mcp()._tool_manager._tools["session_manage"]
        with pytest.raises(PermissionError, match="lifecycle"):
            await tool.fn(action="finish", session_id=session_id)
        with pytest.raises(PermissionError, match="outside"):
            await tool.fn(action="get", session_id="s_old")
        result = await tool.fn(action="get", session_id=session_id)
        assert result["ok"] is True
    finally:
        _CURRENT_PRINCIPAL.reset(token)


def test_control_credential_and_agent_capability_are_separate_http_paths(tmp_path, monkeypatch):
    _settings(tmp_path, monkeypatch)
    monkeypatch.setenv("LOCAL_SHELL_MCP_CONTROL_API_KEY", "trusted-control-key")
    monkeypatch.setenv("LOCAL_SHELL_MCP_REQUIRE_SESSION_CAPABILITY", "true")
    get_settings.cache_clear()
    with TestClient(_build_mcp_http_app(build_mcp()), base_url="http://testserver") as client:
        assert client.post("/api/control/sessions", json={"subject":"shared"}).status_code == 401
        headers = {"X-LSM-Control-Key":"trusted-control-key"}
        created = client.post("/api/control/sessions", headers=headers,
            json={"subject":"shared","idempotency_key":"morrows:run:test"})
        assert created.status_code == 200
        session_id = created.json()["session"]["session_id"]
        issued = client.post(f"/api/control/sessions/{session_id}/capabilities", headers=headers,
            json={"subject":"shared"})
        assert issued.status_code == 200
        capability = issued.json()["capability"]
        assert resolve_capability(capability)["session_id"] == session_id
        initialize = {"jsonrpc":"2.0","id":1,"method":"initialize",
            "params":{"protocolVersion":"2025-06-18","capabilities":{},
                      "clientInfo":{"name":"test","version":"1"}}}
        assert client.post("/mcp", json=initialize,
            headers={"accept":"application/json, text/event-stream",
                     "content-type":"application/json"}).status_code == 401
        agent_headers = {"accept":"application/json, text/event-stream",
                         "content-type":"application/json",
                         "X-LSM-Session-Capability":capability}
        initialized = client.post("/mcp", json=initialize, headers=agent_headers)
        assert initialized.status_code == 200
        agent_headers["mcp-session-id"] = initialized.headers["mcp-session-id"]
        agent_headers["mcp-protocol-version"] = "2025-06-18"
        allowed = client.post("/mcp", json={"jsonrpc":"2.0","id":2,"method":"tools/call",
            "params":{"name":"session_manage","arguments":{"action":"get","session_id":session_id}}},
            headers=agent_headers)
        assert allowed.status_code == 200
        assert session_id in allowed.text
        denied = client.post("/mcp", json={"jsonrpc":"2.0","id":3,"method":"tools/call",
            "params":{"name":"session_manage","arguments":{"action":"finish","session_id":session_id}}},
            headers=agent_headers)
        assert denied.status_code == 200
        assert "Run-bound Agent cannot change Session lifecycle" in denied.text
        old_session = client.post("/mcp", json={"jsonrpc":"2.0","id":4,"method":"tools/call",
            "params":{"name":"session_manage","arguments":{"action":"get","session_id":"s_old"}}},
            headers=agent_headers)
        assert old_session.status_code == 200
        assert "outside the bound Logical Session" in old_session.text
        rotated = client.post(f"/api/control/sessions/{session_id}/capabilities", headers=headers,
            json={"subject":"shared"})
        assert rotated.status_code == 200
        assert resolve_capability(capability) is None
        assert resolve_capability(rotated.json()["capability"]) is not None
        revoke_capability(issued.json()["capability_id"])
        assert resolve_capability(capability) is None


def test_control_cleanup_wait_is_bounded_and_never_reports_false_cancel(tmp_path, monkeypatch):
    _settings(tmp_path, monkeypatch)
    monkeypatch.setenv("LOCAL_SHELL_MCP_CONTROL_API_KEY", "trusted-control-key")
    get_settings.cache_clear()
    manager = get_session_runtime_manager()
    session_id = manager.manage("shared", action="start")["session_id"]
    lease = manager.begin_tool_call(session_id, "ordinary-call", subject="shared")
    capability = issue_capability(session_id, "shared")
    headers = {"X-LSM-Control-Key":"trusted-control-key"}
    with TestClient(_build_mcp_http_app(build_mcp()), base_url="http://testserver") as client:
        wrong_subject = client.post(f"/api/control/sessions/{session_id}/cleanup", headers=headers,
            json={"subject":"other","wait_seconds":0})
        assert wrong_subject.status_code == 400
        assert resolve_capability(capability["capability"]) is not None
        pending = client.post(f"/api/control/sessions/{session_id}/cleanup", headers=headers,
            json={"subject":"shared","wait_seconds":0})
        assert pending.status_code == 200
        assert pending.json()["complete"] is False
        assert pending.json()["session"]["status"] == "active"
        assert resolve_capability(capability["capability"]) is None
        denied = client.post(f"/api/control/sessions/{session_id}/capabilities", headers=headers,
            json={"subject":"shared"})
        assert denied.status_code == 400
        assert "blocked" in denied.json()["error"]
        manager.finish_tool_call(lease, "tool.completed")
        cleaned = client.post(f"/api/control/sessions/{session_id}/cleanup", headers=headers,
            json={"subject":"shared","wait_seconds":0})
        assert cleaned.status_code == 200
        assert cleaned.json()["complete"] is True
        assert cleaned.json()["session"]["status"] == "cancelled"


def test_control_scoped_tool_call_uses_existing_session_without_capability_rotation(tmp_path, monkeypatch):
    _settings(tmp_path, monkeypatch)
    monkeypatch.setenv("LOCAL_SHELL_MCP_AUTH_MODE", "internal")
    monkeypatch.setenv("LOCAL_SHELL_MCP_REQUIRE_SESSION_CAPABILITY", "true")
    monkeypatch.setenv("LOCAL_SHELL_MCP_CONTROL_API_KEY", "trusted-control-key")
    get_settings.cache_clear()
    with TestClient(_build_mcp_http_app(build_mcp()), base_url="http://testserver") as client:
        headers = {"X-LSM-Control-Key": "trusted-control-key"}
        created = client.post(
            "/api/control/sessions",
            headers=headers,
            json={"subject": "shared", "idempotency_key": "morrows:run:scoped-call"},
        )
        assert created.status_code == 200
        session_id = created.json()["session"]["session_id"]

        write = client.post(
            f"/api/control/sessions/{session_id}/tools/call",
            headers=headers,
            json={
                "subject": "shared",
                "tool": "file_write",
                "arguments": {"path": "scoped.txt", "content": "hello runtime"},
            },
        )
        assert write.status_code == 200
        assert write.json()["runtime_scope_id"] == session_id

        read = client.post(
            f"/api/control/sessions/{session_id}/tools/call",
            headers=headers,
            json={
                "subject": "shared",
                "tool": "file_read",
                "arguments": {"path": "scoped.txt"},
            },
        )
        assert read.status_code == 200
        assert "hello runtime" in str(read.json()["result"])

        denied = client.post(
            f"/api/control/sessions/{session_id}/tools/call",
            headers=headers,
            json={"subject": "shared", "tool": "session_manage", "arguments": {"action": "finish"}},
        )
        assert denied.status_code == 400
        assert "not allowed" in denied.text

        escape = client.post(
            f"/api/control/sessions/{session_id}/tools/call",
            headers=headers,
            json={
                "subject": "shared",
                "tool": "file_read",
                "arguments": {"path": "scoped.txt", "logical_session_id": "other"},
            },
        )
        assert escape.status_code == 400
        assert "cannot escape" in escape.text

        assert get_session_runtime_manager().get(session_id)["status"] == "active"

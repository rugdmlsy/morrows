import json
import time

import pytest

import morrow_runtime.jobs as jobs_module
import morrow_runtime.notification_runtime as notification_runtime
from morrow_runtime import remote
from morrow_runtime.settings import get_settings


@pytest.mark.asyncio
async def test_mobile_events_persist_poll_ack_and_deduplicate(tmp_path, monkeypatch):
    monkeypatch.setenv("LOCAL_SHELL_MCP_WORKSPACE_ROOT", str(tmp_path))
    monkeypatch.setenv("LOCAL_SHELL_MCP_STATE_DIR", str(tmp_path / ".state"))
    get_settings.cache_clear()

    manager = remote.RemoteManager()
    manager._registry_loaded = True
    worker = remote.RemoteWorker(
        "iphone", "token-ios", capabilities=["mobile", "mobile.controller_events"]
    )
    manager.workers[worker.name] = worker
    manager.tokens[worker.token] = worker.name
    monkeypatch.setattr(manager, "_wake_is_configured", lambda _worker: False)

    queued = await manager.queue_mobile_event(
        event_id="event-one",
        event_type="notification",
        title="Test event",
        body="Delivered on the poll channel",
        machine="iphone",
    )
    assert queued["queued_machines"] == ["iphone"]

    polled = await manager.poll(worker.token, {"supports_self_update": False})
    assert polled["job"] is None
    assert [event["id"] for event in polled["events"]] == ["event-one"]

    acked = await manager.acknowledge_mobile_events(worker.token, ["event-one"])
    assert acked == {"acked": ["event-one"], "count": 1}
    assert worker.pending_events == []
    assert "event-one" in worker.recent_event_ids

    duplicate = await manager.queue_mobile_event(
        event_id="event-one",
        event_type="notification",
        title="Duplicate",
        body="Must not be redelivered",
        machine="iphone",
    )
    assert duplicate["queued_machines"] == []
    assert duplicate["duplicate_machines"] == ["iphone"]

    reloaded = remote.RemoteManager()
    reloaded.list_machines()
    restored = reloaded.workers["iphone"]
    assert restored.pending_events == []
    assert "event-one" in restored.recent_event_ids


@pytest.mark.asyncio
async def test_legacy_mobile_worker_pending_events_do_not_starve_jobs(tmp_path, monkeypatch):
    monkeypatch.setenv("LOCAL_SHELL_MCP_WORKSPACE_ROOT", str(tmp_path))
    monkeypatch.setenv("LOCAL_SHELL_MCP_STATE_DIR", str(tmp_path / ".state"))
    get_settings.cache_clear()

    manager = remote.RemoteManager()
    manager._registry_loaded = True
    worker = remote.RemoteWorker("iphone", "token-ios", capabilities=["mobile"])
    manager.workers[worker.name] = worker
    manager.tokens[worker.token] = worker.name
    monkeypatch.setattr(manager, "_wake_is_configured", lambda _worker: False)

    await manager.queue_mobile_event(
        event_id="event-for-new-worker",
        event_type="notification",
        title="Deferred until upgrade",
        body="Legacy workers must keep receiving normal jobs.",
        machine="iphone",
    )
    worker.queue.put_nowait({"id": "job-battery", "tool": "mobile_action", "args": {}})

    polled = await manager.poll(worker.token, {"supports_self_update": False})

    assert polled["job"]["id"] == "job-battery"
    assert "events" not in polled
    assert [event["id"] for event in worker.pending_events] == ["event-for-new-worker"]


@pytest.mark.asyncio
async def test_worker_event_fans_out_only_to_mobile_and_records_source(tmp_path, monkeypatch):
    monkeypatch.setenv("LOCAL_SHELL_MCP_WORKSPACE_ROOT", str(tmp_path))
    monkeypatch.setenv("LOCAL_SHELL_MCP_STATE_DIR", str(tmp_path / ".state"))
    get_settings.cache_clear()

    manager = remote.RemoteManager()
    manager._registry_loaded = True
    desktop = remote.RemoteWorker("desktop", "token-desktop", capabilities=["shell", "jobs"])
    iphone = remote.RemoteWorker("iphone", "token-ios", capabilities=["mobile"])
    manager.workers = {desktop.name: desktop, iphone.name: iphone}
    manager.tokens = {desktop.token: desktop.name, iphone.token: iphone.name}
    monkeypatch.setattr(manager, "_wake_is_configured", lambda _worker: False)

    result = await manager.submit_worker_event(
        desktop.token,
        {
            "id": "job-finish:1",
            "type": "job_completed",
            "title": "Render complete",
            "body": "succeeded · exit 0",
            "data": {"job_id": "job-1"},
        },
    )

    assert result["queued_machines"] == ["iphone"]
    assert desktop.pending_events == []
    assert iphone.pending_events[0]["data"]["source_machine"] == "desktop"


@pytest.mark.asyncio
async def test_job_completion_notifications_are_stable_markable_and_not_retroactive(
    tmp_path, monkeypatch
):
    state_dir = tmp_path / ".state"
    state_dir.mkdir(parents=True)
    monkeypatch.setenv("LOCAL_SHELL_MCP_WORKSPACE_ROOT", str(tmp_path))
    monkeypatch.setenv("LOCAL_SHELL_MCP_STATE_DIR", str(state_dir))
    get_settings.cache_clear()

    now = time.time()
    rows = [
        {
            "job_id": "job-new",
            "name": "New render",
            "status": "succeeded",
            "command": "render",
            "cwd": ".",
            "created_at": now - 10,
            "updated_at": now,
            "completed_at": now,
            "exit_code": 0,
            "attempts": 1,
            "notify_on_finish": True,
            "notify_title": "Render complete",
            "notify_delivery_version": 1,
        },
        {
            "job_id": "job-old",
            "name": "Historical job",
            "status": "succeeded",
            "command": "old",
            "cwd": ".",
            "created_at": now - 1000,
            "updated_at": now - 900,
            "completed_at": now - 900,
            "exit_code": 0,
            "attempts": 1,
            "notify_on_finish": True,
        },
    ]
    payload = json.dumps({"version": jobs_module.JOB_STORE_VERSION, "jobs": rows})
    (state_dir / jobs_module.JOB_STORE_FILE_NAME).write_text(payload, encoding="utf-8")
    (state_dir / jobs_module.JOB_STORE_BACKUP_FILE_NAME).write_text(payload, encoding="utf-8")

    async def no_shells():
        return {"sessions": []}

    monkeypatch.setattr(jobs_module, "list_shells", no_shells)

    first = await jobs_module.collect_pending_job_notifications()
    second = await jobs_module.collect_pending_job_notifications()
    assert len(first) == 1
    assert first[0]["id"] == second[0]["id"]
    assert first[0]["data"]["job_id"] == "job-new"
    assert first[0]["title"] == "Render complete"

    assert jobs_module.mark_job_notification_sent(first[0]["id"]) is True
    assert await jobs_module.collect_pending_job_notifications() == []


@pytest.mark.asyncio
async def test_session_watchdog_reports_goal_lease_expiry_without_claiming_exact_platform_timeout(
    monkeypatch,
):
    class FakeSessions:
        def list_sessions(self, *, subject):  # noqa: ARG002
            return [
                {
                    "session_id": "session-1",
                    "label": "Long task",
                    "status": "active",
                    "plan": {
                        "plan_id": "plan-1",
                        "status": "active",
                        "steps": [{"id": "work", "text": "work", "status": "active"}],
                        "last_agent_activity": 100.0,
                        "execution_lease_s": 1800,
                        "continuation_due_at": 1000.0,
                        "continuation_due": True,
                        "continuation_count": 0,
                        "auto_continue_exhausted": False,
                    },
                }
            ]

    calls = []

    class FakeRemote:
        async def queue_mobile_event(self, **kwargs):
            calls.append(kwargs)
            return {"accepted": True, "queued_machines": ["iphone"]}

    monkeypatch.setattr(notification_runtime, "get_session_runtime_manager", lambda: FakeSessions())
    monkeypatch.setattr(notification_runtime, "remote_manager", lambda: FakeRemote())

    await notification_runtime._dispatch_session_notifications()

    assert len(calls) == 1
    assert calls[0]["event_type"] == "agent_interrupted_or_expired"
    assert "30 min" in calls[0]["body"]
    assert "may have been interrupted" in calls[0]["body"]
    assert "platform timeout reached" not in calls[0]["body"].lower()

from __future__ import annotations

import copy
import time

import pytest

from morrow_runtime.session_runtime import PLAN_EXECUTION_LEASE_S, SessionRuntimeManager


def test_resume_persistence_failure_restores_both_sessions(tmp_path, monkeypatch):
    state_dir = tmp_path / ".state"
    manager = SessionRuntimeManager(state_dir)
    first = manager.manage("user", action="start", objective="First")
    target = manager.manage("user", action="start", objective="Target")
    first_id = first["session_id"]
    target_id = target["session_id"]
    original_save = manager._save_locked
    failed = False

    def fail_resume_activity(session):
        nonlocal failed
        if (
            not failed
            and session.session_id == target_id
            and session.activity
            and (session.activity[-1]["type"] == "session.resumed")
        ):
            failed = True
            raise OSError("forced resume persistence failure")
        return original_save(session)

    monkeypatch.setattr(manager, "_save_locked", fail_resume_activity)
    with pytest.raises(OSError, match="forced resume persistence failure"):
        manager.manage("user", action="resume", session_id=target_id)
    assert manager.get(first_id, subject="user")["objective"] == "First"
    target_state = manager.get(target_id, subject="user")
    assert target_state["objective"] == "Target"
    assert not any(event["type"] == "session.resumed" for event in target_state["recent_activity"])
    restored = SessionRuntimeManager(state_dir)
    assert restored.get(first_id, subject="user")["objective"] == "First"
    assert restored.get(target_id, subject="user")["objective"] == "Target"


def test_agent_session_resume_refreshes_goal_lease(tmp_path):
    manager = SessionRuntimeManager(tmp_path / ".state")
    started = manager.manage("user", action="start", objective="Task")
    session_id = started["session_id"]
    manager.manage_plan(
        session_id, action="start", objective="Task", steps=[{"id": "work", "text": "Work"}]
    )
    logical = manager._sessions[session_id]
    assert logical.plan is not None
    logical.plan.last_agent_activity = time.time() - PLAN_EXECUTION_LEASE_S - 1
    before_resume = time.time()
    resumed = manager.manage("user", action="resume", session_id=session_id)
    assert resumed["plan"]["last_agent_activity"] >= before_resume
    assert manager.claim_plan_continuation(session_id, subject="user") is None


@pytest.mark.parametrize(
    ("field", "value", "message"),
    [
        ("progress", [1], "invalid progress state"),
        ("activity", {"bad": 1}, "invalid activity state"),
        ("in_flight_calls", [1], "invalid in-flight tool state"),
    ],
)
def test_session_payload_rejects_malformed_nested_state(tmp_path, field, value, message):
    manager = SessionRuntimeManager(tmp_path / ".state")
    started = manager.manage("user", action="start", objective="Task")
    payload = manager._session_to_payload(manager._sessions[started["session_id"]])
    payload[field] = value
    with pytest.raises(ValueError, match=message):
        manager._session_from_payload(payload)


def test_v1_payload_drops_agent_run_metadata_without_losing_task_state(tmp_path):
    manager = SessionRuntimeManager(tmp_path / ".state")
    started = manager.manage("user", action="start", objective="Legacy task")
    session_id = started["session_id"]
    manager.manage("user", action="report", session_id=session_id, summary="keep me")
    payload = manager._session_to_payload(manager._sessions[session_id])
    payload["version"] = 1
    payload["active_run_id"] = "r_legacy"
    payload["runs"] = [
        {
            "run_id": "r_legacy",
            "session_key": "mcp-session:legacy",
            "created_at": 1.0,
            "updated_at": 2.0,
            "status": "active",
        }
    ]

    migrated = manager._session_from_payload(payload)
    rewritten = manager._session_to_payload(migrated)

    assert migrated.session_id == session_id
    assert migrated.progress.summary == "keep me"
    assert rewritten["version"] == 2
    assert "runs" not in rewritten
    assert "active_run_id" not in rewritten


def test_session_and_plan_payload_reject_invalid_top_level_shapes(tmp_path):
    manager = SessionRuntimeManager(tmp_path / ".state")
    with pytest.raises(ValueError, match="session metadata must be an object"):
        manager._session_from_payload([])
    with pytest.raises(ValueError, match="invalid plan state"):
        manager._plan_from_payload([])
    with pytest.raises(ValueError, match="invalid plan steps"):
        manager._plan_from_payload({"steps": {}})


def test_session_loader_skips_corrupt_state_files(tmp_path):
    state_dir = tmp_path / ".state"
    seeded = SessionRuntimeManager(state_dir)
    started = seeded.manage("user", action="start", objective="Valid")
    sessions_dir = state_dir / "sessions"
    (sessions_dir / "ignored.txt").write_text("not session state", encoding="utf-8")
    (sessions_dir / "broken-json.json").write_text("{", encoding="utf-8")
    (sessions_dir / "broken-utf8.json").write_bytes(b"\xff\xfe")
    restored = SessionRuntimeManager(state_dir)
    assert restored.get(started["session_id"], subject="user")["objective"] == "Valid"
    assert restored._load_session_from_store_locked("missing") is None
    assert restored._load_session_from_store_locked("broken-json") is None


def test_snapshot_restore_reports_rollback_failure(tmp_path, monkeypatch):
    manager = SessionRuntimeManager(tmp_path / ".state")
    started = manager.manage("user", action="start", objective="Task")
    snapshot = copy.deepcopy(manager._sessions[started["session_id"]])
    error = RuntimeError("original failure")

    def fail_save(_session):
        raise OSError("rollback failed")

    monkeypatch.setattr(manager, "_save_locked", fail_save)
    manager._restore_snapshot_locked(snapshot, error, context="Test restore")
    assert error.__notes__ == ["Test restore rollback warning: OSError: rollback failed"]
    assert manager._in_flight_count_locked("missing") == 0

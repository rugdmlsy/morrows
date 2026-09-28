from __future__ import annotations

import json
from types import SimpleNamespace

import pytest

import morrow_runtime.restart_ops as restart
from morrow_runtime.settings import get_settings


def _configure(tmp_path, monkeypatch):
    monkeypatch.setenv("LOCAL_SHELL_MCP_WORKSPACE_ROOT", str(tmp_path / "workspace"))
    monkeypatch.setenv("LOCAL_SHELL_MCP_STATE_DIR", str(tmp_path / "state"))
    monkeypatch.setenv("LOCAL_SHELL_MCP_AUTH_MODE", "none")
    get_settings.cache_clear()


def test_schedule_systemd_restart_is_one_shot_and_rate_limited(tmp_path, monkeypatch):
    _configure(tmp_path, monkeypatch)
    calls = []
    monkeypatch.setattr(restart, "_target", lambda role: ("systemd", "user", "lsm.service"))
    monkeypatch.setattr(restart, "_launcher", lambda role: ["/usr/bin/lsm"])

    def fake_run(command, **kwargs):
        calls.append(command)
        return SimpleNamespace(returncode=0, stdout="", stderr="")

    monkeypatch.setattr(restart, "_run", fake_run)
    first = restart.schedule_restart("controller", delay_s=1, health_timeout_s=999, reason="test")

    assert first["status"] == "scheduled"
    assert first["delay_s"] == restart.MIN_DELAY_S
    assert first["health_timeout_s"] == restart.MAX_HEALTH_TIMEOUT_S
    command = calls[-1]
    assert "--property=Type=oneshot" in command
    assert "--property=Restart=no" in command
    assert "--collect" in command
    assert "--no-block" in command
    assert not any("KeepAlive" in item for item in command)

    with pytest.raises(RuntimeError, match="already scheduled"):
        restart.schedule_restart("controller")


def test_launchd_restart_plist_never_uses_keepalive(tmp_path, monkeypatch):
    _configure(tmp_path, monkeypatch)
    monkeypatch.setattr(restart, "_target", lambda role: ("launchd", "user", "gui/501/lsm"))
    monkeypatch.setattr(restart, "_launcher", lambda role: ["/usr/local/bin/lsm"])
    calls = []

    def fake_run(command, **kwargs):
        calls.append(command)
        return SimpleNamespace(returncode=0, stdout="", stderr="")

    monkeypatch.setattr(restart, "_run", fake_run)
    result = restart.schedule_restart("controller")
    plist = next((tmp_path / "state" / "restarts").glob("*.plist"))
    import plistlib

    with plist.open("rb") as handle:
        payload = plistlib.load(handle)
    assert payload["RunAtLoad"] is True
    assert payload["KeepAlive"] is False
    assert payload["ExitTimeOut"] == 5
    assert calls[-1][:3] == ["launchctl", "bootstrap", f"gui/{restart._user_id()}"]
    assert result["supervisor"].startswith("com.fwerkor.morrow-runtime.restart.")


def test_supervisor_restarts_once_and_records_health_success(tmp_path, monkeypatch):
    _configure(tmp_path, monkeypatch)
    path = tmp_path / "restart.json"
    path.write_text(
        json.dumps({"restart_id": "abc", "role": "controller", "status": "scheduled"}),
        encoding="utf-8",
    )
    calls = []
    monkeypatch.setattr(restart.time, "sleep", lambda seconds: None)
    monkeypatch.setattr(restart, "_restart_target", lambda *args: calls.append(args))
    monkeypatch.setattr(restart, "_manager_active", lambda *args: True)
    pids = iter([101, 202])
    monkeypatch.setattr(restart, "_manager_pid", lambda *args: next(pids))
    monkeypatch.setattr(restart, "_controller_healthy", lambda port: True)
    args = SimpleNamespace(
        status_path=str(path),
        restart_id="abc",
        role="controller",
        manager="systemd",
        scope="system",
        target="lsm.service",
        delay_s=3,
        health_timeout_s=30,
        health_port=8765,
        maintenance_label=None,
        maintenance_plist=None,
    )

    assert restart.run_supervisor(args) == 0
    assert calls == [("systemd", "system", "lsm.service")]
    record = json.loads(path.read_text(encoding="utf-8"))
    assert record["status"] == "succeeded"
    assert record["attempts"] == 1
    assert record["previous_pid"] == 101
    assert record["current_pid"] == 202


def test_supervisor_health_timeout_does_not_retry_restart(tmp_path, monkeypatch):
    _configure(tmp_path, monkeypatch)
    path = tmp_path / "restart.json"
    path.write_text(json.dumps({"restart_id": "abc", "role": "worker"}), encoding="utf-8")
    restart_calls = []
    ticks = iter([0.0, 0.0, 10.0, 10.0])
    monkeypatch.setattr(restart.time, "sleep", lambda seconds: None)
    monkeypatch.setattr(restart.time, "monotonic", lambda: next(ticks, 10.0))
    monkeypatch.setattr(restart, "_restart_target", lambda *args: restart_calls.append(args))
    monkeypatch.setattr(restart, "_manager_active", lambda *args: False)
    monkeypatch.setattr(restart, "_manager_pid", lambda *args: 101)
    args = SimpleNamespace(
        status_path=str(path),
        restart_id="abc",
        role="worker",
        manager="systemd",
        scope="user",
        target="morrow-runtime-worker.service",
        delay_s=3,
        health_timeout_s=1,
        health_port=8765,
        maintenance_label=None,
        maintenance_plist=None,
    )

    assert restart.run_supervisor(args) == 1
    assert len(restart_calls) == 1
    assert json.loads(path.read_text(encoding="utf-8"))["status"] == "failed"


def test_systemd_restart_target_is_nonblocking(monkeypatch):
    calls = []

    def fake_run(command, **kwargs):
        calls.append((command, kwargs))
        return SimpleNamespace(returncode=0, stdout="", stderr="")

    monkeypatch.setattr(restart, "_run", fake_run)
    restart._restart_target("systemd", "system", "lsm.service")  # noqa: SLF001
    assert calls == [
        (["systemctl", "--no-block", "restart", "lsm.service"], {"timeout": 5})
    ]


def test_write_record_preserves_existing_owner(tmp_path, monkeypatch):
    if not hasattr(restart.os, "chown"):
        pytest.skip("ownership preservation requires POSIX chown")
    path = tmp_path / "record.json"
    path.write_text('{"status":"scheduled"}', encoding="utf-8")
    stat = path.stat()
    calls = []
    original_chown = restart.os.chown

    def tracked_chown(target, uid, gid):
        calls.append((uid, gid))
        original_chown(target, uid, gid)

    monkeypatch.setattr(restart.os, "chown", tracked_chown)
    restart._write_record(path, {"status": "succeeded"})  # noqa: SLF001

    assert calls == [(stat.st_uid, stat.st_gid)]
    assert json.loads(path.read_text(encoding="utf-8"))["status"] == "succeeded"


def test_restart_status_returns_latest_record(tmp_path, monkeypatch):
    _configure(tmp_path, monkeypatch)
    directory = tmp_path / "state" / "restarts"
    directory.mkdir(parents=True)
    aid = "a" * 32
    bid = "b" * 32
    (directory / f"{aid}.json").write_text(
        json.dumps({"restart_id": aid, "created_at": 1, "status": "succeeded"})
    )
    (directory / f"{bid}.json").write_text(
        json.dumps({"restart_id": bid, "created_at": 2, "status": "failed"})
    )

    assert restart.restart_status("controller")["restart_id"] == bid
    assert restart.restart_status("controller", "a" * 32)["status"] == "succeeded"


def test_restart_status_rejects_path_traversal_id(tmp_path, monkeypatch):
    _configure(tmp_path, monkeypatch)
    with pytest.raises(ValueError, match="32-character lowercase hex"):
        restart.restart_status("controller", "../../escape")


def test_restart_discovery_and_manager_helpers(monkeypatch, tmp_path):
    _configure(tmp_path, monkeypatch)
    monkeypatch.setattr(restart.shutil, "which", lambda name: "/usr/bin/systemctl")
    results = iter(
        [
            SimpleNamespace(returncode=1, stdout="", stderr=""),
            SimpleNamespace(returncode=0, stdout="", stderr=""),
        ]
    )
    monkeypatch.setattr(restart, "_run", lambda *args, **kwargs: next(results))
    assert restart._systemd_scope("lsm.service") == "system"  # noqa: SLF001

    monkeypatch.setattr(restart.platform, "system", lambda: "Linux")
    monkeypatch.setattr(restart, "_current_systemd_unit", lambda: "lsm.service")
    monkeypatch.setattr(restart, "_systemd_scope", lambda unit: "user")
    assert restart._target("controller") == ("systemd", "user", "lsm.service")  # noqa: SLF001

    calls = []

    def fake_run(command, **kwargs):
        calls.append(command)
        if "show" in command:
            return SimpleNamespace(returncode=0, stdout="123\n", stderr="")
        return SimpleNamespace(returncode=0, stdout="pid = 456\n", stderr="")

    monkeypatch.setattr(restart, "_run", fake_run)
    assert restart._manager_active("systemd", "user", "lsm.service")  # noqa: SLF001
    assert restart._manager_active("launchd", "user", "gui/1/lsm")  # noqa: SLF001
    assert restart._manager_pid("systemd", "user", "lsm.service") == 123  # noqa: SLF001
    assert restart._manager_pid("launchd", "user", "gui/1/lsm") == 456  # noqa: SLF001


def test_restart_failure_and_cooldown_paths(tmp_path, monkeypatch):
    _configure(tmp_path, monkeypatch)
    now = 1000.0
    monkeypatch.setattr(
        restart,
        "_load_records",
        lambda role: [{"status": "succeeded", "created_at": now - 5}],
    )
    with pytest.raises(RuntimeError, match="cooldown"):
        restart._enforce_rate_limit("controller", now)  # noqa: SLF001
    monkeypatch.setattr(
        restart,
        "_load_records",
        lambda role: [
            {"status": "succeeded", "created_at": now - 100},
            {"status": "failed", "created_at": now - 200},
            {"status": "succeeded", "created_at": now - 300},
        ],
    )
    with pytest.raises(RuntimeError, match="rate limit"):
        restart._enforce_rate_limit("controller", now)  # noqa: SLF001

    monkeypatch.setattr(restart, "_load_records", lambda role: [])
    monkeypatch.setattr(restart, "_target", lambda role: ("systemd", "user", "lsm.service"))
    monkeypatch.setattr(restart, "_launcher", lambda role: ["lsm"])
    monkeypatch.setattr(restart, "_schedule_systemd", lambda *args, **kwargs: (_ for _ in ()).throw(RuntimeError("no supervisor")))
    with pytest.raises(RuntimeError, match="no supervisor"):
        restart.schedule_restart("controller")
    record_path = next((tmp_path / "state" / "restarts").glob("*.json"))
    record = json.loads(record_path.read_text(encoding="utf-8"))
    assert record["status"] == "failed"
    assert record["error"] == "no supervisor"


def test_launchd_cleanup_and_health_helpers(monkeypatch, tmp_path):
    calls = []
    monkeypatch.setattr(
        restart.subprocess,
        "Popen",
        lambda command, **kwargs: calls.append((command, kwargs)),
    )
    plist = tmp_path / "supervisor.plist"
    plist.write_text("x")
    restart._cleanup_launchd_supervisor("label", str(plist))  # noqa: SLF001
    assert not plist.exists()
    assert calls[0][0][:2] == ["launchctl", "bootout"]

    monkeypatch.setattr(
        restart.urllib.request,
        "urlopen",
        lambda *args, **kwargs: SimpleNamespace(
            __enter__=lambda self: SimpleNamespace(status=200),
            __exit__=lambda *args: None,
        ),
    )
    # A failed probe is deliberately converted to a false health result.
    monkeypatch.setattr(
        restart.urllib.request,
        "urlopen",
        lambda *args, **kwargs: (_ for _ in ()).throw(OSError("down")),
    )
    assert restart._controller_healthy(8765) is False  # noqa: SLF001


def test_restart_cli_exits_with_supervisor_status(monkeypatch):
    monkeypatch.setattr(restart, "run_supervisor", lambda args: 7)
    with pytest.raises(SystemExit) as exc:
        restart.run_restart_supervisor_cli(
            [
                "--role",
                "worker",
                "--restart-id",
                "a" * 32,
                "--status-path",
                "/tmp/status.json",
                "--manager",
                "systemd",
                "--scope",
                "user",
                "--target",
                "worker.service",
                "--delay-s",
                "3",
                "--health-timeout-s",
                "30",
            ]
        )
    assert exc.value.code == 7


def test_restart_platform_targets_and_launchers(monkeypatch, tmp_path):
    _configure(tmp_path, monkeypatch)
    import morrow_runtime.remote_worker_service as worker_service
    import morrow_runtime.remote_worker_state as worker_state

    monkeypatch.setattr(worker_service, "service_kind", lambda: "systemd")
    assert restart._target("worker") == (  # noqa: SLF001
        "systemd",
        "user",
        "morrow-runtime-worker.service",
    )
    monkeypatch.setattr(worker_service, "service_kind", lambda: "launchd")
    assert restart._target("worker")[0] == "launchd"  # noqa: SLF001
    monkeypatch.setattr(worker_service, "service_kind", lambda: "none")
    with pytest.raises(RuntimeError, match="managed"):
        restart._target("worker")  # noqa: SLF001

    monkeypatch.setattr(restart.platform, "system", lambda: "Darwin")
    monkeypatch.delenv("XPC_SERVICE_NAME", raising=False)
    with pytest.raises(RuntimeError, match="launchd"):
        restart._target("controller")  # noqa: SLF001
    monkeypatch.setenv("XPC_SERVICE_NAME", "com.example.lsm")
    assert restart._target("controller")[2].endswith("/com.example.lsm")  # noqa: SLF001
    monkeypatch.setattr(restart.platform, "system", lambda: "Windows")
    with pytest.raises(RuntimeError, match="supports"):
        restart._target("controller")  # noqa: SLF001

    launcher = tmp_path / "worker-launcher"
    monkeypatch.setattr(worker_state, "worker_launcher_path", lambda: launcher)
    with pytest.raises(RuntimeError, match="not installed"):
        restart._launcher("worker")  # noqa: SLF001
    launcher.write_text("#!/bin/sh\n", encoding="utf-8")
    assert restart._launcher("worker") == [str(launcher)]  # noqa: SLF001


def test_low_level_run_unit_detection_and_status_errors(monkeypatch, tmp_path):
    _configure(tmp_path, monkeypatch)
    monkeypatch.setattr(
        restart.subprocess,
        "run",
        lambda *args, **kwargs: SimpleNamespace(returncode=1, stdout="", stderr="denied"),
    )
    with pytest.raises(RuntimeError, match="denied"):
        restart._run(["systemctl", "restart", "lsm"])  # noqa: SLF001

    original_read_text = restart.Path.read_text

    def cgroup(path, *args, **kwargs):
        if str(path).replace("\\", "/").endswith("/proc/self/cgroup"):
            return "0::/system.slice/morrow-runtime.service\n"
        return original_read_text(path, *args, **kwargs)

    monkeypatch.setattr(restart.Path, "read_text", cgroup)
    assert restart._current_systemd_unit() == "morrow-runtime.service"  # noqa: SLF001
    with pytest.raises(FileNotFoundError):
        restart.restart_status("controller", "f" * 32)


def test_launchd_schedule_failure_removes_plist(monkeypatch, tmp_path):
    _configure(tmp_path, monkeypatch)
    monkeypatch.setattr(restart, "_launcher", lambda role: ["lsm"])
    monkeypatch.setattr(
        restart,
        "_run",
        lambda *args, **kwargs: (_ for _ in ()).throw(RuntimeError("bootstrap failed")),
    )
    with pytest.raises(RuntimeError, match="bootstrap failed"):
        restart._schedule_launchd(  # noqa: SLF001
            "controller",
            "a" * 32,
            tmp_path / "status.json",
            "launchd",
            "user",
            "gui/1/lsm",
            3,
            30,
        )
    assert not list((tmp_path / "state" / "restarts").glob("*.plist"))


def test_launchd_manager_actions(monkeypatch):
    calls = []
    monkeypatch.setattr(
        restart,
        "_run",
        lambda command, **kwargs: calls.append((command, kwargs))
        or SimpleNamespace(returncode=1, stdout="", stderr=""),
    )
    restart._restart_target("launchd", "user", "gui/1/lsm")  # noqa: SLF001
    assert calls[-1][0] == ["launchctl", "kickstart", "-k", "gui/1/lsm"]
    assert restart._manager_pid("launchd", "user", "gui/1/lsm") is None  # noqa: SLF001


def test_controller_launcher_and_remaining_discovery_branches(monkeypatch, tmp_path):
    _configure(tmp_path, monkeypatch)
    executable = tmp_path / "morrow-runtime"
    executable.write_text("", encoding="utf-8")
    monkeypatch.setattr(restart.sys, "argv", [str(executable)])
    assert restart._launcher("controller") == [str(executable.resolve())]  # noqa: SLF001

    monkeypatch.setattr(restart.sys, "argv", ["pytest"])
    monkeypatch.setattr(restart.sys, "frozen", True, raising=False)
    assert restart._launcher("controller") == [restart.sys.executable]  # noqa: SLF001
    monkeypatch.setattr(restart.sys, "frozen", False)
    assert restart._launcher("controller")[-2:] == ["-m", "morrow_runtime.main"]  # noqa: SLF001

    monkeypatch.setattr(restart.Path, "read_text", lambda *args, **kwargs: "0::/user.slice\n")
    assert restart._current_systemd_unit() is None  # noqa: SLF001
    monkeypatch.setattr(restart.platform, "system", lambda: "Linux")
    monkeypatch.setattr(restart, "_current_systemd_unit", lambda: None)
    with pytest.raises(RuntimeError, match="systemd"):
        restart._target("controller")  # noqa: SLF001
    monkeypatch.setattr(restart, "_load_records", lambda role: [])
    assert restart.restart_status("controller") == {"role": "controller", "status": "none"}


def test_systemd_scope_user_and_missing_unit(monkeypatch):
    monkeypatch.setattr(restart.shutil, "which", lambda name: "/usr/bin/systemctl")
    monkeypatch.setattr(
        restart,
        "_run",
        lambda *args, **kwargs: SimpleNamespace(returncode=0, stdout="", stderr=""),
    )
    assert restart._systemd_scope("lsm.service") == "user"  # noqa: SLF001
    monkeypatch.setattr(restart.shutil, "which", lambda name: None)
    with pytest.raises(RuntimeError, match="unable to find"):
        restart._systemd_scope("missing.service")  # noqa: SLF001
    restart._cleanup_launchd_supervisor(None, None)  # noqa: SLF001

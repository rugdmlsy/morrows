from __future__ import annotations

import argparse
import json
import os
import platform
import plistlib
import re
import shutil
import subprocess
import sys
import time
import urllib.request
import uuid
from pathlib import Path
from typing import Any, Literal

from .settings import get_settings

RestartRole = Literal["controller", "worker"]

MIN_DELAY_S = 3
MAX_DELAY_S = 30
MIN_HEALTH_TIMEOUT_S = 5
MAX_HEALTH_TIMEOUT_S = 120
MIN_RESTART_INTERVAL_S = 30
RESTART_WINDOW_S = 600
MAX_RESTARTS_PER_WINDOW = 3
ACTIVE_RESTART_STALE_S = 180


def _user_id() -> int:
    """Return the POSIX user id used in launchd targets.

    Launchd paths are only exercised on macOS in production, but keeping this
    helper portable lets the scheduling logic and plist safety checks run on
    every CI platform.
    """
    getuid = getattr(os, "getuid", None)
    return int(getuid()) if getuid is not None else 0


def _state_root(role: RestartRole) -> Path:
    if role == "worker":
        from .remote_worker_state import worker_state_dir

        return worker_state_dir()
    return get_settings().state_dir


def _restart_dir(role: RestartRole) -> Path:
    path = _state_root(role) / "restarts"
    path.mkdir(parents=True, exist_ok=True)
    with __import__("contextlib").suppress(OSError):
        path.chmod(0o700)
    return path


def _record_path(role: RestartRole, restart_id: str) -> Path:
    if re.fullmatch(r"[0-9a-f]{32}", restart_id) is None:
        raise ValueError("restart_id must be a 32-character lowercase hex id")
    return _restart_dir(role) / f"{restart_id}.json"


def _write_record(path: Path, data: dict[str, Any]) -> None:
    owner: tuple[int, int] | None = None
    with __import__("contextlib").suppress(OSError):
        stat = path.stat()
        owner = (stat.st_uid, stat.st_gid)
    temporary = path.with_name(path.name + f".{uuid.uuid4().hex}.tmp")
    temporary.write_text(json.dumps(data, sort_keys=True, indent=2) + "\n", encoding="utf-8")
    with __import__("contextlib").suppress(OSError):
        temporary.chmod(0o600)
    if owner is not None and hasattr(os, "chown"):
        with __import__("contextlib").suppress(OSError):
            os.chown(temporary, *owner)
    os.replace(temporary, path)


def _load_records(role: RestartRole) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    for path in _restart_dir(role).glob("*.json"):
        try:
            row = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            continue
        if isinstance(row, dict):
            rows.append(row)
    return rows


def _enforce_rate_limit(role: RestartRole, now: float) -> None:
    rows = _load_records(role)
    active = [
        row
        for row in rows
        if row.get("status") in {"scheduled", "restarting"}
        and now - float(row.get("created_at", 0)) < ACTIVE_RESTART_STALE_S
    ]
    if active:
        raise RuntimeError(f"a {role} restart is already scheduled or in progress")
    recent = [row for row in rows if now - float(row.get("created_at", 0)) < RESTART_WINDOW_S]
    if len(recent) >= MAX_RESTARTS_PER_WINDOW:
        raise RuntimeError(
            f"restart rate limit exceeded: at most {MAX_RESTARTS_PER_WINDOW} restarts "
            f"per {RESTART_WINDOW_S // 60} minutes"
        )
    if recent:
        latest = max(float(row.get("created_at", 0)) for row in recent)
        remaining = MIN_RESTART_INTERVAL_S - (now - latest)
        if remaining > 0:
            raise RuntimeError(f"restart cooldown active; retry in {remaining:.0f}s")


def _current_systemd_unit() -> str | None:
    try:
        content = Path("/proc/self/cgroup").read_text(encoding="utf-8")
    except OSError:
        return None
    for line in content.splitlines():
        for part in reversed(line.rsplit(":", 1)[-1].split("/")):
            if part.endswith(".service"):
                return part
    return None


def _run(command: list[str], *, timeout: float = 10, check: bool = True) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(command, capture_output=True, text=True, timeout=timeout, check=False)  # noqa: S603
    if check and result.returncode:
        detail = result.stderr.strip() or result.stdout.strip() or f"exit {result.returncode}"
        raise RuntimeError(f"{' '.join(command[:3])} failed: {detail}")
    return result


def _systemd_scope(unit: str) -> Literal["user", "system"]:
    if shutil.which("systemctl"):
        result = _run(["systemctl", "--user", "is-active", unit], check=False)
        if result.returncode == 0:
            return "user"
        result = _run(["systemctl", "is-active", unit], check=False)
        if result.returncode == 0:
            return "system"
    raise RuntimeError(f"unable to find active systemd unit {unit}")


def _target(role: RestartRole) -> tuple[str, str, str]:
    system = platform.system()
    if role == "worker":
        from .remote_worker_service import service_kind

        kind = service_kind()
        if kind == "systemd":
            return "systemd", "user", "morrow-runtime-worker.service"
        if kind == "launchd":
            return "launchd", "user", f"gui/{_user_id()}/com.fwerkor.morrow-runtime-worker"
        raise RuntimeError("restart requires the worker to be managed by systemd or launchd")
    if system == "Linux":
        unit = _current_systemd_unit()
        if not unit:
            raise RuntimeError("controller is not running in a systemd service")
        return "systemd", _systemd_scope(unit), unit
    if system == "Darwin":
        label = os.getenv("XPC_SERVICE_NAME", "").strip()
        if not label:
            raise RuntimeError("controller is not running in a launchd service")
        return "launchd", "user", f"gui/{_user_id()}/{label}"
    raise RuntimeError("restart currently supports systemd and launchd managed LSM processes")


def _launcher(role: RestartRole) -> list[str]:
    if role == "worker":
        from .remote_worker_state import worker_launcher_path

        launcher = worker_launcher_path()
        if not launcher.exists():
            raise RuntimeError(f"worker launcher is not installed: {launcher}")
        return [str(launcher)]
    candidate = Path(sys.argv[0])
    if candidate.name == "morrow-runtime" and candidate.exists():
        return [str(candidate.resolve())]
    if getattr(sys, "frozen", False):
        return [sys.executable]
    return [sys.executable, "-m", "morrow_runtime.main"]


def _helper_command(
    role: RestartRole,
    restart_id: str,
    status_path: Path,
    manager: str,
    scope: str,
    target: str,
    delay_s: int,
    health_timeout_s: int,
    *,
    maintenance_label: str | None = None,
    maintenance_plist: Path | None = None,
) -> list[str]:
    command = [
        *_launcher(role),
        "restart-supervisor",
        "--role",
        role,
        "--restart-id",
        restart_id,
        "--status-path",
        str(status_path),
        "--manager",
        manager,
        "--scope",
        scope,
        "--target",
        target,
        "--delay-s",
        str(delay_s),
        "--health-timeout-s",
        str(health_timeout_s),
    ]
    if role == "controller":
        command.extend(["--health-port", str(get_settings().port)])
    if maintenance_label:
        command.extend(["--maintenance-label", maintenance_label])
    if maintenance_plist:
        command.extend(["--maintenance-plist", str(maintenance_plist)])
    return command


def _schedule_systemd(command: list[str], restart_id: str, scope: str, timeout_s: int) -> str:
    unit = f"morrow-runtime-restart-{restart_id[:12]}"
    runner = ["systemd-run"]
    if scope == "user":
        runner.append("--user")
    elif os.geteuid() != 0:
        if not shutil.which("sudo"):
            raise RuntimeError("system service restart requires root or passwordless sudo")
        runner = ["sudo", "-n", "systemd-run"]
    runner.extend(
        [
            f"--unit={unit}",
            "--collect",
            "--no-block",
            "--property=Type=oneshot",
            "--property=Restart=no",
            f"--property=TimeoutStartSec={timeout_s}s",
            "--",
            *command,
        ]
    )
    _run(runner, timeout=10)
    return f"{unit}.service"


def _schedule_launchd(
    role: RestartRole,
    restart_id: str,
    status_path: Path,
    manager: str,
    scope: str,
    target: str,
    delay_s: int,
    health_timeout_s: int,
) -> str:
    label = f"com.fwerkor.morrow-runtime.restart.{restart_id[:12]}"
    plist_path = _restart_dir(role) / f"{label}.plist"
    command = _helper_command(
        role,
        restart_id,
        status_path,
        manager,
        scope,
        target,
        delay_s,
        health_timeout_s,
        maintenance_label=label,
        maintenance_plist=plist_path,
    )
    payload = {
        "Label": label,
        "ProgramArguments": command,
        "RunAtLoad": True,
        "KeepAlive": False,
        "ProcessType": "Background",
        "ExitTimeOut": 5,
    }
    with plist_path.open("wb") as handle:
        plistlib.dump(payload, handle)
    domain = f"gui/{_user_id()}"
    try:
        _run(["launchctl", "bootstrap", domain, str(plist_path)])
    except Exception:
        plist_path.unlink(missing_ok=True)
        raise
    return label


def schedule_restart(
    role: RestartRole,
    *,
    delay_s: int = 5,
    health_timeout_s: int = 30,
    reason: str | None = None,
) -> dict[str, Any]:
    delay = max(MIN_DELAY_S, min(int(delay_s), MAX_DELAY_S))
    health_timeout = max(
        MIN_HEALTH_TIMEOUT_S, min(int(health_timeout_s), MAX_HEALTH_TIMEOUT_S)
    )
    now = time.time()
    _enforce_rate_limit(role, now)
    restart_id = uuid.uuid4().hex
    path = _record_path(role, restart_id)
    manager, scope, target = _target(role)
    record: dict[str, Any] = {
        "restart_id": restart_id,
        "role": role,
        "status": "scheduled",
        "created_at": now,
        "updated_at": now,
        "delay_s": delay,
        "health_timeout_s": health_timeout,
        "manager": manager,
        "scope": scope,
        "target": target,
        "reason": (reason or "").strip()[:1000] or None,
        "attempts": 0,
        "max_attempts": 1,
    }
    _write_record(path, record)
    try:
        helper = _helper_command(
            role, restart_id, path, manager, scope, target, delay, health_timeout
        )
        supervisor_timeout = delay + health_timeout + 20
        if manager == "systemd":
            supervisor = _schedule_systemd(helper, restart_id, scope, supervisor_timeout)
        else:
            supervisor = _schedule_launchd(
                role, restart_id, path, manager, scope, target, delay, health_timeout
            )
        record["supervisor"] = supervisor
        record["updated_at"] = time.time()
        _write_record(path, record)
    except Exception as exc:
        record.update(status="failed", updated_at=time.time(), error=str(exc))
        _write_record(path, record)
        raise
    return {
        **record,
        "message": "restart scheduled; the MCP/worker connection may disconnect briefly",
    }


def restart_status(role: RestartRole, restart_id: str | None = None) -> dict[str, Any]:
    if restart_id:
        path = _record_path(role, restart_id)
        if not path.exists():
            raise FileNotFoundError(f"restart record not found: {restart_id}")
        return json.loads(path.read_text(encoding="utf-8"))
    rows = _load_records(role)
    if not rows:
        return {"role": role, "status": "none"}
    return max(rows, key=lambda row: float(row.get("created_at", 0)))


def _manager_active(manager: str, scope: str, target: str) -> bool:
    if manager == "systemd":
        command = ["systemctl"]
        if scope == "user":
            command.append("--user")
        command.extend(["is-active", target])
        return _run(command, timeout=3, check=False).returncode == 0
    return _run(["launchctl", "print", target], timeout=3, check=False).returncode == 0


def _manager_pid(manager: str, scope: str, target: str) -> int | None:
    if manager == "systemd":
        command = ["systemctl"]
        if scope == "user":
            command.append("--user")
        command.extend(["show", "--property=MainPID", "--value", target])
        result = _run(command, timeout=3, check=False)
        value = result.stdout.strip()
        return int(value) if result.returncode == 0 and value.isdigit() and int(value) > 0 else None
    result = _run(["launchctl", "print", target], timeout=3, check=False)
    if result.returncode != 0:
        return None
    match = re.search(r"(?m)^\s*pid\s*=\s*(\d+)\s*$", result.stdout)
    return int(match.group(1)) if match and int(match.group(1)) > 0 else None


def _controller_healthy(port: int) -> bool:
    try:
        with urllib.request.urlopen(f"http://127.0.0.1:{port}/healthz", timeout=1) as response:  # noqa: S310
            return response.status == 200
    except Exception:
        return False


def _restart_target(manager: str, scope: str, target: str) -> None:
    if manager == "systemd":
        command = ["systemctl"]
        if scope == "user":
            command.append("--user")
        command.extend(["--no-block", "restart", target])
        _run(command, timeout=5)
        return
    _run(["launchctl", "kickstart", "-k", target], timeout=15)


def _cleanup_launchd_supervisor(label: str | None, plist_path: str | None) -> None:
    if plist_path:
        Path(plist_path).unlink(missing_ok=True)
    if not label:
        return
    subprocess.Popen(  # noqa: S603
        ["launchctl", "bootout", f"gui/{_user_id()}/{label}"],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        start_new_session=True,
    )


def run_supervisor(args: argparse.Namespace) -> int:
    path = Path(args.status_path)
    try:
        record = json.loads(path.read_text(encoding="utf-8"))
    except Exception:
        record = {"restart_id": args.restart_id, "role": args.role, "created_at": time.time()}
    try:
        time.sleep(args.delay_s)
        previous_pid = _manager_pid(args.manager, args.scope, args.target)
        record.update(
            status="restarting",
            updated_at=time.time(),
            attempts=1,
            previous_pid=previous_pid,
        )
        _write_record(path, record)
        _restart_target(args.manager, args.scope, args.target)
        deadline = time.monotonic() + args.health_timeout_s
        while time.monotonic() < deadline:
            active = _manager_active(args.manager, args.scope, args.target)
            current_pid = _manager_pid(args.manager, args.scope, args.target)
            restarted = current_pid is not None and (
                previous_pid is None or current_pid != previous_pid
            )
            healthy = active and restarted
            if healthy and args.role == "controller":
                healthy = _controller_healthy(args.health_port)
            if healthy:
                record.update(
                    status="succeeded",
                    updated_at=time.time(),
                    completed_at=time.time(),
                    current_pid=current_pid,
                    error=None,
                )
                _write_record(path, record)
                return 0
            time.sleep(0.5)
        raise TimeoutError(f"restart health check timed out after {args.health_timeout_s}s")
    except Exception as exc:
        record.update(
            status="failed",
            updated_at=time.time(),
            completed_at=time.time(),
            error=f"{type(exc).__name__}: {exc}",
        )
        _write_record(path, record)
        return 1
    finally:
        if args.manager == "launchd":
            _cleanup_launchd_supervisor(args.maintenance_label, args.maintenance_plist)


def run_restart_supervisor_cli(argv: list[str]) -> None:
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument("--role", choices=["controller", "worker"], required=True)
    parser.add_argument("--restart-id", required=True)
    parser.add_argument("--status-path", required=True)
    parser.add_argument("--manager", choices=["systemd", "launchd"], required=True)
    parser.add_argument("--scope", choices=["user", "system"], required=True)
    parser.add_argument("--target", required=True)
    parser.add_argument("--delay-s", type=int, required=True)
    parser.add_argument("--health-timeout-s", type=int, required=True)
    parser.add_argument("--health-port", type=int, default=8765)
    parser.add_argument("--maintenance-label", default=None)
    parser.add_argument("--maintenance-plist", default=None)
    raise SystemExit(run_supervisor(parser.parse_args(argv)))

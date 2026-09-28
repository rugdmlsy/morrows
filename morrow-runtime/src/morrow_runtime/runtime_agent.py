"""Machine-local managed provider runtime processes for Morrows.

This module deliberately sits below Morrows semantics. A runtime_id is supplied by
Morrows (normally a LaunchAttempt id); this layer only materializes files, starts a
persistent job, reports process state/output, and stops it on request.
"""

from __future__ import annotations

import contextlib
import json
import os
import re
import shlex
from pathlib import Path
from typing import Any

from .jobs import list_jobs, start_job, stop_job
from .settings import get_settings

_RUNTIME_ID = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$")
_ENV_KEY = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")
_MAX_OUTPUT_SLICE = 128 * 1024


def _root() -> Path:
    root = Path(get_settings().state_dir) / "runtime-agents"
    root.mkdir(parents=True, exist_ok=True)
    with contextlib.suppress(OSError):
        root.chmod(0o700)
    return root


def _dir(runtime_id: str) -> Path:
    if not _RUNTIME_ID.fullmatch(runtime_id):
        raise ValueError("invalid runtime_id")
    return _root() / runtime_id


def _private_write(path: Path, text: str, mode: int = 0o600) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")
    with contextlib.suppress(OSError):
        path.chmod(mode)


def _read_slice(path: Path) -> tuple[str, str]:
    if not path.is_file():
        return "", ""
    data = path.read_bytes()
    head = data[:_MAX_OUTPUT_SLICE].decode("utf-8", errors="replace")
    tail = data[-_MAX_OUTPUT_SLICE:].decode("utf-8", errors="replace")
    return head, tail


def _metadata(runtime_id: str) -> dict[str, Any]:
    path = _dir(runtime_id) / "runtime.json"
    if not path.is_file():
        raise ValueError(f"runtime {runtime_id} not found")
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError("invalid runtime metadata")
    return value


async def launch_runtime_agent(spec: dict[str, Any]) -> dict[str, Any]:
    runtime_id = str(spec.get("runtime_id") or "")
    runtime_dir = _dir(runtime_id)
    if runtime_dir.exists():
        metadata_path = runtime_dir / "runtime.json"
        if metadata_path.is_file():
            # A lost control response or Morrows restart may replay the same
            # runtime_id. Return the existing process state instead of creating
            # a duplicate provider process.
            return await runtime_agent_status(runtime_id)
        raise ValueError(f"runtime {runtime_id} exists but is incomplete")
    runtime_dir.mkdir(parents=True, mode=0o700)

    program = str(spec.get("program") or "")
    cwd = str(spec.get("cwd") or "")
    if not program or not os.path.isabs(program):
        raise ValueError("runtime program must be an absolute path")
    if not cwd or not os.path.isabs(cwd):
        raise ValueError("runtime cwd must be an absolute path")
    if not Path(program).is_file():
        raise ValueError(f"runtime program does not exist: {program}")
    if not Path(cwd).is_dir():
        raise ValueError(f"runtime cwd does not exist: {cwd}")

    raw_args = spec.get("args") or []
    if not isinstance(raw_args, list) or not all(isinstance(v, str) for v in raw_args):
        raise ValueError("runtime args must be a list of strings")
    token = "{runtime_dir}"
    home_token = "{home}"
    def expand(value: str) -> str:
        return value.replace(token, str(runtime_dir)).replace(home_token, str(Path.home()))
    args = [expand(value) for value in raw_args]

    raw_env = spec.get("env") or {}
    if not isinstance(raw_env, dict):
        raise ValueError("runtime env must be an object")
    env: dict[str, str] = {}
    for key, value in raw_env.items():
        if not isinstance(key, str) or not _ENV_KEY.fullmatch(key):
            raise ValueError(f"invalid runtime env key: {key!r}")
        if not isinstance(value, str):
            raise ValueError(f"runtime env value for {key} must be a string")
        env[key] = expand(value)

    files = spec.get("files") or []
    if not isinstance(files, list):
        raise ValueError("runtime files must be a list")
    for item in files:
        if not isinstance(item, dict):
            raise ValueError("runtime file entry must be an object")
        name = str(item.get("name") or "")
        relative = Path(name)
        if not name or relative.is_absolute() or ".." in relative.parts:
            raise ValueError("runtime file name must stay inside runtime_dir")
        content = item.get("content")
        if not isinstance(content, str):
            raise ValueError("runtime generated file content must be text")
        mode = int(item.get("mode", 0o600))
        _private_write(runtime_dir / relative, content, mode)

    stdin_path = runtime_dir / "stdin.txt"
    stdout_path = runtime_dir / "stdout.log"
    stderr_path = runtime_dir / "stderr.log"
    _private_write(stdin_path, str(spec.get("stdin_text") or ""))
    _private_write(stdout_path, "")
    _private_write(stderr_path, "")

    wrapper = runtime_dir / "run.sh"
    lines = ["#!/bin/sh", "set -eu"]
    for key, value in sorted(env.items()):
        lines.append(f"export {key}={shlex.quote(value)}")
    command = shlex.join([program, *args])
    lines.append(
        f"exec {command} < {shlex.quote(str(stdin_path))} "
        f"> {shlex.quote(str(stdout_path))} 2> {shlex.quote(str(stderr_path))}"
    )
    _private_write(wrapper, "\n".join(lines) + "\n", 0o700)

    job = await start_job(
        shlex.join(["/bin/sh", str(wrapper)]),
        cwd=cwd,
        name=f"morrow-runtime-{runtime_id}",
    )
    metadata = {
        "runtime_id": runtime_id,
        "job_id": job["job_id"],
        "program": program,
        "cwd": cwd,
        "stdout_path": str(stdout_path),
        "stderr_path": str(stderr_path),
        "provider": spec.get("provider"),
        "env_keys": sorted(env),
    }
    _private_write(runtime_dir / "runtime.json", json.dumps(metadata, separators=(",", ":")))
    return {**metadata, "status": job.get("status"), "machine": job.get("machine")}


async def runtime_agent_status(runtime_id: str) -> dict[str, Any]:
    metadata = _metadata(runtime_id)
    jobs = (await list_jobs(True, 1000)).get("jobs") or []
    job = next((item for item in jobs if item.get("job_id") == metadata["job_id"]), None)
    if job is None:
        raise ValueError(f"runtime job {metadata['job_id']} not found")
    stdout_head, stdout_tail = _read_slice(Path(metadata["stdout_path"]))
    _, stderr_tail = _read_slice(Path(metadata["stderr_path"]))
    return {
        **metadata,
        "status": job.get("status"),
        "exit_code": job.get("exit_code"),
        "error": job.get("error"),
        "stdout_head": stdout_head,
        "stdout_tail": stdout_tail,
        "stderr_tail": stderr_tail,
        "completed_at": job.get("completed_at"),
    }


async def stop_runtime_agent(runtime_id: str) -> dict[str, Any]:
    metadata = _metadata(runtime_id)
    result = await stop_job(str(metadata["job_id"]))
    return {"runtime_id": runtime_id, "job": result}

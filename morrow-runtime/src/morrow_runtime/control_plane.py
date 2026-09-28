"""Trusted provision, observation, and cleanup API for orchestration clients."""

from __future__ import annotations

import asyncio
import hmac
from contextlib import nullcontext
from typing import Any

from starlette.requests import Request
from starlette.responses import JSONResponse
from starlette.routing import Route

from .audit import audit, audit_request_context, query_audit
from .capabilities import issue_capability, revoke_capability, revoke_session_capabilities
from .execution_scope import execution_session
from .jobs import TERMINAL_STATUSES, list_jobs, stop_job, tail_job
from .remote import remote_manager
from .session_runtime import get_session_runtime_manager
from .settings import get_settings
from .shell_ops import kill_shell, list_shells


def _authorized(request: Request) -> bool:
    expected = get_settings().control_api_key
    supplied = (
        request.headers.get("x-morrow-runtime-control-key")
        or request.headers.get("x-lsm-control-key", "")
    )
    return bool(expected) and hmac.compare_digest(supplied, expected)


def _response(data: dict[str, Any], status_code: int = 200) -> JSONResponse:
    return JSONResponse(data, status_code=status_code)


async def _guarded(request: Request, handler) -> JSONResponse:  # noqa: ANN001
    if not _authorized(request):
        return _response({"error": "control credential required"}, 401)
    mutation = request.method != "GET"
    context = (
        audit_request_context(
            actor="control", ingress="control", logical_session=request.path_params.get("session_id")
        )
        if mutation else nullcontext()
    )
    with context:
        if mutation:
            audit("control_request_started", operation=handler.__name__)
        try:
            result = await handler(request)
            if mutation:
                audit("control_request_completed", operation=handler.__name__, ok=True)
            return _response(result)
        except (KeyError, ValueError, PermissionError) as exc:
            if mutation:
                audit("control_request_completed", operation=handler.__name__, ok=False,
                      error=str(exc))
            return _response({"error": str(exc)}, 400)
        except Exception as exc:  # noqa: BLE001 - return a stable control-plane error.
            if mutation:
                audit("control_request_completed", operation=handler.__name__, ok=False,
                      error=str(exc))
            return _response({"error": f"{type(exc).__name__}: {exc}"}, 500)


def _session_id(request: Request) -> str:
    return str(request.path_params["session_id"])


async def _start(request: Request) -> dict[str, Any]:
    body = await request.json()
    subject = str(body["subject"])
    session = await asyncio.to_thread(
        get_session_runtime_manager().manage,
        subject,
        action="start",
        idempotency_key=body.get("idempotency_key"),
        label=body.get("label"),
        objective=body.get("objective"),
        actor="control",
    )
    audit("control_session_started", logical_session=session["session_id"],
          idempotency_key=body.get("idempotency_key"))
    return {"session": session}


async def _get(request: Request) -> dict[str, Any]:
    session = await asyncio.to_thread(get_session_runtime_manager().get, _session_id(request))
    return {"session": session}


async def _lifecycle(request: Request) -> dict[str, Any]:
    body = await request.json()
    action = str(body["action"])
    if action not in {"finish", "cancel", "delete"}:
        raise ValueError("control lifecycle action must be finish, cancel, or delete")
    session_id = _session_id(request)
    subject = str(body["subject"])
    await asyncio.to_thread(get_session_runtime_manager().get, session_id, subject=subject)
    revoke_session_capabilities(session_id, block=True)
    session = await asyncio.to_thread(
        get_session_runtime_manager().manage,
        subject,
        action=action,
        session_id=session_id,
        actor="control",
    )
    return {"session": session}


async def _issue(request: Request) -> dict[str, Any]:
    body = await request.json()
    session_id = _session_id(request)
    subject = str(body["subject"])
    def verify_active() -> None:
        session = get_session_runtime_manager().get(session_id, subject=subject)
        if session["status"] != "active":
            raise ValueError("cannot issue a capability for a terminal Session")

    issued = await asyncio.to_thread(issue_capability, session_id, subject, verify_active)
    audit("control_capability_issued", logical_session=session_id,
          capability_id=issued["capability_id"])
    return issued


async def _revoke(request: Request) -> dict[str, Any]:
    capability_id = str(request.path_params["capability_id"])
    session_id = revoke_capability(capability_id)
    audit("control_capability_revoked", logical_session=session_id,
          capability_id=capability_id)
    return {"revoked": True, "capability_id": capability_id}


async def _node_call(
    node: str, tool: str, args: dict[str, Any], session_id: str
) -> dict[str, Any]:
    if node == "local":
        mutation = tool in {"job_stop", "shell_kill", "runtime_agent_launch", "runtime_agent_stop"}
        audit_context = (
            audit_request_context(actor="control", ingress="control", logical_session=session_id)
            if mutation else nullcontext()
        )
        with execution_session(session_id), audit_context:
            if tool == "job_list":
                return await list_jobs(args.get("include_finished", True), args.get("limit", 1000))
            if tool == "shell_list":
                return await list_shells()
            if tool == "job_stop":
                return await stop_job(str(args["job_id"]))
            if tool == "job_tail":
                return await tail_job(str(args["job_id"]), int(args.get("lines", 200)))
            if tool == "shell_kill":
                return await kill_shell(str(args["session_id"]))
            if tool in {"runtime_agent_launch", "runtime_agent_status", "runtime_agent_stop"}:
                from .runtime_agent import launch_runtime_agent, runtime_agent_status, stop_runtime_agent
                if tool == "runtime_agent_launch":
                    return await launch_runtime_agent(args)
                if tool == "runtime_agent_status":
                    return await runtime_agent_status(str(args["runtime_id"]))
                return await stop_runtime_agent(str(args["runtime_id"]))
        raise ValueError(f"unsupported local control tool: {tool}")
    result = await remote_manager().call(
        node,
        tool,
        {**args, "_logical_session_id": session_id, "_execution_machine": node,
         **({"_control_actor": True} if tool in {"job_stop", "shell_kill", "runtime_agent_launch", "runtime_agent_status", "runtime_agent_stop"} else {})},
        timeout_s=5,
    )
    if not result.get("ok"):
        raise RuntimeError(str(result.get("message") or "remote worker call failed"))
    data = result.get("data")
    if not isinstance(data, dict):
        raise RuntimeError("remote worker returned an invalid result")
    return data


async def _enumerate(session_id: str, kind: str) -> dict[str, Any]:
    session = await asyncio.to_thread(get_session_runtime_manager().get, session_id)
    nodes = list(session.get("machines_touched") or [])
    tool = "job_list" if kind == "jobs" else "shell_list"
    args = {"include_finished": True, "limit": 1000} if kind == "jobs" else {}

    async def query(node: str) -> tuple[str, dict[str, Any] | Exception]:
        try:
            return node, await _node_call(node, tool, args, session_id)
        except Exception as exc:  # noqa: BLE001 - partial results are explicit.
            return node, exc

    rows: list[dict[str, Any]] = []
    unreachable: list[str] = []
    truncated = False
    for node, result in await asyncio.gather(*(query(node) for node in nodes)):
        if isinstance(result, Exception):
            unreachable.append(node)
            continue
        collection = result.get("jobs" if kind == "jobs" else "sessions") or []
        for item in collection:
            if isinstance(item, dict) and item.get("logical_session_id") == session_id:
                rows.append({**item, "machine": node})
        truncated |= bool(result.get("truncated"))
    return {
        kind: rows,
        "complete": not unreachable and not truncated,
        "unreachable_machines": unreachable,
        "truncated": truncated,
    }


async def _jobs(request: Request) -> dict[str, Any]:
    return await _enumerate(_session_id(request), "jobs")


async def _shells(request: Request) -> dict[str, Any]:
    return await _enumerate(_session_id(request), "shells")


async def _audit(request: Request) -> dict[str, Any]:
    session_id = _session_id(request)
    await asyncio.to_thread(get_session_runtime_manager().get, session_id)
    limit = int(request.query_params.get("limit", "200"))
    return await asyncio.to_thread(query_audit, logical_session_id=session_id, limit=limit)


async def _job_tail(request: Request) -> dict[str, Any]:
    session_id = _session_id(request)
    session = await asyncio.to_thread(get_session_runtime_manager().get, session_id)
    machine = str(request.query_params.get("machine") or "local")
    if machine not in set(session.get("machines_touched") or []):
        raise PermissionError("Machine is outside this Logical Session")
    return await _node_call(machine, "job_tail", {
        "job_id": str(request.path_params["job_id"]),
        "lines": int(request.query_params.get("lines", "200")),
    }, session_id)


async def _runtime_launch(request: Request) -> dict[str, Any]:
    body = await request.json()
    session_id = _session_id(request)
    subject = str(body["subject"])
    machine = str(body.get("machine") or "local")
    runtime_id = str(body["runtime_id"])
    spec = body.get("spec")
    if not isinstance(spec, dict):
        raise ValueError("runtime launch spec must be an object")
    manager = get_session_runtime_manager()
    await asyncio.to_thread(manager.get, session_id, subject=subject)
    call_id = f"runtime-launch:{runtime_id}"
    lease = await asyncio.to_thread(
        manager.begin_tool_call,
        session_id,
        call_id,
        subject=subject,
        data={"tool": "runtime_agent_launch", "runtime_id": runtime_id, "machine": machine},
        execution_machines=[machine],
    )
    try:
        result = await _node_call(
            machine,
            "runtime_agent_launch",
            {**spec, "runtime_id": runtime_id},
            session_id,
        )
    except Exception as exc:
        await asyncio.to_thread(
            manager.finish_tool_call,
            lease,
            "runtime.agent_launch_failed",
            data={"runtime_id": runtime_id, "machine": machine, "error": str(exc)},
        )
        raise
    await asyncio.to_thread(
        manager.finish_tool_call,
        lease,
        "runtime.agent_launched",
        data={"runtime_id": runtime_id, "machine": machine},
    )
    return {"runtime": result, "machine": machine}


async def _runtime_status(request: Request) -> dict[str, Any]:
    session_id = _session_id(request)
    runtime_id = str(request.path_params["runtime_id"])
    machine = str(request.query_params.get("machine") or "local")
    session = await asyncio.to_thread(get_session_runtime_manager().get, session_id)
    if machine not in set(session.get("machines_touched") or []):
        raise PermissionError("Machine is outside this Logical Session")
    result = await _node_call(
        machine,
        "runtime_agent_status",
        {"runtime_id": runtime_id},
        session_id,
    )
    return {"runtime": result, "machine": machine}


async def _runtime_stop(request: Request) -> dict[str, Any]:
    body = await request.json()
    session_id = _session_id(request)
    runtime_id = str(request.path_params["runtime_id"])
    machine = str(body.get("machine") or "local")
    session = await asyncio.to_thread(get_session_runtime_manager().get, session_id)
    if machine not in set(session.get("machines_touched") or []):
        raise PermissionError("Machine is outside this Logical Session")
    result = await _node_call(
        machine,
        "runtime_agent_stop",
        {"runtime_id": runtime_id},
        session_id,
    )
    return {"runtime": result, "machine": machine}


async def _worker_invite(request: Request) -> dict[str, Any]:
    body = await request.json()
    invite = await remote_manager().create_invite(
        name=str(body.get("name") or "").strip() or None,
        workdir=str(body.get("workdir") or "").strip() or None,
        ttl_s=int(body.get("ttl_s") or 600),
    )
    return {"invite": invite}


async def _workers(request: Request) -> dict[str, Any]:  # noqa: ARG001
    return remote_manager().list_machines()


async def _cleanup(request: Request) -> dict[str, Any]:
    body = await request.json()
    session_id = _session_id(request)
    subject = str(body["subject"])
    terminal_action = str(body.get("terminal_action", "cancel"))
    if terminal_action not in {"cancel", "finish"}:
        raise ValueError("terminal_action must be cancel or finish")
    timeout_s = max(0, min(float(body.get("wait_seconds", 30)), 30))
    await asyncio.to_thread(get_session_runtime_manager().get, session_id, subject=subject)
    revoked = revoke_session_capabilities(session_id, block=True)
    audit("control_capabilities_blocked", revoked=revoked)
    jobs = await _enumerate(session_id, "jobs")
    shells = await _enumerate(session_id, "shells")
    failures: list[str] = []
    for job in jobs["jobs"]:
        if job.get("status") in TERMINAL_STATUSES:
            continue
        try:
            await _node_call(str(job["machine"]), "job_stop", {"job_id": job["job_id"]}, session_id)
        except Exception as exc:  # noqa: BLE001 - retain cleanup_pending on any failure.
            failures.append(f"job {job['job_id']}: {exc}")
    for shell in shells["shells"]:
        try:
            await _node_call(
                str(shell["machine"]), "shell_kill", {"session_id": shell["session_id"]}, session_id
            )
        except Exception as exc:  # noqa: BLE001
            failures.append(f"shell {shell['session_id']}: {exc}")
    deadline = asyncio.get_running_loop().time() + timeout_s
    session = await asyncio.to_thread(get_session_runtime_manager().get, session_id, subject=subject)
    while session["in_flight_calls"] and asyncio.get_running_loop().time() < deadline:
        await asyncio.sleep(0.2)
        session = await asyncio.to_thread(get_session_runtime_manager().get, session_id, subject=subject)
    remaining_jobs = await _enumerate(session_id, "jobs")
    remaining_shells = await _enumerate(session_id, "shells")
    active_jobs = [job for job in remaining_jobs["jobs"] if job.get("status") not in TERMINAL_STATUSES]
    active_shells = remaining_shells["shells"]
    unreachable = sorted(set(
        jobs["unreachable_machines"] + shells["unreachable_machines"]
        + remaining_jobs["unreachable_machines"] + remaining_shells["unreachable_machines"]
    ))
    if (failures or unreachable or not jobs["complete"] or not shells["complete"]
        or not remaining_jobs["complete"] or not remaining_shells["complete"]
        or session["in_flight_calls"] or active_jobs or active_shells):
        return {
            "complete": False,
            "session": session,
            "unreachable_machines": unreachable,
            "errors": failures,
            "active_job_ids": [job["job_id"] for job in active_jobs],
            "active_shell_ids": [shell["session_id"] for shell in active_shells],
        }
    if session["status"] == "active":
        session = await asyncio.to_thread(
            get_session_runtime_manager().manage,
            subject,
            action=terminal_action,
            session_id=session_id,
            actor="control",
        )
    expected_status = "cancelled" if terminal_action == "cancel" else "completed"
    return {"complete": session["status"] == expected_status, "session": session,
            "unreachable_machines": [], "errors": []}


def control_routes() -> list[Route]:
    def guarded(handler):  # noqa: ANN001, ANN202
        async def endpoint(request: Request) -> JSONResponse:
            return await _guarded(request, handler)

        return endpoint

    prefix = "/api/control"
    return [
        Route(f"{prefix}/sessions", guarded(_start), methods=["POST"]),
        Route(f"{prefix}/sessions/{{session_id}}", guarded(_get), methods=["GET"]),
        Route(f"{prefix}/sessions/{{session_id}}/lifecycle", guarded(_lifecycle), methods=["POST"]),
        Route(f"{prefix}/sessions/{{session_id}}/capabilities", guarded(_issue), methods=["POST"]),
        Route(f"{prefix}/capabilities/{{capability_id}}/revoke", guarded(_revoke), methods=["POST"]),
        Route(f"{prefix}/sessions/{{session_id}}/jobs", guarded(_jobs), methods=["GET"]),
        Route(f"{prefix}/sessions/{{session_id}}/shells", guarded(_shells), methods=["GET"]),
        Route(f"{prefix}/sessions/{{session_id}}/audit", guarded(_audit), methods=["GET"]),
        Route(f"{prefix}/sessions/{{session_id}}/jobs/{{job_id}}/tail", guarded(_job_tail), methods=["GET"]),
        Route(f"{prefix}/sessions/{{session_id}}/runtime/launch", guarded(_runtime_launch), methods=["POST"]),
        Route(f"{prefix}/sessions/{{session_id}}/runtime/{{runtime_id}}", guarded(_runtime_status), methods=["GET"]),
        Route(f"{prefix}/sessions/{{session_id}}/runtime/{{runtime_id}}/stop", guarded(_runtime_stop), methods=["POST"]),
        Route(f"{prefix}/workers", guarded(_workers), methods=["GET"]),
        Route(f"{prefix}/workers/invite", guarded(_worker_invite), methods=["POST"]),
        Route(f"{prefix}/sessions/{{session_id}}/cleanup", guarded(_cleanup), methods=["POST"]),
    ]

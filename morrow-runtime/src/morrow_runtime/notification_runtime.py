from __future__ import annotations

import asyncio
import contextlib
import time
from typing import Any

from .audit import audit
from .jobs import collect_pending_job_notifications, mark_job_notification_sent
from .remote import remote_manager
from .session_runtime import get_session_runtime_manager

_NOTIFICATION_SCAN_INTERVAL_S = 5


def _unfinished(plan: dict[str, Any]) -> bool:
    return any(
        str(step.get("status") or "") in {"pending", "active"}
        for step in plan.get("steps", [])
        if isinstance(step, dict)
    )


async def _dispatch_local_job_notifications() -> None:
    for event in await collect_pending_job_notifications():
        data = dict(event.get("data") or {})
        data.setdefault("source_machine", "controller")
        result = await remote_manager().queue_mobile_event(
            event_id=str(event["id"]),
            event_type=str(event.get("type") or "job_completed"),
            title=str(event.get("title") or "LSM job completed"),
            body=str(event.get("body") or "Tracked job completed"),
            data=data,
            wake_reason="job_completed",
        )
        if result.get("accepted"):
            mark_job_notification_sent(str(event["id"]))


async def _dispatch_session_notifications() -> None:
    now = time.time()
    manager = get_session_runtime_manager()
    for session in manager.list_sessions(subject=None):
        if session.get("status") != "active":
            continue
        plan = session.get("plan")
        if not isinstance(plan, dict) or plan.get("status") != "active" or not _unfinished(plan):
            continue
        session_id = str(session.get("session_id") or "")
        plan_id = str(plan.get("plan_id") or "")
        label = str(session.get("label") or session.get("objective") or session_id or "LSM task")
        if plan.get("continuation_due"):
            activity = int(float(plan.get("last_agent_activity") or 0))
            event_id = f"agent-lease:{session_id}:{plan_id}:{activity}"
            result = await remote_manager().queue_mobile_event(
                event_id=event_id,
                event_type="agent_interrupted_or_expired",
                title="LSM agent execution paused",
                body=(
                    f"{label}: no agent activity for {int(plan.get('execution_lease_s') or 1800) // 60} min "
                    "while the Goal is unfinished. The ChatGPT turn may have been interrupted; auto-continuation is due."
                ),
                data={
                    "session_id": session_id,
                    "plan_id": plan_id,
                    "continuation_count": plan.get("continuation_count"),
                    "continuation_due_at": plan.get("continuation_due_at"),
                },
                wake_reason="agent_interruption",
            )
            if result.get("accepted"):
                with contextlib.suppress(Exception):
                    audit("agent_interruption_notification_queued", session_id=session_id, plan_id=plan_id)
        if plan.get("auto_continue_exhausted"):
            event_id = f"agent-continuation-exhausted:{session_id}:{plan_id}"
            await remote_manager().queue_mobile_event(
                event_id=event_id,
                event_type="agent_continuation_exhausted",
                title="LSM agent needs attention",
                body=f"{label}: automatic continuation attempts are exhausted while the Goal is still unfinished.",
                data={"session_id": session_id, "plan_id": plan_id},
                wake_reason="agent_interruption",
            )
        _ = now


async def notification_watchdog_loop() -> None:
    """Dispatch durable mobile events independently of a ChatGPT turn.

    This cannot observe the platform's exact per-turn cutoff. It uses LSM's
    30-minute Goal execution lease as a conservative interruption signal and
    sends deterministic completion events for jobs that opted into
    ``notify_on_finish``.
    """
    while True:
        try:
            await _dispatch_local_job_notifications()
            await _dispatch_session_notifications()
        except asyncio.CancelledError:
            raise
        except Exception as exc:  # noqa: BLE001
            with contextlib.suppress(Exception):
                audit("notification_watchdog_error", error=repr(exc))
        await asyncio.sleep(_NOTIFICATION_SCAN_INTERVAL_S)

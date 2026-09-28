"""Durable, revocable capabilities pinned to one Logical Session."""

from __future__ import annotations

import hashlib
import json
import secrets
from collections.abc import Callable
from typing import Any

from .state_store import get_state_store


def _key(capability_id: str) -> str:
    if len(capability_id) != 64 or any(ch not in "0123456789abcdef" for ch in capability_id):
        raise ValueError("invalid capability id")
    return f"session-capabilities/{capability_id}.json"


def _state_key(session_id: str) -> str:
    digest = hashlib.sha256(session_id.encode("utf-8")).hexdigest()
    return f"session-capability-state/{digest}.json"


def _lock_key(session_id: str) -> str:
    return f"session-capability-state/{hashlib.sha256(session_id.encode('utf-8')).hexdigest()}"


def _load_state(session_id: str) -> dict[str, Any]:
    raw = get_state_store().read_bytes(_state_key(session_id))
    if raw is None:
        return {"generation": 0, "blocked": False, "current_id": None}
    state = json.loads(raw)
    if not isinstance(state, dict):
        raise ValueError("invalid Session capability state")
    return state


def _save_state(session_id: str, state: dict[str, Any]) -> None:
    get_state_store().write_bytes(
        _state_key(session_id), json.dumps(state, separators=(",", ":")).encode("utf-8")
    )


def issue_capability(
    session_id: str,
    subject: str,
    verify_active: Callable[[], None] | None = None,
) -> dict[str, str]:
    """Rotate one Session's capability with a durable generation fence.

    State is written before the new token. A crash at that boundary can leave no
    usable token, but cannot leave two; the controller can safely retry issuance.
    The callback runs under the same per-Session lock as cleanup's block fence.
    """
    store = get_state_store()
    with store.lock(_lock_key(session_id)):
        state = _load_state(session_id)
        if state.get("blocked"):
            raise ValueError("Session capabilities are blocked during cleanup")
        if verify_active is not None:
            verify_active()
        old_id = state.get("current_id")
        generation = int(state.get("generation", 0)) + 1
        token = secrets.token_urlsafe(48)
        capability_id = hashlib.sha256(token.encode("utf-8")).hexdigest()
        state.update({"generation": generation, "blocked": False, "current_id": capability_id})
        _save_state(session_id, state)
        record = {"session_id": session_id, "subject": subject, "generation": generation}
        store.write_bytes(
            _key(capability_id), json.dumps(record, separators=(",", ":")).encode("utf-8")
        )
        if old_id:
            store.delete(_key(str(old_id)))
        return {"capability": token, "capability_id": capability_id, **record}


def resolve_capability(token: str | None) -> dict[str, Any] | None:
    if not token:
        return None
    capability_id = hashlib.sha256(token.encode("utf-8")).hexdigest()
    store = get_state_store()
    raw = store.read_bytes(_key(capability_id))
    if raw is None:
        return None
    record = json.loads(raw)
    if not isinstance(record, dict) or not isinstance(record.get("session_id"), str):
        return None
    session_id = record["session_id"]
    # The first read locates the Session lock. Re-read both durable objects
    # inside it: revoke must either follow this admission or finish before it.
    with store.lock(_lock_key(session_id)):
        current_raw = store.read_bytes(_key(capability_id))
        if current_raw is None:
            return None
        current = json.loads(current_raw)
        if not isinstance(current, dict) or current.get("session_id") != session_id:
            return None
        state = _load_state(session_id)
        if (state.get("blocked") or state.get("current_id") != capability_id
            or int(current.get("generation", 0)) != int(state.get("generation", 0))):
            return None
        return {**current, "capability_id": capability_id}


def revoke_capability(capability_id: str) -> str | None:
    store = get_state_store()
    raw = store.read_bytes(_key(capability_id))
    if raw is None:
        return None
    record = json.loads(raw)
    session_id = record.get("session_id") if isinstance(record, dict) else None
    if not isinstance(session_id, str):
        store.delete(_key(capability_id))
        return None
    with store.lock(_lock_key(session_id)):
        state = _load_state(session_id)
        if state.get("current_id") == capability_id:
            state["generation"] = int(state.get("generation", 0)) + 1
            state["current_id"] = None
            _save_state(session_id, state)
        store.delete(_key(capability_id))
    return session_id


def revoke_session_capabilities(session_id: str, *, block: bool = False) -> int:
    store = get_state_store()
    with store.lock(_lock_key(session_id)):
        state = _load_state(session_id)
        state["generation"] = int(state.get("generation", 0)) + 1
        state["blocked"] = bool(state.get("blocked")) or block
        state["current_id"] = None
        _save_state(session_id, state)
        revoked = 0
        for key in store.list_keys("session-capabilities/"):
            raw = store.read_bytes(key)
            if raw is None:
                continue
            try:
                record = json.loads(raw)
            except (ValueError, TypeError):
                continue
            if isinstance(record, dict) and record.get("session_id") == session_id:
                store.delete(key)
                revoked += 1
        return revoked

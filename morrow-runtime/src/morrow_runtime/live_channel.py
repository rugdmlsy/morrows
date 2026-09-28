from __future__ import annotations

import asyncio
import hashlib
import json
import secrets
import threading
import time
import uuid
from collections import deque
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from .session_runtime import (
    PLAN_EXECUTION_LEASE_S as PLAN_EXECUTION_LEASE_S,
)
from .session_runtime import (
    PLAN_MAX_CONTINUATIONS as PLAN_MAX_CONTINUATIONS,
)
from .session_runtime import (
    get_session_runtime_manager,
)

_LIVE_RESOURCE_PATH = Path(__file__).resolve().parent / "ui_static" / "live-workspace.html"
_LIVE_RESOURCE_ALIASES_PATH = (
    Path(__file__).resolve().parent / "ui_static" / "live-workspace-aliases.json"
)
LIVE_RESOURCE_URI = "ui://morrow-runtime/live-workspace.html"
LIVE_RESOURCE_TEMPLATE_URI = "ui://morrow-runtime/live-workspace-{version}.html"


def _versioned_live_resource_uri() -> str:
    try:
        digest = hashlib.sha256(_LIVE_RESOURCE_PATH.read_bytes()).hexdigest()[:16]
    except OSError:
        digest = "unbuilt"
    return LIVE_RESOURCE_TEMPLATE_URI.format(version=digest)


LIVE_RESOURCE_VERSIONED_URI = _versioned_live_resource_uri()


def _compat_live_resource_uris() -> tuple[str, ...]:
    try:
        versions = json.loads(_LIVE_RESOURCE_ALIASES_PATH.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return ()
    if not isinstance(versions, list):
        return ()

    current = LIVE_RESOURCE_VERSIONED_URI
    seen: set[str] = set()
    uris: list[str] = []
    for value in versions[:64]:
        if not isinstance(value, str):
            continue
        version = value.strip().lower()
        if len(version) != 16 or any(char not in "0123456789abcdef" for char in version):
            continue
        uri = LIVE_RESOURCE_TEMPLATE_URI.format(version=version)
        if uri == current or uri in seen:
            continue
        seen.add(uri)
        uris.append(uri)
    return tuple(uris)


LIVE_RESOURCE_COMPAT_URIS = _compat_live_resource_uris()
LIVE_RESOURCE_MIME = "text/html;profile=mcp-app"
LIVE_API_PREFIX = "/api/live"
LIVE_TOKEN_TTL_S = 12 * 60 * 60
LIVE_EVENT_LIMIT = 2_000
LIVE_EVENT_BATCH = 300
LIVE_LONG_POLL_S = 25.0
LIVE_APP_REATTACH_CLAIM_TTL_S = 2 * 60.0


@dataclass(slots=True)
class LiveChannel:
    live_id: str
    subject: str
    scopes: tuple[str, ...]
    token_digest: str
    token_value: str = field(repr=False)
    created_at: float
    expires_at: float
    machine: str = "local"
    cwd: str = "."
    seq: int = 0
    events: deque[dict[str, Any]] = field(
        default_factory=lambda: deque(maxlen=LIVE_EVENT_LIMIT)
    )
    signal: asyncio.Event = field(default_factory=asyncio.Event)
    logical_session_id: str | None = None
    binding_generation: int = 0

    def public_state(self) -> dict[str, Any]:
        session_manager = get_session_runtime_manager()
        session_state = None
        if self.logical_session_id:
            try:
                session_state = session_manager.get(
                    self.logical_session_id, subject=self.subject
                )
            except (PermissionError, ValueError):
                session_state = None
        return {
            "live_id": self.live_id,
            "created_at": self.created_at,
            "expires_at": self.expires_at,
            "seq": self.seq,
            "session_id": self.logical_session_id,
            "session": session_state,
            "plan": session_state.get("plan") if session_state else None,
        }


class LiveChannelManager:
    def __init__(self) -> None:
        self._lock = threading.RLock()
        self._channels: dict[str, LiveChannel] = {}
        self._logical_session_channels: dict[str, str] = {}
        self._recovered_live_ids: dict[str, str] = {}
        self._app_reattach_claims: dict[str, dict[str, float]] = {}
        self._credentials: dict[str, dict[str, Any]] = {}

    @staticmethod
    def _digest(token: str) -> str:
        return hashlib.sha256(token.encode("utf-8")).hexdigest()

    def _issue_credential_locked(
        self,
        channel: LiveChannel,
        *,
        subject: str,
        scopes: tuple[str, ...],
        expires_at: float,
        primary: bool,
    ) -> str:
        token = secrets.token_urlsafe(32)
        digest = self._digest(token)
        if primary:
            self._credentials.pop(channel.token_digest, None)
            channel.token_value = token
            channel.token_digest = digest
        self._credentials[digest] = {
            "live_id": channel.live_id,
            "subject": subject,
            "scopes": scopes,
            "expires_at": expires_at,
        }
        channel.expires_at = max(channel.expires_at, expires_at)
        return token

    def _prune_credentials_locked(self, current: float) -> None:
        self._credentials = {
            digest: credential
            for digest, credential in self._credentials.items()
            if float(credential["expires_at"]) > current
            and str(credential["live_id"]) in self._channels
        }
        expiries_by_live_id: dict[str, list[float]] = {}
        for credential in self._credentials.values():
            expiries_by_live_id.setdefault(str(credential["live_id"]), []).append(
                float(credential["expires_at"])
            )
        for channel in self._channels.values():
            expiries = expiries_by_live_id.get(channel.live_id)
            if expiries:
                channel.expires_at = max(expiries)

    def _set_logical_session_locked(
        self, channel: LiveChannel, logical_session_id: str
    ) -> None:
        previous_session_id = channel.logical_session_id
        if previous_session_id == logical_session_id:
            self._logical_session_channels[logical_session_id] = channel.live_id
            return
        if (
            previous_session_id
            and self._logical_session_channels.get(previous_session_id) == channel.live_id
        ):
            self._logical_session_channels.pop(previous_session_id, None)
        # App recovery may explicitly attach an existing rendered channel to a
        # Session. Preserve its bounded UI event history across that rebind; the
        # durable Activity history itself remains owned by SessionRuntime.
        channel.binding_generation += 1
        channel.logical_session_id = logical_session_id
        self._logical_session_channels[logical_session_id] = channel.live_id

    def _prune_locked(self, now: float | None = None) -> None:
        current = time.time() if now is None else now
        self._prune_credentials_locked(current)
        expired = [
            live_id
            for live_id, channel in self._channels.items()
            if channel.expires_at <= current
            or not any(
                credential["live_id"] == live_id
                for credential in self._credentials.values()
            )
        ]
        for live_id in expired:
            self._channels.pop(live_id, None)
        if expired:
            expired_ids = set(expired)
            self._logical_session_channels = {
                session_id: live_id
                for session_id, live_id in self._logical_session_channels.items()
                if live_id not in expired_ids
            }
            self._recovered_live_ids = {
                stale_id: live_id
                for stale_id, live_id in self._recovered_live_ids.items()
                if live_id not in expired_ids
            }
            self._credentials = {
                digest: credential
                for digest, credential in self._credentials.items()
                if credential["live_id"] not in expired_ids
            }
        self._app_reattach_claims = {
            subject: {
                live_id: deadline
                for live_id, deadline in claims.items()
                if deadline > current and live_id in self._channels
            }
            for subject, claims in self._app_reattach_claims.items()
        }
        self._app_reattach_claims = {
            subject: claims
            for subject, claims in self._app_reattach_claims.items()
            if claims
        }

    def open(
        self,
        *,
        subject: str,
        scopes: tuple[str, ...],
        parent_expires_at: float | None = None,
        live_id: str | None = None,
        logical_session_id: str | None = None,
        app_reattach: bool = False,
        machine: str | None = None,
        cwd: str | None = None,
    ) -> tuple[LiveChannel, str]:
        now = time.time()
        expires_at = now + LIVE_TOKEN_TTL_S
        if parent_expires_at is not None:
            expires_at = min(expires_at, parent_expires_at)
        with self._lock:
            self._prune_locked(now)
            logical_live_id = (
                self._logical_session_channels.get(logical_session_id)
                if logical_session_id
                else None
            )
            requested_live_id = live_id
            recovered_empty_app = False
            resolved_live_id = self._recovered_live_ids.get(live_id or "", live_id or "")
            channel = self._channels.get(resolved_live_id or "")
            if app_reattach and channel is not None and channel.logical_session_id:
                # App reconnect payloads may carry the Session id from the last
                # rendered snapshot. The live channel is authoritative after a
                # model-side Session switch, so a stale app payload must follow it.
                logical_session_id = channel.logical_session_id
                logical_live_id = self._logical_session_channels.get(logical_session_id)
            canonical_channel = self._channels.get(logical_live_id or "")
            if canonical_channel is not None:
                if canonical_channel.subject != subject:
                    raise PermissionError("Logical session workspace belongs to a different principal")
                if (
                    channel is not None
                    and channel is not canonical_channel
                    and channel.logical_session_id == logical_session_id
                ):
                    channel.logical_session_id = None
                    channel.events.clear()
                    channel.binding_generation += 1
                    self._publish_locked(
                        channel,
                        "session.detached",
                        actor="system",
                        data={"session_id": logical_session_id},
                    )
                channel = canonical_channel
            if (
                channel is not None
                and logical_session_id
                and channel.logical_session_id not in {None, logical_session_id}
            ):
                channel = self._channels.get(logical_live_id or "")
            if channel is not None and channel.subject != subject:
                if live_id:
                    raise PermissionError("Live workspace belongs to a different principal")
                channel = None
            if channel is None and app_reattach and not live_id and not logical_session_id:
                claims = self._app_reattach_claims.get(subject, {})
                candidates = [
                    self._channels[claim_live_id]
                    for claim_live_id, deadline in claims.items()
                    if deadline > now and claim_live_id in self._channels
                ]
                if len(candidates) == 1:
                    channel = candidates[0]
                    recovered_empty_app = True
                elif len(candidates) > 1:
                    raise ValueError(
                        "Live workspace reattach is ambiguous; provide live_id or session_id"
                    )
                elif any(existing.subject == subject for existing in self._channels.values()):
                    raise ValueError(
                        "Live workspace reattach has no recoverable context; provide live_id or session_id"
                    )
            if channel is None:
                token = secrets.token_urlsafe(32)
                digest = self._digest(token)
                channel = LiveChannel(
                    live_id=uuid.uuid4().hex,
                    subject=subject,
                    scopes=scopes,
                    token_digest=digest,
                    token_value=token,
                    created_at=now,
                    expires_at=expires_at,
                    machine=machine or "local",
                    cwd=cwd or ".",
                    logical_session_id=logical_session_id,
                )
                self._channels[channel.live_id] = channel
                self._credentials[digest] = {
                    "live_id": channel.live_id,
                    "subject": subject,
                    "scopes": scopes,
                    "expires_at": expires_at,
                }
                if requested_live_id:
                    self._recovered_live_ids[requested_live_id] = channel.live_id
            elif app_reattach:
                token = self._issue_credential_locked(
                    channel,
                    subject=subject,
                    scopes=scopes,
                    expires_at=expires_at,
                    primary=False,
                )
            elif live_id is not None:
                token = channel.token_value
            else:
                # An explicit user/model open is a new authorization boundary.
                token = self._issue_credential_locked(
                    channel,
                    subject=subject,
                    scopes=scopes,
                    expires_at=expires_at,
                    primary=True,
                )
            if not recovered_empty_app:
                if machine:
                    channel.machine = machine
                if cwd:
                    channel.cwd = cwd
            if not app_reattach:
                self._app_reattach_claims.setdefault(subject, {})[channel.live_id] = (
                    now + LIVE_APP_REATTACH_CLAIM_TTL_S
                )
            if logical_session_id:
                self._set_logical_session_locked(channel, logical_session_id)
            channel.subject = subject
            channel.scopes = scopes
            self._prune_credentials_locked(now)
            self._publish_locked(
                channel,
                "channel.opened",
                actor="system",
                data={},
            )
            return channel, token

    def authenticate(self, token: str | None) -> LiveChannel | None:
        context = self.authenticate_context(token)
        return context[0] if context is not None else None

    def authenticate_context(
        self, token: str | None
    ) -> tuple[LiveChannel, str, tuple[str, ...]] | None:
        if not token:
            return None
        digest = self._digest(token)
        now = time.time()
        with self._lock:
            self._prune_locked(now)
            credential = self._credentials.get(digest)
            if credential is None or float(credential["expires_at"]) <= now:
                return None
            channel = self._channels.get(str(credential["live_id"]))
            if channel is None:
                return None
            return (
                channel,
                str(credential["subject"]),
                tuple(str(scope) for scope in credential["scopes"]),
            )
        return None

    def active_for_logical_session(
        self, logical_session_id: str, *, subject: str | None = None
    ) -> LiveChannel | None:
        with self._lock:
            self._prune_locked()
            live_id = self._logical_session_channels.get(logical_session_id)
            channel = self._channels.get(live_id or "")
            if channel is None:
                return None
            if subject is not None and channel.subject != subject:
                return None
            return channel

    def detach_logical_session(self, logical_session_id: str) -> list[LiveChannel]:
        with self._lock:
            self._prune_locked()
            self._logical_session_channels.pop(logical_session_id, None)
            detached: list[LiveChannel] = []
            for channel in self._channels.values():
                if channel.logical_session_id != logical_session_id:
                    continue
                channel.logical_session_id = None
                channel.binding_generation += 1
                detached.append(channel)
                self._publish_locked(
                    channel,
                    "session.detached",
                    actor="system",
                    data={"session_id": logical_session_id},
                )
            return detached

    def by_id(self, live_id: str) -> LiveChannel | None:
        with self._lock:
            self._prune_locked()
            return self._channels.get(live_id)

    def _publish_locked(
        self,
        channel: LiveChannel,
        event_type: str,
        *,
        actor: str,
        data: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        channel.seq += 1
        now = time.time()
        event = {
            "seq": channel.seq,
            "ts": now,
            "type": event_type,
            "actor": actor,
            "data": data or {},
        }
        channel.events.append(event)
        channel.signal.set()
        return event

    def publish_channel(
        self,
        live_id: str,
        event_type: str,
        *,
        actor: str,
        data: dict[str, Any] | None = None,
    ) -> dict[str, Any] | None:
        with self._lock:
            channel = self._channels.get(live_id)
            if channel is None:
                return None
            return self._publish_locked(channel, event_type, actor=actor, data=data)

    def events_since(
        self,
        channel: LiveChannel,
        after: int,
        limit: int = LIVE_EVENT_BATCH,
    ) -> list[dict[str, Any]]:
        with self._lock:
            return [event for event in channel.events if int(event["seq"]) > after][:limit]

    def event_batch(
        self,
        channel: LiveChannel,
        after: int,
        limit: int = LIVE_EVENT_BATCH,
    ) -> dict[str, Any]:
        with self._lock:
            events = [event for event in channel.events if int(event["seq"]) > after][:limit]
            return {
                "events": events,
                "session_id": channel.logical_session_id,
                "binding_generation": channel.binding_generation,
                "seq": channel.seq,
                "cursor": events[-1]["seq"] if events else after,
            }

    def snapshot_batch(
        self, channel: LiveChannel, limit: int = LIVE_EVENT_BATCH
    ) -> dict[str, Any]:
        with self._lock:
            after = max(0, channel.seq - limit)
            return self.event_batch(channel, after, limit)

    def binding_matches(
        self,
        channel: LiveChannel,
        session_id: str | None,
        binding_generation: int,
    ) -> bool:
        with self._lock:
            return (
                channel.logical_session_id == session_id
                and channel.binding_generation == binding_generation
            )

    def binding_state(self, channel: LiveChannel) -> tuple[str | None, int]:
        with self._lock:
            return channel.logical_session_id, channel.binding_generation

    async def wait_events(
        self,
        channel: LiveChannel,
        after: int,
        timeout_s: float = LIVE_LONG_POLL_S,
    ) -> list[dict[str, Any]]:
        events = self.events_since(channel, after)
        if events:
            return events
        channel.signal.clear()
        events = self.events_since(channel, after)
        if events:
            return events
        try:
            await asyncio.wait_for(channel.signal.wait(), timeout=max(0.1, timeout_s))
        except TimeoutError:
            return []
        return self.events_since(channel, after)

    async def wait_event_batch(
        self,
        channel: LiveChannel,
        after: int,
        timeout_s: float = LIVE_LONG_POLL_S,
    ) -> dict[str, Any]:
        batch = self.event_batch(channel, after)
        if batch["events"]:
            return batch
        channel.signal.clear()
        batch = self.event_batch(channel, after)
        if batch["events"]:
            return batch
        try:
            await asyncio.wait_for(channel.signal.wait(), timeout=max(0.1, timeout_s))
        except TimeoutError:
            return self.event_batch(channel, after)
        return self.event_batch(channel, after)


_MANAGER = LiveChannelManager()


def get_live_channel_manager() -> LiveChannelManager:
    return _MANAGER



def live_id_from_claims(claims: dict[str, Any]) -> str | None:
    value = claims.get("live_id")
    return str(value) if value else None

from __future__ import annotations

import asyncio
import contextlib
import hashlib
import json
import os
import secrets
import shlex
import threading
import time
import uuid
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any

from starlette.exceptions import HTTPException
from starlette.requests import Request
from starlette.responses import JSONResponse, Response
from starlette.routing import Route

from . import __version__
from .audit import audit, audit_request_context
from .auth import Principal, current_principal
from .oauth import ALL_OAUTH_SCOPES, issue_access_token, public_base_url
from .settings import get_settings

CLIENT_API_VERSION = "v1"
CLIENT_VERSION = "1.0.0"
CLIENT_REGISTRY_FILE_NAME = "container-clients.json"
CLIENT_REGISTRY_BACKUP_FILE_NAME = "container-clients.json.bak"
CLIENT_ID_PREFIX = "ccs_"
INVITE_PREFIX = "lsmcp_cli_inv_"
RESERVED_TOOLS = frozenset({"lsm.tools_list", "lsm.session_info"})


@dataclass(slots=True)
class ClientInvite:
    code: str
    created_at: int
    expires_at: int
    used: bool = False


@dataclass(slots=True)
class ClientSession:
    session_id: str
    jti: str
    created_at: int
    expires_at: int
    client_version: str
    revoked_at: int | None = None

    def public_dict(self) -> dict[str, Any]:
        return {
            "client_id": self.session_id,
            "created_at": self.created_at,
            "expires_at": self.expires_at,
            "client_version": self.client_version,
            "revoked_at": self.revoked_at,
            "active": self.revoked_at is None and self.expires_at > int(time.time()),
        }


def _private_write(path: Path, data: str) -> None:
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(descriptor, "w", encoding="utf-8") as handle:
        handle.write(data)
        handle.flush()
        os.fsync(handle.fileno())
    with contextlib.suppress(OSError):
        path.chmod(0o600)


def _install_command(install_url: str, invite_code: str) -> str:
    """Build a POSIX shell command whose status reflects the installer download.

    A curl-to-sh pipeline reports the downstream shell's status in POSIX shells,
    so an empty script can turn a DNS or TLS failure into a false success. Download
    to a private temporary file first and let ``set -e`` preserve curl's status.
    An already-active client is reused so replaying the bootstrap command in the
    same live container does not consume the invitation again.
    """
    session_envelope = '{"tool":"lsm.session_info","arguments":{}}'
    script = (
        "set -eu; umask 077; "
        'state_root=${LSM_CLIENT_STATE_DIR:-${XDG_STATE_HOME:-${HOME:?HOME is not set}/.local/state}/lsm-client}; '
        'persistent_dir=${LSM_CLIENT_BIN_DIR:-${HOME}/.local/bin}; '
        "existing=; "
        'if [ -x "$persistent_dir/lsm" ]; then existing=$persistent_dir/lsm; '
        'elif command -v lsm >/dev/null 2>&1; then existing=$(command -v lsm); fi; '
        'if [ -n "$existing" ] && [ -r "$state_root/curl.conf" ] '
        '&& "$existing" "$3" >/dev/null 2>&1; then '
        'echo "lsm is already active at $existing"; exit 0; fi; '
        'installer=$(mktemp "${TMPDIR:-/tmp}/lsm-install.XXXXXX"); '
        # Trap zero runs for success and failure without replacing the original status.
        "trap 'rm -f \"$installer\"' 0; "
        'curl -fsSL "$1" -o "$installer"; '
        'sh "$installer" --invite "$2"'
    )
    return (
        f"sh -c {shlex.quote(script)} sh "
        f"{shlex.quote(install_url)} {shlex.quote(invite_code)} "
        f"{shlex.quote(session_envelope)}"
    )


class ContainerClientManager:
    """Own one-time invites and the persistent revocation registry.

    Invites deliberately stay in memory because they live for only ten minutes.
    Sessions survive restarts, but the registry stores only the JWT identifier and
    metadata; the bearer itself exists solely in the client-side curl config.
    """

    def __init__(self, state_dir: Path | None = None) -> None:
        self.state_dir = Path(state_dir or get_settings().state_dir)
        self.registry_path = self.state_dir / CLIENT_REGISTRY_FILE_NAME
        self.backup_path = self.state_dir / CLIENT_REGISTRY_BACKUP_FILE_NAME
        self.invites: dict[str, ClientInvite] = {}
        self.sessions: dict[str, ClientSession] = {}
        self._active_calls: dict[str, int] = {}
        self._async_lock = asyncio.Lock()
        self._state_lock = threading.RLock()
        with self._state_lock:
            self._load_registry_unlocked()

    @staticmethod
    def _now() -> int:
        return int(time.time())

    def _load_path(self, path: Path) -> dict[str, Any] | None:
        try:
            value = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            return None
        return value if isinstance(value, dict) else None

    def _load_registry_unlocked(self) -> None:
        payload = self._load_path(self.registry_path) or self._load_path(self.backup_path)
        if payload is None:
            self.sessions = {}
            return
        loaded: dict[str, ClientSession] = {}
        for raw in payload.get("sessions", []):
            if not isinstance(raw, dict):
                continue
            try:
                session = ClientSession(
                    session_id=str(raw["session_id"]),
                    jti=str(raw["jti"]),
                    created_at=int(raw["created_at"]),
                    expires_at=int(raw["expires_at"]),
                    client_version=str(raw.get("client_version") or "unknown"),
                    revoked_at=(
                        int(raw["revoked_at"]) if raw.get("revoked_at") is not None else None
                    ),
                )
            except (KeyError, TypeError, ValueError):
                continue
            loaded[session.session_id] = session
        self.sessions = loaded
        self._prune_unlocked()

    def _prune_unlocked(self) -> None:
        now = self._now()
        self.sessions = {
            session_id: session
            for session_id, session in self.sessions.items()
            if session.expires_at > now
        }
        self.invites = {
            code: invite
            for code, invite in self.invites.items()
            if invite.expires_at > now
        }
        self._active_calls = {
            session_id: count
            for session_id, count in self._active_calls.items()
            if session_id in self.sessions and count > 0
        }

    def _save_registry_unlocked(self) -> None:
        self.state_dir.mkdir(parents=True, exist_ok=True)
        with contextlib.suppress(OSError):
            self.state_dir.chmod(0o700)
        payload = json.dumps(
            {
                "version": 1,
                "sessions": [
                    asdict(session)
                    for session in sorted(self.sessions.values(), key=lambda item: item.created_at)
                ],
            },
            indent=2,
            sort_keys=True,
        )
        primary_tmp = self.registry_path.with_name(f".{self.registry_path.name}.{uuid.uuid4().hex}")
        backup_tmp = self.backup_path.with_name(f".{self.backup_path.name}.{uuid.uuid4().hex}")
        try:
            _private_write(primary_tmp, payload)
            _private_write(backup_tmp, payload)
            os.replace(primary_tmp, self.registry_path)
            os.replace(backup_tmp, self.backup_path)
            with contextlib.suppress(OSError):
                self.registry_path.chmod(0o600)
                self.backup_path.chmod(0o600)
        finally:
            primary_tmp.unlink(missing_ok=True)
            backup_tmp.unlink(missing_ok=True)

    async def create_invite(self, *, base_url: str) -> dict[str, Any]:
        settings = get_settings()
        now = self._now()
        ttl_s = settings.container_client_invite_ttl_s
        invite = ClientInvite(
            code=INVITE_PREFIX + secrets.token_urlsafe(24),
            created_at=now,
            expires_at=now + ttl_s,
        )
        async with self._async_lock:
            with self._state_lock:
                self._prune_unlocked()
                pending_count = sum(not item.used for item in self.invites.values())
                if pending_count >= settings.container_client_max_sessions:
                    raise RuntimeError("Too many pending container client invites")
                self.invites[invite.code] = invite
        install_url = base_url.rstrip("/") + "/client/install.sh"
        command = _install_command(install_url, invite.code)
        audit("container_client_invite_created", expires_at=invite.expires_at)
        return {
            "invite": invite.code,
            "created_at": invite.created_at,
            "expires_at": invite.expires_at,
            "ttl_s": ttl_s,
            "install_url": install_url,
            "command": command,
        }

    async def register(
        self, invite_code: str, *, base_url: str, client_version: str
    ) -> tuple[ClientSession, str]:
        settings = get_settings()
        now = self._now()
        async with self._async_lock:
            with self._state_lock:
                self._load_registry_unlocked()
                invite = self.invites.get(invite_code)
                if invite is None:
                    raise ValueError("invalid invite code")
                if invite.used:
                    raise ValueError("invite code has already been used")
                if invite.expires_at <= now:
                    self.invites.pop(invite_code, None)
                    raise ValueError("invite code has expired")
                self._prune_unlocked()
                active_count = sum(
                    item.revoked_at is None and item.expires_at > now
                    for item in self.sessions.values()
                )
                if active_count >= settings.container_client_max_sessions:
                    raise RuntimeError("Container client session limit reached")

                session = ClientSession(
                    session_id=CLIENT_ID_PREFIX + secrets.token_urlsafe(12),
                    jti="ccj_" + secrets.token_urlsafe(24),
                    created_at=now,
                    expires_at=now + settings.container_client_token_ttl_s,
                    client_version=client_version[:64] or "unknown",
                )
                token = issue_access_token(
                    client_id="morrow-runtime-container-client",
                    scope=" ".join(ALL_OAUTH_SCOPES),
                    resource=base_url.rstrip("/"),
                    subject=f"container-client:{session.session_id}",
                    issuer=base_url.rstrip("/"),
                    expires_in_s=settings.container_client_token_ttl_s,
                    additional_claims={
                        "token_kind": "container-client",
                        "jti": session.jti,
                        "client_session_id": session.session_id,
                        "client_version": session.client_version,
                    },
                )
                # Consumption and persistence share one critical section, so even
                # concurrent register requests can never mint two usable bearers.
                invite.used = True
                self.sessions[session.session_id] = session
                self._save_registry_unlocked()
        audit(
            "container_client_registered",
            client_id=session.session_id,
            expires_at=session.expires_at,
            client_version=session.client_version,
        )
        return session, token

    def list_sessions(self) -> dict[str, Any]:
        with self._state_lock:
            self._load_registry_unlocked()
            return {
                "sessions": [
                    session.public_dict()
                    for session in sorted(
                        self.sessions.values(), key=lambda item: item.created_at, reverse=True
                    )
                ]
            }

    def revoke(self, session_id: str) -> dict[str, Any]:
        with self._state_lock:
            self._load_registry_unlocked()
            session = self.sessions.get(session_id)
            if session is None:
                raise KeyError(f"Unknown container client: {session_id}")
            if session.revoked_at is None:
                session.revoked_at = self._now()
                self._save_registry_unlocked()
        audit("container_client_revoked", client_id=session_id)
        return session.public_dict()

    def _session_for_principal_unlocked(self, principal: Principal | None) -> ClientSession:
        if principal is None or principal.claims.get("token_kind") != "container-client":
            raise HTTPException(status_code=403, detail="A container client token is required")
        session_id = str(principal.claims.get("client_session_id") or "")
        jti = str(principal.claims.get("jti") or "")
        session = self.sessions.get(session_id)
        if session is None or not secrets.compare_digest(session.jti, jti):
            raise HTTPException(status_code=401, detail="Container client session is not active")
        if session.revoked_at is not None:
            raise HTTPException(status_code=401, detail="Container client session has been revoked")
        if session.expires_at <= self._now():
            raise HTTPException(status_code=401, detail="Container client session has expired")
        return session

    def begin_call(self, principal: Principal | None) -> ClientSession:
        with self._state_lock:
            self._load_registry_unlocked()
            session = self._session_for_principal_unlocked(principal)
            active = self._active_calls.get(session.session_id, 0)
            if active >= get_settings().container_client_max_concurrent_calls:
                raise HTTPException(
                    status_code=429, detail="Container client concurrent call limit reached"
                )
            self._active_calls[session.session_id] = active + 1
            return session

    def end_call(self, session_id: str) -> None:
        with self._state_lock:
            active = self._active_calls.get(session_id, 0)
            if active <= 1:
                self._active_calls.pop(session_id, None)
            else:
                self._active_calls[session_id] = active - 1


_MANAGER: ContainerClientManager | None = None
_MANAGER_LOCK = threading.Lock()


def container_client_manager() -> ContainerClientManager:
    global _MANAGER
    state_dir = Path(get_settings().state_dir)
    with _MANAGER_LOCK:
        if _MANAGER is None or _MANAGER.state_dir != state_dir:
            _MANAGER = ContainerClientManager(state_dir)
        return _MANAGER


def reset_container_client_manager() -> None:
    global _MANAGER
    with _MANAGER_LOCK:
        _MANAGER = None


CLIENT_SCRIPT = r'''#!/bin/sh
set -eu

state_root=${LSM_CLIENT_STATE_DIR:-${XDG_STATE_HOME:-${HOME:?HOME is not set}/.local/state}/lsm-client}
config=$state_root/curl.conf

if [ ! -r "$config" ]; then
    echo "lsm: no client configuration; install again with a fresh Container Client invite" >&2
    exit 78
fi
if [ "$#" -gt 1 ]; then
    echo "usage: lsm '{\"tool\":\"...\",\"arguments\":{...}}' (or pipe JSON on stdin)" >&2
    exit 64
fi

request_file=$(mktemp "${TMPDIR:-/tmp}/lsm-request.XXXXXX")
response_file=$(mktemp "${TMPDIR:-/tmp}/lsm-response.XXXXXX")
trap 'rm -f "$request_file" "$response_file"' EXIT HUP INT TERM

if [ "$#" -eq 1 ]; then
    printf '%s' "$1" >"$request_file"
else
    cat >"$request_file"
fi
if [ ! -s "$request_file" ]; then
    echo "lsm: expected one JSON envelope on argv or stdin" >&2
    exit 64
fi

http_code=$(curl --config "$config" --data-binary @"$request_file" \
    --output "$response_file" --write-out '%{http_code}') || {
    status=$?
    [ -s "$response_file" ] && cat "$response_file" >&2
    exit "$status"
}
cat "$response_file"
case "$http_code" in
    2??) exit 0 ;;
    *) echo >&2; echo "lsm: server returned HTTP $http_code" >&2; exit 22 ;;
esac
'''


def _curl_config(base_url: str, token: str) -> str:
    call_url = base_url.rstrip("/") + "/client/v1/call"
    return "\n".join(
        (
            f'url = "{call_url}"',
            'request = "POST"',
            'header = "Content-Type: application/json"',
            f'header = "Authorization: Bearer {token}"',
            "silent",
            "show-error",
            "",
        )
    )


def _manifest(base_url: str) -> dict[str, Any]:
    encoded = CLIENT_SCRIPT.encode("utf-8")
    return {
        "api_version": CLIENT_API_VERSION,
        "client_version": CLIENT_VERSION,
        "size": len(encoded),
        "sha256": hashlib.sha256(encoded).hexdigest(),
        "download_url": base_url.rstrip("/") + "/client/v1/lsm",
    }


def _install_script(base_url: str) -> str:
    register_url = base_url.rstrip("/") + "/client/v1/register"
    manifest_url = base_url.rstrip("/") + "/client/v1/manifest.json"
    return rf'''#!/bin/sh
set -eu
umask 077

invite=
if [ "${{1:-}}" = "--invite" ] && [ "$#" -eq 2 ]; then
    invite=$2
else
    echo "usage: install.sh --invite INVITE" >&2
    exit 64
fi
command -v curl >/dev/null 2>&1 || {{ echo "install.sh: curl is required" >&2; exit 69; }}

tmp_dir=$(mktemp -d "${{TMPDIR:-/tmp}}/lsm-install.XXXXXX")
trap 'rm -rf "$tmp_dir"' EXIT HUP INT TERM
curl -fsSL {shlex.quote(manifest_url)} -o "$tmp_dir/manifest.json"
sha256=$(sed -n 's/.*"sha256":"\([0-9a-f]*\)".*/\1/p' "$tmp_dir/manifest.json")
download_url=$(sed -n 's/.*"download_url":"\([^"]*\)".*/\1/p' "$tmp_dir/manifest.json")
[ -n "$sha256" ] && [ -n "$download_url" ] || {{ echo "install.sh: invalid manifest" >&2; exit 65; }}
curl -fsSL "$download_url" -o "$tmp_dir/lsm"
if command -v sha256sum >/dev/null 2>&1; then
    actual=$(sha256sum "$tmp_dir/lsm" | sed 's/ .*//')
elif command -v shasum >/dev/null 2>&1; then
    actual=$(shasum -a 256 "$tmp_dir/lsm" | sed 's/ .*//')
elif command -v openssl >/dev/null 2>&1; then
    actual=$(openssl dgst -sha256 "$tmp_dir/lsm" | sed 's/.*= //')
else
    echo "install.sh: sha256sum, shasum, or openssl is required for verification" >&2
    exit 69
fi
[ "$actual" = "$sha256" ] || {{ echo "install.sh: client checksum mismatch" >&2; exit 65; }}

persistent_dir=${{LSM_CLIENT_BIN_DIR:-${{HOME:?HOME is not set}}/.local/bin}}
mkdir -p "$persistent_dir"
persistent_path=$persistent_dir/lsm
chmod 0755 "$tmp_dir/lsm"
mv "$tmp_dir/lsm" "$persistent_path"

# A stable absolute path survives new shell processes. Add a PATH shortcut only
# when it can be created without replacing an unrelated command.
old_ifs=$IFS
IFS=:
path_command=
for candidate in $PATH; do
    if [ -n "$candidate" ] && [ -d "$candidate" ]; then
        if [ "$candidate" = "$persistent_dir" ]; then
            path_command=lsm
            break
        fi
        if [ -e "$candidate/lsm" ]; then
            break
        fi
        if [ -w "$candidate" ]; then
            ln -s "$persistent_path" "$candidate/lsm"
            path_command=lsm
            break
        fi
    fi
done
IFS=$old_ifs

state_root=${{LSM_CLIENT_STATE_DIR:-${{XDG_STATE_HOME:-${{HOME}}/.local/state}}/lsm-client}}
mkdir -p "$state_root"
payload='{{"invite":"'"$invite"'","client_version":"{CLIENT_VERSION}"}}'
http_code=$(printf '%s' "$payload" | curl -sS -H 'Content-Type: application/json' \
    --data-binary @- -o "$tmp_dir/curl.conf" -w '%{{http_code}}' {shlex.quote(register_url)}) || exit $?
case "$http_code" in
    2??) ;;
    *) cat "$tmp_dir/curl.conf" >&2; echo >&2; echo "install.sh: registration returned HTTP $http_code" >&2; exit 22 ;;
esac
chmod 0600 "$tmp_dir/curl.conf"
mv "$tmp_dir/curl.conf" "$state_root/curl.conf"
chmod 0600 "$state_root/curl.conf"

echo "Installed persistent lsm at $persistent_path"
if [ "$path_command" = lsm ]; then
    echo "Use in this and later shells: lsm '{{\"tool\":\"lsm.session_info\",\"arguments\":{{}}}}'"
else
    echo "Use in this and later shells: $persistent_path '{{\"tool\":\"lsm.session_info\",\"arguments\":{{}}}}'"
    echo "Optional for this shell: export PATH=\"$persistent_dir:\$PATH\""
fi
'''


def _json_error(message: str, status_code: int, error: str) -> JSONResponse:
    return JSONResponse(
        {"ok": False, "error": error, "message": message},
        status_code=status_code,
        headers={"Cache-Control": "no-store"},
    )


def _content_dict(item: Any) -> dict[str, Any]:
    if hasattr(item, "model_dump"):
        value = item.model_dump(mode="json", by_alias=True, exclude_none=True)
    elif isinstance(item, dict):
        value = dict(item)
    else:
        value = {"type": "text", "text": str(item)}
    return value if isinstance(value, dict) else {"type": "text", "text": str(value)}


def _tool_is_error(structured: Any, content: list[dict[str, Any]]) -> bool:
    if isinstance(structured, dict) and structured.get("ok") is False:
        return True
    for item in content:
        if item.get("type") != "text" or not isinstance(item.get("text"), str):
            continue
        with contextlib.suppress(json.JSONDecodeError):
            parsed = json.loads(item["text"])
            if isinstance(parsed, dict) and parsed.get("ok") is False:
                return True
    return False


def _annotation_dict(value: Any) -> dict[str, Any]:
    if value is None:
        return {}
    if hasattr(value, "model_dump"):
        result = value.model_dump(mode="json", by_alias=True, exclude_none=True)
        return result if isinstance(result, dict) else {}
    return dict(value) if isinstance(value, dict) else {}


def _tools_list(mcp: Any) -> dict[str, Any]:
    tools = []
    for name, tool in sorted(mcp._tool_manager._tools.items()):  # noqa: SLF001
        tools.append(
            {
                "name": name,
                "description": tool.description or "",
                "inputSchema": dict(tool.parameters or {}),
                "annotations": _annotation_dict(tool.annotations),
            }
        )
    return {"tools": tools}


async def client_install(request: Request) -> Response:
    return Response(
        _install_script(public_base_url(request)),
        media_type="text/x-shellscript",
        headers={"Cache-Control": "no-store"},
    )


async def client_manifest(request: Request) -> Response:
    return JSONResponse(
        _manifest(public_base_url(request)),
        headers={"Cache-Control": "no-store"},
    )


async def client_download(request: Request) -> Response:  # noqa: ARG001
    return Response(
        CLIENT_SCRIPT,
        media_type="text/x-shellscript",
        headers={"Cache-Control": "public, max-age=300"},
    )


async def client_register(request: Request) -> Response:
    try:
        payload = await request.json()
    except (json.JSONDecodeError, UnicodeDecodeError):
        return _json_error("Request body must be valid JSON", 400, "invalid_json")
    if not isinstance(payload, dict) or set(payload) - {"invite", "client_version"}:
        return _json_error("Registration envelope has unknown fields", 422, "invalid_envelope")
    invite = payload.get("invite")
    version = payload.get("client_version", "unknown")
    if not isinstance(invite, str) or not invite.startswith(INVITE_PREFIX):
        return _json_error("A valid invite is required", 422, "invalid_invite")
    if not isinstance(version, str):
        return _json_error("client_version must be a string", 422, "invalid_envelope")
    try:
        _, token = await container_client_manager().register(
            invite, base_url=public_base_url(request), client_version=version
        )
    except ValueError as exc:
        return _json_error(str(exc), 410 if "expired" in str(exc) else 409, "invalid_invite")
    except RuntimeError as exc:
        return _json_error(str(exc), 429, "session_limit")
    return Response(
        _curl_config(public_base_url(request), token),
        media_type="text/plain",
        headers={"Cache-Control": "no-store", "Pragma": "no-cache"},
    )


def _validate_call_envelope(payload: Any) -> tuple[str, dict[str, Any]]:
    if not isinstance(payload, dict):
        raise HTTPException(status_code=422, detail="Call envelope must be a JSON object")
    if set(payload) != {"tool", "arguments"}:
        raise HTTPException(
            status_code=422,
            detail="Call envelope must contain only tool and arguments",
        )
    tool = payload.get("tool")
    arguments = payload.get("arguments")
    if not isinstance(tool, str) or not tool.strip():
        raise HTTPException(status_code=422, detail="tool must be a non-empty string")
    if not isinstance(arguments, dict):
        raise HTTPException(status_code=422, detail="arguments must be an object")
    return tool, arguments


async def client_call(request: Request) -> Response:
    request_id = "cc_" + uuid.uuid4().hex
    manager = container_client_manager()
    try:
        session = manager.begin_call(current_principal())
    except HTTPException as exc:
        return _json_error(str(exc.detail), exc.status_code, "client_auth_failed")
    try:
        try:
            payload = await request.json()
        except (json.JSONDecodeError, UnicodeDecodeError):
            return _json_error("Request body must be valid JSON", 400, "invalid_json")
        try:
            tool_name, arguments = _validate_call_envelope(payload)
        except HTTPException as exc:
            return _json_error(str(exc.detail), exc.status_code, "invalid_envelope")

        mcp = request.app.state.container_client_mcp
        if tool_name not in RESERVED_TOOLS and tool_name not in mcp._tool_manager._tools:  # noqa: SLF001
            return _json_error(f"Unknown tool: {tool_name}", 404, "unknown_tool")
        machine = arguments.get("machine")
        audit_fields = {
            "source": "container_client",
            "client_id": session.session_id,
            "request_id": request_id,
        }
        if isinstance(machine, str) and machine:
            audit_fields["machine"] = machine
        with audit_request_context(**audit_fields):
            audit("container_client_call_start", tool=tool_name)
            if tool_name == "lsm.tools_list":
                if arguments:
                    return _json_error(
                        "lsm.tools_list does not accept arguments", 422, "invalid_arguments"
                    )
                structured: Any = _tools_list(mcp)
                content: list[dict[str, Any]] = []
            elif tool_name == "lsm.session_info":
                if arguments:
                    return _json_error(
                        "lsm.session_info does not accept arguments", 422, "invalid_arguments"
                    )
                structured = {
                    "client_id": session.session_id,
                    "server_version": __version__,
                    "client_version": session.client_version,
                    "api_version": CLIENT_API_VERSION,
                    "expires_at": session.expires_at,
                }
                content = []
            else:
                try:
                    raw_content, structured = await mcp.call_tool(tool_name, arguments)
                    content = [_content_dict(item) for item in raw_content]
                except Exception as exc:  # FastMCP validation/execution errors are tool results here.
                    structured = {
                        "ok": False,
                        "error": type(exc).__name__,
                        "message": str(exc) or type(exc).__name__,
                    }
                    content = [{"type": "text", "text": structured["message"]}]
            is_error = _tool_is_error(structured, content)
            response = {
                "ok": not is_error,
                "request_id": request_id,
                "tool": tool_name,
                "is_error": is_error,
                "structured_content": structured if structured is not None else {},
                "content": content,
            }
            audit("container_client_call_end", tool=tool_name, ok=not is_error)
            return JSONResponse(response, headers={"Cache-Control": "no-store"})
    finally:
        manager.end_call(session.session_id)


def container_client_routes(mcp: Any) -> list[Any]:
    async def call_route(request: Request) -> Response:
        request.app.state.container_client_mcp = mcp
        return await client_call(request)

    return [
        Route("/client/install.sh", client_install, methods=["GET"]),
        Route("/client/v1/manifest.json", client_manifest, methods=["GET"]),
        Route("/client/v1/lsm", client_download, methods=["GET"]),
        Route("/client/v1/register", client_register, methods=["POST"]),
        Route("/client/v1/call", call_route, methods=["POST"]),
    ]

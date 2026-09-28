from __future__ import annotations

import contextlib
import hashlib
import json
import secrets
import socket
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any

from .fs_ops import resolve_path
from .transfer_ops import (
    transfer_abort_write,
    transfer_begin_write,
    transfer_finish_write,
    transfer_mark_complete_write,
)

_RECEIVER_PREFIX = "/morrow-runtime-transfer/"
_RECEIVER_CHUNK_BYTES = 1024 * 1024
_RECEIVERS: dict[str, tuple[ThreadingHTTPServer, str, str]] = {}
_RECEIVER_LOCK = threading.RLock()


def _json_response(handler: BaseHTTPRequestHandler, status: int, payload: dict[str, Any]) -> None:
    body = json.dumps(payload, separators=(",", ":")).encode("utf-8")
    handler.send_response(status)
    handler.send_header("Content-Type", "application/json")
    handler.send_header("Content-Length", str(len(body)))
    handler.send_header("Cache-Control", "no-store")
    handler.end_headers()
    handler.wfile.write(body)


def open_peer_receiver(
    *,
    path: str,
    overwrite: bool,
    expected_bytes: int,
    expected_sha256: str,
    bind_host: str = "0.0.0.0",
    port: int = 0,
    advertise_host: str | None = None,
    timeout_s: int = 3600,
) -> dict[str, Any]:
    expected_size = int(expected_bytes)
    if expected_size < 0:
        raise ValueError("expected_bytes must be >= 0")
    digest = str(expected_sha256 or "").lower()
    if len(digest) != 64 or any(ch not in "0123456789abcdef" for ch in digest):
        raise ValueError("expected_sha256 must be a SHA-256 hex digest")

    begin = transfer_begin_write(path, overwrite, expected_size)
    transfer_id = str(begin["transfer_id"])
    temporary = resolve_path(str(begin["temp_path"]), follow_final_symlink=False)
    token = secrets.token_urlsafe(32)

    class Handler(BaseHTTPRequestHandler):
        server_version = "morrow-runtime-transfer"
        sys_version = ""

        def log_message(self, format: str, *args: Any) -> None:  # noqa: A002
            del format, args

        def do_PUT(self) -> None:  # noqa: N802
            if self.path != _RECEIVER_PREFIX + token:
                _json_response(self, 404, {"ok": False, "error": "not_found"})
                return
            raw_length = self.headers.get("Content-Length")
            try:
                content_length = int(raw_length or "-1")
            except ValueError:
                content_length = -1
            if content_length != expected_size:
                _json_response(
                    self,
                    400,
                    {
                        "ok": False,
                        "error": "size_mismatch",
                        "expected_bytes": expected_size,
                    },
                )
                return

            received = 0
            hasher = hashlib.sha256()
            try:
                with temporary.open("r+b") as handle:
                    while received < expected_size:
                        chunk = self.rfile.read(min(_RECEIVER_CHUNK_BYTES, expected_size - received))
                        if not chunk:
                            raise ValueError(
                                f"upload ended at {received} bytes, expected {expected_size}"
                            )
                        handle.write(chunk)
                        hasher.update(chunk)
                        received += len(chunk)
                    handle.flush()
                if hasher.hexdigest() != digest:
                    raise ValueError("file sha256 mismatch")
                transfer_mark_complete_write(path, transfer_id)
                finish = transfer_finish_write(path, transfer_id, expected_size, digest)
                _json_response(
                    self,
                    200,
                    {
                        "ok": True,
                        "data": {
                            "path": finish["path"],
                            "bytes": finish["bytes"],
                            "sha256": finish["sha256"],
                        },
                    },
                )
            except Exception as exc:
                with contextlib.suppress(Exception):
                    transfer_abort_write(path, transfer_id)
                _json_response(
                    self,
                    400,
                    {"ok": False, "error": type(exc).__name__, "message": str(exc)},
                )
            finally:
                threading.Thread(target=self.server.shutdown, daemon=True).start()

    try:
        server = ThreadingHTTPServer((bind_host, int(port)), Handler)
    except Exception:
        with contextlib.suppress(Exception):
            transfer_abort_write(path, transfer_id)
        raise
    server.daemon_threads = True
    actual_port = int(server.server_address[1])
    advertised = advertise_host or socket.getfqdn() or socket.gethostname()
    if ":" in advertised and not advertised.startswith("["):
        advertised = f"[{advertised}]"
    url = f"http://{advertised}:{actual_port}{_RECEIVER_PREFIX}{token}"

    def serve() -> None:
        try:
            server.serve_forever(poll_interval=0.2)
        finally:
            server.server_close()
            with _RECEIVER_LOCK:
                _RECEIVERS.pop(token, None)

    def expire() -> None:
        thread.join(timeout=max(1, int(timeout_s)))
        if thread.is_alive():
            server.shutdown()
            with contextlib.suppress(Exception):
                transfer_abort_write(path, transfer_id)

    with _RECEIVER_LOCK:
        _RECEIVERS[token] = (server, path, transfer_id)
    thread = threading.Thread(target=serve, name=f"lsm-peer-transfer-{token[:8]}", daemon=True)
    thread.start()
    threading.Thread(target=expire, name=f"lsm-peer-expiry-{token[:8]}", daemon=True).start()
    return {
        "receiver_id": token,
        "url": url,
        "path": path,
        "expected_bytes": expected_size,
        "expires_in_s": max(1, int(timeout_s)),
    }


def close_peer_receiver(receiver_id: str) -> dict[str, Any]:
    with _RECEIVER_LOCK:
        state = _RECEIVERS.pop(receiver_id, None)
    if state is None:
        return {"receiver_id": receiver_id, "closed": False}
    server, path, transfer_id = state
    server.shutdown()
    server.server_close()
    with contextlib.suppress(Exception):
        transfer_abort_write(path, transfer_id)
    return {"receiver_id": receiver_id, "closed": True}

"""Logical Session ownership propagated through one MCP execution call."""

from __future__ import annotations

from collections.abc import Iterator
from contextlib import contextmanager
from contextvars import ContextVar

_SESSION: ContextVar[str | None] = ContextVar("lsm_execution_session", default=None)
_MACHINE: ContextVar[str] = ContextVar("lsm_execution_machine", default="local")


def current_execution_session() -> str | None:
    return _SESSION.get()


def current_execution_machine() -> str:
    return _MACHINE.get()


@contextmanager
def execution_session(session_id: str | None) -> Iterator[None]:
    token = _SESSION.set(session_id)
    try:
        yield
    finally:
        _SESSION.reset(token)


@contextmanager
def execution_machine(machine: str) -> Iterator[None]:
    token = _MACHINE.set(machine)
    try:
        yield
    finally:
        _MACHINE.reset(token)

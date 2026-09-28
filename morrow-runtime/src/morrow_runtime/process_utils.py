from __future__ import annotations

import subprocess
import sys
from typing import Any


def managed_process_creationflags() -> int:
    """Return Windows flags for LSM-managed subprocesses that never need a desktop console."""
    if sys.platform != "win32":
        return 0
    return int(getattr(subprocess, "CREATE_NO_WINDOW", 0))


def managed_process_kwargs() -> dict[str, Any]:
    """Keyword arguments for subprocess APIs, preserving non-Windows call signatures."""
    flags = managed_process_creationflags()
    return {"creationflags": flags} if flags else {}

from __future__ import annotations

import contextlib
import ctypes
import os
import platform
import re
import shutil
import subprocess
import threading
from functools import lru_cache
from pathlib import Path
from typing import Any

_CPU_SAMPLE_LOCK = threading.Lock()
_CPU_SAMPLE: tuple[int, int] | None = None


def _command_output(command: list[str], timeout_s: float = 2.0) -> str:
    try:
        result = subprocess.run(
            command,
            check=False,
            capture_output=True,
            text=True,
            timeout=timeout_s,
        )
    except (OSError, subprocess.TimeoutExpired):
        return ""
    return result.stdout.strip() if result.returncode == 0 else ""


def _read_linux_cpu_times() -> tuple[int, int] | None:
    try:
        fields = Path("/proc/stat").read_text(encoding="utf-8").splitlines()[0].split()
        values = [int(value) for value in fields[1:]]
    except (OSError, ValueError, IndexError):
        return None
    if len(values) < 4:
        return None
    idle = values[3] + (values[4] if len(values) > 4 else 0)
    return sum(values), idle


def _read_linux_memory() -> tuple[int, int] | None:
    try:
        rows: dict[str, int] = {}
        for line in Path("/proc/meminfo").read_text(encoding="utf-8").splitlines():
            key, value = line.split(":", 1)
            rows[key] = int(value.strip().split()[0]) * 1024
        total = rows["MemTotal"]
        available = rows.get("MemAvailable", rows.get("MemFree", 0))
    except (OSError, ValueError, KeyError):
        return None
    return total, max(0, total - available)


def _read_macos_memory() -> tuple[int, int] | None:
    total_raw = _command_output(["/usr/sbin/sysctl", "-n", "hw.memsize"])
    vm_stat = _command_output(["/usr/bin/vm_stat"])
    if not total_raw or not vm_stat:
        return None
    try:
        total = int(total_raw)
        page_match = re.search(r"page size of (\d+) bytes", vm_stat)
        page_size = int(page_match.group(1)) if page_match else 4096
        pages: dict[str, int] = {}
        for line in vm_stat.splitlines()[1:]:
            if ":" not in line:
                continue
            key, value = line.split(":", 1)
            pages[key.strip()] = int(value.strip().rstrip("."))
        reclaimable = sum(
            pages.get(key, 0)
            for key in ("Pages free", "Pages inactive", "Pages speculative")
        ) * page_size
    except (ValueError, AttributeError):
        return None
    return total, max(0, min(total, total - reclaimable))


def _read_windows_memory() -> tuple[int, int] | None:
    if os.name != "nt":
        return None

    class MemoryStatus(ctypes.Structure):
        _fields_ = [
            ("length", ctypes.c_ulong),
            ("memory_load", ctypes.c_ulong),
            ("total_phys", ctypes.c_ulonglong),
            ("avail_phys", ctypes.c_ulonglong),
            ("total_page_file", ctypes.c_ulonglong),
            ("avail_page_file", ctypes.c_ulonglong),
            ("total_virtual", ctypes.c_ulonglong),
            ("avail_virtual", ctypes.c_ulonglong),
            ("avail_extended_virtual", ctypes.c_ulonglong),
        ]

    status = MemoryStatus()
    status.length = ctypes.sizeof(MemoryStatus)
    try:
        ok = ctypes.windll.kernel32.GlobalMemoryStatusEx(ctypes.byref(status))
    except (AttributeError, OSError):
        return None
    if not ok:
        return None
    return int(status.total_phys), max(0, int(status.total_phys - status.avail_phys))


def _memory_snapshot() -> tuple[int, int] | None:
    if sys_platform := platform.system().lower():
        if sys_platform == "linux":
            return _read_linux_memory()
        if sys_platform == "darwin":
            return _read_macos_memory()
        if sys_platform == "windows":
            return _read_windows_memory()
    return None


def _cpu_percent() -> tuple[float | None, float | None]:
    global _CPU_SAMPLE

    load_1m: float | None = None
    with contextlib.suppress(OSError, AttributeError):
        load_1m = round(float(os.getloadavg()[0]), 2)

    cpu_count = max(1, os.cpu_count() or 1)
    cpu_times = _read_linux_cpu_times()
    cpu_percent: float | None = None
    if cpu_times is not None:
        with _CPU_SAMPLE_LOCK:
            if _CPU_SAMPLE is not None:
                total_delta = cpu_times[0] - _CPU_SAMPLE[0]
                idle_delta = cpu_times[1] - _CPU_SAMPLE[1]
                if total_delta > 0:
                    cpu_percent = round(
                        max(0.0, min(100.0, (total_delta - idle_delta) * 100.0 / total_delta)),
                        1,
                    )
            _CPU_SAMPLE = cpu_times
    if cpu_percent is None and load_1m is not None:
        cpu_percent = round(max(0.0, min(100.0, load_1m * 100.0 / cpu_count)), 1)
    return cpu_percent, load_1m


def _percent(used: int | None, total: int | None) -> float | None:
    if used is None or total is None or total <= 0:
        return None
    return round(max(0.0, min(100.0, used * 100.0 / total)), 1)


def _cpu_model() -> str | None:
    system = platform.system().lower()
    if system == "linux":
        try:
            for line in Path("/proc/cpuinfo").read_text(encoding="utf-8").splitlines():
                if line.lower().startswith(("model name", "hardware")):
                    value = line.split(":", 1)[1].strip()
                    if value:
                        return value
        except (OSError, IndexError):
            pass
    elif system == "darwin":
        value = _command_output(["/usr/sbin/sysctl", "-n", "machdep.cpu.brand_string"])
        if value:
            return value
        value = _command_output(["/usr/sbin/sysctl", "-n", "hw.model"])
        if value:
            return value
    elif system == "windows":
        value = os.getenv("PROCESSOR_IDENTIFIER")
        if value:
            return value
    return platform.processor() or None


def _gpu_names() -> list[str]:
    nvidia_smi = shutil.which("nvidia-smi")
    if nvidia_smi:
        output = _command_output(
            [nvidia_smi, "--query-gpu=name", "--format=csv,noheader,nounits"]
        )
        names = [line.strip() for line in output.splitlines() if line.strip()]
        if names:
            return list(dict.fromkeys(names))

    system = platform.system().lower()
    if system == "darwin":
        output = _command_output(["/usr/sbin/system_profiler", "SPDisplaysDataType"], timeout_s=5)
        names = [
            line.split(":", 1)[1].strip()
            for line in output.splitlines()
            if line.strip().startswith("Chipset Model:") and ":" in line
        ]
        return list(dict.fromkeys(name for name in names if name))
    if system == "linux":
        lspci = shutil.which("lspci")
        if lspci:
            output = _command_output([lspci])
            names = []
            for line in output.splitlines():
                lowered = line.lower()
                if "vga compatible controller:" in lowered or "3d controller:" in lowered:
                    names.append(line.split(":", 2)[-1].strip())
            return list(dict.fromkeys(name for name in names if name))
    return []


@lru_cache(maxsize=1)
def machine_hardware_info() -> dict[str, Any]:
    memory = _memory_snapshot()
    return {
        "system": platform.system(),
        "release": platform.release(),
        "architecture": platform.machine(),
        "cpu_count": max(1, os.cpu_count() or 1),
        "cpu_model": _cpu_model(),
        "memory_total_bytes": memory[0] if memory else None,
        "gpus": _gpu_names(),
    }


def machine_resource_snapshot(workdir: str | Path) -> dict[str, Any]:
    cpu_percent, load_1m = _cpu_percent()
    memory = _memory_snapshot()
    try:
        disk = shutil.disk_usage(workdir)
    except OSError:
        disk = None
    memory_total = memory[0] if memory else machine_hardware_info().get("memory_total_bytes")
    memory_used = memory[1] if memory else None
    return {
        "cpu_percent": cpu_percent,
        "load_1m": load_1m,
        "memory_percent": _percent(memory_used, memory_total),
        "memory_used_bytes": memory_used,
        "memory_total_bytes": memory_total,
        "disk_percent": _percent(disk.used, disk.total) if disk else None,
        "disk_used_bytes": disk.used if disk else None,
        "disk_total_bytes": disk.total if disk else None,
    }

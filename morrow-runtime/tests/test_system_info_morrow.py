from __future__ import annotations

from types import SimpleNamespace

import morrow_runtime.system_info as info


def test_command_output_handles_success_failure_and_spawn_errors(monkeypatch):
    monkeypatch.setattr(
        info.subprocess,
        "run",
        lambda *args, **kwargs: SimpleNamespace(returncode=0, stdout=" value \n"),
    )
    assert info._command_output(["probe"]) == "value"  # noqa: SLF001

    monkeypatch.setattr(
        info.subprocess,
        "run",
        lambda *args, **kwargs: SimpleNamespace(returncode=1, stdout="ignored"),
    )
    assert info._command_output(["probe"]) == ""  # noqa: SLF001

    def fail(*args, **kwargs):
        raise OSError("missing")

    monkeypatch.setattr(info.subprocess, "run", fail)
    assert info._command_output(["probe"]) == ""  # noqa: SLF001


def test_proc_parsers_cover_valid_and_invalid_inputs(monkeypatch):
    original_read_text = info.Path.read_text

    def read_text(path, *args, **kwargs):
        normalized = str(path).replace("\\", "/")
        if normalized.endswith("/proc/stat"):
            return "cpu  1 2 3 4 5\n"
        if normalized.endswith("/proc/meminfo"):
            return "MemTotal: 100 kB\nMemAvailable: 40 kB\n"
        return original_read_text(path, *args, **kwargs)

    monkeypatch.setattr(info.Path, "read_text", read_text)
    assert info._read_linux_cpu_times() == (15, 9)  # noqa: SLF001
    assert info._read_linux_memory() == (102400, 61440)  # noqa: SLF001

    monkeypatch.setattr(info.Path, "read_text", lambda *args, **kwargs: "bad")
    assert info._read_linux_cpu_times() is None  # noqa: SLF001
    assert info._read_linux_memory() is None  # noqa: SLF001


def test_macos_and_windows_memory_readers(monkeypatch):
    outputs = iter(
        [
            "16384",
            "Mach Virtual Memory Statistics: (page size of 4096 bytes)\n"
            "Pages free: 1.\nPages inactive: 1.\nPages speculative: 1.\n",
        ]
    )
    monkeypatch.setattr(info, "_command_output", lambda *args, **kwargs: next(outputs))
    assert info._read_macos_memory() == (16384, 4096)  # noqa: SLF001

    monkeypatch.setattr(info, "_command_output", lambda *args, **kwargs: "")
    assert info._read_macos_memory() is None  # noqa: SLF001

    monkeypatch.setattr(info.os, "name", "nt")

    def global_memory_status(pointer):
        pointer._obj.total_phys = 1000
        pointer._obj.avail_phys = 250
        return 1

    fake_kernel = SimpleNamespace(GlobalMemoryStatusEx=global_memory_status)
    monkeypatch.setattr(info.ctypes, "windll", SimpleNamespace(kernel32=fake_kernel), raising=False)
    assert info._read_windows_memory() == (1000, 750)  # noqa: SLF001
    fake_kernel.GlobalMemoryStatusEx = lambda pointer: 0
    assert info._read_windows_memory() is None  # noqa: SLF001


def test_platform_memory_dispatch_and_cpu_sampling(monkeypatch):
    monkeypatch.setattr(info.platform, "system", lambda: "Linux")
    monkeypatch.setattr(info, "_read_linux_memory", lambda: (10, 4))
    assert info._memory_snapshot() == (10, 4)  # noqa: SLF001
    monkeypatch.setattr(info.platform, "system", lambda: "Darwin")
    monkeypatch.setattr(info, "_read_macos_memory", lambda: (20, 5))
    assert info._memory_snapshot() == (20, 5)  # noqa: SLF001
    monkeypatch.setattr(info.platform, "system", lambda: "Windows")
    monkeypatch.setattr(info, "_read_windows_memory", lambda: (30, 6))
    assert info._memory_snapshot() == (30, 6)  # noqa: SLF001
    monkeypatch.setattr(info.platform, "system", lambda: "Other")
    assert info._memory_snapshot() is None  # noqa: SLF001

    samples = iter([(100, 50), (200, 75)])
    monkeypatch.setattr(info, "_read_linux_cpu_times", lambda: next(samples))
    monkeypatch.setattr(info.os, "getloadavg", lambda: (2.0, 0.0, 0.0), raising=False)
    monkeypatch.setattr(info.os, "cpu_count", lambda: 4)
    info._CPU_SAMPLE = None  # noqa: SLF001
    assert info._cpu_percent() == (50.0, 2.0)  # noqa: SLF001
    assert info._cpu_percent() == (75.0, 2.0)  # noqa: SLF001
    assert info._percent(None, 10) is None  # noqa: SLF001
    assert info._percent(25, 100) == 25.0  # noqa: SLF001
    assert info._percent(200, 100) == 100.0  # noqa: SLF001


def test_cpu_and_gpu_names_across_platforms(monkeypatch):
    monkeypatch.setattr(info.platform, "system", lambda: "Darwin")
    values = iter(["Apple M4 Max", ""])
    monkeypatch.setattr(info, "_command_output", lambda *args, **kwargs: next(values))
    assert info._cpu_model() == "Apple M4 Max"  # noqa: SLF001

    monkeypatch.setattr(info.shutil, "which", lambda name: "/usr/bin/nvidia-smi")
    monkeypatch.setattr(info, "_command_output", lambda *args, **kwargs: "GPU A\nGPU A\nGPU B")
    assert info._gpu_names() == ["GPU A", "GPU B"]  # noqa: SLF001

    monkeypatch.setattr(info.shutil, "which", lambda name: None)
    monkeypatch.setattr(info.platform, "system", lambda: "Darwin")
    monkeypatch.setattr(
        info,
        "_command_output",
        lambda *args, **kwargs: "Chipset Model: Apple M4\nChipset Model: Apple M4",
    )
    assert info._gpu_names() == ["Apple M4"]  # noqa: SLF001

    monkeypatch.setattr(info.platform, "system", lambda: "Linux")
    monkeypatch.setattr(
        info.shutil,
        "which",
        lambda name: "/usr/bin/lspci" if name == "lspci" else None,
    )
    monkeypatch.setattr(
        info,
        "_command_output",
        lambda *args, **kwargs: "00:02.0 VGA compatible controller: Example GPU\n",
    )
    assert info._gpu_names() == ["Example GPU"]  # noqa: SLF001


def test_hardware_and_resource_snapshots(monkeypatch, tmp_path):
    info.machine_hardware_info.cache_clear()
    monkeypatch.setattr(info, "_memory_snapshot", lambda: (1000, 250))
    monkeypatch.setattr(info, "_cpu_model", lambda: "CPU")
    monkeypatch.setattr(info, "_gpu_names", lambda: ["GPU"])
    monkeypatch.setattr(info.os, "cpu_count", lambda: 8)
    hardware = info.machine_hardware_info()
    assert hardware["cpu_count"] == 8
    assert hardware["memory_total_bytes"] == 1000
    assert hardware["gpus"] == ["GPU"]

    monkeypatch.setattr(info, "_cpu_percent", lambda: (12.5, 0.5))
    monkeypatch.setattr(
        info.shutil,
        "disk_usage",
        lambda path: SimpleNamespace(total=2000, used=500, free=1500),
    )
    snapshot = info.machine_resource_snapshot(tmp_path)
    assert snapshot == {
        "cpu_percent": 12.5,
        "load_1m": 0.5,
        "memory_percent": 25.0,
        "memory_used_bytes": 250,
        "memory_total_bytes": 1000,
        "disk_percent": 25.0,
        "disk_used_bytes": 500,
        "disk_total_bytes": 2000,
    }

    monkeypatch.setattr(info.shutil, "disk_usage", lambda path: (_ for _ in ()).throw(OSError()))
    without_disk = info.machine_resource_snapshot(tmp_path)
    assert without_disk["disk_percent"] is None
    assert without_disk["disk_total_bytes"] is None


def test_cpu_model_fallbacks_and_empty_gpu_probe(monkeypatch):
    original_read_text = info.Path.read_text

    def read_cpuinfo(path, *args, **kwargs):
        if str(path).replace("\\", "/").endswith("/proc/cpuinfo"):
            return "processor: 0\nmodel name: Example CPU\n"
        return original_read_text(path, *args, **kwargs)

    monkeypatch.setattr(info.platform, "system", lambda: "Linux")
    monkeypatch.setattr(info.Path, "read_text", read_cpuinfo)
    assert info._cpu_model() == "Example CPU"  # noqa: SLF001

    monkeypatch.setattr(info.platform, "system", lambda: "Darwin")
    values = iter(["", "MacModel1,1"])
    monkeypatch.setattr(info, "_command_output", lambda *args, **kwargs: next(values))
    assert info._cpu_model() == "MacModel1,1"  # noqa: SLF001

    monkeypatch.setattr(info.platform, "system", lambda: "Windows")
    monkeypatch.setenv("PROCESSOR_IDENTIFIER", "Windows CPU")
    assert info._cpu_model() == "Windows CPU"  # noqa: SLF001

    monkeypatch.setattr(info.shutil, "which", lambda name: None)
    monkeypatch.setattr(info.platform, "system", lambda: "Other")
    assert info._gpu_names() == []  # noqa: SLF001

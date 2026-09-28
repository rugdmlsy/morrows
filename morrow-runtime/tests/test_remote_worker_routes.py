from __future__ import annotations

import hashlib
import io
import json
import shutil
import subprocess
import sys
import tarfile
from types import SimpleNamespace

import pytest

from morrow_runtime import remote_worker_routes as routes
from morrow_runtime.settings import get_settings


@pytest.mark.asyncio
async def test_worker_bundle_and_manifest_are_stable(tmp_path, monkeypatch):
    monkeypatch.setenv("LOCAL_SHELL_MCP_WORKSPACE_ROOT", str(tmp_path))
    monkeypatch.setenv("LOCAL_SHELL_MCP_PUBLIC_BASE_URL", "https://example.test")
    get_settings.cache_clear()
    routes.worker_bundle_bytes.cache_clear()
    first = routes.worker_bundle_bytes()
    routes.worker_bundle_bytes.cache_clear()
    second = routes.worker_bundle_bytes()
    assert first == second

    response = await routes.worker_manifest(None)  # type: ignore[arg-type]
    data = json.loads(response.body)
    assert data["sha256"] == hashlib.sha256(first).hexdigest()
    assert data["url"] == (
        "https://example.test/remote/worker-bundle.tgz?sha256=" + data["sha256"]
    )
    assert response.headers["cache-control"] == "no-store"

    public_manifest = await routes.worker_bundle(SimpleNamespace(query_params={"manifest": "1"}))
    assert json.loads(public_manifest.body) == data
    assert public_manifest.headers["cache-control"] == "no-store"
    bundle = await routes.worker_bundle(None)  # type: ignore[arg-type]
    assert bundle.body == first
    assert bundle.headers["cache-control"] == "no-store"
    with tarfile.open(fileobj=io.BytesIO(first), mode="r:gz") as tar:
        names = tar.getnames()
    assert any(name.startswith("vendor/httpx/") for name in names)
    assert any(name.startswith("vendor/httpcore/") for name in names)


@pytest.mark.asyncio
async def test_join_script_caches_bundle_and_removes_invite_from_worker_process(tmp_path, monkeypatch):
    monkeypatch.setenv("LOCAL_SHELL_MCP_WORKSPACE_ROOT", str(tmp_path))
    monkeypatch.setenv("LOCAL_SHELL_MCP_PUBLIC_BASE_URL", "https://example.test")
    get_settings.cache_clear()
    response = await routes.join_script(None)  # type: ignore[arg-type]
    script = response.body.decode("utf-8")
    assert "/remote/worker-bundle.tgz?manifest=1" in script
    assert "bundle.sha256" in script
    assert "checksum mismatch" in script
    assert "install-service" in script
    assert "install-launcher" in script
    assert 'export PATH="$HOME/.local/bin:$PATH"' in script
    assert "--invite-stdin" in script
    assert "Python 3.11 or newer is required" in script
    assert "python3.14 python3.13 python3.12 python3.11 python3" in script
    assert 'exec "$PYTHON_BIN" -m morrow_runtime.remote_worker run' in script
    assert "remote_worker run --invite" not in script


@pytest.mark.asyncio
async def test_powershell_join_script_supports_persistent_windows_workers(tmp_path, monkeypatch):
    monkeypatch.setenv("LOCAL_SHELL_MCP_WORKSPACE_ROOT", str(tmp_path))
    monkeypatch.setenv("LOCAL_SHELL_MCP_PUBLIC_BASE_URL", "https://example.test")
    get_settings.cache_clear()
    response = await routes.powershell_join_script(None)  # type: ignore[arg-type]
    script = response.body.decode("utf-8")
    assert "/remote/worker-bundle.tgz?manifest=1" in script
    assert "Get-FileHash -Algorithm SHA256" in script
    assert "tarfile.open" in script
    assert '"--invite-stdin"' in script
    assert '"install-service"' in script
    assert "[switch]$Persist" in script
    assert "$Invite | & $PythonExe @EnrollArgs" in script
    assert "-Invite $Invite" not in script
    assert "sys.version_info >= (3, 11)" in script
    assert script.index("sys.version_info >= (3, 11)") < script.index("Downloading worker bundle")


@pytest.mark.asyncio
async def test_powershell_join_script_escapes_server_url(tmp_path, monkeypatch):
    monkeypatch.setenv("LOCAL_SHELL_MCP_WORKSPACE_ROOT", str(tmp_path))
    monkeypatch.setenv("LOCAL_SHELL_MCP_PUBLIC_BASE_URL", "https://example.test/worker's")
    get_settings.cache_clear()
    response = await routes.powershell_join_script(None)  # type: ignore[arg-type]
    script = response.body.decode("utf-8")
    assert "$Server = 'https://example.test/worker''s'" in script


@pytest.mark.skipif(sys.platform != "win32", reason="requires Windows PowerShell")
@pytest.mark.asyncio
async def test_powershell_join_script_parses_on_windows(tmp_path, monkeypatch):
    monkeypatch.setenv("LOCAL_SHELL_MCP_WORKSPACE_ROOT", str(tmp_path))
    monkeypatch.setenv("LOCAL_SHELL_MCP_PUBLIC_BASE_URL", "https://example.test")
    get_settings.cache_clear()
    response = await routes.powershell_join_script(None)  # type: ignore[arg-type]
    script_path = tmp_path / "join.ps1"
    script_path.write_bytes(response.body)
    shell = shutil.which("pwsh") or shutil.which("powershell")
    assert shell
    script_literal = "'" + str(script_path).replace("'", "''") + "'"
    parser = """
$tokens = $null
$errors = $null
[System.Management.Automation.Language.Parser]::ParseFile(__SCRIPT_PATH__, [ref]$tokens, [ref]$errors) | Out-Null
if ($errors.Count -gt 0) {
  $errors | ForEach-Object { [Console]::Error.WriteLine($_.Message) }
  exit 1
}
""".replace("__SCRIPT_PATH__", script_literal)
    result = subprocess.run(  # noqa: S603
        [shell, "-NoProfile", "-NonInteractive", "-Command", parser],
        check=False,
        capture_output=True,
        text=True,
    )
    assert result.returncode == 0, result.stderr


def test_remote_routes_replace_worker_bootstrap_endpoints():
    paths = [route.path for route in routes.remote_routes()]
    assert paths[:4] == [
        "/join",
        "/join.ps1",
        "/remote/worker-manifest.json",
        "/remote/worker-bundle.tgz",
    ]
    assert "/remote/register" in paths
    assert "/remote/resume" in paths


def test_worker_runtime_imports_without_native_archive_codec(tmp_path):
    script = """
import builtins

original_import = builtins.__import__

def without_zstandard(name, *args, **kwargs):
    if name == "zstandard":
        raise ModuleNotFoundError("No module named 'zstandard'")
    return original_import(name, *args, **kwargs)

builtins.__import__ = without_zstandard
import sys
import tarfile
from morrow_runtime import audit

assert audit is not None
assert "morrow_runtime.audit_archive_codec" not in sys.modules
assert "zstandard" not in sys.modules
"""
    completed = subprocess.run(
        [sys.executable, "-c", script],
        check=False,
        capture_output=True,
        text=True,
        cwd=tmp_path,
    )
    assert completed.returncode == 0, completed.stderr

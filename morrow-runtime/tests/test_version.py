import tomllib
from pathlib import Path

from fastapi.testclient import TestClient

from morrow_runtime import __version__
from morrow_runtime.http_app import build_http_app
from morrow_runtime.main import main
from morrow_runtime.settings import get_settings
from morrow_runtime.version import format_version_info, version_info


def test_runtime_version_matches_project_metadata():
    project = tomllib.loads((Path(__file__).parents[1] / "pyproject.toml").read_text(encoding="utf-8"))

    assert project["project"]["version"] == __version__


def test_version_info_reports_package_metadata():
    info = version_info()

    assert info["version"] == __version__
    assert info["package_version"]
    assert info["python"]
    assert info["platform"]
    assert format_version_info(info).startswith("morrow-runtime ")


def test_cli_version_subcommand_prints_version(capsys):
    main(["version"])

    assert f"morrow-runtime {__version__}" in capsys.readouterr().out


def test_cli_short_version_prints_raw_version(capsys):
    main(["--version"])

    assert capsys.readouterr().out.strip() == __version__


def test_http_version_endpoint(tmp_path, monkeypatch):
    monkeypatch.setenv("LOCAL_SHELL_MCP_WORKSPACE_ROOT", str(tmp_path))
    monkeypatch.setenv("LOCAL_SHELL_MCP_AUTH_MODE", "none")
    get_settings.cache_clear()

    response = TestClient(build_http_app()).get("/version")

    assert response.status_code == 200
    assert response.json()["version"] == __version__

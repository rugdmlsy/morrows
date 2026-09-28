from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path

import pytest

SCRIPT = Path(__file__).parents[1] / "scripts" / "migrate-morrow-state.py"
SPEC = importlib.util.spec_from_file_location("migrate_morrow_state", SCRIPT)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)


def _source(tmp_path: Path) -> Path:
    source = tmp_path / "state-v3"
    source.mkdir()
    (source / "oauth-jwt-secret").write_text("test-secret\n", encoding="utf-8")
    (source / "oauth-clients.json").write_text('{"clients": []}\n', encoding="utf-8")
    (source / "todos.json").write_text('{"todos": []}\n', encoding="utf-8")
    (source / "jobs").mkdir()
    (source / "jobs" / "one.log").write_text("done\n", encoding="utf-8")
    (source / "tmp").mkdir()
    (source / "tmp" / "ignored").write_text("ignore me", encoding="utf-8")
    return source


def test_dry_run_does_not_create_destination(tmp_path: Path) -> None:
    source = _source(tmp_path)
    destination = tmp_path / "state-v4"

    manifest = MODULE.migrate(source, destination, apply=False)

    assert manifest["mode"] == "dry-run"
    assert not destination.exists()
    assert {Path(row["destination"]).name for row in manifest["entries"]} >= {
        "oauth-jwt-secret",
        "oauth-clients.json",
        "todos.json",
        "jobs",
    }


def test_apply_splits_durable_and_legacy_state(tmp_path: Path) -> None:
    source = _source(tmp_path)
    external = tmp_path / "external-mcp.toml"
    external.write_text('[[instances]]\nname = "unused"\n', encoding="utf-8")
    destination = tmp_path / "state-v4"

    MODULE.migrate(source, destination, apply=True, legacy_config=external)

    assert (destination / "oauth-jwt-secret").read_text(encoding="utf-8") == "test-secret\n"
    assert (destination / "jobs" / "one.log").read_text(encoding="utf-8") == "done\n"
    assert (destination / "legacy-v3" / "todos.json").is_file()
    assert (destination / "legacy-v3" / "external-mcp.toml").is_file()
    assert not (destination / "tmp").exists()
    assert json.loads((destination / "migration-manifest.json").read_text())["mode"] == "apply"
    if sys.platform != "win32":
        assert (destination / "oauth-jwt-secret").stat().st_mode & 0o777 == 0o600


def test_refuses_nonempty_or_nested_destination(tmp_path: Path) -> None:
    source = _source(tmp_path)
    destination = tmp_path / "occupied"
    destination.mkdir()
    (destination / "existing").touch()

    with pytest.raises(ValueError, match="absent or empty"):
        MODULE.migrate(source, destination, apply=True)
    with pytest.raises(ValueError, match="separate"):
        MODULE.migrate(source, source / "nested", apply=False)


def test_apply_accepts_empty_existing_destination(tmp_path: Path) -> None:
    source = _source(tmp_path)
    destination = tmp_path / "state-v4"
    destination.mkdir()

    MODULE.migrate(source, destination, apply=True)

    assert (destination / "oauth-clients.json").is_file()


def test_rejects_invalid_json_before_copy(tmp_path: Path) -> None:
    source = _source(tmp_path)
    (source / "remote-workers.json").write_text("not-json", encoding="utf-8")

    with pytest.raises(json.JSONDecodeError):
        MODULE.migrate(source, tmp_path / "state-v4", apply=True)

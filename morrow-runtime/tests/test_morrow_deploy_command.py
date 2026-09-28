from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

REPOSITORY = Path(__file__).parents[1]
DEPLOY_COMMAND = REPOSITORY / "deploy/morrow/deploy-vps.sh"
MCP_PROBE = REPOSITORY / "scripts/probe-mcp.py"
HOST_CONFIG = REPOSITORY / "deploy/morrow/host.yaml.example"
HOST_LAUNCHER = REPOSITORY / "deploy/morrow/run-host-vps.sh"
HOST_UNIT = REPOSITORY / "deploy/morrow/morrow-runtime.service"
ROUTER_CONFIG = REPOSITORY / "deploy/morrow/mcp-router.caddy"
INSTALL_TOPOLOGY = REPOSITORY / "deploy/morrow/install-production-topology.sh"
ACTIVATE_TOPOLOGY = REPOSITORY / "deploy/morrow/activate-production-topology.sh"
AUTHORIZE_RELEASE = REPOSITORY / "deploy/morrow/authorize-release.sh"
ENSURE_PRODUCTION_SECRETS = REPOSITORY / "deploy/morrow/ensure-production-secrets.sh"
VERIFY_RELEASE = REPOSITORY / "deploy/morrow/verify-release.sh"
CHECK_RELEASE_PROCESS = REPOSITORY / "deploy/morrow/check-release-process.sh"
CURRENT_RELEASE = REPOSITORY / "deploy/morrow/current-release.sh"
SSH_GUARD = REPOSITORY / "deploy/morrow/ssh-guard.sh"
TIMEOUT_HELPER = REPOSITORY / "deploy/morrow/run-command-with-timeout.py"


def test_deploy_command_is_executable_and_parses() -> None:
    assert os.access(DEPLOY_COMMAND, os.X_OK)
    subprocess.run(["bash", "-n", str(DEPLOY_COMMAND)], check=True)
    help_result = subprocess.run(
        [str(DEPLOY_COMMAND), "--help"],
        check=True,
        capture_output=True,
        text=True,
    )
    assert "--dry-run" in help_result.stdout
    assert "--rotate-control-key" in help_result.stdout


def test_deploy_command_keeps_release_and_rollback_guards() -> None:
    script = DEPLOY_COMMAND.read_text(encoding="utf-8")

    for required in (
        "git status --porcelain",
        "git tag -a",
        "build-release.sh",
        "switch-release.sh",
        "rollback-release.sh",
        "morrow-runtime-cloudflared.service",
        "LSM_DEPLOY_UV_BIN",
        "sudo -n systemctl --no-block restart",
        "wait_for_release_process",
        "ControlMaster=auto",
        "ControlMaster=no",
        "establish_ssh_master",
        "initial SSH handshake failed; retry",
        "ServerAliveInterval=5",
        "ServerAliveCountMax=2",
        "remote_fresh_guarded",
        "run-command-with-timeout.py",
        "verify-release.sh",
        "mcp-router.caddy",
        "install-production-topology.sh",
        "activate-production-topology.sh",
        "authorize-release.sh",
        "ensure-production-secrets.sh",
        "--rotate-control-key",
        "stage_remote_file",
        "activate_production_topology",
        "post_switch_transport_uncertain",
        "ssh-guard.sh",
        "post_switch_run_script",
        "preserving the current release; refusing automatic rollback",
        "cleanup_ssh",
        "LSM_DEPLOY_SERVICE_ENV",
        "post-switch verification failed; rolling back",
    ):
        assert required in script

    assert 'local release_name="$1"' not in script
    assert "remote_guarded()" in script
    assert "remote_fresh_guarded()" in script
    assert 'test "$(uname -s)" != Darwin' in script
    assert "local deployment is forbidden" in script
    assert 'readonly repository_url="https://github.com/rugdmlsy/morrow-runtime.git"' in script
    assert 'test "$(git remote get-url origin)" = "${repository_url}"' in script
    assert "LSM_DEPLOY_REPOSITORY_URL" not in script
    assert script.index("if ${post_switch_transport_uncertain}; then") < script.index(
        'echo "post-switch verification failed; rolling back"'
    )


def test_post_switch_verifier_and_timeout_helper_are_deterministic() -> None:
    verifier = VERIFY_RELEASE.read_text(encoding="utf-8")
    process_check = CHECK_RELEASE_PROCESS.read_text(encoding="utf-8")
    guard = SSH_GUARD.read_text(encoding="utf-8")
    assert "probe-mcp.py" in verifier
    assert "--pin-env LOCAL_SHELL_MCP_OAUTH_ADMIN_PIN" in verifier
    assert "/proc/${pid}/cmdline" in process_check
    assert 'test "$1" -eq 124 || test "$1" -eq 255' in guard
    for script in (
        VERIFY_RELEASE,
        CHECK_RELEASE_PROCESS,
        CURRENT_RELEASE,
        SSH_GUARD,
        INSTALL_TOPOLOGY,
        ACTIVATE_TOPOLOGY,
        AUTHORIZE_RELEASE,
        ENSURE_PRODUCTION_SECRETS,
    ):
        subprocess.run(["bash", "-n", str(script)], check=True)

    success = subprocess.run(
        [sys.executable, str(TIMEOUT_HELPER), "2", sys.executable, "-c", "print('ok')"],
        check=False,
        capture_output=True,
        text=True,
    )
    assert success.returncode == 0
    assert success.stdout.strip() == "ok"

    child_failure = subprocess.run(
        [sys.executable, str(TIMEOUT_HELPER), "2", sys.executable, "-c", "raise SystemExit(7)"],
        check=False,
    )
    assert child_failure.returncode == 7

    expired = subprocess.run(
        [
            sys.executable,
            str(TIMEOUT_HELPER),
            "0.05",
            sys.executable,
            "-c",
            "import time; time.sleep(1)",
        ],
        check=False,
        capture_output=True,
        text=True,
    )
    assert expired.returncode == 124
    assert "exceeded 0.05s deadline" in expired.stderr


def test_post_switch_ssh_policy_fault_injection(tmp_path: Path) -> None:
    payload = tmp_path / "payload.sh"
    payload.write_text("exit 0\n", encoding="utf-8")
    marker = tmp_path / "fresh-called"
    harness = r'''
sleep() { :; }
remote_guarded() { cat >/dev/null; return "${PRIMARY_STATUS}"; }
remote_fresh_guarded() {
  cat >/dev/null
  printf 'called\n' >> "${MARKER}"
  return "${FRESH_STATUS}"
}
. "${SSH_GUARD_PATH}"
if post_switch_run_script "${PAYLOAD}" demo; then
  status=0
else
  status=$?
fi
printf 'status=%s\n' "${status}"
'''

    def run(primary: int, fresh: int) -> subprocess.CompletedProcess[str]:
        marker.unlink(missing_ok=True)
        env = {
            **os.environ,
            "PRIMARY_STATUS": str(primary),
            "FRESH_STATUS": str(fresh),
            "MARKER": str(marker),
            "SSH_GUARD_PATH": str(SSH_GUARD),
            "PAYLOAD": str(payload),
        }
        return subprocess.run(
            ["bash", "-c", harness],
            check=False,
            capture_output=True,
            text=True,
            env=env,
        )

    recovered = run(124, 0)
    assert recovered.returncode == 0
    assert "status=0" in recovered.stdout
    assert marker.exists()

    explicit_remote_failure = run(7, 0)
    assert explicit_remote_failure.returncode == 0
    assert "status=7" in explicit_remote_failure.stdout
    assert not marker.exists(), "explicit remote failures must not try an alternate transport"

    indeterminate = run(255, 255)
    assert indeterminate.returncode == 0
    assert "status=75" in indeterminate.stdout
    assert marker.exists()


def test_production_control_credential_is_generated_privately_and_idempotently(
    tmp_path: Path,
) -> None:
    service_env = tmp_path / "service.env"
    service_env.write_text(
        "CLOUDFLARE_TUNNEL_TOKEN=keep-existing\n"
        "LOCAL_SHELL_MCP_OAUTH_ADMIN_PIN=keep-pin\n",
        encoding="utf-8",
    )
    service_env.chmod(0o600)

    first = subprocess.run(
        [str(ENSURE_PRODUCTION_SECRETS), str(service_env)],
        check=True,
        capture_output=True,
        text=True,
    )
    content = service_env.read_text(encoding="utf-8")
    control_lines = [
        line
        for line in content.splitlines()
        if line.startswith("LOCAL_SHELL_MCP_CONTROL_API_KEY=")
    ]
    assert len(control_lines) == 1
    generated_value = control_lines[0].split("=", 1)[1]
    assert len(generated_value) >= 48
    assert generated_value not in first.stdout
    assert "CLOUDFLARE_TUNNEL_TOKEN=keep-existing" in content
    assert "LOCAL_SHELL_MCP_OAUTH_ADMIN_PIN=keep-pin" in content
    assert service_env.stat().st_mode & 0o777 == 0o600

    second = subprocess.run(
        [str(ENSURE_PRODUCTION_SECRETS), str(service_env)],
        check=True,
        capture_output=True,
        text=True,
    )
    assert service_env.read_text(encoding="utf-8") == content
    assert generated_value not in second.stdout

    rotated = subprocess.run(
        [str(ENSURE_PRODUCTION_SECRETS), "--rotate-control-key", str(service_env)],
        check=True,
        capture_output=True,
        text=True,
    )
    rotated_content = service_env.read_text(encoding="utf-8")
    rotated_lines = [
        line
        for line in rotated_content.splitlines()
        if line.startswith("LOCAL_SHELL_MCP_CONTROL_API_KEY=")
    ]
    assert len(rotated_lines) == 1
    rotated_value = rotated_lines[0].split("=", 1)[1]
    assert rotated_value != generated_value
    assert len(rotated_value) >= 48
    assert rotated_value not in rotated.stdout
    assert generated_value not in rotated.stdout
    assert "LSM control credential rotated" in rotated.stdout
    assert "CLOUDFLARE_TUNNEL_TOKEN=keep-existing" in rotated_content
    assert "LOCAL_SHELL_MCP_OAUTH_ADMIN_PIN=keep-pin" in rotated_content
    assert service_env.stat().st_mode & 0o777 == 0o600


def test_authorized_release_guard_rejects_manual_switch(tmp_path: Path) -> None:
    deploy_root = tmp_path / "lsm-controller"
    releases = deploy_root / "releases"
    releases.mkdir(parents=True)
    sha1 = "1" * 40
    sha2 = "2" * 40
    release1 = releases / "morrow-v4.3.2-test1-111111111111"
    release2 = releases / "morrow-v4.3.2-test2-222222222222"
    for release, sha in ((release1, sha1), (release2, sha2)):
        (release / ".venv/bin").mkdir(parents=True)
        executable = release / ".venv/bin/morrow-runtime"
        executable.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
        executable.chmod(0o755)
        (release / "READY").write_text(sha + "\n", encoding="utf-8")

    subprocess.run(
        [str(AUTHORIZE_RELEASE), release1.name, sha1, str(deploy_root)],
        check=True,
        capture_output=True,
        text=True,
    )
    assert (deploy_root / "AUTHORIZED_RELEASE").read_text(encoding="utf-8") == (
        f"{release1.name}\t{sha1}\n"
    )

    blocked = subprocess.run(
        [
            str(REPOSITORY / "deploy/morrow/switch-release.sh"),
            release2.name,
            str(deploy_root),
            "not-a-real.service",
        ],
        check=False,
        capture_output=True,
        text=True,
    )
    assert blocked.returncode == 78
    assert "Deploy with: ./deploy/morrow/deploy-vps.sh" in blocked.stderr
    assert not (deploy_root / "current").exists()


def test_production_topology_owns_shared_caddy_edge() -> None:
    config = HOST_CONFIG.read_text(encoding="utf-8")
    launcher = HOST_LAUNCHER.read_text(encoding="utf-8")
    unit = HOST_UNIT.read_text(encoding="utf-8")
    router = ROUTER_CONFIG.read_text(encoding="utf-8")
    verifier = VERIFY_RELEASE.read_text(encoding="utf-8")
    installer = INSTALL_TOPOLOGY.read_text(encoding="utf-8")

    assert "port: 8766" in config
    assert "LOCAL_SHELL_MCP_HOST=127.0.0.1" in launcher
    assert "LOCAL_SHELL_MCP_PORT=8766" in launcher
    assert 'rm -f "${config_root}/control-api-key"' in launcher
    assert "control_key_tmp" not in launcher
    assert "AUTHORIZED_RELEASE" in launcher
    assert "refusing to start an unmanaged Morrow Runtime production release" in launcher
    assert "Deploy with: ./deploy/morrow/deploy-vps.sh" in launcher
    assert "RestartPreventExitStatus=78" in unit
    assert router.lstrip().startswith("# Shared loopback edge")
    assert "\n:8765 {" in router
    assert "bind 127.0.0.1" in router
    assert "reverse_proxy 127.0.0.1:8766" in router
    assert "reverse_proxy 127.0.0.1:8787" in router
    assert "@morrows_browser_root {" in router
    assert "method GET" in router
    assert "header Accept *text/html*" in router
    assert "redir * /morrows/ui/ 302" in router
    assert "@morrows_mcp path /morrows /morrows/" in router
    morrows_mcp = router.split("@morrows_mcp path /morrows /morrows/", 1)[1].split(
        "@morrows_health", 1
    )[0]
    assert "reverse_proxy 127.0.0.1:8766" in morrows_mcp
    assert "reverse_proxy 127.0.0.1:8787" not in morrows_mcp
    assert "rewrite * /mcp" not in morrows_mcp
    morrows_api = router.split("handle /morrows/api/*", 1)[1].split(
        "@morrows_ui_root", 1
    )[0]
    assert "reverse_proxy 127.0.0.1:8766" in morrows_api
    assert "reverse_proxy 127.0.0.1:8787" not in morrows_api
    assert "strip_prefix /morrows" not in morrows_api
    assert router.index("@morrows_ui_root path /morrows/ui") < router.index("handle /morrows/ui/*")
    assert "redir * /morrows/ui/ 308" in router
    assert "http://127.0.0.1:8766/healthz" in verifier
    assert "http://127.0.0.1:8765/healthz" in verifier
    assert "http://127.0.0.1:8765/morrows" in verifier
    assert "http://127.0.0.1:8765/morrows/api/operator-session" in verifier
    assert "www-authenticate: Bearer" in verifier
    assert "install -m 0755" in installer
    assert "morrow-runtime.service" in installer
    assert "morrows-router.caddy" in installer
    assert "caddy validate" in installer
    assert "systemctl daemon-reload" in installer


def test_production_owns_official_live_workspace_and_goal_continuation() -> None:
    config = HOST_CONFIG.read_text(encoding="utf-8")
    launcher = HOST_LAUNCHER.read_text(encoding="utf-8")

    assert "disable_local: false" in config
    assert "logical_sessions_enabled: true" in config
    assert "live_workspace_enabled: true" in config
    assert "LOCAL_SHELL_MCP_DISABLE_LOCAL=false" in launcher
    assert "LOCAL_SHELL_MCP_LOGICAL_SESSIONS_ENABLED=true" in launcher
    assert "LOCAL_SHELL_MCP_LIVE_WORKSPACE_ENABLED=true" in launcher
    assert "do not delegate continuation to Morrow Chat" in (
        REPOSITORY / "deploy/morrow/README.md"
    ).read_text(encoding="utf-8")


def test_probe_accepts_pin_from_environment_without_a_command_line_secret() -> None:
    help_result = subprocess.run(
        [sys.executable, str(MCP_PROBE), "--help"],
        check=True,
        capture_output=True,
        text=True,
    )

    assert "--pin-env NAME" in help_result.stdout

#!/usr/bin/env bash
set -euo pipefail

readonly root="${MORROWS_ROOT:-/srv/morrow/workspaces/morrows}"
readonly runtime_root="${MORROWS_RUNTIME_ROOT:-${root}/morrow-runtime}"
readonly runtime_bin="${MORROWS_RUNTIME_BIN:-${runtime_root}/.venv/bin/morrow-runtime}"
readonly runtime_env="${MORROWS_RUNTIME_ENV:-/home/morrow/.config/morrows/runtime.env}"
readonly guard_file="${MORROWS_DEPLOY_GUARD_FILE:-/home/morrow/.config/morrows/DEPLOYED_RELEASE}"
readonly installed_unit="${MORROWS_RUNTIME_SYSTEMD_UNIT:-/etc/systemd/system/morrow-runtime.service}"

deployment_guard_error() {
  echo "ERROR: refusing to start an unmanaged morrow-runtime production deployment." >&2
  echo "Deploy with: ./scripts/deploy-vps.sh" >&2
  exit 78
}

test -f "${guard_file}" || deployment_guard_error
declare deployed_commit="" artifacts=""
while IFS='=' read -r key value; do
  case "${key}" in
    commit) deployed_commit="${value}" ;;
    artifacts) artifacts="${value}" ;;
  esac
done < "${guard_file}"

[[ "${deployed_commit}" =~ ^[0-9a-f]{40}$ ]] || deployment_guard_error
test -n "${artifacts}" && test -d "${artifacts}" || deployment_guard_error
test "$(git -C "${root}" rev-parse HEAD)" = "${deployed_commit}" || deployment_guard_error
git -C "${root}" diff --quiet -- || deployment_guard_error
git -C "${root}" diff --cached --quiet -- || deployment_guard_error
test -x "${runtime_bin}" || deployment_guard_error
cmp -s "${root}/scripts/run-morrow-runtime-vps.sh" "${artifacts}/runtime-launcher" || deployment_guard_error
cmp -s "${installed_unit}" "${artifacts}/runtime-unit" || deployment_guard_error
test -r "${runtime_env}" || deployment_guard_error

set +u
# shellcheck disable=SC1090
source "${runtime_env}"
set -u
test -n "${MORROWS_RUNTIME_CONTROL_KEY:-}" || deployment_guard_error

export LOCAL_SHELL_MCP_HOST="${MORROWS_RUNTIME_BIND_HOST:-127.0.0.1}"
export LOCAL_SHELL_MCP_PORT="${MORROWS_RUNTIME_PORT:-8790}"
export LOCAL_SHELL_MCP_MODE=mcp
export LOCAL_SHELL_MCP_WORKSPACE_ROOT="${MORROWS_RUNTIME_WORKSPACE_ROOT:-/srv/morrow/workspaces}"
export LOCAL_SHELL_MCP_STATE_DIR="${MORROWS_RUNTIME_STATE_DIR:-/home/morrow/.local/state/morrow-runtime}"
export LOCAL_SHELL_MCP_AUTH_MODE=none
export LOCAL_SHELL_MCP_REQUIRE_SESSION_CAPABILITY=true
export LOCAL_SHELL_MCP_AUTH_BYPASS_LOCALHOST=true
export LOCAL_SHELL_MCP_CONTROL_API_KEY="${MORROWS_RUNTIME_CONTROL_KEY}"
export LOCAL_SHELL_MCP_PUBLIC_BASE_URL="${MORROWS_RUNTIME_PUBLIC_URL:-https://mcp.xycdev.com/morrows/ui/runtime}"
export LOCAL_SHELL_MCP_REMOTE_ENABLED=true
export LOCAL_SHELL_MCP_DISABLE_LOCAL=true
export LOCAL_SHELL_MCP_UI_ENABLED=false
export LOCAL_SHELL_MCP_LIVE_WORKSPACE_ENABLED=false
export LOCAL_SHELL_MCP_MORROWS_JOB_EVENT_URL=""

exec "${runtime_bin}" --mode mcp

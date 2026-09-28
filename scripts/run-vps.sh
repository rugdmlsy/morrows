#!/usr/bin/env bash
set -euo pipefail

readonly root="${MORROWS_ROOT:-/srv/morrow/workspaces/morrows}"
readonly server_bin="${MORROWS_SERVER_BIN:-${root}/target/release/morrows-server}"
readonly runtime_env="${MORROWS_RUNTIME_ENV:-/home/morrow/.config/morrows/runtime.env}"
readonly guard_file="${MORROWS_DEPLOY_GUARD_FILE:-/home/morrow/.config/morrows/DEPLOYED_RELEASE}"
readonly installed_unit="${MORROWS_SYSTEMD_UNIT:-/etc/systemd/system/morrows.service}"
readonly installed_runtime_unit="${MORROWS_RUNTIME_SYSTEMD_UNIT:-/etc/systemd/system/morrow-runtime.service}"

deployment_guard_error() {
  echo "ERROR: refusing to start an unmanaged Morrows production deployment." >&2
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
test -x "${server_bin}" || deployment_guard_error
cmp -s "${server_bin}" "${artifacts}/server" || deployment_guard_error
diff -qr "${root}/web/dist" "${artifacts}/web" >/dev/null || deployment_guard_error
cmp -s "${root}/scripts/run-vps.sh" "${artifacts}/launcher" || deployment_guard_error
cmp -s "${root}/scripts/run-morrow-runtime-vps.sh" "${artifacts}/runtime-launcher" || deployment_guard_error
cmp -s "${installed_unit}" "${artifacts}/unit" || deployment_guard_error
cmp -s "${installed_runtime_unit}" "${artifacts}/runtime-unit" || deployment_guard_error
command -v "${MORROWS_RG_BIN:-rg}" >/dev/null 2>&1 || deployment_guard_error
test -r "${runtime_env}" || deployment_guard_error
runtime_control_key="$(
  set +u
  # shellcheck disable=SC1090
  source "${runtime_env}"
  printf '%s' "${MORROWS_RUNTIME_CONTROL_KEY:-}"
)"
test -n "${runtime_control_key}" || deployment_guard_error
export MORROWS_RUNTIME_CONTROL_KEY="${runtime_control_key}"
export MORROWS_RUNTIME_CONTROL_URL="${MORROWS_RUNTIME_CONTROL_URL:-http://127.0.0.1:8790}"
export MORROWS_RUNTIME_PROXY_URL="${MORROWS_RUNTIME_PROXY_URL:-http://127.0.0.1:8790}"
export MORROWS_RUNTIME_MCP_URL="${MORROWS_RUNTIME_MCP_URL:-https://mcp.xycdev.com/morrows/ui/runtime/mcp}"
export MORROWS_AGENT_MCP_URL="${MORROWS_AGENT_MCP_URL:-https://mcp.xycdev.com/morrows/ui/agent-mcp}"
export MORROWS_RUNTIME_SUBJECT="${MORROWS_RUNTIME_SUBJECT:-morrows-runtime}"
unset runtime_control_key

exec "${server_bin}"

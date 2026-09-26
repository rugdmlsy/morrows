#!/usr/bin/env bash
set -euo pipefail

readonly root="${MORROWS_ROOT:-/srv/morrow/workspaces/morrows}"
readonly server_bin="${MORROWS_SERVER_BIN:-${root}/target/release/morrows-server}"
readonly lsm_env="${MORROWS_LSM_SOURCE_ENV:-/home/morrow/.config/local-shell-mcp/service.env}"
readonly guard_file="${MORROWS_DEPLOY_GUARD_FILE:-/home/morrow/.config/morrows/DEPLOYED_RELEASE}"
readonly installed_unit="${MORROWS_SYSTEMD_UNIT:-/etc/systemd/system/morrows.service}"

deployment_guard_error() {
  echo "ERROR: refusing to start an unmanaged Morrows production deployment." >&2
  echo "Deploy with: ./scripts/deploy-vps.sh" >&2
  exit 78
}

web_dist_sha256() {
  (
    cd "${root}"
    find web/dist -type f -print0 |
      LC_ALL=C sort -z |
      xargs -0 sha256sum |
      sha256sum |
      awk '{print $1}'
  )
}

test -f "${guard_file}" || deployment_guard_error
declare deployed_commit="" server_sha256="" web_sha256="" launcher_sha256="" unit_sha256=""
while IFS='=' read -r key value; do
  case "${key}" in
    commit) deployed_commit="${value}" ;;
    server_sha256) server_sha256="${value}" ;;
    web_sha256) web_sha256="${value}" ;;
    launcher_sha256) launcher_sha256="${value}" ;;
    unit_sha256) unit_sha256="${value}" ;;
  esac
done < "${guard_file}"

[[ "${deployed_commit}" =~ ^[0-9a-f]{40}$ ]] || deployment_guard_error
for digest in "${server_sha256}" "${web_sha256}" "${launcher_sha256}" "${unit_sha256}"; do
  [[ "${digest}" =~ ^[0-9a-f]{64}$ ]] || deployment_guard_error
done

test "$(git -C "${root}" rev-parse HEAD)" = "${deployed_commit}" || deployment_guard_error
git -C "${root}" diff --quiet -- || deployment_guard_error
git -C "${root}" diff --cached --quiet -- || deployment_guard_error
test -x "${server_bin}" || deployment_guard_error
test "$(sha256sum "${server_bin}" | awk '{print $1}')" = "${server_sha256}" || deployment_guard_error
test "$(web_dist_sha256)" = "${web_sha256}" || deployment_guard_error
test "$(sha256sum "${root}/scripts/run-vps.sh" | awk '{print $1}')" = "${launcher_sha256}" || deployment_guard_error
test -f "${installed_unit}" || deployment_guard_error
test "$(sha256sum "${installed_unit}" | awk '{print $1}')" = "${unit_sha256}" || deployment_guard_error

if [[ -r "${lsm_env}" ]]; then
  lsm_control_key="$(
    set +u
    # shellcheck disable=SC1090
    source "${lsm_env}"
    printf '%s' "${LOCAL_SHELL_MCP_CONTROL_API_KEY:-}"
  )"
  if [[ -n "${lsm_control_key}" ]]; then
    export MORROWS_LSM_CONTROL_KEY="${lsm_control_key}"
    export MORROWS_LSM_CONTROL_URL="${MORROWS_LSM_CONTROL_URL:-http://127.0.0.1:8766}"
    export MORROWS_LSM_SUBJECT="${MORROWS_LSM_SUBJECT:-local-mcp-client}"
  fi
  unset lsm_control_key
fi

exec "${server_bin}"

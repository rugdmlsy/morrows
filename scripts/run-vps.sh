#!/usr/bin/env bash
set -euo pipefail

readonly server_bin="${MORROWS_SERVER_BIN:-/srv/morrow/workspaces/morrows/target/release/morrows-server}"
readonly lsm_env="${MORROWS_LSM_SOURCE_ENV:-/home/morrow/.config/local-shell-mcp/service.env}"

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

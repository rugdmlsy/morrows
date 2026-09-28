#!/usr/bin/env bash
set -euo pipefail

readonly root="${MORROWS_ROOT:-/srv/morrow/workspaces/morrows}"
readonly guard_file="${MORROWS_DEPLOY_GUARD_FILE:-/home/morrow/.config/morrows/DEPLOYED_RELEASE}"
readonly installed_unit="${MORROWS_EDGE_SYSTEMD_UNIT:-/etc/systemd/system/morrows-cloudflared.service}"

deployment_guard_error() {
  echo "ERROR: refusing to start an unmanaged Morrows Cloudflare edge." >&2
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
cmp -s "${root}/scripts/run-morrows-cloudflared-vps.sh" "${artifacts}/edge-launcher" || deployment_guard_error
cmp -s "${installed_unit}" "${artifacts}/edge-unit" || deployment_guard_error
test -n "${CLOUDFLARE_TUNNEL_TOKEN:-}" || deployment_guard_error

export TUNNEL_TOKEN="${CLOUDFLARE_TUNNEL_TOKEN}"
unset CLOUDFLARE_TUNNEL_TOKEN
exec /usr/bin/cloudflared tunnel --no-autoupdate --metrics 127.0.0.1:20243 run

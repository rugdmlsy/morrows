#!/usr/bin/env bash
set -euo pipefail

REMOTE="${MORROWS_VPS_HOST:-ovh-vps}"
REMOTE_ROOT="${MORROWS_VPS_ROOT:-/srv/morrow/workspaces/morrows}"
REPO="${MORROWS_GIT_REPO:-https://github.com/rugdmlsy/morrows.git}"
BRANCH="${MORROWS_GIT_BRANCH:-main}"
HEALTH="${MORROWS_VPS_HEALTH_URL:-http://127.0.0.1:8787/api/health}"

ssh "$REMOTE" bash -s -- "$REMOTE_ROOT" "$REPO" "$BRANCH" "$HEALTH" <<'REMOTE_SCRIPT'
set -euo pipefail
root="$1"
repo="$2"
branch="$3"
health="$4"

if [[ ! -x "$HOME/.cargo/bin/cargo" ]]; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs |
    sh -s -- -y --profile minimal --default-toolchain stable
fi
source "$HOME/.cargo/env"

if [[ ! -d "$root/.git" ]]; then
  mkdir -p "$(dirname "$root")"
  git clone --branch "$branch" "$repo" "$root"
else
  git -C "$root" fetch origin "$branch"
  git -C "$root" checkout "$branch"
  git -C "$root" reset --hard "origin/$branch"
fi

cd "$root"
mkdir -p data/launches data/session-runtimes
chmod 755 scripts/run-vps.sh
(
  cd web
  npm ci
  npm run build
)
cargo build --release -p morrows-server

sudo install -m 0644 deploy/morrows.service /etc/systemd/system/morrows.service
sudo systemctl daemon-reload
sudo systemctl enable --now morrows.service
sudo systemctl restart morrows.service

deadline=$((SECONDS + 45))
while (( SECONDS < deadline )); do
  if body="$(curl -fsS "$health" 2>/dev/null)"; then
    pid="$(systemctl show morrows.service -p MainPID --value)"
    test -n "$pid"
    test "$pid" != 0
    env_names="$(tr '\0' '\n' < "/proc/$pid/environ")"
    grep -q '^MORROWS_LSM_CONTROL_URL=http://127.0.0.1:8766$' <<<"$env_names"
    grep -q '^MORROWS_LSM_CONTROL_KEY=.' <<<"$env_names"
    ! grep -q '^CLOUDFLARE_TUNNEL_TOKEN=' <<<"$env_names"
    ! grep -q '^LOCAL_SHELL_MCP_OAUTH_ADMIN_PIN=' <<<"$env_names"
    printf '%s\n' "$body"
    exit 0
  fi
  sleep 1
done
sudo journalctl -u morrows.service -n 120 --no-pager >&2 || true
exit 1
REMOTE_SCRIPT

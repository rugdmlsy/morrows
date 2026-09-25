#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TMUX_SOCKET="${MORROWS_TMUX_SOCKET:-morrows}"
TMUX_SESSION="${MORROWS_TMUX_SESSION:-morrows-server}"
HEALTH_URL="${MORROWS_HEALTH_URL:-http://127.0.0.1:8787/api/health}"
HEALTH_TIMEOUT_SECONDS="${MORROWS_DEPLOY_HEALTH_TIMEOUT_SECONDS:-30}"
FAST=0

usage() {
  cat <<'EOF'
Usage: ./scripts/deploy.sh [--fast]

Build, restart, and health-check the local Morrows daemon.

Default:
  - cargo test --workspace
  - npm run build
  - cargo build -p morrows-server
  - restart tmux session
  - wait for /api/health

--fast:
  Skip the full Rust test suite, but still build the frontend and server.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --fast)
      FAST=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

cd "$ROOT"

echo "==> Building Morrows"
if [[ "$FAST" -eq 0 ]]; then
  cargo test --workspace
fi
(
  cd web
  npm run build
)
cargo build -p morrows-server

echo "==> Restarting tmux session ${TMUX_SOCKET}:${TMUX_SESSION}"
if tmux -L "$TMUX_SOCKET" has-session -t "$TMUX_SESSION" 2>/dev/null; then
  tmux -L "$TMUX_SOCKET" kill-session -t "$TMUX_SESSION"
fi
tmux -L "$TMUX_SOCKET" new-session \
  -d \
  -s "$TMUX_SESSION" \
  -c "$ROOT" \
  ./scripts/run.sh

echo "==> Waiting for ${HEALTH_URL}"
deadline=$((SECONDS + HEALTH_TIMEOUT_SECONDS))
while (( SECONDS < deadline )); do
  if health="$(curl -fsS "$HEALTH_URL" 2>/dev/null)"; then
    echo "$health"
    echo "==> Morrows deployed successfully"
    exit 0
  fi
  sleep 0.5
done

echo "Morrows failed to become healthy within ${HEALTH_TIMEOUT_SECONDS}s." >&2
echo "--- tmux log ---" >&2
tmux -L "$TMUX_SOCKET" capture-pane -pt "${TMUX_SESSION}:0" -S -160 >&2 || true
exit 1

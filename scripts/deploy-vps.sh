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

git diff --quiet --
git diff --cached --quiet --

web_dist_sha256() {
  (
    cd "$root"
    find web/dist -type f -print0 |
      LC_ALL=C sort -z |
      xargs -0 sha256sum |
      sha256sum |
      awk '{print $1}'
  )
}

commit="$(git rev-parse HEAD)"
server_sha256="$(sha256sum target/release/morrows-server | awk '{print $1}')"
web_sha256="$(web_dist_sha256)"
launcher_sha256="$(sha256sum scripts/run-vps.sh | awk '{print $1}')"
unit_sha256="$(sha256sum deploy/morrows.service | awk '{print $1}')"

sudo install -m 0644 deploy/morrows.service /etc/systemd/system/morrows.service
test "$(sha256sum /etc/systemd/system/morrows.service | awk '{print $1}')" = "$unit_sha256"

guard_dir=/home/morrow/.config/morrows
guard_file="$guard_dir/DEPLOYED_RELEASE"
mkdir -p "$guard_dir"
chmod 700 "$guard_dir"
umask 022
guard_tmp="$(mktemp "$guard_dir/.DEPLOYED_RELEASE.XXXXXX")"
cat > "$guard_tmp" <<EOF
commit=$commit
server_sha256=$server_sha256
web_sha256=$web_sha256
launcher_sha256=$launcher_sha256
unit_sha256=$unit_sha256
EOF
chmod 0444 "$guard_tmp"
mv -f "$guard_tmp" "$guard_file"

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

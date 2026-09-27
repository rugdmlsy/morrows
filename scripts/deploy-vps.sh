#!/usr/bin/env bash
set -euo pipefail

REMOTE="${MORROWS_VPS_HOST:-ovh-vps}"
REMOTE_ROOT="${MORROWS_VPS_ROOT:-/srv/morrow/workspaces/morrows}"
REPO="https://github.com/rugdmlsy/morrows.git"
BRANCH="${MORROWS_GIT_BRANCH:-main}"
HEALTH="${MORROWS_VPS_HEALTH_URL:-http://127.0.0.1:8787/api/health}"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "Morrows source/deploy orchestration must run from the Mac source checkout; the VPS is deploy-only" >&2
  exit 1
fi
case "$REMOTE" in
  local|localhost|127.0.0.1|::1)
    echo "MORROWS_VPS_HOST must name the remote VPS; local deployment is forbidden" >&2
    exit 1
    ;;
esac

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repository_root"
test "$(git remote get-url origin)" = "$REPO" || {
  echo "origin must be $REPO" >&2
  exit 1
}
test "$(git branch --show-current)" = "$BRANCH" || {
  echo "deploy branch must be checked out locally: $BRANCH" >&2
  exit 1
}
test -z "$(git status --porcelain --untracked-files=no)" || {
  echo "tracked source changes must be committed on the Mac before deployment" >&2
  exit 1
}
git fetch --quiet origin "$BRANCH"
expected_commit="$(git rev-parse HEAD)"
remote_commit="$(git rev-parse "origin/$BRANCH")"
test "$expected_commit" = "$remote_commit" || {
  echo "Mac HEAD must be pushed to origin/$BRANCH before deployment" >&2
  exit 1
}

ssh "$REMOTE" bash -s -- "$REMOTE_ROOT" "$REPO" "$BRANCH" "$HEALTH" "$expected_commit" <<'REMOTE_SCRIPT'
set -euo pipefail
root="$1"
repo="$2"
branch="$3"
health="$4"
expected_commit="$5"

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
test "$(git -C "$root" remote get-url origin)" = "$repo"
test "$(git -C "$root" rev-parse HEAD)" = "$expected_commit" || {
  echo "VPS checkout does not match the pushed Mac commit $expected_commit" >&2
  exit 1
}

cd "$root"
mkdir -p data/launches data/session-runtimes
chmod 755 scripts/run-vps.sh
(
  cd web
  npm ci
  npm run build
)
cargo build --release -p morrows-server -p morrows-cli
mkdir -p "$HOME/.local/bin"
install -m 0755 target/release/morrows "$HOME/.local/bin/morrows"

command -v rg >/dev/null 2>&1

git diff --quiet --
git diff --cached --quiet --

# Keep exact deployed artifacts for byte comparison. No digest/checksum pass.
commit="$(git rev-parse HEAD)"
sudo install -m 0644 deploy/morrows.service /etc/systemd/system/morrows.service
cmp -s deploy/morrows.service /etc/systemd/system/morrows.service

guard_dir=/home/morrow/.config/morrows
guard_file="$guard_dir/DEPLOYED_RELEASE"
mkdir -p "$guard_dir"
chmod 700 "$guard_dir"

# Preserve the currently active release as the one-step rollback/forensics copy.
# Older snapshots are pruned only after the replacement is healthy.
previous_release=""
if [[ -f "$guard_file" ]]; then
  while IFS='=' read -r key value; do
    if [[ "$key" == artifacts ]]; then
      previous_release="$value"
    fi
  done < "$guard_file"
fi
case "$previous_release" in
  "$guard_dir"/release.*) ;;
  *) previous_release="" ;;
esac

release_dir="$(mktemp -d "$guard_dir/release.XXXXXX")"
install -m 0444 target/release/morrows-server "$release_dir/server"
install -m 0444 scripts/run-vps.sh "$release_dir/launcher"
install -m 0444 deploy/morrows.service "$release_dir/unit"
cp -R web/dist "$release_dir/web"
chmod -R a-w "$release_dir"
guard_tmp="$(mktemp "$guard_dir/.DEPLOYED_RELEASE.XXXXXX")"
printf 'commit=%s\nartifacts=%s\n' "$commit" "$release_dir" > "$guard_tmp"
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
    python3 -c 'import json,sys; data=json.loads(sys.stdin.read()); assert data["memory_search"]["enabled"] is True and data["memory_search"]["engine"] == "ripgrep"' <<<"$body"
    ! grep -q '^CLOUDFLARE_TUNNEL_TOKEN=' <<<"$env_names"
    ! grep -q '^LOCAL_SHELL_MCP_OAUTH_ADMIN_PIN=' <<<"$env_names"

    # Keep only current + immediately previous successful release.
    for stale_release in "$guard_dir"/release.*; do
      [[ -d "$stale_release" ]] || continue
      if [[ "$stale_release" != "$release_dir" && "$stale_release" != "$previous_release" ]]; then
        chmod -R u+w "$stale_release"
        rm -rf -- "$stale_release"
      fi
    done

    printf '%s\n' "$body"
    exit 0
  fi
  sleep 1
done
sudo journalctl -u morrows.service -n 120 --no-pager >&2 || true
exit 1
REMOTE_SCRIPT

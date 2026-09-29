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
if ! command -v uv >/dev/null 2>&1; then
  curl -LsSf https://astral.sh/uv/install.sh | sh
  export PATH="$HOME/.local/bin:$PATH"
fi

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
chmod 755 scripts/run-vps.sh scripts/run-morrow-runtime-vps.sh scripts/run-morrows-cloudflared-vps.sh
uv sync --project morrow-runtime --frozen --no-dev
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
sudo install -m 0644 deploy/morrow-runtime.service /etc/systemd/system/morrow-runtime.service
sudo install -m 0644 deploy/morrows-cloudflared.service /etc/systemd/system/morrows-cloudflared.service
cmp -s deploy/morrows.service /etc/systemd/system/morrows.service
cmp -s deploy/morrow-runtime.service /etc/systemd/system/morrow-runtime.service
cmp -s deploy/morrows-cloudflared.service /etc/systemd/system/morrows-cloudflared.service

guard_dir=/home/morrow/.config/morrows
guard_file="$guard_dir/DEPLOYED_RELEASE"
mkdir -p "$guard_dir"
chmod 700 "$guard_dir"
runtime_env="$guard_dir/runtime.env"
if [[ ! -s "$runtime_env" ]] || ! grep -q '^MORROWS_RUNTIME_CONTROL_KEY=.' "$runtime_env"; then
  runtime_key="$(python3 -c 'import secrets; print("mrw_runtime_ctl_"+secrets.token_urlsafe(36))')"
  runtime_env_tmp="$(mktemp "$guard_dir/.runtime.env.XXXXXX")"
  printf 'MORROWS_RUNTIME_CONTROL_KEY=%s\n' "$runtime_key" > "$runtime_env_tmp"
  chmod 0600 "$runtime_env_tmp"
  mv -f "$runtime_env_tmp" "$runtime_env"
  unset runtime_key
fi
chmod 0600 "$runtime_env"

# Own public-edge and OAuth secrets under Morrows. On the first cutover only,
# copy the existing tunnel/JWT material from the old locations so current
# clients keep working during a bounded migration window. No later service
# startup reads standalone LSM configuration.
service_env="$guard_dir/service.env"
legacy_lsm_env=/home/morrow/.config/local-shell-mcp/service.env
legacy_runtime_oauth_secret=/home/morrow/.local/state/morrow-runtime/oauth-jwt-secret
python3 - "$service_env" "$legacy_lsm_env" "$legacy_runtime_oauth_secret" <<'PY'
from pathlib import Path
import secrets
import sys
import time

service_path = Path(sys.argv[1])
lsm_path = Path(sys.argv[2])
runtime_secret_path = Path(sys.argv[3])

def parse(path: Path) -> dict[str, str]:
    result: dict[str, str] = {}
    if not path.is_file():
        return result
    for raw in path.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        result[key.strip()] = value.strip().strip('"').strip("'")
    return result

def upsert(lines: list[str], key: str, value: str) -> None:
    prefix = key + "="
    for index, line in enumerate(lines):
        if line.startswith(prefix):
            lines[index] = prefix + value
            return
    lines.append(prefix + value)

service_path.parent.mkdir(parents=True, exist_ok=True)
lines = service_path.read_text(encoding="utf-8").splitlines() if service_path.exists() else []
current = parse(service_path)
legacy = parse(lsm_path)

pin = current.get("MORROWS_OAUTH_ADMIN_PIN", "")
if len(pin) < 16:
    upsert(lines, "MORROWS_OAUTH_ADMIN_PIN", "mrw_oauth_pin_" + secrets.token_urlsafe(32))

if not current.get("CLOUDFLARE_TUNNEL_TOKEN") and legacy.get("CLOUDFLARE_TUNNEL_TOKEN"):
    upsert(lines, "CLOUDFLARE_TUNNEL_TOKEN", legacy["CLOUDFLARE_TUNNEL_TOKEN"])

legacy_material = False
if not current.get("MORROWS_OAUTH_LEGACY_RUNTIME_JWT_SECRET") and runtime_secret_path.is_file():
    value = runtime_secret_path.read_text(encoding="utf-8").strip()
    if len(value.encode()) >= 32:
        upsert(lines, "MORROWS_OAUTH_LEGACY_RUNTIME_JWT_SECRET", value)
        legacy_material = True
elif current.get("MORROWS_OAUTH_LEGACY_RUNTIME_JWT_SECRET"):
    legacy_material = True

if not current.get("MORROWS_OAUTH_LEGACY_LSM_JWT_SECRET"):
    value = legacy.get("LOCAL_SHELL_MCP_OAUTH_JWT_SECRET", "")
    if len(value.encode()) >= 32:
        upsert(lines, "MORROWS_OAUTH_LEGACY_LSM_JWT_SECRET", value)
        legacy_material = True
elif current.get("MORROWS_OAUTH_LEGACY_LSM_JWT_SECRET"):
    legacy_material = True

if legacy_material and not current.get("MORROWS_OAUTH_LEGACY_UNTIL"):
    # Fixed on first migration; later deploys do not extend the compatibility window.
    upsert(lines, "MORROWS_OAUTH_LEGACY_UNTIL", str(int(time.time()) + 30 * 86400))

upsert(lines, "MORROWS_MCP_URL", "https://mcp.xycdev.com/morrows")
upsert(lines, "MORROWS_OAUTH_ISSUER", "https://mcp.xycdev.com/morrows/auth")
service_path.write_text("\n".join(lines).rstrip() + "\n", encoding="utf-8")
service_path.chmod(0o600)
PY
chmod 0600 "$service_env"

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
install -m 0444 scripts/run-morrow-runtime-vps.sh "$release_dir/runtime-launcher"
install -m 0444 scripts/run-morrows-cloudflared-vps.sh "$release_dir/edge-launcher"
install -m 0444 deploy/morrows.service "$release_dir/unit"
install -m 0444 deploy/morrow-runtime.service "$release_dir/runtime-unit"
install -m 0444 deploy/morrows-cloudflared.service "$release_dir/edge-unit"
install -m 0444 deploy/morrows-edge.caddy "$release_dir/edge-caddy"
cp -R web/dist "$release_dir/web"
chmod -R a-w "$release_dir"
guard_tmp="$(mktemp "$guard_dir/.DEPLOYED_RELEASE.XXXXXX")"
printf 'commit=%s\nartifacts=%s\n' "$commit" "$release_dir" > "$guard_tmp"
chmod 0444 "$guard_tmp"
mv -f "$guard_tmp" "$guard_file"

sudo systemctl daemon-reload
sudo systemctl enable --now morrow-runtime.service
sudo systemctl restart morrow-runtime.service
runtime_deadline=$((SECONDS + 45))
while (( SECONDS < runtime_deadline )); do
  if curl -fsS http://127.0.0.1:8790/healthz >/dev/null 2>&1; then
    break
  fi
  sleep 1
done
curl -fsS http://127.0.0.1:8790/healthz >/dev/null
sudo systemctl enable --now morrows.service
sudo systemctl restart morrows.service

deadline=$((SECONDS + 45))
while (( SECONDS < deadline )); do
  if body="$(curl -fsS "$health" 2>/dev/null)"; then
    pid="$(systemctl show morrows.service -p MainPID --value)"
    test -n "$pid"
    test "$pid" != 0
    env_names="$(tr '\0' '\n' < "/proc/$pid/environ")"
    grep -q '^MORROWS_RUNTIME_CONTROL_URL=http://127.0.0.1:8790$' <<<"$env_names"
    grep -q '^MORROWS_RUNTIME_PROXY_URL=http://127.0.0.1:8790$' <<<"$env_names"
    grep -q '^MORROWS_RUNTIME_CONTROL_KEY=.' <<<"$env_names"
    grep -q '^MORROWS_RUNTIME_MCP_URL=https://mcp.xycdev.com/morrows/ui/runtime/mcp$' <<<"$env_names"
    grep -q '^MORROWS_MCP_URL=https://mcp.xycdev.com/morrows$' <<<"$env_names"
    grep -q '^MORROWS_OAUTH_ISSUER=https://mcp.xycdev.com/morrows/auth$' <<<"$env_names"
    grep -q '^MORROWS_OAUTH_ADMIN_PIN=.' <<<"$env_names"
    test "$(systemctl is-active morrow-runtime.service)" = active
    curl -fsS http://127.0.0.1:8790/healthz >/dev/null
    runtime_pid="$(systemctl show morrow-runtime.service -p MainPID --value)"
    runtime_env_names="$(tr '\0' '\n' < "/proc/$runtime_pid/environ")"
    grep -q '^LOCAL_SHELL_MCP_AUTH_MODE=internal$' <<<"$runtime_env_names"
    python3 -c 'import json,sys; data=json.loads(sys.stdin.read()); assert data["memory_search"]["enabled"] is True and data["memory_search"]["engine"] == "ripgrep"' <<<"$body"
    ! grep -q '^CLOUDFLARE_TUNNEL_TOKEN=' <<<"$env_names"
    ! grep -q '^LOCAL_SHELL_MCP_OAUTH_ADMIN_PIN=' <<<"$env_names"

    # Preserve already-registered legacy OAuth client IDs so a client can
    # reauthorize without depending on morrow-runtime state after this cutover.
    python3 - "$service_env" "$root" <<'PY'
from pathlib import Path
import json
import sqlite3
import sys

service_env = Path(sys.argv[1])
root = Path(sys.argv[2])

def parse(path: Path) -> dict[str, str]:
    result: dict[str, str] = {}
    for raw in path.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        result[key.strip()] = value.strip().strip('"').strip("'")
    return result

database_url = parse(service_env).get("MORROWS_DATABASE_URL", "")
if database_url.startswith("sqlite://"):
    database_path = database_url[len("sqlite://"):]
    path = Path(database_path)
    if not path.is_absolute():
        path = root / path
    legacy_files = [
        Path("/home/morrow/.local/state/morrow-runtime/oauth-clients.json"),
        Path("/home/morrow/.local/state/local-shell-mcp/oauth-clients.json"),
    ]
    imported: dict[str, dict] = {}
    for legacy_file in legacy_files:
        if not legacy_file.is_file():
            continue
        try:
            payload = json.loads(legacy_file.read_text(encoding="utf-8"))
        except Exception:
            continue
        clients = payload.get("clients", {}) if isinstance(payload, dict) else {}
        if not isinstance(clients, dict):
            continue
        for client_id, row in clients.items():
            if not isinstance(client_id, str) or not isinstance(row, dict):
                continue
            redirects = row.get("redirect_uris")
            if not isinstance(redirects, list) or not all(isinstance(v, str) for v in redirects):
                continue
            imported[client_id] = {
                "name": str(row.get("client_name") or "Migrated OAuth client")[:256],
                "redirects": redirects[:10],
            }
    if imported:
        connection = sqlite3.connect(path)
        try:
            raw = connection.execute(
                "SELECT state FROM morrows_oauth_state WHERE id=1"
            ).fetchone()[0]
            state = json.loads(raw)
            clients = state.setdefault("clients", {})
            for client_id, row in imported.items():
                clients.setdefault(client_id, row)
            connection.execute(
                "UPDATE morrows_oauth_state SET state=? WHERE id=1",
                (json.dumps(state, separators=(",", ":")),),
            )
            connection.commit()
        finally:
            connection.close()
PY

    # Morrows now owns /morrows and the Cloudflare connector lifecycle. The
    # root fallback remains standalone LSM, but an LSM restart cannot remove
    # the tunnel or the Morrows routes.
    sudo install -m 0644 deploy/morrows-edge.caddy /etc/caddy/morrows-router.caddy
    cmp -s deploy/morrows-edge.caddy /etc/caddy/morrows-router.caddy
    if ! sudo grep -Fxq 'import /etc/caddy/morrows-router.caddy' /etc/caddy/Caddyfile; then
      printf '\n# Morrows / standalone LSM shared loopback edge\nimport /etc/caddy/morrows-router.caddy\n' |
        sudo tee -a /etc/caddy/Caddyfile >/dev/null
    fi
    sudo caddy validate --config /etc/caddy/Caddyfile
    sudo systemctl reload caddy.service

    sudo systemctl daemon-reload
    sudo systemctl enable --now morrows-cloudflared.service
    edge_deadline=$((SECONDS + 30))
    while (( SECONDS < edge_deadline )); do
      if curl -fsS http://127.0.0.1:20243/metrics 2>/dev/null |
        awk '/cloudflared_tunnel_ha_connections/ && $2 + 0 > 0 {ok=1} END {exit !ok}'; then
        break
      fi
      sleep 1
    done
    curl -fsS http://127.0.0.1:20243/metrics |
      awk '/cloudflared_tunnel_ha_connections/ && $2 + 0 > 0 {ok=1} END {exit !ok}'
    sudo systemctl disable --now local-shell-mcp-cloudflared.service >/dev/null 2>&1 || true
    test "$(systemctl is-active morrows-cloudflared.service)" = active
    ! systemctl show morrows-cloudflared.service -p Requires --value | grep -q 'local-shell-mcp'
    curl -fsS https://mcp.xycdev.com/morrows/health >/dev/null
    # Discovery must name the MCP resource, not the separately mounted issuer.
    curl -fsS https://mcp.xycdev.com/.well-known/oauth-protected-resource/morrows |
      "$root/morrow-runtime/.venv/bin/python" -c 'import json,sys; metadata=json.load(sys.stdin); assert metadata["resource"] == "https://mcp.xycdev.com/morrows"; assert metadata["authorization_servers"] == ["https://mcp.xycdev.com/morrows/auth"]'

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
sudo journalctl -u morrow-runtime.service -n 120 --no-pager >&2 || true
exit 1
REMOTE_SCRIPT

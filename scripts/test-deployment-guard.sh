#!/usr/bin/env bash
# Exercise production release matching without starting a daemon or reading secrets.
set -euo pipefail
source_root="$(cd "$(dirname "$0")/.." && pwd)"
fixture="$(mktemp -d "${TMPDIR:-/tmp}/morrows-guard.XXXXXX")"
trap 'rm -rf "$fixture"' EXIT
mkdir -p "$fixture/repo/scripts" "$fixture/repo/web/dist" "$fixture/artifacts/web"
cp "$source_root/scripts/run-vps.sh" "$fixture/repo/scripts/run-vps.sh"
cp "$source_root/scripts/run-morrow-runtime-vps.sh" "$fixture/repo/scripts/run-morrow-runtime-vps.sh"
printf '/web/dist/\n/server\n' > "$fixture/repo/.gitignore"
printf '#!/bin/sh\necho guarded-server-started\n' > "$fixture/repo/server"
chmod +x "$fixture/repo/server"
printf 'test frontend\n' > "$fixture/repo/web/dist/index.html"
printf 'test unit\n' > "$fixture/unit"
printf 'test runtime unit\n' > "$fixture/runtime-unit"
printf 'MORROWS_RUNTIME_CONTROL_KEY=test-runtime-control-key\n' > "$fixture/runtime.env"
git -C "$fixture/repo" init -q
git -C "$fixture/repo" add .
git -C "$fixture/repo" -c user.name=Test -c user.email=test@local commit -qm fixture
cp "$fixture/repo/server" "$fixture/artifacts/server"
cp "$fixture/repo/scripts/run-vps.sh" "$fixture/artifacts/launcher"
cp "$fixture/repo/scripts/run-morrow-runtime-vps.sh" "$fixture/artifacts/runtime-launcher"
cp "$fixture/unit" "$fixture/artifacts/unit"
cp "$fixture/runtime-unit" "$fixture/artifacts/runtime-unit"
cp "$fixture/repo/web/dist/index.html" "$fixture/artifacts/web/index.html"
printf 'commit=%s\nartifacts=%s\n' "$(git -C "$fixture/repo" rev-parse HEAD)" "$fixture/artifacts" > "$fixture/guard"
export MORROWS_ROOT="$fixture/repo"
export MORROWS_SERVER_BIN="$fixture/repo/server"
export MORROWS_DEPLOY_GUARD_FILE="$fixture/guard"
export MORROWS_SYSTEMD_UNIT="$fixture/unit"
export MORROWS_RUNTIME_SYSTEMD_UNIT="$fixture/runtime-unit"
export MORROWS_RUNTIME_ENV="$fixture/runtime.env"
launch() { bash "$fixture/repo/scripts/run-vps.sh"; }
expect_rejected() {
  local status=0
  launch > "$fixture/output" 2>&1 || status=$?
  test "$status" -eq 78
  grep -q 'refusing to start an unmanaged' "$fixture/output"
}
test "$(launch)" = guarded-server-started
for path in server web/dist/index.html scripts/run-vps.sh scripts/run-morrow-runtime-vps.sh; do
  cp "$fixture/repo/$path" "$fixture/original"
  printf '\n# unexpected replacement\n' >> "$fixture/repo/$path"
  expect_rejected
  cp "$fixture/original" "$fixture/repo/$path"
done
printf 'different unit\n' > "$fixture/unit"
expect_rejected
cp "$fixture/artifacts/unit" "$fixture/unit"
printf 'different runtime unit\n' > "$fixture/runtime-unit"
expect_rejected
cp "$fixture/artifacts/runtime-unit" "$fixture/runtime-unit"
printf 'extra asset\n' > "$fixture/repo/web/dist/extra.js"
expect_rejected
rm "$fixture/repo/web/dist/extra.js"
test "$(launch)" = guarded-server-started
printf 'deployment guard tests passed\n'

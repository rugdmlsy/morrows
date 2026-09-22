#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
if [[ ! -f web/dist/index.html ]]; then
  (
    cd web
    npm install
    npm run build
  )
fi
exec cargo run -p ac-server

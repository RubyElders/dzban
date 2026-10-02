#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
binary="$repo_root/target/debug/dzban"

if [[ ! -x "$binary" ]]; then
    echo "Development binary not found: $binary" >&2
    echo "Run: cargo build -p dzban-ui" >&2
    exit 1
fi

cd "$repo_root"
exec "$binary" "$@"

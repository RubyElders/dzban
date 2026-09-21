#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
compose_file="$repo_root/docker-compose.test.yml"

cleanup() {
    docker compose -f "$compose_file" down
}
trap cleanup EXIT

cd "$repo_root"
docker compose -f "$compose_file" up -d --wait
CCACHE_DISABLE=1 cargo +1.88.0 test -p dzban-db -- --ignored --test-threads=1

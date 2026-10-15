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
CCACHE_DISABLE=1 cargo +1.88.0 test -p dzban-db -- --ignored --skip certificate_validation_and_cancel_recovery --test-threads=1
CCACHE_DISABLE=1 cargo +1.88.0 test -p dzban-cli --test database -- --ignored --test-threads=1
DZBAN_TEST_DATABASES=1 bash scripts/test-cli.sh
CCACHE_DISABLE=1 cargo +1.88.0 test -p dzban-gtk --test connection_integration_tests --test clickhouse_integration_tests --test streamed_connection_tests --test runtime_bridge_isolation_tests --test config_tests --test manager_tests --test query_tab_tests -- --ignored --test-threads=1

#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
compose_file="$repo_root/docker-compose.test.yml"
binary="$repo_root/target/debug/dzban"

cleanup() {
    docker compose -f "$compose_file" down
}
trap cleanup EXIT

cd "$repo_root"
CCACHE_DISABLE=1 cargo +1.88.0 build -p dzban-ui
docker compose -f "$compose_file" up -d --wait postgres15

system_paths=$(
    /usr/bin/python3 -c 'import gi; from pathlib import Path; print(Path(gi.__file__).parent.parent)'
)
if [[ -n ${DZBAN_UI_SCENARIO:-} ]]; then
    scenarios=("$DZBAN_UI_SCENARIO")
else
    scenarios=(empty database)
fi

for scenario in "${scenarios[@]}"; do
    PYTHONPATH="$system_paths${PYTHONPATH:+:$PYTHONPATH}" \
        NO_AT_BRIDGE=1 \
        DZBAN_BINARY="$binary" \
        DZBAN_UI_SCENARIO="$scenario" \
        uv run --python /usr/bin/python3 tests/ui/dzban_screens.py
done

#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
CCACHE_DISABLE=1 cargo +1.88.0 build --locked -p dzban-cli --bin dzb
shellspec spec/cli_spec.sh

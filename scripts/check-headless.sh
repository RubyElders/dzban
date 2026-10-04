#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

export CCACHE_DISABLE=1
export PKG_CONFIG=/bin/false
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$repo_root/target/headless}"
toolchain="${DZBAN_TEST_TOOLCHAIN:-1.88.0}"

cargo "+$toolchain" build --locked -p dzban-cli --bin dzb
cargo "+$toolchain" metadata --locked --format-version 1 > /dev/null
cargo "+$toolchain" test --locked -p dzban-core -p dzban-db -p dzban-cli --all-targets

#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

test_root=$(mktemp -d "${TMPDIR:-/tmp}/dzban-headless.XXXXXX")
trap 'rm -rf "$test_root"' EXIT
export DZBAN_CONFIG=/dev/null
export XDG_CONFIG_HOME="$test_root/config"
export XDG_CONFIG_DIRS="$test_root/system-config"
export XDG_DATA_HOME="$test_root/data"
export CCACHE_DISABLE=1
export PKG_CONFIG=/bin/false
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$repo_root/target/headless}"
toolchain="${DZBAN_TEST_TOOLCHAIN:-1.88.0}"

cargo "+$toolchain" build --locked -p dzban-cli --bin dzb
bash scripts/check-portable.sh
cargo "+$toolchain" metadata --locked --format-version 1 > /dev/null
cargo "+$toolchain" test --locked -p dzban-core -p dzban-db -p dzban-cli --all-targets
cargo "+$toolchain" tree --color never --locked -p dzban-macos-bridge \
    --all-features --target all --edges normal,build,dev --prefix none --format '{p}' \
    | bash scripts/check-portable.sh --check-tree
cargo "+$toolchain" test --locked -p dzban-macos-bridge --all-targets --all-features

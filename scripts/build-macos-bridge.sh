#!/usr/bin/env bash
set -euo pipefail
repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"
export MACOSX_DEPLOYMENT_TARGET=14.0
export CARGO_TARGET_DIR="$repo_root/target/macos-rust"
toolchain=${DZBAN_MACOS_TOOLCHAIN:-1.98.1}
case ${1:-} in
    '') cargo "+$toolchain" build --locked -p dzban-macos-bridge ;;
    --fixtures) cargo "+$toolchain" build --locked -p dzban-macos-bridge --features test-fixtures ;;
    *) echo 'Usage: bash scripts/build-macos-bridge.sh [--fixtures]' >&2; exit 2 ;;
esac

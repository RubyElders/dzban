#!/usr/bin/env bash
set -euo pipefail
repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"
export MACOSX_DEPLOYMENT_TARGET=14.0
export CARGO_TARGET_DIR="${DZBAN_RUST_TARGET_DIR:-$repo_root/target/macos-rust}"
toolchain=${DZBAN_MACOS_TOOLCHAIN:-1.98.1}
case ${DZBAN_RUST_PROFILE:-debug} in
    debug) cargo_profile=dev ;;
    release) cargo_profile=release ;;
    *) echo 'DZBAN_RUST_PROFILE must be debug or release' >&2; exit 2 ;;
esac
case ${1:-} in
    '') cargo "+$toolchain" build --locked --profile "$cargo_profile" -p dzban-macos-bridge ;;
    --fixtures) cargo "+$toolchain" build --locked --profile "$cargo_profile" -p dzban-macos-bridge --features test-fixtures ;;
    *) echo 'Usage: bash scripts/build-macos-bridge.sh [--fixtures]' >&2; exit 2 ;;
esac

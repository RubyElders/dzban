#!/usr/bin/env bash
set -euo pipefail
repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"
test_root=$(mktemp -d "${TMPDIR:-/tmp}/dzban-native-tests.XXXXXX")
trap 'rm -rf "$test_root"' EXIT
export DZBAN_CONFIG=/dev/null
export XDG_CONFIG_HOME="$test_root/config"
export XDG_CONFIG_DIRS="$test_root/system-config"
export XDG_DATA_HOME="$test_root/data"
export TMPDIR="$test_root/tmp"
mkdir -p "$TMPDIR"
export MACOSX_DEPLOYMENT_TARGET=14.0
bash scripts/build-macos-bridge.sh --fixtures
cargo +"${DZBAN_MACOS_TOOLCHAIN:-1.98.1}" test --locked -p dzban-macos-bridge --all-targets --features test-fixtures
swift_arguments=(--package-path macos --scratch-path target/macos-swift
    -Xcc -DDZBAN_TEST_FIXTURES -Xswiftc -DDZBAN_TEST_FIXTURES
    -Xlinker "$repo_root/target/macos-rust/debug/libdzban_macos_bridge.a"
    -Xlinker -framework -Xlinker CoreFoundation
    -Xlinker -framework -Xlinker SystemConfiguration
    -Xlinker -framework -Xlinker Security
    -Xlinker -framework -Xlinker CoreFoundation
    -Xlinker -liconv -Xlinker -lSystem -Xlinker -lc -Xlinker -lm)
swift run "${swift_arguments[@]}" DzbanBridgeCheck
if xcrun --find xctest >/dev/null 2>&1; then
    swift test "${swift_arguments[@]}"
else
    echo 'Skipped XCTest: full Xcode is required. Standalone Swift lifecycle scenarios ran.'
fi

python3 - <<'PY'
import os
from pathlib import Path
remaining = list(Path(os.environ['TMPDIR']).glob('dzban-*'))
assert not remaining, f'Bridge scenarios retained temporary artifacts: {remaining}'
print('Bridge scenarios released temporary result and history artifacts')
PY

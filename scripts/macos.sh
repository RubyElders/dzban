#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat >&2 <<'EOF'
Usage: bash scripts/macos.sh COMMAND

  bridge [--fixtures]                  Build the Rust bridge archive for Xcode
  resources WORK_DIRECTORY RESOURCES   Build the application icon for Xcode
  build [--fixtures]                   Build the debug application
  test                                 Run bridge, Swift and UI tests with fixtures
  test-bridge                          Run Rust bridge and Swift package tests
  build-ui-tests                       Build the application and UI tests with fixtures
  test-ui SHARD COUNT                  Run one of COUNT shares of the built UI tests
  test-live [APP_BUNDLE]               Run live database UI tests against Compose fixtures
EOF
    exit 2
}

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"
toolchain=${DZBAN_MACOS_TOOLCHAIN:-1.98.1}
fixture_settings=(DZBAN_MACOS_FIXTURES=YES DZBAN_BUNDLE_IDENTIFIER=com.rubyelders.dzban.macos.dev.fixtures
    'SWIFT_ACTIVE_COMPILATION_CONDITIONS=$(inherited) DEBUG DZBAN_TEST_FIXTURES'
    'OTHER_SWIFT_FLAGS=$(inherited) -Xcc -DDZBAN_TEST_FIXTURES')
xcode_debug=(-project macos/Dzban.xcodeproj -scheme Dzban -configuration Debug
    -destination 'platform=macOS,arch=arm64')

bridge() {
    export MACOSX_DEPLOYMENT_TARGET=14.0
    export CARGO_TARGET_DIR="${DZBAN_RUST_TARGET_DIR:-$repo_root/target/macos-rust}"
    local cargo_profile
    case ${DZBAN_RUST_PROFILE:-debug} in
        debug) cargo_profile=dev ;;
        release) cargo_profile=release ;;
        *) echo 'DZBAN_RUST_PROFILE must be debug or release' >&2; exit 2 ;;
    esac
    case ${1:-} in
        '') cargo "+$toolchain" build --locked --profile "$cargo_profile" -p dzban-macos-bridge ;;
        --fixtures) cargo "+$toolchain" build --locked --profile "$cargo_profile" -p dzban-macos-bridge --features test-fixtures ;;
        *) usage ;;
    esac
}

resources() {
    [[ $# == 2 ]] || usage
    local iconset="$1/Dzban.iconset" icon_source="$repo_root/macos/Resources/DzbanIcon.png" size doubled
    mkdir -p "$iconset" "$2"
    for size in 16 32 128 256 512; do
        /usr/bin/sips -z "$size" "$size" "$icon_source" --out "$iconset/icon_${size}x${size}.png" >/dev/null
        doubled=$((size * 2))
        /usr/bin/sips -z "$doubled" "$doubled" "$icon_source" --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
    done
    /usr/bin/iconutil -c icns "$iconset" -o "$2/Dzban.icns"
}

build() {
    local arguments=(DZBAN_MACOS_FIXTURES=NO) variant=macos-xcode
    case ${1:-} in
        '') ;;
        --fixtures)
            variant=macos-xcode-fixtures
            arguments=("${fixture_settings[@]}")
            ;;
        *) usage ;;
    esac
    xcodebuild "${xcode_debug[@]}" -derivedDataPath "$repo_root/target/$variant" "${arguments[@]}" build
    echo "Application: $repo_root/target/$variant/Build/Products/Debug/Dzban.app"
}

test_bridge() {
    test_root=$(mktemp -d "${TMPDIR:-/tmp}/dzban-native-tests.XXXXXX")
    trap 'rm -rf "$test_root"' EXIT
    export DZBAN_CONFIG=/dev/null
    export XDG_CONFIG_HOME="$test_root/config"
    export XDG_CONFIG_DIRS="$test_root/system-config"
    export XDG_DATA_HOME="$test_root/data"
    export TMPDIR="$test_root/tmp"
    mkdir -p "$TMPDIR"
    export MACOSX_DEPLOYMENT_TARGET=14.0
    (bridge --fixtures)
    cargo "+$toolchain" test --locked -p dzban-macos-bridge --all-targets --features test-fixtures
    local swift_arguments=(--package-path macos --scratch-path target/macos-swift
        -Xcc -DDZBAN_TEST_FIXTURES -Xswiftc -DDZBAN_TEST_FIXTURES
        -Xlinker "$repo_root/target/macos-rust/debug/libdzban_macos_bridge.a"
        -Xlinker -framework -Xlinker CoreFoundation
        -Xlinker -framework -Xlinker SystemConfiguration
        -Xlinker -framework -Xlinker Security
        -Xlinker -framework -Xlinker CoreFoundation
        -Xlinker -liconv -Xlinker -lSystem -Xlinker -lc -Xlinker -lm)
    swift run "${swift_arguments[@]}" DzbanBridgeCheck
    if xcrun --find xctest >/dev/null 2>&1; then
        local filter_arguments=()
        if [[ -n ${DZBAN_SWIFT_TEST_FILTER:-} ]]; then
            filter_arguments=(--filter "$DZBAN_SWIFT_TEST_FILTER")
        fi
        swift test "${swift_arguments[@]}" "${filter_arguments[@]}"
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
}

build_ui_tests() {
    xcodebuild "${xcode_debug[@]}" -derivedDataPath "$repo_root/target/macos-xcode-fixtures" \
        "${fixture_settings[@]}" build-for-testing
}

test_ui() {
    [[ $# == 2 && $1 =~ ^[0-9]+$ && $2 =~ ^[0-9]+$ ]] && (( 1 <= $1 && $1 <= $2 )) || usage
    local products="$repo_root/target/macos-xcode-fixtures/Build/Products" run name index=0 result_root tests=()
    run=$(find "$products" -maxdepth 1 -name '*.xctestrun' ! -name 'DzbanLive-*' | head -n 1)
    [[ -n $run ]] || { echo 'Run build-ui-tests first' >&2; exit 2; }
    while read -r name; do
        if (( index % $2 == $1 - 1 )); then tests+=("-only-testing:DzbanUITests/ShellTests/$name"); fi
        index=$((index + 1))
    done < <(grep -o 'func test[A-Za-z0-9_]*' macos/UITests/ShellTests.swift | cut -d ' ' -f 2 | sort)
    mkdir -p target
    result_root=$(mktemp -d "$repo_root/target/macos-ui-results.XXXXXX")
    xcodebuild -xctestrun "$run" -destination 'platform=macOS,arch=arm64' \
        -resultBundlePath "$result_root/Shell.xcresult" -parallel-testing-enabled NO \
        -test-timeouts-enabled YES -default-test-execution-time-allowance 180 \
        -maximum-test-execution-time-allowance 300 \
        "${tests[@]}" test-without-building
}

test_all() {
    (test_bridge)
    build_ui_tests
    test_ui 1 1
}

test_live() {
    (( $# <= 1 )) || usage
    app_bundle=${1:-}
    if [[ -n "$app_bundle" ]]; then app_bundle=$(cd "$app_bundle" && pwd); fi
    mkdir -p target
    fixture_root=$(mktemp -d "$repo_root/target/macos-live.XXXXXX")
    project="dzban-native-$(basename "$fixture_root" | tr '[:upper:].' '[:lower:]-')"
    compose=(docker compose -p "$project" -f "$fixture_root/compose.json")
    cleanup() {
        status=$?
        trap - EXIT
        if (( status != 0 )); then
            "${compose[@]}" ps > "$fixture_root/services.log" 2>&1 || true
            "${compose[@]}" logs --no-color --tail 100 >> "$fixture_root/services.log" 2>&1 || true
        fi
        if ! "${compose[@]}" down --volumes --remove-orphans; then status=1; fi
        if [[ -n ${live_run:-} ]]; then rm -f "$live_run"; fi
        if [[ -n ${installed_root:-} ]]; then rm -rf "$installed_root"; fi
        exit "$status"
    }
    docker compose -p "$project" -f docker-compose.test.yml config --format json > "$fixture_root/base.json"
    python3 - "$fixture_root" <<'PY'
import json
from pathlib import Path
import sys
root = Path(sys.argv[1])
specification = json.loads((root / "base.json").read_text())
specification["services"] = {name: service for name, service in specification["services"].items() if name in ("postgres15", "clickhouse")}
for service in specification["services"].values():
    service.pop("container_name", None)
    for port in service["ports"]:
        port["host_ip"] = "127.0.0.1"
        port["published"] = "0"
(root / "compose.json").write_text(json.dumps(specification))
PY
    trap cleanup EXIT
    local expected=3 installed_app= pg_port ch_port
    local workflows=(-only-testing:DzbanUITests/ShellTests/testLivePostgreSQLWorkflow
        -only-testing:DzbanUITests/ShellTests/testLiveClickHouseWorkflow)
    if [[ -n "$app_bundle" ]]; then
        installed_root=$(mktemp -d /tmp/dzban-native-installed.XXXXXX)
        installed_app="$installed_root/Dzban.app"
        ditto "$app_bundle" "$installed_app"
        expected=2
    else
        workflows+=(-only-testing:DzbanUITests/ShellTests/testNativeAppearanceResizeAndAccessibleInspection)
    fi
    "${compose[@]}" up -d --wait
    pg_port=$("${compose[@]}" port postgres15 5432)
    ch_port=$("${compose[@]}" port clickhouse 8123)
    xcodebuild "${xcode_debug[@]}" -derivedDataPath "$repo_root/target/macos-xcode-fixtures" \
        "${fixture_settings[@]}" build-for-testing
    live_run="$repo_root/target/macos-xcode-fixtures/Build/Products/DzbanLive-$(basename "$fixture_root").xctestrun"
    python3 - "$repo_root/target/macos-xcode-fixtures/Build/Products" "$live_run" "$installed_app" "${pg_port##*:}" "${ch_port##*:}" <<'PYTEST'
from pathlib import Path
import plistlib
import sys
products = Path(sys.argv[1])
candidates = [path for path in products.glob('*.xctestrun') if not path.name.startswith('DzbanLive-')]
source = max(candidates, key=lambda path: path.stat().st_mtime_ns)
specification = plistlib.loads(source.read_bytes())
if 'TestConfigurations' in specification:
    targets = [target for configuration in specification['TestConfigurations'] for target in configuration['TestTargets']]
else:
    targets = [target for target in specification.values() if isinstance(target, dict) and 'TestBundlePath' in target]
assert targets, 'No test runner targets in generated xctestrun'
for target in targets:
    environment = target.setdefault('EnvironmentVariables', {})
    environment['DZBAN_TEST_DATABASES'] = '1'
    environment['DZBAN_TEST_PG_PORT'] = sys.argv[4]
    environment['DZBAN_TEST_CH_PORT'] = sys.argv[5]
    if sys.argv[3]:
        original = target['UITargetAppPath']
        target['UITargetAppPath'] = sys.argv[3]
        target['DependentProductPaths'] = [sys.argv[3] if path == original else path for path in target.get('DependentProductPaths', [])]
Path(sys.argv[2]).write_bytes(plistlib.dumps(specification))
PYTEST
    xcodebuild -xctestrun "$live_run" -destination 'platform=macOS,arch=arm64' \
        -resultBundlePath "$fixture_root/Native.xcresult" -parallel-testing-enabled NO \
        "${workflows[@]}" test-without-building
    xcrun xcresulttool get test-results summary --path "$fixture_root/Native.xcresult" > "$fixture_root/summary.json"
    python3 - "$fixture_root/summary.json" "$expected" <<'PYRESULT'
import json
import sys
summary = json.load(open(sys.argv[1]))
assert summary['skippedTests'] == 0, 'Live gate skipped required tests'
assert summary['failedTests'] == 0 and summary['passedTests'] == int(sys.argv[2]), 'Live gate did not pass every required workflow'
PYRESULT
}

command=${1:-}
[[ -n $command ]] || usage
shift
case $command in
    bridge) bridge "$@" ;;
    resources) resources "$@" ;;
    build) build "$@" ;;
    test) [[ $# == 0 ]] || usage; test_all ;;
    test-bridge) [[ $# == 0 ]] || usage; test_bridge ;;
    build-ui-tests) [[ $# == 0 ]] || usage; build_ui_tests ;;
    test-ui) test_ui "$@" ;;
    test-live) test_live "$@" ;;
    *) usage ;;
esac

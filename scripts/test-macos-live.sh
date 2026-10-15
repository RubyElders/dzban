#!/usr/bin/env bash
set -euo pipefail
repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"
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
(root / "compose.json").write_text(json.dumps(specification))
PY
trap cleanup EXIT
"${compose[@]}" up -d --wait
xcode_arguments=(-project macos/Dzban.xcodeproj -scheme Dzban -configuration Debug
    -derivedDataPath "$repo_root/target/macos-xcode-fixtures" -destination 'platform=macOS,arch=arm64'
    DZBAN_MACOS_FIXTURES=YES DZBAN_BUNDLE_IDENTIFIER=com.rubyelders.dzban.macos.dev.fixtures
    'SWIFT_ACTIVE_COMPILATION_CONDITIONS=$(inherited) DEBUG DZBAN_TEST_FIXTURES'
    'OTHER_SWIFT_FLAGS=$(inherited) -Xcc -DDZBAN_TEST_FIXTURES')
xcodebuild "${xcode_arguments[@]}" build-for-testing
live_run="$repo_root/target/macos-xcode-fixtures/Build/Products/DzbanLive-$(basename "$fixture_root").xctestrun"
python3 - "$repo_root/target/macos-xcode-fixtures/Build/Products" "$live_run" <<'PYTEST'
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
    target.setdefault('EnvironmentVariables', {})['DZBAN_TEST_DATABASES'] = '1'
Path(sys.argv[2]).write_bytes(plistlib.dumps(specification))
PYTEST
xcodebuild -xctestrun "$live_run" -destination 'platform=macOS,arch=arm64' \
    -resultBundlePath "$fixture_root/Native.xcresult" -parallel-testing-enabled NO \
    -only-testing:DzbanUITests/ShellTests/testLivePostgreSQLWorkflow \
    -only-testing:DzbanUITests/ShellTests/testLiveClickHouseWorkflow \
    -only-testing:DzbanUITests/ShellTests/testNativeAppearanceResizeAndAccessibleInspection test-without-building
xcrun xcresulttool get test-results summary --path "$fixture_root/Native.xcresult" > "$fixture_root/summary.json"
python3 - "$fixture_root/summary.json" <<'PYRESULT'
import json
import sys
summary = json.load(open(sys.argv[1]))
assert summary['skippedTests'] == 0, 'Live gate skipped required tests'
assert summary['failedTests'] == 0 and summary['passedTests'] == 3, 'Live gate did not pass all three workflows'
PYRESULT

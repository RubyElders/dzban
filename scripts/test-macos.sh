#!/usr/bin/env bash
set -euo pipefail
repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"
bash scripts/test-macos-bridge.sh
mkdir -p target
result_root=$(mktemp -d "$repo_root/target/macos-ui-results.XXXXXX")
xcodebuild -project macos/Dzban.xcodeproj -scheme Dzban -configuration Debug \
    -derivedDataPath "$repo_root/target/macos-xcode-fixtures" -destination 'platform=macOS,arch=arm64' \
    -resultBundlePath "$result_root/Shell.xcresult" -parallel-testing-enabled NO \
    DZBAN_MACOS_FIXTURES=YES DZBAN_BUNDLE_IDENTIFIER=com.rubyelders.dzban.macos.dev.fixtures \
    'SWIFT_ACTIVE_COMPILATION_CONDITIONS=$(inherited) DEBUG DZBAN_TEST_FIXTURES' \
    'OTHER_SWIFT_FLAGS=$(inherited) -Xcc -DDZBAN_TEST_FIXTURES' test

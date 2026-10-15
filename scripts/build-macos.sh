#!/usr/bin/env bash
set -euo pipefail
repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"
arguments=(DZBAN_MACOS_FIXTURES=NO)
variant=macos-xcode
case ${1:-} in
    '') ;;
    --fixtures)
        variant=macos-xcode-fixtures
        arguments=(DZBAN_MACOS_FIXTURES=YES
            'SWIFT_ACTIVE_COMPILATION_CONDITIONS=$(inherited) DEBUG DZBAN_TEST_FIXTURES'
            'OTHER_SWIFT_FLAGS=$(inherited) -Xcc -DDZBAN_TEST_FIXTURES'
            DZBAN_BUNDLE_IDENTIFIER=com.rubyelders.dzban.macos.dev.fixtures)
        ;;
    *) echo 'Usage: bash scripts/build-macos.sh [--fixtures]' >&2; exit 2 ;;
esac
xcodebuild -project macos/Dzban.xcodeproj -scheme Dzban -configuration Debug \
    -derivedDataPath "$repo_root/target/$variant" -destination 'platform=macOS,arch=arm64' \
    "${arguments[@]}" build
echo "Application: $repo_root/target/$variant/Build/Products/Debug/Dzban.app"

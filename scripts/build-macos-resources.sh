#!/usr/bin/env bash
set -euo pipefail
repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
if [[ $# != 2 ]]; then
    echo 'Usage: bash scripts/build-macos-resources.sh WORK_DIRECTORY RESOURCE_DIRECTORY' >&2
    exit 2
fi
iconset="$1/Dzban.iconset"
mkdir -p "$iconset" "$2"
for size in 16 32 128 256 512; do
    /usr/bin/sips -z "$size" "$size" "$repo_root/data/icons/com.rubyelders.dzban.png" --out "$iconset/icon_${size}x${size}.png" >/dev/null
    doubled=$((size * 2))
    /usr/bin/sips -z "$doubled" "$doubled" "$repo_root/data/icons/com.rubyelders.dzban.png" --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
/usr/bin/iconutil -c icns "$iconset" -o "$2/Dzban.icns"

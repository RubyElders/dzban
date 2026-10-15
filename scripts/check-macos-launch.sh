#!/usr/bin/env bash
set -euo pipefail
if [[ $(uname -s) != Darwin || $# != 2 ]]; then
    echo 'Usage on macOS: bash scripts/check-macos-launch.sh APP_BUNDLE direct|launchservices' >&2
    exit 2
fi
app_bundle=$(cd "$1" && pwd)
mode=$2
case "$mode" in direct|launchservices) ;; *) echo 'Unknown launch mode' >&2; exit 2 ;; esac
launch_root=$(mktemp -d /tmp/dzban-native-launch.XXXXXX)
trap 'rm -rf "$launch_root"' EXIT
ditto "$app_bundle" "$launch_root/Dzban.app"
mkdir -p "$launch_root/home" "$launch_root/tmp" "$launch_root/config" "$launch_root/data"
cd "$launch_root"
environment=("HOME=$launch_root/home" 'PATH=/usr/bin:/bin' "TMPDIR=$launch_root/tmp/"
    'DZBAN_CONFIG=/dev/null' "XDG_CONFIG_HOME=$launch_root/config"
    "XDG_CONFIG_DIRS=$launch_root/system-config" "XDG_DATA_HOME=$launch_root/data")
printf 'Copied app: %s\nVerify the empty connection chooser, then quit with Command-Q.\n' "$launch_root/Dzban.app"
if [[ "$mode" == direct ]]; then
    env -i "${environment[@]}" "$launch_root/Dzban.app/Contents/MacOS/Dzban" -ApplePersistenceIgnoreState YES
else
    arguments=()
    for variable in "${environment[@]}"; do arguments+=(--env "$variable"); done
    /usr/bin/open -n -W -a "$launch_root/Dzban.app" "${arguments[@]}" --args -ApplePersistenceIgnoreState YES
fi
echo 'Application exited; copied bundle and disposable data removed on exit.'

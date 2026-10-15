#!/usr/bin/env bash
set -euo pipefail
if [[ $# != 2 || $2 != --submit ]]; then
    echo 'Usage: DZBAN_NOTARY_PROFILE=PROFILE bash scripts/notarize-macos.sh APP_BUNDLE --submit' >&2
    exit 2
fi
: "${DZBAN_NOTARY_PROFILE:?Set an explicit notarytool keychain profile}"
app_bundle=$(cd "$1" && pwd)
output_root=$(dirname "$app_bundle")
destination="$output_root/Dzban-macos-arm64-notarized.zip"
if [[ -e "$destination" || -e "$output_root/Notarization.json" ]]; then
    echo 'Notarization output already exists' >&2
    exit 2
fi
codesign --verify --deep --strict "$app_bundle"
python3 - "$app_bundle" <<'PY'
import subprocess
import sys
details = subprocess.run(['codesign', '-dv', '--verbose=4', sys.argv[1]], check=True, capture_output=True, text=True).stderr
assert 'Authority=Developer ID Application:' in details and 'runtime' in details, 'A hardened Developer ID build is required'
PY
submission_root=$(mktemp -d /tmp/dzban-notary.XXXXXX)
trap 'rm -rf "$submission_root"' EXIT
ditto -c -k --keepParent "$app_bundle" "$submission_root/Dzban.zip"
xcrun notarytool submit "$submission_root/Dzban.zip" --keychain-profile "$DZBAN_NOTARY_PROFILE" --wait --output-format json > "$output_root/Notarization.json"
python3 - "$output_root/Notarization.json" <<'PY'
import json
import sys
receipt = json.load(open(sys.argv[1]))
assert receipt.get('status') == 'Accepted', 'Apple did not accept the notarization submission'
PY
xcrun stapler staple "$app_bundle"
xcrun stapler validate "$app_bundle"
codesign --verify --deep --strict "$app_bundle"
spctl --assess --type execute --verbose "$app_bundle"
ditto -c -k --keepParent "$app_bundle" "$destination"
shasum -a 256 "$destination" > "$destination.sha256"
echo "Notarized artifact: $destination"

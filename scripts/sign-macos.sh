#!/usr/bin/env bash
set -euo pipefail
if [[ $# != 2 ]]; then
    echo 'Usage: DZBAN_SIGN_IDENTITY="Developer ID Application: ..." bash scripts/sign-macos.sh APP_BUNDLE RUST_ARCHIVE' >&2
    exit 2
fi
: "${DZBAN_SIGN_IDENTITY:?Set an explicit Developer ID Application signing identity}"
repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
python3 "$repo_root/scripts/verify-macos.py" "$1" "$2" >/dev/null
python3 - "$1" <<'PY'
import json
from pathlib import Path
import plistlib
import sys
app = Path(sys.argv[1])
info_path = app / 'Contents/Info.plist'
info = plistlib.loads(info_path.read_bytes())
info['CFBundleIdentifier'] = 'com.rubyelders.dzban'
info_path.write_bytes(plistlib.dumps(info))
report_path = app / 'Contents/Resources/BuildInfo.json'
report = json.loads(report_path.read_text())
report['signing'] = 'Developer ID Application'
report.pop('notarized', None)
report['notarizationProof'] = 'Verify the stapled ticket and external notarization receipt'
report_path.write_text(json.dumps(report, indent=2, sort_keys=True) + '\n')
PY
codesign --force --options runtime --timestamp --sign "$DZBAN_SIGN_IDENTITY" "$1"
codesign --verify --deep --strict "$1"
python3 - "$1" <<'PY'
import subprocess
import sys
details = subprocess.run(['codesign', '-dv', '--verbose=4', sys.argv[1]], check=True, capture_output=True, text=True).stderr
assert 'Authority=Developer ID Application:' in details, 'Expected a Developer ID Application certificate'
assert 'runtime' in details, 'Hardened runtime was not enabled'
PY
echo 'Developer ID signature verified. Notarization is a separate explicit submission.'

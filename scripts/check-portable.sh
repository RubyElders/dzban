#!/usr/bin/env bash
set -euo pipefail

check_tree() {
    local forbidden
    forbidden=$(awk '{print $1}' | sort -u | grep -E '^(dzban-(gtk|cli)|gtk[0-9]*(-.*)?|gdk[0-9]*(-.*)?|glib(-.*)?|gio(-.*)?|gobject(-.*)?|libadwaita(-.*)?|pango(-.*)?|cairo-rs|cairo-sys-rs|gsk[0-9]*(-.*)?|objc2-app-kit|objc2-ui-kit|cocoa|swiftui|ratatui(-.*)?|tui|crossterm(-.*)?|termion|reedline|rustyline|inquire|dialoguer|clap(-.*)?|comfy-table|tabled)$' || true)
    if [[ -n "$forbidden" ]]; then
        printf 'Frontend dependencies in the shared layer:\n%s\n' "$forbidden" >&2
        return 1
    fi
}

if [[ ${1:-} == --check-tree && $# == 1 ]]; then
    check_tree
    exit
fi
if [[ $# != 0 ]]; then
    printf 'Usage: bash scripts/check-portable.sh\n' >&2
    exit 2
fi

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"
toolchain=${DZBAN_TEST_TOOLCHAIN:-1.88.0}
cargo "+$toolchain" tree --color never --locked -p dzban-core -p dzban-db \
    --all-features --target all --edges normal,build,dev --prefix none --format '{p}' | check_tree
printf 'Shared core and adapters have no frontend dependencies.\n'

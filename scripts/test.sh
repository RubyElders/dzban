#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat >&2 <<'EOF'
Usage: bash scripts/test.sh COMMAND

  portable     Check that the shared crates have no frontend dependencies
  headless     Build and test core, adapters, CLI and the macOS bridge without native libraries
  gtk          Run GTK widget and lifecycle tests under Xvfb
  cli          Run ShellSpec CLI process contracts
  databases    Run PostgreSQL, ClickHouse and terminal tests against Compose fixtures
  screens      Run Dogtail screen workflows against Compose fixtures
  display      Open a shell with a private Xvfb display and D-Bus session
EOF
    exit 2
}

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"
toolchain=${DZBAN_TEST_TOOLCHAIN:-1.98.1}
compose_file="$repo_root/docker-compose.test.yml"
export CCACHE_DISABLE=1

check_tree() {
    local forbidden
    forbidden=$(awk '{print $1}' | sort -u | grep -E '^(dzban-(gtk|cli)|gtk[0-9]*(-.*)?|gdk[0-9]*(-.*)?|glib(-.*)?|gio(-.*)?|gobject(-.*)?|libadwaita(-.*)?|pango(-.*)?|cairo-rs|cairo-sys-rs|gsk[0-9]*(-.*)?|objc2-app-kit|objc2-ui-kit|cocoa|swiftui|ratatui(-.*)?|tui|crossterm(-.*)?|termion|reedline|rustyline|inquire|dialoguer|clap(-.*)?|comfy-table|tabled)$' || true)
    if [[ -n "$forbidden" ]]; then
        printf 'Frontend dependencies in the shared layer:\n%s\n' "$forbidden" >&2
        return 1
    fi
}

dependency_tree() {
    cargo "+$toolchain" tree --color never --locked "$@" \
        --all-features --target all --edges normal,build,dev --prefix none --format '{p}'
}

portable() {
    dependency_tree -p dzban-core -p dzban-db | check_tree
    printf 'Shared core and adapters have no frontend dependencies.\n'
}

headless() {
    test_root=$(mktemp -d "${TMPDIR:-/tmp}/dzban-headless.XXXXXX")
    trap 'rm -rf "$test_root"' EXIT
    export DZBAN_CONFIG=/dev/null
    export XDG_CONFIG_HOME="$test_root/config"
    export XDG_CONFIG_DIRS="$test_root/system-config"
    export XDG_DATA_HOME="$test_root/data"
    export PKG_CONFIG=/bin/false
    export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$repo_root/target/headless}"
    cargo "+$toolchain" build --locked -p dzban-cli --bin dzb
    portable
    cargo "+$toolchain" metadata --locked --format-version 1 > /dev/null
    cargo "+$toolchain" test --locked -p dzban-core -p dzban-db -p dzban-cli --all-targets
    dependency_tree -p dzban-macos-bridge | check_tree
    cargo "+$toolchain" test --locked -p dzban-macos-bridge --all-targets --all-features
}

in_display() {
    env -u WAYLAND_DISPLAY GDK_BACKEND=x11 GTK_A11Y=none \
        dbus-run-session -- xvfb-run -a -s "-screen 0 1400x900x24" "$@"
}

gtk() {
    test_root=$(mktemp -d "${TMPDIR:-/tmp}/dzban-gtk-tests.XXXXXX")
    trap 'rm -rf "$test_root"' EXIT
    DZBAN_CONFIG=/dev/null XDG_DATA_HOME="$test_root/data" in_display \
        cargo "+$toolchain" test --locked -p dzban-gtk --bin dzban --test result_model \
        -- --include-ignored --test-threads=1
}

cli() {
    cargo "+$toolchain" build --locked -p dzban-cli --bin dzb
    shellspec spec/cli_spec.sh
}

with_fixtures() {
    trap 'docker compose -f "$compose_file" down' EXIT
    docker compose -f "$compose_file" up -d --wait "$@"
}

databases() {
    with_fixtures
    cargo "+$toolchain" test --locked -p dzban-db -- --ignored \
        --skip certificate_validation_and_cancel_recovery --test-threads=1
    cargo "+$toolchain" test --locked -p dzban-cli --test database -- --ignored --test-threads=1
    DZBAN_TEST_DATABASES=1 cli
}

screens() {
    local system_paths scenarios scenario
    cargo "+$toolchain" build --locked -p dzban-gtk
    with_fixtures postgres15 clickhouse
    system_paths=$(/usr/bin/python3 -c 'import gi; from pathlib import Path; print(Path(gi.__file__).parent.parent)')
    if [[ -n ${DZBAN_UI_SCENARIO:-} ]]; then
        scenarios=("$DZBAN_UI_SCENARIO")
    else
        scenarios=(empty database clickhouse shutdown restore streaming-postgres streaming-clickhouse result-grid)
    fi
    for scenario in "${scenarios[@]}"; do
        PYTHONPATH="$system_paths${PYTHONPATH:+:$PYTHONPATH}" \
            NO_AT_BRIDGE=1 \
            DZBAN_BINARY="$repo_root/target/debug/dzban" \
            DZBAN_UI_SCENARIO="$scenario" \
            uv run --python /usr/bin/python3 tests/ui/dzban_screens.py
    done
}

if [[ ${1:-} == portable && ${2:-} == --check-tree && $# == 2 ]]; then
    check_tree
    exit
fi
[[ $# == 1 ]] || usage
case $1 in
    portable | headless | gtk | cli | databases | screens) "$1" ;;
    display) in_display bash ;;
    *) usage ;;
esac

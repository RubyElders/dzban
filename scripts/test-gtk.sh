#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
test_root=$(mktemp -d /tmp/dzban-gtk-tests.XXXXXX)
trap 'rm -rf "$test_root"' EXIT

cd "$repo_root"
env -u WAYLAND_DISPLAY \
    GDK_BACKEND=x11 GTK_A11Y=none CCACHE_DISABLE=1 \
    DZBAN_CONFIG=/dev/null XDG_DATA_HOME="$test_root/data" \
    dbus-run-session -- xvfb-run -a \
    cargo +1.88.0 test -p dzban-gtk --bin dzban --test connection_lifecycle --test result_model \
    -- --ignored --test-threads=1

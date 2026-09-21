#!/usr/bin/env bash
set -euo pipefail

unset WAYLAND_DISPLAY
export GDK_BACKEND=x11
export GTK_A11Y=none

exec xvfb-run -a -s "-screen 0 1400x900x24" dbus-run-session -- bash

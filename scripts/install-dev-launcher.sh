#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
binary="$repo_root/target/debug/dzban"
launcher="$repo_root/scripts/run-dev.sh"
desktop_template="$repo_root/data/com.fuga.dzban.desktop.in"
icon_source="$repo_root/data/icons/com.fuga.dzban.png"
data_home=${XDG_DATA_HOME:-"$HOME/.local/share"}
applications_dir="$data_home/applications"
icon_theme_dir="$data_home/icons/hicolor"
desktop_file="$applications_dir/com.fuga.dzban.desktop"

if [[ ! -x "$binary" ]]; then
    echo "Development binary not found: $binary" >&2
    echo "Run: cargo build -p dzban-gtk" >&2
    exit 1
fi

if command -v magick >/dev/null 2>&1; then
    image_command=(magick)
elif command -v convert >/dev/null 2>&1; then
    image_command=(convert)
else
    echo "ImageMagick is required to create launcher icons." >&2
    exit 1
fi

temporary_dir=$(mktemp -d)
trap 'rm -rf "$temporary_dir"' EXIT

escaped_launcher=${launcher//\\/\\\\}
escaped_launcher=${escaped_launcher//&/\\&}
escaped_launcher=${escaped_launcher//|/\\|}
escaped_repo_root=${repo_root//\\/\\\\}
escaped_repo_root=${escaped_repo_root//&/\\&}
escaped_repo_root=${escaped_repo_root//|/\\|}
sed \
    -e "s|@LAUNCHER@|$escaped_launcher|g" \
    -e "s|@WORKING_DIRECTORY@|$escaped_repo_root|g" \
    "$desktop_template" > "$temporary_dir/com.fuga.dzban.desktop"

if command -v desktop-file-validate >/dev/null 2>&1; then
    desktop-file-validate "$temporary_dir/com.fuga.dzban.desktop"
fi

install -Dm644 "$temporary_dir/com.fuga.dzban.desktop" "$desktop_file"

for size in 32 48 64 128 256 512; do
    rendered_icon="$temporary_dir/com.fuga.dzban-${size}.png"
    "${image_command[@]}" "$icon_source" -strip -filter Lanczos -resize "${size}x${size}" "$rendered_icon"
    install -Dm644 "$rendered_icon" "$icon_theme_dir/${size}x${size}/apps/com.fuga.dzban.png"
done

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$applications_dir"
fi

if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache --force --ignore-theme-index "$icon_theme_dir"
fi

echo "Installed development launcher: $desktop_file"
echo "Launcher target: $binary"

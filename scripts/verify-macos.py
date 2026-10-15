#!/usr/bin/env python3
import argparse
import json
from pathlib import Path
import plistlib
import re
import subprocess


def output(*arguments):
    return subprocess.check_output(arguments, text=True, stderr=subprocess.PIPE)


def require(condition, message):
    if not condition:
        raise SystemExit(message)


def versions(commands):
    minimums = []
    for block in re.split(r"^Load command \d+\n", commands, flags=re.MULTILINE):
        if "cmd LC_BUILD_VERSION" in block:
            value = re.search(r"^\s*minos (\d+(?:\.\d+)+)\s*$", block, re.MULTILINE)
        elif "cmd LC_VERSION_MIN_MACOSX" in block:
            value = re.search(r"^\s*version (\d+(?:\.\d+)+)\s*$", block, re.MULTILINE)
        else:
            continue
        require(value is not None, "Malformed Mach-O deployment command")
        minimums.append(tuple(map(int, value.group(1).split("."))))
    return minimums


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("app", type=Path)
    parser.add_argument("archive", type=Path)
    args = parser.parse_args()
    app = args.app.resolve()
    info = plistlib.loads((app / "Contents/Info.plist").read_bytes())
    require(info["LSMinimumSystemVersion"] == "14.0", "App deployment target must be macOS 14.0")
    require(info["CFBundleIdentifier"].startswith("com.rubyelders.dzban"), "Unexpected application identifier")
    for name in ("Dzban.icns", "ThirdPartyNotices.txt", "ThirdPartyInventory.json", "BuildInfo.json"):
        require((app / "Contents/Resources" / name).is_file(), f"Missing bundled resource: {name}")
    require(output("lipo", "-archs", str(args.archive)).strip() == "arm64", "Rust archive must contain only arm64")
    archive_versions = versions(output("otool", "-l", str(args.archive)))
    require(archive_versions and all(version <= (14, 0, 0) for version in archive_versions), "Rust archive requires a later macOS deployment target")
    sysroot = Path(output("rustc", "+1.98.1", "--print", "sysroot").strip())
    host = re.search(r"^host: (.+)$", output("rustc", "+1.98.1", "-vV"), re.MULTILINE).group(1)
    nm = sysroot / "lib/rustlib" / host / "bin/llvm-nm"
    require(nm.is_file(), "Install matching archive tools: rustup component add llvm-tools --toolchain 1.98.1")
    symbols = output(str(nm), "--extern-only", "--defined-only", str(args.archive))
    require(not re.search(r"\b_dzban_create_fixture\s*$", symbols, re.MULTILINE), "Release archive exposes fixture adapters")
    files = []
    for path in sorted(app.rglob("*")):
        if path.is_symlink():
            require(path.resolve().is_relative_to(app), f"Bundle symlink escapes app: {path.relative_to(app)}")
        if not path.is_file() or path.is_symlink():
            continue
        description = output("file", "-b", str(path))
        if "Mach-O" not in description:
            continue
        require(output("lipo", "-archs", str(path)).strip() == "arm64", f"Unexpected architecture: {path.name}")
        commands = output("otool", "-l", str(path))
        require("cmd LC_UUID" in commands, f"Missing required loader UUID: {path.name}")
        minimums = versions(commands)
        require(minimums and all(version <= (14, 0, 0) for version in minimums), f"Unsupported deployment target: {path.name}")
        for rpath in re.findall(r"cmd LC_RPATH\n.*?\n\s*path (.*?) \(offset", commands):
            require(rpath.startswith(("@executable_path/", "@loader_path/", "/System/Library/", "/usr/lib/")), f"External runtime search path: {rpath}")
            if rpath.startswith(("@executable_path/", "@loader_path/")):
                prefix = path.parent if rpath.startswith("@loader_path/") else app / "Contents/MacOS"
                require((prefix / rpath.split("/", 1)[1]).resolve().is_relative_to(app), f"Runtime search path escapes app: {rpath}")
        imports = []
        for line in output("otool", "-L", str(path)).splitlines()[1:]:
            library = line.strip().split(" (", 1)[0]
            imports.append(library)
            if library.startswith(("/System/Library/", "/usr/lib/")):
                continue
            require(library.startswith(("@rpath/", "@loader_path/", "@executable_path/")), f"External runtime dependency: {library}")
            if library.startswith("@rpath/"):
                require(any(candidate.name == Path(library).name for candidate in app.rglob(Path(library).name)), f"Unbundled runtime dependency: {library}")
            else:
                prefix = path.parent if library.startswith("@loader_path/") else app / "Contents/MacOS"
                resolved = (prefix / library.split("/", 1)[1]).resolve()
                require(resolved.is_relative_to(app) and resolved.exists(), f"Runtime dependency escapes app: {library}")
        require(not any(re.search(r"(?:gtk|glib|adwaita|gobject|gtksource)", library, re.IGNORECASE) for library in imports), "Native app links a GTK/GNOME library")
        files.append({"path": str(path.relative_to(app)), "architectures": ["arm64"], "imports": imports})
    require(files, "No native executable found")
    output("codesign", "--verify", "--deep", "--strict", str(app))
    print(json.dumps({"bundleIdentifier": info["CFBundleIdentifier"], "deploymentTarget": "14.0", "rustArchiveArchitectures": ["arm64"], "machOFiles": files}, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()

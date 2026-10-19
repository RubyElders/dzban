#!/usr/bin/env python3
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import plistlib
import re
import shutil
import stat
import struct
import subprocess
import time
import zipfile
import uuid


REPO = Path(__file__).resolve().parent.parent


def capture(*arguments):
    return subprocess.check_output(arguments, cwd=REPO, text=True).strip()


def source_fingerprint():
    paths = {REPO / name for name in ("Cargo.toml", "Cargo.lock", "macos/Package.swift", "macos/Dzban.xcodeproj/project.pbxproj", "macos/Resources/DzbanIcon.png")}
    for name in ("crates/dzban-core/src", "crates/dzban-db/src", "crates/dzban-macos-bridge/src", "crates/dzban-macos-bridge/include", "macos/Sources", "macos/Resources", "scripts"):
        paths.update(path for path in (REPO / name).rglob("*") if path.is_file() and path.suffix in (".rs", ".swift", ".h", ".modulemap", ".plist", ".sh", ".py", ".svg", ".json"))
    paths.update(REPO / name / "Cargo.toml" for name in ("crates/dzban-core", "crates/dzban-db", "crates/dzban-macos-bridge"))
    digest = hashlib.sha256()
    for path in sorted(paths):
        if not path.resolve().is_relative_to(REPO):
            raise SystemExit("Release source symlink escapes the checkout")
        digest.update(str(path.relative_to(REPO)).encode() + b"\0")
        digest.update(hashlib.sha256(path.read_bytes()).digest())
    return digest.hexdigest()


def normalize_uuid(executable):
    data = bytearray(executable.read_bytes())
    if struct.unpack_from("<I", data)[0] != 0xFEEDFACF:
        raise SystemExit("Expected a little-endian 64-bit Mach-O executable")
    count = struct.unpack_from("<I", data, 16)[0]
    offset = 32
    field = None
    for _ in range(count):
        command, length = struct.unpack_from("<II", data, offset)
        if length < 8 or offset + length > len(data):
            raise SystemExit("Invalid Mach-O load command")
        if command == 0x1B:
            if length != 24 or field is not None:
                raise SystemExit("Invalid Mach-O UUID command")
            field = offset + 8
        offset += length
    if field is None:
        raise SystemExit("The macOS loader requires an LC_UUID command")
    data[field:field + 16] = bytes(16)
    identifier = uuid.uuid5(uuid.NAMESPACE_OID, hashlib.sha256(data).hexdigest())
    data[field:field + 16] = identifier.bytes
    executable.write_bytes(data)


def package(app, destination, epoch):
    date = time.gmtime(max(epoch, 315532800))[:6]
    with zipfile.ZipFile(destination, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        for path in [app, *sorted(app.rglob("*"))]:
            name = str(path.relative_to(app.parent))
            if path.is_dir() and not path.is_symlink():
                name += "/"
            entry = zipfile.ZipInfo(name, date)
            entry.create_system = 3
            entry.compress_type = zipfile.ZIP_DEFLATED
            entry.external_attr = path.lstat().st_mode << 16
            data = os.readlink(path).encode() if path.is_symlink() else b"" if path.is_dir() else path.read_bytes()
            archive.writestr(entry, data)


def cargo(*arguments):
    return subprocess.check_output(["cargo", "+1.98.1", *arguments], cwd=REPO, text=True)


def licenses(output):
    metadata = json.loads(cargo("metadata", "--locked", "--format-version", "1", "--filter-platform", "aarch64-apple-darwin"))
    packages = {(package["name"], package["version"]): package for package in metadata["packages"]}
    tree = cargo("tree", "--locked", "-p", "dzban-macos-bridge", "--target", "aarch64-apple-darwin", "--edges", "normal,build", "--prefix", "none", "--format", "{p}")
    selected = set()
    for line in tree.splitlines():
        name, version, *_ = line.split()
        selected.add((name, version.removeprefix("v")))
    sections = []
    inventory = []
    for identity in sorted(selected):
        package = packages[identity]
        if package["id"] in metadata["workspace_members"]:
            continue
        root = Path(package["manifest_path"]).parent.resolve()
        files = set()
        if package.get("license_file"):
            files.add(root / package["license_file"])
        for path in root.rglob("*"):
            if not path.is_file():
                continue
            relative = path.relative_to(root)
            if any(part.upper().startswith(("LICENSE", "LICENCE", "COPYING", "COPYRIGHT", "NOTICE")) for part in relative.parts):
                files.add(path)
        if not files:
            raise SystemExit(f"No license/notice text found for {identity[0]} {identity[1]}")
        texts = []
        for path in sorted(files):
            resolved = path.resolve()
            if not resolved.is_relative_to(root):
                raise SystemExit(f"License path escapes package {identity[0]}")
            texts.append(f"--- {path.relative_to(root)} ---\n{path.read_text(encoding='utf-8').rstrip()}\n")
        name, version = identity
        sections.append(f"{name} {version}\nLicense: {package.get('license') or 'See included license text'}\nSource: {package.get('repository') or package.get('source') or 'Path dependency'}\n\n" + "\n".join(texts))
        inventory.append({"name": name, "version": version, "license": package.get("license"), "files": [str(path.relative_to(root)) for path in sorted(files)]})
    output.mkdir(parents=True, exist_ok=True)
    (output / "ThirdPartyNotices.txt").write_text("Third-party components in the native Rust bridge\n\n" + "\n\n".join(sections), encoding="utf-8")
    (output / "ThirdPartyInventory.json").write_text(json.dumps(inventory, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"Included license and notice texts for {len(inventory)} third-party packages")


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


def verify(app, archive):
    app = app.resolve()
    info = plistlib.loads((app / "Contents/Info.plist").read_bytes())
    require(info["LSMinimumSystemVersion"] == "14.0", "App deployment target must be macOS 14.0")
    require(info["CFBundleIdentifier"].startswith("com.rubyelders.dzban"), "Unexpected application identifier")
    for name in ("Dzban.icns", "ThirdPartyNotices.txt", "ThirdPartyInventory.json", "BuildInfo.json"):
        require((app / "Contents/Resources" / name).is_file(), f"Missing bundled resource: {name}")
    require(output("lipo", "-archs", str(archive)).strip() == "arm64", "Rust archive must contain only arm64")
    archive_versions = versions(output("otool", "-l", str(archive)))
    require(archive_versions and all(version <= (14, 0, 0) for version in archive_versions), "Rust archive requires a later macOS deployment target")
    sysroot = Path(output("rustc", "+1.98.1", "--print", "sysroot").strip())
    host = re.search(r"^host: (.+)$", output("rustc", "+1.98.1", "-vV"), re.MULTILINE).group(1)
    nm = sysroot / "lib/rustlib" / host / "bin/llvm-nm"
    require(nm.is_file(), "Install matching archive tools: rustup component add llvm-tools --toolchain 1.98.1")
    symbols = output(str(nm), "--extern-only", "--defined-only", str(archive))
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
    return json.dumps({"bundleIdentifier": info["CFBundleIdentifier"], "deploymentTarget": "14.0", "rustArchiveArchitectures": ["arm64"], "machOFiles": files}, indent=2, sort_keys=True) + "\n"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("output", type=Path)
    parser.add_argument("--rust-target", type=Path, help="Reuse an existing Rust target directory for a local incremental build")
    parser.add_argument("--allow-dirty", action="store_true", help="Prepare a local artifact from uncommitted sources")
    args = parser.parse_args()
    if platform.system() != "Darwin" or platform.machine() != "arm64":
        raise SystemExit("The validated release build target is Apple Silicon macOS")
    xcode = capture("xcodebuild", "-version")
    swift = capture("xcrun", "swift", "--version")
    rust = capture("rustc", "+1.98.1", "--version")
    if xcode != "Xcode 27.0\nBuild version 27A266a" or "Apple Swift version 6.4 " not in swift or not rust.startswith("rustc 1.98.1 "):
        raise SystemExit("Release inputs require Xcode 27.0 (27A266a), Swift 6.4, and Rust 1.98.1")
    sysroot = Path(capture("rustc", "+1.98.1", "--print", "sysroot"))
    if not (sysroot / "lib/rustlib/aarch64-apple-darwin/bin/llvm-nm").is_file():
        raise SystemExit("Install matching archive tools: rustup component add llvm-tools --toolchain 1.98.1")
    commit = capture("git", "rev-parse", "HEAD")
    dirty = bool(capture("git", "status", "--porcelain", "--untracked-files=normal"))
    if dirty and not args.allow_dirty:
        raise SystemExit("Release requires a clean checkout; --allow-dirty prepares a local validation artifact")
    fingerprint = source_fingerprint()
    root = args.output.resolve()
    if not root.is_relative_to(REPO / "target"):
        raise SystemExit("Release build output must be inside the ignored target directory")
    rust_target = args.rust_target.resolve() if args.rust_target else root / "rust"
    if not rust_target.is_relative_to(REPO / "target"):
        raise SystemExit("Rust build cache must be inside the ignored target directory")
    root.mkdir(parents=True, exist_ok=False)
    artifact = root / "artifact"
    artifact.mkdir()
    epoch = int(capture("git", "show", "-s", "--format=%ct", commit))
    environment = os.environ.copy()
    for variable in ("RUSTFLAGS", "RUSTC", "RUSTDOC", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "CARGO_BUILD_TARGET"):
        environment.pop(variable, None)
    cargo_home = Path(environment.get("CARGO_HOME", Path.home() / ".cargo"))
    environment["CARGO_ENCODED_RUSTFLAGS"] = f"--remap-path-prefix={REPO}=/dzban\x1f--remap-path-prefix={cargo_home / 'registry'}=/cargo/registry"
    environment["DZBAN_MACOS_TOOLCHAIN"] = "1.98.1"
    environment["SOURCE_DATE_EPOCH"] = str(epoch)
    environment["CCACHE_DISABLE"] = "1"
    command = ["xcodebuild", "-project", "macos/Dzban.xcodeproj", "-scheme", "Dzban", "-configuration", "Release",
               "-derivedDataPath", str(root / "xcode"), "-destination", "platform=macOS,arch=arm64",
               "DZBAN_RUST_TARGET_DIR=" + str(rust_target), "DZBAN_BUNDLE_IDENTIFIER=com.rubyelders.dzban.macos.unsigned",
               "DZBAN_MACOS_FIXTURES=NO",
               "CODE_SIGNING_ALLOWED=NO", "ENABLE_DEBUG_DYLIB=NO",
               f'OTHER_SWIFT_FLAGS=$(inherited) -file-prefix-map "{REPO}=/dzban" -debug-prefix-map "{REPO}=/dzban"',
               "build"]
    print(f"Building local unsigned release in {root}", flush=True)
    with (root / "build.log").open("w") as log:
        subprocess.run(command, cwd=REPO, env=environment, stdout=log, stderr=subprocess.STDOUT, check=True)
    app = artifact / "Dzban.app"
    shutil.copytree(root / "xcode/Build/Products/Release/Dzban.app", app, symlinks=True)
    subprocess.run(["codesign", "--remove-signature", str(app)], check=True)
    subprocess.run(["xcrun", "strip", "-S", "-x", str(app / "Contents/MacOS/Dzban")], check=True)
    normalize_uuid(app / "Contents/MacOS/Dzban")
    resources = app / "Contents/Resources"
    licenses(resources)
    if source_fingerprint() != fingerprint:
        raise SystemExit("Release sources changed during the build")
    report = {"gitCommit": commit, "dirty": dirty, "sourceSHA256": fingerprint,
              "cargoLockSHA256": hashlib.sha256((REPO / "Cargo.lock").read_bytes()).hexdigest(),
              "rust": rust, "swift": swift, "xcode": xcode, "sourceEpoch": epoch,
              "architecture": "arm64", "minimumMacOS": "14.0", "signing": "ad-hoc, local only", "notarized": False}
    (resources / "BuildInfo.json").write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")
    for path in [app, *app.rglob("*")]:
        if not path.is_symlink():
            path.chmod(0o755 if path.is_dir() or path.stat().st_mode & stat.S_IXUSR else 0o644)
    subprocess.run(["codesign", "--force", "--sign", "-", "--timestamp=none", str(app)], check=True)
    for path in [app, *app.rglob("*")]:
        os.utime(path, (epoch, epoch), follow_symlinks=False)
    archive = rust_target / "release/libdzban_macos_bridge.a"
    verification = verify(app, archive)
    (artifact / "Verification.json").write_text(verification)
    (artifact / "BuildInfo.json").write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")
    destination = artifact / "Dzban-macos-arm64-unsigned.zip"
    if source_fingerprint() != fingerprint:
        raise SystemExit("Release sources changed during packaging")
    package(app, destination, epoch)
    (artifact / "SHA256SUMS").write_text(hashlib.sha256(destination.read_bytes()).hexdigest() + "  " + destination.name + "\n")
    print(f"Local unsigned artifact: {destination}")


if __name__ == "__main__":
    main()

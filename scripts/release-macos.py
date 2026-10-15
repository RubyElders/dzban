#!/usr/bin/env python3
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import stat
import struct
import subprocess
import sys
import time
import zipfile
import uuid


REPO = Path(__file__).resolve().parent.parent


def capture(*arguments):
    return subprocess.check_output(arguments, cwd=REPO, text=True).strip()


def source_fingerprint():
    paths = {REPO / name for name in ("Cargo.toml", "Cargo.lock", "macos/Package.swift", "macos/Dzban.xcodeproj/project.pbxproj", "data/icons/com.rubyelders.dzban.png")}
    for name in ("crates/dzban-core/src", "crates/dzban-db/src", "crates/dzban-macos-bridge/src", "crates/dzban-macos-bridge/include", "macos/Sources", "macos/Resources", "scripts"):
        paths.update(path for path in (REPO / name).rglob("*") if path.is_file() and path.suffix in (".rs", ".swift", ".h", ".modulemap", ".plist", ".sh", ".py"))
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
    subprocess.run([sys.executable, str(REPO / "scripts/macos-licenses.py"), str(resources)], cwd=REPO, check=True)
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
    verification = subprocess.check_output([sys.executable, str(REPO / "scripts/verify-macos.py"), str(app), str(archive)], cwd=REPO, text=True)
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

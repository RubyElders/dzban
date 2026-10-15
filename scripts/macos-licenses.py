#!/usr/bin/env python3
import argparse
import json
from pathlib import Path
import subprocess


def cargo(*arguments):
    return subprocess.check_output(["cargo", "+1.98.1", *arguments], text=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
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
    args.output.mkdir(parents=True, exist_ok=True)
    (args.output / "ThirdPartyNotices.txt").write_text("Third-party components in the native Rust bridge\n\n" + "\n\n".join(sections), encoding="utf-8")
    (args.output / "ThirdPartyInventory.json").write_text(json.dumps(inventory, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"Included license and notice texts for {len(inventory)} third-party packages")


if __name__ == "__main__":
    main()

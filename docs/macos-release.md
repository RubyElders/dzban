# Local macOS release builds

The local artifact targets Apple Silicon with a macOS 14 deployment target. Runtime verification applies to the Mac on which the app was exercised; it does not establish compatibility with every macOS release. Intel, App Store distribution, clean-machine installation, and public distribution are separate gates.

Use Xcode 27.0 build 27A266a, Swift 6.4, Rust 1.98.1, and the matching archive inspection tools:

```sh
rustup component add llvm-tools --toolchain 1.98.1
python3 scripts/release-macos.py target/macos-release-local
```

The builder requires a clean checkout. `--allow-dirty` prepares a local validation artifact and records that fact; it does not turn uncommitted sources into a distributable release. Output directories must be inside the ignored `target/` directory and must not already exist. Builds use independent Rust/Xcode output directories, the locked Cargo graph, source fingerprints, source-date timestamps, path remapping, stripped local/debug symbols, a deterministic loader UUID, and a deterministic ZIP layout. `--rust-target PATH` can reuse a Rust build cache for a local incremental build; omit it for independent full builds. Release links the optimized Rust archive rather than the debug archive.

`artifact/Dzban.app` is signed ad hoc for local execution, uses `com.rubyelders.dzban.macos.unsigned`, and is not notarized. The same directory contains the local unsigned ZIP, its SHA-256, build information, and the verification report. Build information records tool versions, Git identity, dirty state, source fingerprint, and the Cargo lockfile hash. The app bundles license and notice texts for the native bridge's third-party dependency graph; GTK frontend dependencies are excluded.

The verifier checks the Rust archive and bundled Mach-O architectures, minimum deployment versions, absence of fixture exports, required resources, signature integrity, runtime search paths, and imported libraries. Non-Apple runtime libraries must resolve inside the bundle. A successful static check is followed by an actual launch:

```sh
bash scripts/check-macos-launch.sh target/macos-release-local/artifact/Dzban.app direct
bash scripts/check-macos-launch.sh target/macos-release-local/artifact/Dzban.app launchservices
```

Each mode uses a copied app outside the checkout with disposable HOME/config/data. Verify its empty connection chooser and quit with Command-Q. Run `bash scripts/test-macos-live.sh target/macos-release-local/artifact/Dzban.app` for connect, execute, inspect, cancel, restart, history, and quit on a copied production app with disposable database services.

To compare independent builds from identical sources, build into two fresh directories and compare their `artifact/Dzban-macos-arm64-unsigned.zip` files. Matching source fingerprints and tool versions are prerequisites; signing timestamps and notarization receipts belong to a later distribution step and are not part of the local reproducibility check.

The manual `Native macOS artifact` workflow runs the dependency guard, packaging unit tests, and full artifact builder/verifier. It requires a self-hosted runner labeled `dzban-macos-native` with the pinned full Xcode installed. It does not run for pull requests or publish an artifact. GUI/database runtime gates remain separate; no remote CI run is implied by local validation.

## Optional distribution tooling

Local use does not require a Developer ID certificate or notarization. The following tools are prepared for a future explicitly requested distribution; do not treat their presence as evidence that a build is signed or notarized for distribution.

With a configured Developer ID Application identity, sign a prepared app and verify the resulting hardened signature:

```sh
DZBAN_SIGN_IDENTITY='Developer ID Application: YOUR IDENTITY' bash scripts/sign-macos.sh APP_BUNDLE RUST_ARCHIVE
```

The signing step changes the bundle identifier to `com.rubyelders.dzban` and updates build metadata before signing. It requires an explicit identity and verifies the certificate type and hardened runtime. Keep the original local ZIP if an unsigned copy is wanted.

Notarization uploads the signed app to Apple. It is a separate explicit command using an already configured `notarytool` keychain profile; credentials are not accepted in repository files or command-line password arguments:

```sh
DZBAN_NOTARY_PROFILE=YOUR_PROFILE bash scripts/notarize-macos.sh APP_BUNDLE --submit
```

The command requires Apple's Accepted response, staples and validates the ticket, verifies the signature and Gatekeeper assessment, and creates a notarized ZIP with a checksum and external receipt. It refuses existing output files. Public publication is a separate action. Actual Developer ID signing and notarization are outside the current local-use verification scope.

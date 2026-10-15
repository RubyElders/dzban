# Native macOS development

The native frontend targets macOS 14 and later on Apple Silicon. It uses SwiftUI and AppKit over the shared Rust core and database adapters. GTK remains a separate frontend and is not a prerequisite for native development. Intel and App Store distribution are outside the initial supported matrix.

## Toolchains

Use Rust 1.98.1 for native development and Rust 1.88.0 for shared-code compatibility checks. The native Swift toolchain baseline is Swift 6.4. Native application development requires full Xcode with a compatible macOS SDK and Swift toolchain; record the exact Xcode version when native application validation becomes available. Command Line Tools alone can run shared Rust checks and Swift bridge tests but cannot run Xcode application schemes or XCUITest. Record `xcodebuild -version`, `swift --version`, and `rustc --version` with native validation results; an installed newer SDK does not change the deployment target.

Install the Rust toolchains with `rustup toolchain install 1.98.1 1.88.0 --profile minimal`. Select the installed Xcode using `xcode-select` before building the application. Build artifacts must remain in the repository's ignored `target/` directory.

## Shared baseline

Run the checks without installing GTK or other native frontend libraries:

```sh
DZBAN_TEST_TOOLCHAIN=1.98.1 bash scripts/check-headless.sh
bash scripts/check-portable.sh
```

The headless runner disables native package discovery and isolates configuration and history in a disposable directory. It runs core, database adapter, CLI, and Rust macOS bridge tests, including a dependency check for the bridge. Ignored service-dependent tests require the disposable Compose fixtures described in [the testing skill](../skills/dzban-testing/SKILL.md). It does not validate native windows, TLS connections, or database runtime behavior by itself.

The initial inspection passed shared core/adapter tests on Apple Silicon with Rust 1.98.1. Four CLI process tests failed because they assumed `/bin/true` and `/bin/false`; these tests use explicit shell exit commands for portable success/failure behavior. The macOS CI matrix checks both Rust versions. Remote CI results are separate from local validation.

## Application and distribution scope

Run `bash scripts/test-macos-bridge.sh` to compile the Rust archive and verify its Swift consumer. The [bridge contract](macos-bridge.md) describes handle ownership, snapshot delivery, and the fixture lifecycle checks. Full Xcode enables the XCTest run; Command Line Tools can run the standalone Swift scenarios.

The first milestone covers connect, execute, streaming result pages, cancellation, and safe shutdown in a native window. The application must preserve core configuration/history formats and run without Homebrew or a repository working directory. Initial development retains existing XDG discovery semantics; do not silently migrate credentials or create a separate native history format.

Release builds will use direct `.app` distribution with signing and notarization. Local unsigned builds must be identified as such. A release requires clean-machine runtime validation on the supported architecture and operating system matrix, independent of successful compilation.

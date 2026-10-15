# Native macOS development

The native frontend targets macOS 14 and later on Apple Silicon. It uses SwiftUI and AppKit over the shared Rust core and database adapters. GTK remains a separate frontend and is not a prerequisite for native development. Intel and App Store distribution are outside the initial supported matrix.

## Toolchains

Use Rust 1.98.1 for native development and Rust 1.88.0 for shared-code compatibility checks. The native Swift toolchain baseline is Swift 6.4. Native application development uses Xcode 27.0 (build 27A266a) with Swift 6.4; full Xcode is required. Command Line Tools alone can run shared Rust checks and Swift bridge tests but cannot run Xcode application schemes or XCUITest. Record `xcodebuild -version`, `swift --version`, and `rustc --version` with native validation results; an installed newer SDK does not change the deployment target.

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

Build the development application with `bash scripts/build-macos.sh`. The bundle is `target/macos-xcode/Build/Products/Debug/Dzban.app`; open it from Finder or with `open` after setting disposable configuration for testing. Its development identifier is `com.rubyelders.dzban.macos.dev`. The `--fixtures` build uses a separate `.fixtures` identifier and injected in-memory adapters. Both are locally signed development artifacts.

Run `bash scripts/test-macos.sh` for the Rust bridge tests, standalone Swift scenarios, XCTest store/lifecycle tests, and native XCUITest shell workflows. Fixture connections use disposable history; the empty-configuration workflow explicitly uses `/dev/null`. UI test results are saved under `target/macos-ui-results.*`. XCUITest requires an interactive macOS session with automation available.

Boolean fixture launch options use explicit `YES` values to avoid ambiguous macOS launch argument parsing. Copy tests use private named pasteboards and leave the general clipboard untouched. Fixture-only Testing menu actions record bridge retention, accounted cache bytes, RSS, and main-thread timer gaps as separate measurements.

The native application provides connections, workspace/query navigation, an AppKit SQL editor, native run/cancel commands, and deferred close/quit. Command-Return or Command-Shift-Return runs selected SQL or the current statement; Escape cancels active work in the selected workspace. Editing, selection, undo/redo, and composition stay local to each document. Query phases, structured errors, and streaming results are visible. The native table pages the visible region in bounded tiles, preserves SQL null identity, and offers original-value copy and chunked inspection. Large previews are truncated for display while their original values remain in shared storage. The Preview Plan toolbar/menu action opens a pinned estimate and plan. Command-Shift-P taps to pin or holds to peek; switching apps resets held input. The Catalog browser derives its object families and describe actions from backend descriptors. Inspect Row, or Space while a result cell is selected, opens bounded row inspection; the cell inspector retrieves original values. The native AppKit window toolbar places the query history toggle on the left, an action search field in the center, and preview/new-query/run controls on the right. The field displays running/canceling status and a progress indicator. Command-Shift-K focuses the field; search actions, configured connections, open workspaces, and query tabs, then use Up/Down and Return to invoke an enabled match. Open connections appear in a separate tab strip above query tabs; each can be selected or disconnected without losing other workspaces. Query history stays in the left sidebar so saved SQL can be restored beside the editor. The global bottom bar uses the GTK PostgreSQL and ClickHouse artwork and shows connection health, measured execution time, returned rows, ping latency, and server version. Idle connections are probed every five seconds; click the ping to refresh immediately. The connection chooser offers individual probes and Test All Connections without opening workspaces. F1 or Command-Shift-/ opens the shortcut reference. Command-T/Command-W opens/closes a query tab; Command-Shift-[ and Command-Shift-] switch query tabs; Command-Option-Left/Right switches connections. Command-B toggles history and Command-period cancels work. Hold Command or Control to show query actions, add Shift to estimate the current statement in the native titlebar, then press Enter to run or Space to inspect the plan. A quick Space tap pins the plan; holding Space peeks until release. This chord uses the shared input reducer. The History browser refreshes the shared profile history and restores SQL plus the retained sample without executing it. Saved samples retain legacy text values, so they cannot recover original column types or SQL null identity; saved plans load separately.

The first milestone covers connect, execute, streaming result pages, cancellation, and safe shutdown in a native window. The application must preserve core configuration/history formats and run without Homebrew or a repository working directory. Initial development retains existing XDG discovery semantics; do not silently migrate credentials or create a separate native history format.

Release builds will use direct `.app` distribution with signing and notarization. Local unsigned builds must be identified as such. A release requires clean-machine runtime validation on the supported architecture and operating system matrix, independent of successful compilation.

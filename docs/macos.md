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

The native window has one query tab strip. Each tab owns an independent shared-core session and retained draft. Startup and New Tab use the same inline connection chooser: the first row receives focus, Space probes it, and Return connects and focuses the editor. History starts hidden. The native toolbar keeps History on the left, Actions search in the center, and Plan/Run-or-Stop on the right. The New Tab button belongs to the tab strip. There is no native catalog browser or nested connection/query tab strip.

The AppKit editor retains native text storage, IME composition, selection and undo. Run and Stop use the same shared semantic action; a nonempty selection executes that text and an empty selection uses the shared current-statement selector. Holding Command-Shift requests a live estimate. The plan viewer uses readable shared plan text without a duplicate estimate header. Actions search accepts typing or Down/Up followed by Return; it also contains explicit actions for focusing the editor and results.

The native table pages bounded visible tiles and preserves typed SQL nulls. Space pins the selected row preview; holding Space peeks. In the row preview, Space or Return opens the selected cell's full value, Left/Right changes the result row, and Escape goes back or closes. Cell text supports normal system selection/copy. Copying selected result rows or Select All copies Markdown with headers and complete values. Matching-schema reruns retain scroll position, widths and selection; changed schemas reset and shorter results clamp.

Query progress stays inside results, with a centered noninteractive summary showing elapsed/start time and estimates. The bottom bar follows the selected tab and shows connection state, execution duration, captured rows and estimate comparison, completed cost, ping latency and server version. Click the ping to probe. Shared health scheduling checks idle tabs every five seconds and respects query/preview admission. Background tabs show progress and unseen-result indicators; shared attention policy drives the native Dock effect without changing selection or focus.

History restores SQL and its retained sample without executing it. Saved samples keep text previews, so original column types and SQL null identity cannot be recovered. Saved plans use the shared text projection. Quit waits for shared cleanup; failed cleanup remains retryable. Relaunch restores ordered tabs, drafts and selection without rerunning SQL. Shared workspace storage retains the existing `$XDG_DATA_HOME/dzban/native-workspace.json` path, defaulting to `~/.local/share/dzban/native-workspace.json`, with private atomic writes. Configuration and profile-scoped history retain shared XDG discovery.

| Where | Action | Shortcut |
| --- | --- | --- |
| Query | Run or stop | Command-Return |
| Query | Estimate and run | Command-Shift-Return |
| Query | Open plan | Command-Shift-Space |
| Query | Live estimate | Hold Command-Shift |
| Window | New / close tab | Command-T / Command-W |
| Window | Next / previous tab | Control-Tab / Control-Shift-Tab |
| Window | Cycle available editor/results focus | Command-K |
| Window | Search Actions | Command-L |
| Window | Toggle history | Command-B |
| Window | Shortcut reference | Command-? |
| Connections | Probe / select | Space / Return |
| Results / row preview | Inspect row / cell | Space, then Space or Return |
| Row preview | Previous / next row | Left / Right |
| Current view | Back, dismiss or stop | Escape |
| Outside text input | Arrow navigation | H / J / K / L |
| Text | Select all, copy, paste, cut, undo, redo | Native system commands |
| App | Quit after cleanup | Command-Q |

The native shortcut reference uses platform symbols and accessible spoken descriptions. Removed shortcut aliases are not part of this interface. The shared/native ownership and migration contracts are documented in [frontend integration](frontend-integration.md).

The local application covers connect, execute, streaming result pages, cancellation, and safe shutdown in a native window. The application must preserve core configuration/history formats and run without Homebrew or a repository working directory. The local refactor retains existing XDG discovery semantics; do not silently migrate credentials or create a separate native history format.

A future distributed release will use direct `.app` distribution with signing and notarization. Local-use refactoring does not require a Developer ID identity, notarization or clean-machine distribution gates. Local unsigned builds must be identified as such. A release requires clean-machine runtime validation on the supported architecture and operating system matrix, independent of successful compilation.

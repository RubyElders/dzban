# Native macOS validation

Run native UI gates in an interactive Apple Silicon macOS session with Xcode 27.0. Use disposable configuration, history, and database services; never point these gates at a user connection file. GTK validation remains a separate Linux gate.

| Contract | Shared and bridge coverage | Native coverage |
| --- | --- | --- |
| Connection admission, selection, and retirement | Core application/document tests and Swift lifecycle scenarios | Fixture connection/navigation/close workflows; live PostgreSQL and ClickHouse connect/quit |
| SQL selection, Unicode offsets, composition, and undo | Rust SQL selection tests; Swift UTF-16/scalar and AppKit composition/undo tests | Native key/menu execution, draft restoration, and document switching |
| Streaming results and original values | Adapter schema/null/order/cutoff tests; bridge page/chunk generation tests | Wide-table scrolling, original copy, SQL-null inspection, huge-cell chunks, and read retry |
| Commands and transaction planning | Adapter command and PostgreSQL transaction tests | Live PostgreSQL BEGIN/SELECT/ROLLBACK and ClickHouse command acknowledgment |
| Structured errors and restart | Adapter native-code and cleanup tests | Live PostgreSQL/ClickHouse server errors followed by successful execution |
| Cancellation, busy quit, truncation cleanup | Engine admission/cancellation and adapter stream tests | Live busy-quit retry, cancellation/restart, row limit, and subsequent execution on both adapters |
| History interoperability and replacement | Shared concurrent-writer/recovery tests; native external-publication and saved-generation tests | Fixture and live restore without execution, native undo, saved samples, and saved-plan presentation |
| Preview and row visibility | Shared peek state machine, admission revisions, and Swift navigation tests | Native plan/row tap and timed hold, focus reset, catalog navigation, and inspector handoff |
| Appearance and accessibility | Bounded table accessibility and viewport contracts | Light/dark AppKit appearance, window resize, visible-cell selection, row inspection, and history sheets |
| Paths and TLS | Shared configuration discovery and history-format tests | Explicit/empty/malformed configuration; copied-app direct and LaunchServices gates; disposable macOS TLS trust/rejection/cancel/recovery |

`bash scripts/test-macos.sh` runs the ordinary Rust/Swift fixture gates and UI suite. Live database UI tests report explicit skips there. `bash scripts/test-macos-live.sh` starts owned PostgreSQL and ClickHouse services, injects the opt-in flag into a generated test-runner configuration, and runs the two live workflows plus the appearance workflow serially. Its result summary must contain three passing tests and zero skips. It removes its containers, volumes, network, and temporary test-runner file on exit; results and fixture service logs remain under the ignored `target/macos-live.*` directory.

Appearance selection is a fixture-only launch option. It sets the application's AppKit appearance and leaves system preferences untouched. Inspect exported images for both actual theme colors and unobstructed controls; a test name or successful accessibility lookup does not prove the intended appearance rendered. Record the display backing scale with the result. App-window captures can crop a separate popover, so row-popover captures include the full display and may contain the task's background windows.

Run `python3 scripts/test-macos-tls.py` for the separate TLS fixture gate. Run both modes of `scripts/check-macos-launch.sh` against the production bundle for copied-app startup and normal quit. LaunchServices coverage on a development Mac does not establish literal Finder interaction, a clean installation, or runtime support across every advertised macOS version.

The initial platform is Apple Silicon with a macOS 14 deployment target. Runtime evidence from a newer macOS version does not prove macOS 14 compatibility. Clean supported-Mac installation, default Finder environment, Intel support, distribution signing, and notarization require their own evidence before claiming those gates passed.

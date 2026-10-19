# Dzban agent instructions

## Required validation

`scripts/test.sh` is the single Linux test entry point and `scripts/macos.sh` the macOS one; run either without arguments for its commands.
Run `bash scripts/test.sh portable` for the all-target dependency guard and
`bash scripts/test.sh headless` to build/test without native frontend libraries.

Choose tests based on the behavior changed. Compilation is not runtime or visual validation.

- Pure logic: add and run ordinary unit tests.
- Cross-crate/public behavior: add integration tests under the owning crate's `tests/` directory.
- PostgreSQL or ClickHouse behavior: use `docker-compose.test.yml` and run ignored database tests serially against the disposable services.
- GTK state and signals: use `#[gtk4::test]`.
- Keyboard and chord semantics: isolate the key-to-action state machine in ordinary Rust tests; use `#[gtk4::test]` for widget state, signals, focus, and accessibility contracts.
- External semantic UI automation: prefer an AT-SPI driver when installed. Give interactive widgets stable accessible labels and descriptions.
- Theme rendering, animation, popovers, and layout: run the actual app under Xvfb with a disposable D-Bus session (`bash scripts/test.sh display`), capture relevant states, compare the target region, and inspect the images. Coordinate replay is only a visual smoke-test aid.

Run `bash scripts/test.sh screens` for screen-level workflows; set `DZBAN_UI_SCENARIO` to run one scenario from `tests/ui/dzban_screens.py`. It uses pinned Dogtail through `uv`, a private Mutter/D-Bus/AT-SPI session, temporary application data, and disposable PostgreSQL and ClickHouse fixtures.

Run `bash scripts/test.sh gtk` for display-backed widget and injected-adapter lifecycle tests. It uses Xvfb, a private D-Bus session, empty configuration, and temporary application data without database services.

Run the workspace test suite, strict Clippy, formatting check, and `git diff --check` before handoff. Report ignored and skipped coverage explicitly.

Run `bash scripts/test.sh cli` for ShellSpec CLI process contracts and `bash scripts/test.sh databases` for the serial PostgreSQL, ClickHouse and terminal suites against Compose fixtures. The database runner enables its PTY case with `DZBAN_TEST_DATABASES=1`. Test actual terminal key sequences, not only editor reducer calls or `\g` submission; cover Enter, Ctrl+Enter, multiline input, quit, catalog listing, cancellation, and pager handoff.

## Adding tests

- Assert observable outcomes rather than implementation calls.
- Add regression coverage that fails for the reported bug.
- Keep normal tests independent of external services.
- Mark service-dependent tests ignored with the required service in the reason.
- Reuse Compose fixtures and temporary directories; do not read the user's Dzban configuration.
- For transient UI, test loading, completion/error, cancellation, and restart. Verify both state and rendering when appearance matters.

Never copy or expose credentials from a user-owned Dzban configuration. It must not be included in commits or test artifacts.

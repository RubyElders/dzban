# Dzban agent instructions

## Project skills

- Use `skills/dzban-testing/SKILL.md` for Rust and database test planning and execution.
- Use `skills/dzban-ui-testing/SKILL.md` for GTK/libadwaita interaction, rendering, animation, accessibility, and layout changes.

## Required validation

Choose tests based on the behavior changed. Compilation is not runtime or visual validation.

- Pure logic: add and run ordinary unit tests.
- Cross-crate/public behavior: add integration tests under the owning crate's `tests/` directory.
- PostgreSQL or ClickHouse behavior: use `docker-compose.test.yml` and run ignored database tests serially against the disposable services.
- GTK state and signals: use `#[gtk4::test]`.
- Keyboard and chord semantics: isolate the key-to-action state machine in ordinary Rust tests; use `#[gtk4::test]` for widget state, signals, focus, and accessibility contracts.
- External semantic UI automation: prefer an AT-SPI driver when installed. Give interactive widgets stable accessible labels and descriptions.
- Theme rendering, animation, popovers, and layout: run the actual app under Xvfb with a disposable D-Bus session, capture relevant states, compare the target region, and inspect the images. Coordinate replay is only a visual smoke-test aid.

Run `skills/dzban-ui-testing/scripts/run-dogtail-integration.sh` for screen-level workflows. It uses pinned Dogtail through `uv`, a private Mutter/D-Bus/AT-SPI session, temporary application data, and disposable PostgreSQL and ClickHouse fixtures.

Run the workspace test suite, strict Clippy, formatting check, and `git diff --check` before handoff. Report ignored and skipped coverage explicitly.

## Adding tests

- Assert observable outcomes rather than implementation calls.
- Add regression coverage that fails for the reported bug.
- Keep normal tests independent of external services.
- Mark service-dependent tests ignored with the required service in the reason.
- Reuse Compose fixtures and temporary directories; do not read the user's Dzban configuration.
- For transient UI, test loading, completion/error, cancellation, and restart. Verify both state and rendering when appearance matters.

Never copy or expose credentials from a user-owned Dzban configuration. It must not be included in commits or test artifacts.

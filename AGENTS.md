# Dzban agent instructions

## Project skills

- Use `skills/dzban-testing/SKILL.md` for Rust and PostgreSQL test planning and execution.
- Use `skills/dzban-ui-testing/SKILL.md` for GTK/libadwaita interaction, rendering, animation, accessibility, and layout changes.

## Required validation

Choose tests based on the behavior changed. Compilation is not runtime or visual validation.

- Pure logic: add and run ordinary unit tests.
- Cross-crate/public behavior: add integration tests under the owning crate's `tests/` directory.
- PostgreSQL behavior: use `docker-compose.test.yml` and run ignored database tests serially against the disposable services.
- GTK state and signals: use `#[gtk4::test]`.
- Theme rendering, keyboard routing, focus, animation, popovers, and layout: run the actual app under Xvfb with a disposable D-Bus session, capture relevant states, compare the target region, and inspect the images.

Run the workspace test suite, strict Clippy, formatting check, and `git diff --check` before handoff. Report ignored and skipped coverage explicitly.

## Adding tests

- Assert observable outcomes rather than implementation calls.
- Add regression coverage that fails for the reported bug.
- Keep normal tests independent of external services.
- Mark service-dependent tests ignored with the required service in the reason.
- Reuse Compose fixtures and temporary directories; do not read the user's `dzban.toml`.
- For transient UI, test loading, completion/error, cancellation, and restart. Verify both state and rendering when appearance matters.

Never copy or expose credentials from the local `dzban.toml`. It is user-owned and must not be included in commits or test artifacts.

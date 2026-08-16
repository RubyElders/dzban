# Testing `dzban-db`

## Unit tests

Run the tests that do not need PostgreSQL:

```bash
cargo test --workspace --all-targets
```

Unit tests use the standard Rust harness. Database tests that pump the GLib main context use `#[gtk4::test]` so their local callbacks stay on one worker thread.

## PostgreSQL integration tests

Start the test databases and run the ignored database tests serially:

```bash
docker compose -f docker-compose.test.yml up -d --wait
cargo test -p dzban-db -- --ignored --test-threads=1
docker compose -f docker-compose.test.yml down
```

The Compose setup provides PostgreSQL 15, 16, and 17 on ports 5415, 5416, and 5417. Port 5418 provides a server configured for cancellation and timeout tests.

The integration suite covers connections, pings, multi-version configuration, parallel queries, returned rows, execution timing, query state, errors, and cancellation.

## GTK widget tests

The query-routing and statusbar-binding regression tests need a display:

```bash
xvfb-run -a cargo test -p dzban-ui --bin dzban -- --ignored
```

If Xvfb cannot create its socket, verify the ownership and permissions of `/tmp/.X11-unix`.

## Required checks

```bash
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
git diff --check
```

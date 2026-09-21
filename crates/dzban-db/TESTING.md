# Testing `dzban-db`

## Unit tests

Run the tests that do not need PostgreSQL:

```bash
cargo test --workspace --all-targets
```

All `dzban-db` tests use the standard Rust harness and do not require a graphical display. Run ignored database tests with one test thread to keep their shared database fixtures deterministic.

## PostgreSQL integration tests

Start the test databases and run the ignored database tests serially:

```bash
docker compose -f docker-compose.test.yml up -d --wait
cargo test -p dzban-db -- --ignored --test-threads=1
docker compose -f docker-compose.test.yml down
```

The Compose setup provides PostgreSQL 15, 16, and 17 on ports 5415, 5416, and 5417. Port 5418 provides a server configured for cancellation and timeout tests.

The integration suite covers connections, pings, multi-version configuration, parallel queries, returned rows, execution timing, query state, errors, and cancellation.

## Required checks

```bash
cargo +1.88.0 check --workspace --all-targets
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets
cargo fmt --all -- --check
git diff --check
```

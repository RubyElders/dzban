# Database integration tests

The repository defines disposable PostgreSQL 15, 16, 17, slow-query, and ClickHouse services in `docker-compose.test.yml`. Test credentials in that file are fixtures and are unrelated to user configuration.

Run the suite serially because tests share service fixtures:

```bash
docker compose -f docker-compose.test.yml up -d --wait
CCACHE_DISABLE=1 cargo +1.88.0 test -p dzban-db -- --ignored --skip certificate_validation_and_cancel_recovery --test-threads=1
CCACHE_DISABLE=1 cargo +1.88.0 test -p dzban-cli --test database -- --ignored --test-threads=1
CCACHE_DISABLE=1 cargo +1.88.0 test -p dzban-gtk --test connection_integration_tests --test clickhouse_integration_tests --test streamed_connection_tests --test runtime_bridge_isolation_tests --test config_tests --test manager_tests --test query_tab_tests -- --ignored --test-threads=1
docker compose -f docker-compose.test.yml down
```

The reusable wrapper is `skills/dzban-testing/scripts/run-db-integration.sh`. It runs `DZBAN_TEST_DATABASES=1 bash scripts/test-cli.sh`, using ShellSpec process contracts and `scripts/test-cli-pty.py` against the disposable services. Run `bash scripts/test-cli.sh` alone for database-free process contracts; its PTY case is explicitly skipped without the fixture flag. The terminal test requires Python `pexpect` and `pspg`, uses temporary history data, and checks the picker, query submission, plan/history inspection, cancellation, restart, resize, and pager return. Explicit wire-level checks cover Enter, Ctrl+Enter, Shift+Enter, table listing and quitting with an unfinished draft on both adapters. It never reads a user connection file.

The ordinary Compose runner excludes `certificate_validation_and_cancel_recovery`, which needs its separate TLS endpoints. On macOS, run `python3 scripts/test-macos-tls.py` for the isolated TLS trust, rejection, and cancellation gate.

## Adding coverage

- Put pure parsing, formatting, state, and error-chain checks in ordinary `#[test]` functions near the implementation.
- Put public database workflows in `crates/dzban-db/tests/`.
- Mark tests that need a Compose service with a precise `#[ignore = "requires ..."]` reason so the normal workspace suite remains self-contained.
- Reuse the ports and credentials from `docker-compose.test.yml`; do not use `config.toml`.
- Add schema or data required by a server version to `crates/dzban-db/tests/fixtures/` and mount it from Compose.
- Assert observable contracts: returned values, detailed errors, connection/query state, cancellation, timing, and isolation. Avoid tests that merely call a function without checking its result.
- Keep shared-fixture integration tests serial unless each test creates and drops isolated resources.
- If a new service is necessary, give it a health check and a deterministic cleanup path.

After failures, inspect `docker compose -f docker-compose.test.yml ps` and service logs before changing application code. Tear the environment down when finished.

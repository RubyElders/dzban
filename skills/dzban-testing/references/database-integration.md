# Database integration tests

The repository defines disposable PostgreSQL 15, 16, 17, slow-query, and ClickHouse services in `docker-compose.test.yml`. Test credentials in that file are fixtures and are unrelated to user configuration.

Run the suite serially because tests share service fixtures:

```bash
docker compose -f docker-compose.test.yml up -d --wait
CCACHE_DISABLE=1 cargo +1.88.0 test -p dzban-db -- --ignored --test-threads=1
docker compose -f docker-compose.test.yml down
```

The reusable wrapper is `skills/dzban-testing/scripts/run-db-integration.sh`.

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

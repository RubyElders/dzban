---
name: dzban-testing
description: Build and test Dzban across Rust unit tests and disposable PostgreSQL integration services. Use when changing database, configuration, query execution, persistence, or cross-crate behavior in this repository.
---

# Dzban Testing

Run checks in proportion to the changed behavior, then report passed, ignored, and skipped coverage separately. Do not treat compilation as runtime validation.

Use Rust 1.88 for the compatibility check and disable `ccache` when sandboxed because its default cache is outside the workspace:

```bash
CCACHE_DISABLE=1 cargo +1.88.0 check --workspace --all-targets
cargo clippy --workspace --all-targets --all-features -- -D warnings
CCACHE_DISABLE=1 cargo +1.88.0 test --workspace --all-targets
cargo fmt --all -- --check
git diff --check
```

When database behavior changed, read [references/database-integration.md](references/database-integration.md) and run the ignored suite against the Compose services. Docker access may require approval. Always tear down the disposable services after the run.

Never use credentials from the repository's local `dzban.toml` for tests or copy them into logs, fixtures, screenshots, skills, or commits.

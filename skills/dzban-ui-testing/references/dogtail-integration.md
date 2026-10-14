# Dogtail GTK integration testing

Run the real application through AT-SPI for screen-level workflows:

```bash
skills/dzban-ui-testing/scripts/run-dogtail-integration.sh
```

The runner builds Dzban, starts the PostgreSQL 15 and ClickHouse Compose fixtures, runs each scenario in a private headless Mutter/D-Bus/AT-SPI session, and tears everything down. Dogtail is pinned in the test script's PEP 723 metadata and resolved by `uv`. PyGObject comes from the distribution because it binds to native GTK and AT-SPI libraries.

Keep these invariants:

- Run one `HermeticSession` per Python process. Split independent scenarios into separate invocations because libatspi caches its first desktop connection.
- Start the Python runner with `NO_AT_BRIDGE=1`. Dogtail imports GTK 3 before the hermetic bus exists; disabling that runner-side bridge prevents it from attaching to the user's accessibility bus. `HermeticSession.launch()` sets `NO_AT_BRIDGE=0` for Dzban.
- Launch Dzban from a temporary working directory. Put only fixture credentials in its temporary `config.toml`; never read the repository's user-owned file.
- Prefer `do_action_named()` and editable-text operations. Use raw input only for behavior that is specifically about keyboard routing. Avoid coordinate actions.
- Select nodes by stable accessible name and role. Add labels and descriptions to icon-only controls and custom widgets before testing them.
- Poll observable UI state with bounded timeouts. Include `dump_tree()` in failures instead of adding fixed sleeps.
- Exercise dynamic screens in the same application process when they are part of one workflow. Restarting an application within the same Python process can leave stale libatspi application objects.

The maintained scenarios cover the empty connection state, shortcut reference, configured PostgreSQL and ClickHouse workspaces, SQL editing, successful results, persisted history plan, plan inspector, and query failure state. The shutdown scenario requests window close during an active query, verifies that the application remains open with a retry message, cancels the query, and checks successful process exit after another close request.

The `streaming-postgres` and `streaming-clickhouse` scenarios verify rows appearing
while the query remains cancelable, cancellation and restart, the 10,000-row cutoff
notice, the 32-MiB cutoff with wide values, and a row preview retaining values
longer than the table projection. Hover coverage checks the full cell tooltip and
its dismissal when moving away. Use the action menu to focus results before
keyboard replay; a table's AT-SPI focus request alone does not prove GTK keyboard
routing. Set explicit short input delays
when testing tap-to-pin so the driver does not accidentally exercise hold-to-peek.

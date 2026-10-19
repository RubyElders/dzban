# Dzban console

Build `dzb` with `cargo build -p dzban-cli`. The GTK binary remains `dzban`, built with `cargo build -p dzban-gtk`.

Running `dzb` prints help. Start the console with `dzb console` or `dzb c`. The console offers a searchable connection picker; arrows and Enter select a profile. Use `--connection NAME` to bypass the picker. Escape cancels selection.

```sh
dzb info connections
dzb info config
dzb --connection local c --pager pspg
dzb --connection local q 'SELECT 1'
dzb --connection local query --file query.sql --format csv
printf 'SELECT 1' | dzb --connection local q --format json
```

`--config PATH` overrides `DZBAN_CONFIG` and normal configuration discovery. `--database NAME` temporarily overrides the selected profile's database. Neither option rewrites configuration. Diagnostics show the selected config path and source.

## Catalog commands

Catalog reads are also available without the console:

```sh
dzb --connection local list tables 'public.*' --format json
dzb --connection local ls databases --format csv
dzb --connection local describe table public.users --full
dzb --connection local d view public.user_post_counts --format json
```

`list`/`ls` families are `databases` (`db`), `schemas` (`s`), `tables` (`t`), `views` (`v`), `users` (`u`), `roles` (`r`) and `indexes` (`i`). Use `--system` to include system objects. `describe`/`d` accepts `table` (`t`) or `view` (`v`) with an exact object name and optional `--full`. Object-family selection is enforced. Schema listing is PostgreSQL-only; unsupported backend actions report an error. These commands require an explicit connection, never open a picker or pager, write only results to stdout, and do not record metadata reads as query history. Formats and exit codes follow batch query conventions.

## Console input

Enter submits the whole buffer only when SQL ends with a semicolon outside quotes, comments and parentheses. Otherwise it inserts a newline, like psql. Ctrl+Enter follows the same rule on supporting terminals. Shift+Enter or Ctrl+J always inserts a newline. A lightweight boundary scanner recognizes quoted strings, identifiers, dollar quotes and nested comments; it does not validate SQL or implement psql scripts. Put `\g` on its own line and press Enter to submit without a semicolon. The whole buffer is sent unchanged; there is no SQL completion or client-side statement splitting. Use `\gx` instead for expanded output on that submission only; it does not change the persistent `\x` setting. Output format is captured when a query starts. Bracketed paste inserts text without submitting it. Up/Down browse shared query history at the first/last input line. Ctrl+P/Ctrl+N browse from any line. Returning past the newest entry restores the unfinished draft. Ctrl+L clears and redraws the screen without changing the draft or running query. Ctrl+A/Ctrl+E move to the start/end of the line; Ctrl+B/Ctrl+F move one character. Ctrl+K kills the line suffix, Ctrl+U the prefix, Ctrl+W the previous word, and Ctrl+Y inserts the last killed text. Ctrl+H deletes the previous character. Ctrl+R searches shared profile history. Select with arrows and Enter to load SQL without executing it; Escape preserves the current draft. Dismissing history search with Escape or Ctrl+C also cancels an active query.

Ctrl+C or Escape cancels an active query. Idle Ctrl+C clears input. Input remains editable while a query runs, but a second submission on that connection is rejected. An error or cancellation returns to the prompt.

Queries capture at most 10,000 rows or 32 MiB, including schema/value bytes and encoded spool payload. Up to 200 rows stay in a byte-bounded memory cache; additional rows use private temporary storage. A cutoff drains the native response without retaining further rows; it does not implicitly cancel writes. Ctrl+C or Escape can still cancel during draining.

The console previews the first few rows while receiving results and leaves the editor usable. Completed output and pager input read bounded pages from the same store. Batch output waits for successful completion so a SQL error leaves stdout empty. Limited JSON includes a `cutoff` field; CSV reports the limit on stderr; table/expanded output includes a limit notice. Human tables shorten cells at 120 display columns. Use expanded, JSON or CSV for full captured values.

| Command | Action |
| --- | --- |
| `\?` | Show console help |
| `\d`, `\dt [glob]` | List user tables in the current database |
| `\dv [glob]` | List views in the current database |
| `\d <object>`, `\d+ <object>` | Describe a table/view, optionally including native column details |
| `\l [glob]` | List server databases, not configured connection profiles |
| `\dn [glob]` | List PostgreSQL schemas; unsupported on ClickHouse |
| `\du [glob]`, `\dg [glob]` | List users and roles with backend-specific semantics |
| `\di [glob]` | List PostgreSQL indexes or ClickHouse data-skipping indexes |
| `\q` | Quit when no query is running |
| `\r` | Clear input |
| `\e`, `\edit` | Edit the current buffer, or last submitted SQL when empty, without execution |
| `\w <file>`, `\write <file>` | Save the current buffer, or last submitted SQL when empty, to a new file |
| `\cancel` | Cancel the active query without leaving the console |
| `\p` | Show the current buffer, or the last submitted SQL when empty |
| `\plan` | Show the captured plan |
| `\errverbose` | Show structured diagnostics for the last query or cancellation error |
| `\s`, `\history` | List shared profile history |
| `\conninfo` | Show redacted current connection details |
| `\connections` | List configured profiles without connecting |
| `\c [profile]`, `\connect [profile]` | Switch profiles, or open the picker when omitted |
| `\x` | Toggle expanded result output |
| `\pager [on\|off]` | Inspect or change paging for this console session |
| `\timing [on\|off]` | Toggle timing, or explicitly enable/disable it |

Catalog patterns are case-sensitive globs: `*` matches any text and `?` one Unicode character. They match bare names or schema/database-qualified names, not SQL LIKE patterns or psql regular expressions. Quote arguments containing spaces with shell quotes, for example `\dt 'catalog odd*'`. Patterns never become executable SQL. Append `S` to a catalog command to include system objects where the adapter defines them, for example `\dnS` or `\dtS`. Help lists only supported catalog capabilities. Descriptions use exact bare or schema/database-qualified names in the current connection. Quote names containing spaces with shell quotes. Ambiguous bare names require qualification. Column rows retain catalog order, native type names and default expressions; `\d+` also includes native comments and storage/codec details. `+` is supported only for object descriptions, not listing commands. PostgreSQL retains schemas; ClickHouse has no schema level, and its data-skipping indexes are not PostgreSQL indexes. Full psql pattern/identifier syntax is not implemented.

Every submitted query attempts plain EXPLAIN before execution. PostgreSQL estimates describe output rows and cost; ClickHouse estimates describe scan work. Estimates are not a percentage or execution-time prediction. The console status shows elapsed time from submission, including planning. Core snapshots retain the final duration after completion, failure, or cancellation. With timing enabled, completed requests report total, planning, and execution time, including failed and canceled requests. Pager viewing time is not included. Profile switching is rejected during active work. Failed attempts keep the current connection. Switching uses the selected profile's database, not the startup override. Inside a PostgreSQL transaction, planning uses an internal savepoint so an unsupported EXPLAIN does not abort valid SQL. This adds protocol round trips. A real query error still aborts the transaction normally; issue `ROLLBACK` explicitly to recover.

## Output and storage

External editor precedence is `DZBAN_EDITOR`, `VISUAL`, then `EDITOR`, with `vi` as the fallback. The console restores normal terminal mode before launching the editor. Editor commands are split into executable and arguments without an implicit shell. SQL is supplied in a private temporary file, removed after editing. Editor failures return the original SQL to the buffer; edited SQL requires explicit submission. Buffer saving preserves the draft and never runs SQL. Quote paths containing spaces. Existing files are not overwritten; new files are private to the user on Unix. Put a client command such as `\e`, `\p`, or `\?` on its own final line and press Enter to use it while keeping a multiline draft. `\e` edits that draft, falling back to the last submitted SQL when empty. `\r` clears it. Ctrl+Enter uses the same semicolon rule as Enter unless its final line is a client command; pasted SQL is not executed automatically.

Batch formats are `table`, `csv`, `json`, and `expanded`. Result output uses stdout. Commands without row columns report completion rather than an empty result table. PostgreSQL reports the affected count; ClickHouse may leave it unknown. JSON uses a `command` object and CSV uses an `affected_rows` column for these acknowledgments. Zero-row SELECTs still expose their result columns. Acknowledgments are kept in history. Errors and batch progress use stderr. Batch input can be an argument, a file, or stdin. Each is one opaque request, not a psql script with embedded client commands.

Console pager precedence is `--pager`, `DZBAN_PAGER`, then `PAGER`. `--no-pager` disables paging. Pager selection is resolved when the console starts; an empty explicit or environment selection disables fallback to lower-priority settings. Commands are parsed into executable and arguments without implicit shell evaluation. Missing or failed pagers fall back to plain output. Batch output does not automatically launch a pager.

History is shared with GTK under the existing user data directory and profile identity. The visible list keeps the newest 100 entries. Stored result snapshots retain at most 200 rows and 1,000,000 bytes of column names and displayed values. Live results use the bounded memory and disk-spooled store described above. Immutable records avoid replacing another frontend's writes. Physical record cleanup is not implemented, so the visible retention limit is not a disk usage limit. Damaged, unreadable, future-version, and foreign-profile records are skipped with a warning and left untouched; valid history remains usable and new writes can continue. Core owns publication for submitted CLI queries, even if their caller stops waiting. Storage failures are reported on stderr without replacing the SQL result or its exit code.

Batch exit codes are 0 for success, 1 for query/output failure, 2 for arguments, configuration or connection failure, and 130 for cancellation.

## Tests

Run `bash scripts/test-cli.sh` for ShellSpec process contracts. The Compose integration runner enables its real-terminal workflow, covering Enter, Ctrl+Enter, multiline input, table listing, and quit on both backends, in addition to pager and cancellation behavior.

Run `bash scripts/check-headless.sh` to build `dzb` and test core, adapters, and CLI with Rust 1.88. It uses a separate target directory and disables pkg-config, so native frontend libraries cannot be discovered. The headless CI job runs this check in the Rust container without installing GTK. Service-dependent tests remain ignored here; run the Compose integration routine separately.

`cargo test -p dzban-cli` covers editor/picker logic, output, pager failures, command processes, diagnostics, and transitive dependency boundaries without a database. Service tests are ignored in that run.

Run `bash scripts/test-gtk.sh` for the GTK widget suite and deterministic connection lifecycle regressions with injected adapters. It requires Xvfb and D-Bus but no database, and isolates configuration and application data from the user's setup.

Run `bash skills/dzban-testing/scripts/run-db-integration.sh` for disposable PostgreSQL/ClickHouse tests and the real-terminal workflow. The terminal workflow requires Python `pexpect` and `pspg`; it covers selection, submission, plan/history, cancellation, restart, resize, paste, pager return, and terminal cleanup.

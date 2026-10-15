# Frontend integration

This is the shared Rust boundary for a macOS or other frontend. The shipping GTK
frontend is `dzban-gtk`; it is not a dependency of either shared crate. A new
frontend needs `dzban-core` and the adapter registry from `dzban-db`, not GTK's
GObjects or runtime bridge. Toolkit choice and the Rust/native ABI remain Mac-side
work. Linux validation is not evidence that a macOS binary runs.

## Ownership

| Layer | Owns |
| --- | --- |
| `dzban-core` | Configuration, history, session admission/cleanup, documents, execution, preview invalidation, interaction policy, bounded indexed results, action/capability contracts |
| `dzban-db` | PostgreSQL and ClickHouse adapters, transport, decoding, native diagnostics and cancellation |
| Frontend | Windows/widgets, native keys and menus, focus, selection gestures, styling, accessibility, viewport/cache and event-loop bridge |

The workspace members in the root `Cargo.toml` are the build graph. The older
root `src/` tree is not a workspace package or a frontend integration API.

## Runtime and connection lifetime

Keep one Tokio runtime alive for the application, including pending connections,
queries, result-page reads and shutdown. A synchronous native callback can enter
that runtime to admit a command, then bridge completion/snapshots to its own event
loop. Do not block the UI thread with `block_on`, SQL or disk I/O. GTK's
`RuntimeBridge` is one platform implementation, not a required dependency.

Create `Arc<ApplicationContext>` with `dzban_db::adapters::registry()`. Tests can
inject an `AdapterRegistry` of `AdapterFactory` implementations instead. Register
one factory per backend; each factory creates a `Send + Sync` `DatabaseSession`.

Resolve the selected profile with `Config::resolve_profile`, then call
`ApplicationContext::start_connect`. Keep its request ID, subscribe to the handle,
and await `wait()` on the runtime. `ConnectionSelection::begin/apply` rejects old
connection responses. The context retains pending requests and connected engines
even if a frontend waiter disappears. Explicitly close a connection the user no
longer wants; dropping a view is not disconnecting.

Store stable `SessionId`, `DocumentId`, `QueryId` and output-generation identities.
Names, tab positions and widget addresses are not operation identities.

## Documents and submission

Keep a `QueryDocuments` collection for each connection workspace. Its methods own
document order, selection, rename and close policy. A `QueryDocument` starts empty;
`set_draft` changes its shared draft. Translate cursor/selection context changes
to `query_context_changed` to invalidate an estimate for the previous SQL region.
`query_selection::query_at_cursor` accepts character offsets and returns character
bounds. It preserves simple semicolon splitting; it is not a dialect parser.

Submit using `QueryDocument::submit(engine, sql, limits, history)`. Core admits one
operation per session, attempts plain EXPLAIN, executes, streams/spools, publishes
history and updates retained document snapshots. A different session has its own
admission. Do not run another EXPLAIN or SQL path in the frontend. Core legacy
projection helpers such as `start`, `finish` and `set_execution_plan` are not the
supported frontend execution path.

Subscribe to the document before submission, or read its current snapshot after
subscribing. `watch` is a coalescing state channel, not a lossless event log: always
render the retained current snapshot before awaiting `changed()`. Completion and
errors remain in `DocumentSnapshot::completion`; consumers need not observe every
intermediate phase. Dropping the `DocumentQueryHandle` or a receiver does not
cancel the query. Use its `wait()` result when direct completion delivery is wanted.
Inspect both the outer result and `StreamedQueryCompletion::result`; native query
failure can be a retained completion with an error result.

Update the originating document even when it is not selected. Use
`accepts_output(generation)` before displaying a delivered capture/error. Loading
saved history calls `replace_output`, so late live output cannot replace that view.

Use the engine's `capabilities` and `action_registry` for backend actions, and the
typed catalog requests for object listing/description. A backend can report an
unsupported capability without the frontend guessing SQL dialects. Native focus,
window and menu commands remain frontend commands; share database action IDs and
descriptions rather than copying PostgreSQL catalog SQL into a new frontend.

## Controls and input

Maintain `interaction::AppState` in the frontend controller, not in widgets.
Select a connection with `AppEvent::SessionSelected(Some(SessionInteraction::from_engine(engine)))`;
select the chooser with `SessionSelected(None)`. Refresh the projection when the
engine's operation/query/session subscriptions change. This derives run/stop,
canceling and availability without frontend operation-to-phase switches.

Use `AppEvent::scoped_document_execution` for document publication. The reducer
checks the selected session/query scope again when the event is delivered, so a
queued old event cannot change controls after navigation. Execute the returned
`AppCommand`s and render `primary_action`, `action_hint` and preview visibility.
Do not invent another set of running/canceling booleans.

Normalize physical input to roles; display actual platform key names only in the
frontend's help and labels:

| Role | GTK binding | Meaning |
| --- | --- | --- |
| `PrimaryModifier` | Control | Query guidance and submission modifier |
| `PreviewModifier` | Shift | Add estimation/plan guidance |
| `Inspect` | Space | Tap to pin; hold to peek |
| `Submit` | Enter/keypad Enter | Submit with primary modifier |
| `Cancel` | Escape | Request cancellation while active |

macOS may map Command rather than Control; core does not choose that binding.
Send press/release times as monotonic milliseconds. Preserve the original press
time across repeats. Collapse left/right physical aliases: release the role only
after its last physical key is up. Reset native tracking and send `KeyboardReset`
on focus loss. Menu/button actions dispatch semantic events directly; they need
not synthesize key presses. A terminal can map Ctrl+C to `CancelRequested` without
changing query cancellation policy.

## Preview and cancellation

`PlanPreviewService::schedule` owns debounce and a request target containing the
session, document and revision. Subscribe to its snapshots; render a result only
when `accepts(snapshot, document)` succeeds. Edits, context changes, execution and
document close invalidate stale previews. Notify `workspace_changed` when leaving
that workspace and `cancel` when dismissing the preview context. View invalidation
does not abort the native EXPLAIN transport; engine admission stays occupied until
cleanup finishes. Do not interpret a hidden overlay as an idle connection.

`PeekState` and `preview_release` share tap/hold/pin semantics for native plan and
row overlays. Core emits visibility intent; widgets only display it. Query start
closes the plan. Keep preview state separate from the submitted query's stored plan.

Cancel with `QueryDocument::request_cancel(engine)`, not by dropping its handle
or aborting a Tokio task. Cancellation is scoped to the query ID. Render canceling
until actual terminal completion, and retain any rejection/error. Capturing the
row/byte limit is not cancellation: core/adapters drain the operation to preserve
writes and transaction state. Show `Draining` as active, not a completed query.

## Results and history

Render `ResultSnapshot` as it arrives. Before schema/rows, show a compact waiting
state; once available, render the table while still indicating active work.
`ResultHandle::read_page(start, count)` offloads disk access to blocking workers.
Pages are capped at `MAX_PAGE_ROWS` (200). Keep a bounded native cache rather than
building widgets or collecting values for every row. Preserve viewport positions
when valid; clamp only when the available range shrinks.

Default capture limits are 10,000 rows or 32 MiB, with an initial in-memory working
set of at most 200 rows/2 MiB and the remainder spooled to private temporary files.
These bound stored payload, not total process RSS: schema and display bytes are
accounted, and encoded disk rows have a separate remaining-spool check. Oversized
schema/cells and encoding expansion have explicit rejection/cutoff behavior.
Use `cutoff` and terminal state to label truncated results. Retain handles while a
view needs pages; release them on close so their temporary spool can be removed.

Keep original column/row order, full values and `ResultCell::Null` distinct from
the text `NULL`. Ellipsis, newline flattening, alignment, tooltips and expanded
row presentation belong to the frontend. They must not change stored values.
Use `ExecutionError` and its native backend/code/detail/hint for useful errors.

Use the same history key as existing GTK/CLI: the selected profile name. Pass a
fresh `QueryHistoryStore` to each submission; refresh a history view after other
writers publish. Readers retain the latest 100 entries, with bounded saved result
snapshots (200 rows/1,000,000 bytes), not the full live capture. Immutable private
entry files and rename-based publication support concurrent frontends; do not
replace them with a frontend-owned read/modify/write history file.

## Closing and shutdown

Use `close_in_workspace` for a query tab. `Busy` keeps it open; `Closed` removes an
ordinary tab; `CloseWorkspace` leaves the final draft intact while session cleanup
is requested. `ApplicationContext::close(id)` admits/owns backend cleanup. Keep
the page on rejection or cleanup failure. Only after success call
`retire_closed_session(engine)` and remove the workspace. `ConnectionSelection`
tracks `begin_close/finish_close`, blocks reconnect during cleanup and restores
the ready selection on failure.

On application exit, call `start_shutdown`, await its handle and inspect the
outcome. It owns pending connection drainage and session cleanup. A busy/failing
shutdown restores admission for retry; keep the native window/runtime alive and
offer cancellation/retry. Never treat dropping the runtime as successful shutdown.

## Configuration and platform work

Existing discovery is explicit `DZBAN_CONFIG`, local `config.toml`, user
`$XDG_CONFIG_HOME/dzban/config.toml` (fallback `~/.config/dzban/config.toml`), then
`$XDG_CONFIG_DIRS/dzban/config.toml` (fallback `/etc/xdg/dzban/config.toml`). It does
not walk parent directories. An explicit missing/invalid file is an error; it does
not silently fall through. With no file, configuration is empty.

History uses `$XDG_DATA_HOME/dzban` or `~/.local/share/dzban`. Mac-native directory
defaults, Finder launch, trust-store behavior, packaging, accessibility and input
conventions must be verified on the Mac. Preserve shared history/config semantics;
do not silently copy credentials or introduce a native history format. No native
toolkit, ABI, supported macOS version/architecture matrix, signing or notarization
is selected by this extraction.

## Native history and TLS gates

The macOS bridge lists at most 100 history summaries with SQL previews bounded to 256 Unicode scalars. Each list has an opaque revision; restore requires that revision and its entry index. Refresh invalidates previous selections. Restored results retain their loaded history snapshot under the document's new output generation, independently of a live query that may still be finishing. A new submission replaces the saved output. Saved plans are fetched separately from document snapshots and remain subject to the bridge record-size limit.

The shared legacy history format stores sample cells as text, without original types or SQL null identity. Native saved samples label this limitation and preserve the literal text `NULL`. Live results retain typed SQL nulls. History keeps the selected profile name and existing XDG paths; restoring a query does not execute it or migrate files.

`bash scripts/check-macos-launch.sh APP_BUNDLE direct` copies a production app outside the checkout and starts it from a different working directory with a minimal environment and disposable HOME/config/data. The `launchservices` mode opens the same isolated copy through macOS LaunchServices. For each mode, verify the empty connection chooser and quit normally with Command-Q; process survival alone is insufficient. The UI test runner is sandboxed and cannot launch an arbitrary copied executable, so these launch checks run separately from XCUITest. They do not prove installation on a clean Mac.

`python3 scripts/test-macos-tls.py` creates disposable PostgreSQL and ClickHouse TLS services and certificates, runs the macOS adapters serially, and removes its resources. It checks private-CA rejection with the default macOS roots, rejection with an unrelated explicit CA, hostname mismatch, and trusted execution/cancellation/recovery with a process-local `SSL_CERT_FILE`. The trusted case verifies an explicit test anchor; it does not prove that a private CA was installed in the user's macOS keychain. The runner does not modify keychain trust or disable certificate verification.

## Executable contracts

The platform-free end-to-end example is a public integration test:

```sh
cargo +1.88.0 test -p dzban-core --test frontend_workflow
bash scripts/check-portable.sh
bash scripts/check-headless.sh
```

`frontend_workflow` injects an adapter and exercises connection, preview, early
spooled rows, navigation, dropped observers, history, cancellation/restart and
safe close/shutdown. Other public tests cover failures, capability denial,
cutoffs, stale responses and concurrent history writers. The dependency gate
checks all-target/all-feature normal, build and development edges for known
native/terminal frontend packages. The headless runner disables native package
discovery and builds/tests core, adapters and CLI; it is also wired into CI.

For GTK changes run `bash scripts/test-gtk.sh` and the maintained Dogtail runner.
For adapter/runtime changes run the serial database runner documented in
`skills/dzban-testing`. Workspace tests alone skip service/display cases.
Add corresponding native interaction tests on the Mac; Linux screenshots and
successful dependency checks do not replace macOS runtime validation.

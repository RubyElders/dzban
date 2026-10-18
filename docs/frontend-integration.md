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

Resolve a saved profile with `Config::resolve_profile`. Desktop frontends call `ApplicationContext::start_connect_tab`, which allocates a stable `TabId` and a retained query document before starting the connection. `ConnectionHandle::tab` identifies that tab; its connection request ID and snapshots retain the existing asynchronous connection contract. The context owns pending connections even if a frontend waiter disappears. Explicitly close a successful connection the user no longer wants. The lower-level `start_connect`/`connect_profile` APIs remain available for the CLI and temporary profile probes without creating desktop tabs.

`ApplicationContext::tabs` owns ordered query tabs, selection and the one-document/one-session association. Read `snapshot()` or subscribe to retained tab snapshots. Each tab has a profile identity, backend, document, connection phase and, when connected, its own engine. Two tabs using the same profile are independent database sessions. PostgreSQL transactions, temporary tables and settings are scoped to the tab's session. Use `tabs.select` or `tabs.select_session` when changing selection; no adapter owns UI tab policy.

Store stable `SessionId`, `DocumentId`, `QueryId` and output-generation identities.
Names, tab positions and widget addresses are not operation identities.

## Documents and submission

A desktop query tab has exactly one `QueryDocument`, provided by the shared tab model. Frontends may use `QueryDocuments::from_document` to adapt existing editor/result presentation to that retained document, but must not create an additional nested query collection or reuse one tab's engine for another tab. GTK and the native macOS bridge use the same core tab document. New Tab chooses a profile and opens another session rather than calling `QueryDocuments::open` on an existing connection.

`set_draft` changes the shared draft. Translate cursor/selection context changes to `query_context_changed` to invalidate an estimate for the previous SQL region. `query_selection::query_at_cursor` accepts character offsets and returns character bounds. It preserves simple semicolon splitting; it is not a dialect parser.

Submit through `QueryTab::submit(sql, limits, history)`, which uses the tab's retained document and engine. The lower-level `QueryDocument::submit` remains available for direct consumers. Core admits one operation per session, attempts plain EXPLAIN, executes, streams/spools, publishes history and updates retained document snapshots. Different query tabs have different admission locks and can run concurrently. Do not add an EXPLAIN or SQL execution path in a frontend.

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

The lower-level `QueryDocuments::close_in_workspace` API remains available for direct consumers of document collections. Its `Busy` result retains an active document. Desktop tab closure instead uses the shared session cleanup path below. Native presentation must retain a failed tab until backend cleanup succeeds; `ConnectionSelection::begin_close/finish_close` tracks cleanup in event-loop adapters.

For desktop tabs, `ApplicationContext::close` and `begin_close` use `QueryEngine::start_close_draining`: stop new session admission, cancel the scoped active query, await operation cleanup, disconnect, retire the shared document and remove the tab. Cancellation/disconnection rejection restores an open session and retains the tab for retry. Direct sessions created without desktop tabs retain their existing idle-only close contract.

On application exit, call `start_shutdown`, await its handle and inspect the outcome. It owns pending connection drainage and cleanup of all sessions, including cancellation of active desktop-tab queries. A failure restores application admission for retry; keep the native window/runtime alive and reconcile the surviving tabs with the shared inventory. Some other tabs may already have closed successfully. Never treat dropping the runtime as successful shutdown.

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

## Native tab presentation

The macOS bridge's `connect` command returns both Tab and Session IDs. `open_document` retrieves the connected tab's sole retained document; it does not allocate another query in the same session. The `tabs` command returns the ordered shared inventory and selected Tab ID without connection configuration or credentials. Session/document identities continue to scope execution, reads, preview and cancellation commands. The native UI presents one tab strip with backend artwork, profile name and query title. The native frontend creates an unconnected presentation tab at startup and for New Tab, rendering the same inline connection selector in both cases. Selecting a profile attaches the shared-core session to that existing page identity. Unconnected pages allocate no database session, remain selectable with the other tabs, and can be closed independently. Query actions and progress belong to the selected tab, and the footer follows its health and result metrics. GTK's outer strip presents the same independent session tabs, and its old inner query strip is hidden.

## Native history and TLS gates

Live document snapshots expose the shared query start wall time, monotonic elapsed duration, and a compact execution-plan estimate without the full plan document. Native results render the elapsed timer while active and retain the frozen duration after cleanup. Saved history does not display the active query timing summary.

The native macOS frontend saves its ordered open pages, connection profile names, SQL drafts and selected page to `$XDG_DATA_HOME/dzban/native-workspace.json` (or `~/.local/share/dzban/native-workspace.json`) before shutting down sessions. The file is written atomically with owner-only permissions. On launch it opens independent sessions for saved connected tabs and restores their drafts without submitting SQL. Unconnected pages remain unconnected. An unavailable profile retains its page and draft for reconnection. Live results, active executions and database session state are not restored.

The macOS bridge lists at most 100 history summaries with SQL previews bounded to 256 Unicode scalars. Each list has an opaque revision; restore requires that revision and its entry index. Refresh invalidates previous selections. Restored results retain their loaded history snapshot under the document's new output generation, independently of a live query that may still be finishing. A new submission replaces the saved output. Saved plans are fetched separately from document snapshots and remain subject to the bridge record-size limit. The preview response includes `plan.text`, rendered by `QueryPlan::text()` from the captured PostgreSQL or ClickHouse plan document. Native plan inspection displays this text while the structured document remains available for estimates and diagnostics.

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

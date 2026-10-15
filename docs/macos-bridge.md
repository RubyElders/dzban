# Native macOS bridge contract

`dzban-macos-bridge` exposes a C ABI over `dzban-core` and `dzban-db`. Swift consumers use `macos/Sources/DzbanBridge`. ABI version 1 and JSON schema version 1 are under development; the native application integration is not complete.

## Ownership and delivery

Create an application with `dzban_create`. A nonzero opaque handle owns its Tokio runtime and shared application context. `dzban_send` copies a UTF-8 JSON command of at most 1 MiB and returns a response-stream handle. A zero handle means the bridge could not admit the request. Command validation errors arrive as response records. Never pass an invalid memory address; the readable input range must remain valid until `dzban_send` returns.

Call `dzban_wait` from a dedicated worker, never from the UI thread or a Swift cooperative executor. Pass the last observed revision, initially zero. Normal commands publish one terminal response. `observe_document` instead publishes the current document snapshot followed by coalesced updates on that same stream. Consumers may miss intermediate states and must render the latest retained snapshot, including terminal completion. Only one thread may wait on a stream at a time.

Each record has `schema`, `revision`, and `payload`. A command failure is `payload.failure` with `kind`, `message`, and optional structured `diagnostic`. A document's `error` field is a retained query outcome and is independent of command failure. A successful submission admits work; its response is not evidence that SQL succeeded.

There is one retained record per stream, capped at 8 MiB. Oversized state returns a `record_too_large` failure. Handles are capped at 4096 per process; outstanding foreign buffers are capped at 64. The consumer must release handles and buffers promptly. These limits bound bridge retention and do not bound total process memory, native view caches, SQL transport, or Rust result storage.

Copy a returned `BridgeBuffer` before releasing its token with `dzban_buffer_release`. Never free its pointer with a Swift or C allocator. Buffers survive stream release and have a separate lifetime. Repeated release returns false. Handles are monotonically allocated and never reused.

Release each operation or observation stream with `dzban_release`. Release wakes a blocked waiter and ends subscription delivery. A waiter racing release may already own a buffer; discard it if the consumer has detached. Releasing observation or canceling a Swift task does not cancel database work. Explicit `cancel` requests core cancellation scoped to the document's query identity.

Run `shutdown`, inspect its outcome, and then release the application. Releasing a live application is rejected. Failed or busy cleanup keeps the runtime available for cancellation and retry. Admitted response tasks retain the runtime through their final publication; observer setup registers for application teardown before releasing its runtime lease. Process-aborting failures are not recoverable through the ABI. Unwinding command-worker panics become `worker_failed`; exported entry points contain unwinding panics and return their documented failure status.

## Command inventory

| Command | Inputs | Response |
| --- | --- | --- |
| `profiles` | None | Profile names/backend only; loads shared configuration off the caller thread |
| `connect` | `profile`, optional navigation `revision` | Connection request/session UUIDs and whether selection accepted completion; requires profiles to be loaded |
| `connections` | None | Shared connection-request identities, profiles, and pending state |
| `select_session` | Optional `session`, navigation `revision` | Shared interaction projection; null session shows the connection chooser |
| `controls` | None | Selected session/query identities and shared primary action |
| `open_document` | `session`, optional selection `revision` | Initial document snapshot |
| `select_document` | `session`, `document`, optional selection `revision` | Selected document snapshot; older selection revisions are rejected |
| `close_document` | `session`, `document` | Selected replacement snapshot or workspace-closed acknowledgment; busy work and failed cleanup preserve the document |
| `editor_action` | `session`, `document`, `draft`, scalar `start`/`end`, `intent` | Shared submit/primary/cancel/reset semantics; optional submission identity and cancellation acknowledgment |
| `set_draft` | `session`, `document`, `draft` | Retained document snapshot |
| `preview` | `session`, `document`, `sql`, `revision`, optional paired `start`/`end` and `request_revision` | Accepted plan and target identity; rejects stale draft revisions |
| `cancel_preview` | `session`, optional `request_revision` | Preview invalidation acknowledgment; transport cleanup may still be active |
| `history` | `session` | A fresh read of shared history entries for the selected profile name |
| `actions` | `session` | Backend-capability-filtered action IDs, names, descriptions, effects, and argument contracts |
| `catalog` | `session`, typed `request` | Typed identities and ordered catalog entries, retaining SQL null attributes |
| `context_changed` | `session`, `document` | Advance cursor/selection context revision and invalidate stale estimates |
| `peek` | `session`, `surface`, `event`, optional `elapsed_ms` | Shared plan/row tap, hold, pin, close, and focus-reset visibility |
| `observe_controls` | `session` | Coalesced shared control projection on session, operation, or query changes |
| `observe_document` | `session`, `document` | Initial snapshot and subsequent coalesced snapshots |
| `submit` | `session`, `document`, `sql` | Query UUID and output generation after admission |
| `cancel` | `session`, `document` | Cancellation acknowledgment; terminal completion arrives on the document stream |
| `result_schema` | `session`, `document`, `generation` | Ordered column names and types without result rows |
| `read_viewport` | `session`, `document`, `generation`, `start`, `count`, `column_start`, `column_count` | At most 200 rows and 16 columns of bounded cell previews |
| `read_page` | `session`, `document`, `generation`, `start`, `count` | Schema and original ordered values; count at most 200 |
| `read_cell` | `session`, `document`, `generation`, `row`, `column`, `offset`, `count` | Full-value chunk, total scalar length, and null identity; count at most 65536 Unicode scalars |
| `close_session` | `session` | Closed session UUID after cleanup succeeds |
| `shutdown` | None | Successful shared cleanup acknowledgment, or a retryable failure |

Document snapshots carry session/document/query UUIDs, output generation, draft revision, title/draft, execution state, result phase and availability, captured/received row counts, capture bytes, cutoff, termination, capture diagnostics, command acknowledgments, and retained error. Result pages and cell chunks carry their source document and generation. Reject stale generations before and after asynchronous storage reads. SQL null is encoded as JSON null; the text `NULL` remains text.

`read_page` reports `page_too_large` if its JSON response exceeds the record budget. Request fewer rows or use `read_cell` for a large individual value. Cell offsets/counts use Unicode scalars rather than UTF-16 or grapheme clusters; concatenate chunks without altering the stored text. Null has no text and length zero.

Viewport cells carry nullable `text`, original scalar `length`, and `truncated`. Each preview contains at most 256 Unicode scalars. Display escaping does not alter original values. The native viewport requests 64-row/eight-column tiles, retains at most eight tiles within an 8 MiB accounted budget including schema, and admits at most two schema/page reads across output changes. Its budget counts UTF-8 payloads and estimated collection overhead; it is not a heap or RSS limit. Explicit full-cell copy has a separate 32 MiB UTF-8 limit. The inspector fetches 2048-scalar chunks and supports native text selection.

Result reads acquire one of 16 bridge read permits. Successful document/session close and shutdown drain admitted reads before retiring their result ownership. A retired native viewport discards late responses by both source identity and a local epoch; canceling its Swift task alone does not cancel Rust storage reads.

Navigation revisions reject stale selection commands; late connection completions report whether selection accepted them so a detached native view can explicitly close the resulting session. Document rename commands remain to be added. Query submission loads a fresh shared history store off the caller thread. Fixtures use a private disposable history directory instead of user configuration/data. The fixture lifecycle exercises preview, submission, early pages, history, cancellation/restart, and cleanup; native widget behavior remains a separate integration layer.

The native editor converts AppKit UTF-16 selections to Unicode scalar offsets before `editor_action`. Explicit nonempty selections execute their original text; an empty selection uses the shared current-statement selector. Bounds are validated in Rust. Command-Return and Command-Shift-Return submit the selection/current statement; Escape requests cancellation. The toolbar primary action uses the shared run/stop projection. Busy submission is suppressed by shared interaction policy and core admission remains authoritative. Cancellation targets the active document in the specified session even when another tab is selected.

Each open document owns its native text view, selection, and undo manager. Snapshots do not replace locally edited text. A single worker per document coalesces draft writes; close and shutdown await admitted draft writes. Native composition keeps editing keys operational and prevents menu submission before composition commits. Physical submit repeats are consumed without another submission; focus loss resets shared keyboard/preview state. UTF-16 positions inside a surrogate pair clamp to the beginning of that scalar; combining marks remain separate scalar positions.

## Local verification

```sh
bash scripts/test-macos-bridge.sh
```

The runner isolates configuration/data, builds the fixture-enabled static archive, runs Rust transport/C-ABI tests, and executes shared lifecycle scenarios through a standalone Swift consumer. If XCTest is available it also runs those scenarios through XCTest; otherwise it explicitly reports that coverage as skipped. The standalone consumer is not a substitute for native window or XCUITest validation.

The `test-fixtures` Cargo feature and `DZBAN_TEST_FIXTURES` Swift/C compilation definitions enable the fixture constructor and check scenarios. Production builds must omit them. `bash scripts/build-macos-bridge.sh` builds the default archive without fixtures. The development linker arguments correspond to Rust's `--print native-static-libs` output: CoreFoundation, SystemConfiguration, Security, CoreFoundation, iconv, System, c, and m. Recheck that output when dependencies or build targets change.

Catalog requests use an `operation` of `list` or `describe`. Listing supplies a typed object `kind`, optional exact database/schema and glob, and `include_system`. Description supplies the returned database/schema/kind/name identity and a `full` flag. The bridge calls the core catalog API; Swift contains no backend catalog SQL. Catalog failures and record-size rejection remain explicit errors. Canceling native observation does not imply backend catalog cancellation.

Preview requests accept optional paired scalar `start`/`end` bounds so the shared selector chooses the selected/current statement. Replies retain request, session, document, and draft revision. Native context updates are coalesced and drained before requesting a preview. Monotonic preview intents let cancellation reject an older request even if runtime admission is delayed. Preview admission and cancellation update that revision under the workspace lock. Presentation epochs reject late replies, and close/reset uses the core preview and peek policies. Submitted result plans remain separate from transient previews. Row inspection reads one row and at most 16 columns of bounded previews; full cells use the existing chunked inspector.

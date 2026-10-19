# Native macOS bridge contract

`dzban-macos-bridge` exposes a C ABI over `dzban-core` and `dzban-db`. Swift consumers use `macos/Sources/DzbanBridge`. ABI version 1 and record schema version 1 are built together with the native client. Shared application services own workflows; this crate owns foreign transport, runtime/config composition and bounded handles. The complete refactoring parity audit remains separate from successful compilation.

## Ownership and delivery

Create an application with `dzban_create`. A nonzero opaque handle owns its Tokio runtime and shared application context. `dzban_send` copies a UTF-8 JSON command of at most 1 MiB (32 MiB for `save_workspace` only) and returns a response-stream handle. A zero handle means the bridge could not admit the request. Command validation errors arrive as response records. Never pass an invalid memory address; the readable input range must remain valid until `dzban_send` returns.

Call `dzban_wait` from a dedicated worker, never from the UI thread or a Swift cooperative executor. Pass the last observed revision, initially zero. Normal commands publish one terminal response. Observation commands publish retained snapshots followed by coalesced updates on the same stream. `restore_workspace` streams retained restoration progress and workspace snapshots until completion. Consumers may miss intermediate states and must render the latest retained snapshot, including terminal completion. Only one thread may wait on a stream at a time.

Each record has `schema`, `revision`, and `payload`. A command failure is `payload.failure` with `kind`, `message`, and optional structured `diagnostic`. A document's `error` field is a retained query outcome and is independent of command failure. A successful submission admits work; its response is not evidence that SQL succeeded.

There is one retained record per stream, capped at 8 MiB. Oversized state returns a `record_too_large` failure. Handles are capped at 4096 per process; outstanding foreign buffers are capped at 64. The consumer must release handles and buffers promptly. These limits bound bridge retention and do not bound total process memory, native view caches, SQL transport, or Rust result storage.

Copy a returned `BridgeBuffer` before releasing its token with `dzban_buffer_release`. Never free its pointer with a Swift or C allocator. Buffers survive stream release and have a separate lifetime. Repeated release returns false. Handles are monotonically allocated and never reused.

Release each operation or observation stream with `dzban_release`. Release wakes a blocked waiter and ends subscription delivery. A waiter racing release may already own a buffer; discard it if the consumer has detached. Releasing observation or canceling a Swift task does not cancel database work. Explicit `cancel` requests core cancellation scoped to the document's query identity.

Run `shutdown`, inspect its outcome, and then release the application. Releasing a live application is rejected. Failed or busy cleanup keeps the runtime available for cancellation and retry. Admitted response tasks retain the runtime through their final publication; observer setup registers for application teardown before releasing its runtime lease. Process-aborting failures are not recoverable through the ABI. Unwinding command-worker panics become `worker_failed`; exported entry points contain unwinding panics and return their documented failure status.

## Command inventory

| Command | Inputs | Response |
| --- | --- | --- |
| `profiles` | None | Profile names/backend only; loads shared configuration off the caller thread |
| `tabs` | None | Typed ordered workspace inventory, selection and document projections |
| `open_tab` | Optional `profile`, `draft`, navigation `revision` | Stable chooser tab ID and workspace inventory |
| `select_tab`, `cycle_tabs` | `tab` or signed `direction`, optional navigation `revision` | Workspace inventory; rejects stale navigation |
| `close_tab`, `cancel_tab_connection` | `tab` | Workspace after shared cleanup; cancellation can retain the chooser and draft |
| `set_tab_draft` | `tab`, `draft` | Workspace inventory |
| `restore_workspace` | Typed `location` host/path | Streamed retained restoration progress and inventory; no query execution |
| `save_workspace` | `location`, ordered native `drafts` overrides | Atomic shared save acknowledgment |
| `connect` | `profile`, optional `tab` and navigation `revision` | Request/session/tab UUIDs and selection outcome; requires profiles to be loaded |
| `connections` | None | Shared pending connection identities and profiles |
| `select_session` | Optional `session`, navigation `revision` | Compatibility selection route; null clears shared selection |
| `controls`, `observe_controls` | None, or `session` for observation | Typed primary action, identities and shared revision |
| `open_document` | `session` | Compatibility lookup of the canonical tab's existing document |
| `close_document` | `session`, `document` | Compatibility close acknowledgment; active work/failed cleanup preserve ownership |
| `editor_action` | `session`, `document`, captured `draft`, scalar `start`/`end`, `intent` | Shared submit/primary/cancel/reset outcome |
| `set_draft`, `context_changed` | `session`, `document`, and `draft` for edits | Retained document snapshot with advanced draft/context revision |
| `interact` | `session`, `document`, expected draft `revision`, captured `draft`, scalar selection, typed `input` | Managed shared interaction projection and command outcome |
| `observe_interaction`, `await_plan` | `session`, plus `revision` for a plan waiter | Retained interaction updates, or settled/retired plan projection |
| `input` | `session`, `document`, expected draft `revision`, typed `input` | Reducer compatibility route; new native callers use managed `interact` |
| `preview` | `session`, `document`, `sql`, draft `revision`, optional paired scalar bounds and `request_revision` | Scoped shared plan presentation |
| `cancel_preview` | `session`, optional `request_revision` | Shared preview invalidation acknowledgment |
| `peek` | `session`, plan/row `surface`, `event`, optional `elapsed_ms` | Shared peek/pin visibility |
| `history`, `history_list` | `session` | Raw stored-entry compatibility view, or typed retained history-list revision/entries |
| `restore_history` | `session`, `document`, `history_revision`, `index` | Typed saved-output/document projection; does not submit SQL |
| `saved_plan` | `session`, `document`, `generation` | Shared live/legacy-compatible text plan presentation |
| `actions`, `catalog` | `session`, plus typed `request` for catalog | Shared adapter capabilities/action contracts or typed catalog entries |
| `observe_health`, `observe_attention` | None | Retained typed service snapshots |
| `check_health` | `session` | Shared retained health after a scoped check |
| `check_profile_health`, `check_profiles_health` | `profile`, or none for bulk | Shared checks, deduplication and temporary-session cleanup |
| `probe_session`, `probe_profile` | `session`/`include_version`, or `profile` | Lower-level probe compatibility result |
| `await_idle` | `session` | Wait for temporary-operation cleanup; report an active query without awaiting it |
| `observe_document` | `session`, `document` | Retained typed output/document updates |
| `submit`, `cancel` | `session`, `document`, and `sql` for submission | Scoped admission or cancellation acknowledgment; completion is retained separately |
| `result_schema` | `session`, `document`, `generation` | Ordered names/types without rows |
| `read_viewport` | Output scope, `start`/`count`, `column_start`/`column_count` | At most 200 rows and 16 columns of 256-scalar previews |
| `read_page` | Output scope, `start`, `count` | Original values/schema; at most 200 rows, subject to record budget |
| `read_cell` | Output scope, `row`, `column`, scalar `offset`/`count` | Original value chunk and null identity; at most 65536 scalars |
| `start_export` | Caller `export` UUID, output scope, cell or row `ranges` | Owned shared export reader; at most 16 per app |
| `read_export`, `close_export` | `export` UUID | Lossless bounded text chunk or reader release |
| `close_session` | `session` | Successful canonical session cleanup acknowledgment |
| `shutdown` | None | Shared cleanup acknowledgment and final attention snapshot |

`select_document` and its nested selection revisions are removed. Production native models call typed `ApplicationClient` methods; the table describes the lower-level foreign wire format, not another application controller. Catalog remains a shared/CLI capability and is not exposed as a native browser.

### Compatibility routes and adoption

The native application's feature models use `ApplicationClient` for managed `editor_action` and `interact` commands, tab navigation, restoration, health, attention and output access. Generic request/response pairing stays in `DzbanBridge`. Fixture launch/scenario code deliberately uses lower-level commands to test the public transport independently of the native controller.

| Retained route | Shared authority | Intended use |
| --- | --- | --- |
| `select_session`, `open_document` | `QueryTabs` selection and canonical document | Compatibility selection/lookup; neither creates a second workspace or document |
| `input` | `QueryTabs::input` and `TabInput` | Reducer-only compatibility tests; returns effects without executing them |
| `preview`, `cancel_preview`, `peek` | Shared tab preview/peek lifecycle | Explicit lower-level preview consumers; native command orchestration uses `interact` |
| `submit`, `cancel` | `QueryTab::submit`, `QueryDocument` and engine admission | Explicit lower-level execution/transport tests; native Run/Stop uses managed commands |
| `close_document`, `close_session` | `ApplicationContext::close` | Compatibility cleanup; the former rejects active work before closing the canonical session |
| `probe_session`, `probe_profile` | `HealthService` | Direct probe results; native retained status uses shared check/observation methods |
| `history` | Shared profile history store | Raw persisted-entry compatibility; native history uses typed `history_list` and `restore_history` |

GTK's existing `ConnectionSelection` and `QueryDocuments` consumers and CLI session-only consumers remain supported. Their later migration must adopt shared application workflows explicitly; this refactor does not claim they already use the desktop tab facade. No compatibility route retains a separate bridge engine/document inventory.

Document snapshots carry session/document/query UUIDs, output generation, draft revision, title/draft, execution state, result phase and availability, captured/received row counts, capture bytes, cutoff, termination, capture diagnostics, command acknowledgments, retained error, and optional `execution_seconds` from the result duration (including restored history). Result pages and cell chunks carry their source document and generation. Reject stale generations before and after asynchronous storage reads. SQL null is encoded as JSON null; the text `NULL` remains text.

`read_page` reports `page_too_large` if its JSON response exceeds the record budget. Request fewer rows or use `read_cell` for a large individual value. Cell offsets/counts use Unicode scalars rather than UTF-16 or grapheme clusters; concatenate chunks without altering the stored text. Null has no text and length zero.

Viewport cells carry nullable `text`, original scalar `length`, and `truncated`. Each preview contains at most 256 Unicode scalars. Display escaping does not alter original values. The native viewport requests 64-row/eight-column tiles, retains at most eight tiles within an 8 MiB accounted budget including schema, and admits at most two schema/page reads across output changes. Its budget counts UTF-8 payloads and estimated collection overhead; it is not a heap or RSS limit. Explicit full-cell copy has a separate 32 MiB UTF-8 limit. Full-value inspection uses shared exports within the same limit and supports native text selection.

Result reads acquire one of 16 bridge read permits. Successful document/session close and shutdown drain admitted reads before retiring their result ownership. A retired native viewport discards late responses by both source identity and a local epoch; canceling its Swift task alone does not cancel Rust storage reads.

Navigation revisions reject stale selection commands. Core owns stable chooser/tab identities and cleanup of superseded connections; foreign observers do not become session owners. Document rename commands remain to be added. Query submission loads a fresh shared history store off the caller thread. Fixtures use a private disposable history directory instead of user configuration/data. The fixture lifecycle exercises preview, submission, early pages, history, cancellation/restart, and cleanup; native widget behavior remains a separate integration layer.

The native editor converts AppKit UTF-16 selections to Unicode scalar offsets before `editor_action`. Explicit nonempty selections execute their original text; an empty selection uses the shared current-statement selector. Bounds are validated in Rust. Command-Return and Command-Shift-Return submit the selection/current statement; Escape requests cancellation. The toolbar primary action uses the shared run/stop projection. Busy submission is suppressed by shared interaction policy and core admission remains authoritative. Cancellation targets the active document in the specified session even when another tab is selected.

Each open document owns its native text view, selection, and undo manager. Snapshots do not replace locally edited text. A single worker per document coalesces draft writes; close and shutdown await admitted draft writes. Native composition keeps editing keys operational and prevents menu submission before composition commits. Physical submit repeats are consumed without another submission; focus loss resets shared keyboard/preview state. UTF-16 positions inside a surrogate pair clamp to the beginning of that scalar; combining marks remain separate scalar positions.

## Local verification

```sh
bash scripts/test-macos-bridge.sh
```

The runner isolates configuration/data, builds the fixture-enabled static archive, runs Rust transport/C-ABI tests, and executes shared lifecycle scenarios through a standalone Swift consumer. If XCTest is available it also runs those scenarios through XCTest; otherwise it explicitly reports that coverage as skipped. The standalone consumer is not a substitute for native window or XCUITest validation.

The `test-fixtures` Cargo feature and `DZBAN_TEST_FIXTURES` Swift/C compilation definitions enable the fixture constructor and check scenarios. Production builds must omit them. `bash scripts/build-macos-bridge.sh` builds the default archive without fixtures. The development linker arguments correspond to Rust's `--print native-static-libs` output: CoreFoundation, SystemConfiguration, Security, CoreFoundation, iconv, System, c, and m. Recheck that output when dependencies or build targets change.

Catalog requests use an `operation` of `list` or `describe`. Listing supplies a typed object `kind`, optional exact database/schema and glob, and `include_system`. Description supplies the returned database/schema/kind/name identity and a `full` flag. The bridge calls the core catalog API; Swift contains no backend catalog SQL. Catalog failures and record-size rejection remain explicit errors. Canceling native observation does not imply backend catalog cancellation.

Preview requests accept optional paired scalar `start`/`end` bounds so the shared selector chooses the selected/current statement. Replies retain request, session, document, and draft revision. Native context updates are coalesced and drained before requesting a preview. Monotonic preview intents let cancellation reject an older request even if runtime admission is delayed. Preview admission and cancellation update that revision in the shared tab/preview owner. Presentation epochs reject late replies, and close/reset uses the core preview and peek policies. Submitted result plans remain separate from transient previews. Row inspection reads one row and at most 16 columns of bounded previews; full cells use shared export readers.

## Pure native-boundary helpers

`dzban_result_view` evaluates typed logical selection/restoration policy without I/O and accepts at most 32 MiB of JSON. `dzban_attention` reports host visibility/viewed/lifecycle context without I/O, accepts at most 1 MiB and returns a normal schema-1 record. Both use owned foreign buffer tokens and can run synchronously where native responder/window attachment requires it; publishing native model state still occurs outside rendering.

`dzban_syntax_create` copies at most 32 MiB of UTF-8 source and returns an independent lexer handle. `dzban_syntax_read` returns sequential binary batches of at most 512 spans, tagged with exact source revision and byte/scalar/UTF-16 lengths. This endpoint uses its binary schema-1 header rather than the JSON record envelope; see [the C header](../crates/dzban-macos-bridge/include/dzban_macos.h) for word layout and kind codes. Syntax work runs on a background worker. Release every buffer and lexer handle, and retire the reader on failure. Native code coalesces edits, rejects stale revisions and retains IME/text storage/undo/colors.

Exports return at most 65536 UTF-8 bytes per pull and stop at the shared 32 MiB encoded limit. The client waits for admission, schedules reader release even after cancellation, and checks cancellation between chunks. Publish only a terminal successful result to the pasteboard; never replace it with partial data. Detailed shared ownership, typed projections and frontend adoption seams are in [frontend integration](frontend-integration.md).

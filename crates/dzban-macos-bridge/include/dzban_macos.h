#ifndef DZBAN_MACOS_H
#define DZBAN_MACOS_H
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

typedef struct {
    uint32_t status;
    uint64_t token;
    const uint8_t *data;
    size_t length;
} BridgeBuffer;

uint32_t dzban_abi_version(void);
/* Pure policy with no I/O; may run on the UI thread. Input is at most 32 MiB JSON.
 * Status 7 means invalid input. Successful buffers use the normal token release rules. */
BridgeBuffer dzban_result_view(const uint8_t *data, size_t length);
uint64_t dzban_create(void);
/* Syntax work has no I/O but belongs on a background worker. Copies up to 32 MiB of UTF-8.
 * Read sequential binary batches of at most 512 spans; release each buffer token and the lexer handle.
 * Little-endian u64 header: schema=1, revision, source bytes/scalars/UTF-16, span count, done.
 * Each span: start bytes/scalars/UTF-16, end bytes/scalars/UTF-16, kind.
 * Kinds: 0 keyword, 1 string, 2 number, 3 comment, 4 identifier, 5 function.
 * Abort and release the reader after a failed read. */
uint64_t dzban_syntax_create(const uint8_t *data, size_t length, uint64_t revision);
BridgeBuffer dzban_syntax_read(uint64_t handle);
/* Host attention input does no I/O and may run on the UI thread. Input is at most 1 MiB.
 * Status 7 means invalid input; status 6 includes the 8 MiB response limit.
 * Successful buffers use the normal token release rules. */
BridgeBuffer dzban_attention(uint64_t app, const uint8_t *data, size_t length);
#ifdef DZBAN_TEST_FIXTURES
uint64_t dzban_create_fixture(void);
#endif
/* Copies at most 1 MiB of readable UTF-8 JSON, or 32 MiB for workspace saves.
 * Zero means admission failed. */
uint64_t dzban_send(uint64_t app, const uint8_t *data, size_t length);
/* One waiter per stream; never call on the UI thread. Timeout is capped at 60 seconds.
 * Status: 0 record, 1 timeout, 2 released, 3 concurrent waiter, 4 invalid handle,
 * 5 bridge panic, 6 buffer capacity. Records include schema, revision, and payload.
 * Records coalesce; wait again with the last revision to observe subsequent state.
 * A returned buffer remains valid until its token is released. */
BridgeBuffer dzban_wait(uint64_t stream, uint64_t after_revision, uint32_t timeout_ms);
/* Release detaches observation, wakes a waiter, and discards retained state.
 * A racing wait may already own a record; the caller must discard it after release.
 * Release does not cancel SQL. An app can only be released after successful shutdown.
 * Unknown or already released handles return false. Handles are never reused. */
bool dzban_release(uint64_t handle);
/* Release only by token, once. Do not free data with the consumer allocator. */
bool dzban_buffer_release(uint64_t token);
#endif

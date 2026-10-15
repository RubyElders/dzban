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
uint64_t dzban_create(void);
#ifdef DZBAN_TEST_FIXTURES
uint64_t dzban_create_fixture(void);
#endif
/* Copies at most 1 MiB of readable UTF-8 JSON before returning. Zero means admission failed. */
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

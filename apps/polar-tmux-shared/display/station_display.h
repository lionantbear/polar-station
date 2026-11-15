#ifndef POLAR_STATION_DISPLAY_H
#define POLAR_STATION_DISPLAY_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Versioned, processless submission boundary. Not a terminal/TUI renderer.
 * All methods run serially on the creator thread. The synchronous sink may
 * borrow bytes only until it returns; it must copy for asynchronous use.
 * No queue, PTY, subprocess, layout or message state is owned here.
 */
#define PS_DISPLAY_ABI_VERSION 1u
#define PS_DISPLAY_MAX_BYTES (64u * 1024u)

typedef struct ps_display ps_display;
typedef enum {
    PS_DISPLAY_OK = 0,
    PS_DISPLAY_INVALID = 1,
    PS_DISPLAY_WRONG_THREAD = 2,
    PS_DISPLAY_RETIRED = 3,
    PS_DISPLAY_STALE = 4,
    PS_DISPLAY_RESET_REQUIRED = 5,
    PS_DISPLAY_LIMIT = 6,
    PS_DISPLAY_UNSAFE_TEXT = 7,
    PS_DISPLAY_BUSY = 8,
    PS_DISPLAY_SINK_FAILED = 9,
    PS_DISPLAY_NO_MEMORY = 10
} ps_display_status;

typedef enum {
    PS_DISPLAY_REPLACE_WINDOW = 1,
    PS_DISPLAY_APPEND_TEXT = 2,
    /* UTF-8 JSON array of typed text/style runs; sink validates decoded fields. */
    PS_DISPLAY_REPLACE_RUNS = 3,
    PS_DISPLAY_APPEND_RUNS = 4
} ps_display_operation;

/* A nonzero return means NO update was committed. The native adapter must
 * not perform partial output and then report failure. Window replacement
 * invalidates semantic hit maps upstream; this module never guesses them.
 */
typedef int (*ps_display_sink)(void *context, uint32_t operation,
                               const uint8_t *utf8, size_t byte_count);

typedef struct {
    uint32_t abi_version;
    uint64_t surface_generation;
    size_t max_bytes;
    ps_display_sink sink;
    void *context;
} ps_display_config;

typedef struct {
    uint64_t surface_generation;
    uint64_t content_epoch;
    uint64_t base_revision;
    uint64_t next_revision;
    uint32_t operation;
    const uint8_t *utf8;
    size_t byte_count;
} ps_display_batch;

typedef struct {
    uint64_t content_epoch;
    uint64_t revision;
    uint64_t accepted_batches;
    uint64_t accepted_bytes;
    int retired;
} ps_display_snapshot;

ps_display_status ps_display_create(const ps_display_config *config, ps_display **out);
/* Bind establishes a new content identity. First update MUST replace window.
 * Epochs increase monotonically even when returning to a previous Thread.
 */
ps_display_status ps_display_bind(ps_display *display, uint64_t content_epoch);
ps_display_status ps_display_submit(ps_display *display, const ps_display_batch *batch);
ps_display_status ps_display_read(const ps_display *display, ps_display_snapshot *out);
ps_display_status ps_display_retire(ps_display *display);
/* destroy cannot be called inside the sink. It consumes the handle on OK;
 * the caller must ensure no future call, queued callback or concurrent user.
 */
ps_display_status ps_display_destroy(ps_display *display);

#ifdef __cplusplus
}
#endif
#endif

#include "station_display.h"

#include <pthread.h>
#include <stdlib.h>
#include <string.h>

struct ps_display {
    pthread_t owner;
    uint64_t generation;
    uint64_t epoch;
    uint64_t revision;
    uint64_t batches;
    uint64_t bytes;
    size_t max_bytes;
    ps_display_sink sink;
    void *context;
    int busy;
    int retired;
    int needs_snapshot;
};

static ps_display_status check_owner(const ps_display *d) {
    if (d == NULL) return PS_DISPLAY_INVALID;
    if (!pthread_equal(d->owner, pthread_self())) return PS_DISPLAY_WRONG_THREAD;
    return PS_DISPLAY_OK;
}

/* Strict UTF-8; no overlong sequences, surrogates, out-of-range values or
 * terminal control codes. LF/TAB are logical text, not caller-supplied VT.
 * The eventual Ghostty sink owns the translation LF -> CRLF when needed.
 */
static int safe_text(const uint8_t *s, size_t n) {
    size_t i = 0;
    while (i < n) {
        uint32_t cp, minimum;
        size_t more;
        uint8_t b = s[i++];
        if (b < 0x80) { cp = b; more = 0; minimum = 0; }
        else if (b >= 0xc2 && b <= 0xdf) { cp = b & 0x1fu; more = 1; minimum = 0x80; }
        else if (b >= 0xe0 && b <= 0xef) { cp = b & 0x0fu; more = 2; minimum = 0x800; }
        else if (b >= 0xf0 && b <= 0xf4) { cp = b & 0x07u; more = 3; minimum = 0x10000; }
        else return 0;
        if (more > n - i) return 0;
        while (more-- > 0) {
            uint8_t c = s[i++];
            if ((c & 0xc0u) != 0x80u) return 0;
            cp = (cp << 6) | (c & 0x3fu);
        }
        if (cp < minimum || cp > 0x10ffff || (cp >= 0xd800 && cp <= 0xdfff)) return 0;
        if ((cp < 0x20 && cp != '\n' && cp != '\t') || (cp >= 0x7f && cp <= 0x9f)) return 0;
    }
    return 1;
}

ps_display_status ps_display_create(const ps_display_config *c, ps_display **out) {
    if (out == NULL) return PS_DISPLAY_INVALID;
    *out = NULL;
    if (c == NULL || c->abi_version != PS_DISPLAY_ABI_VERSION ||
        c->surface_generation == 0 || c->sink == NULL ||
        c->max_bytes == 0 || c->max_bytes > PS_DISPLAY_MAX_BYTES) return PS_DISPLAY_INVALID;
    ps_display *d = calloc(1, sizeof(*d));
    if (d == NULL) return PS_DISPLAY_NO_MEMORY;
    d->owner = pthread_self();
    d->generation = c->surface_generation;
    d->max_bytes = c->max_bytes;
    d->sink = c->sink;
    d->context = c->context;
    d->needs_snapshot = 1;
    *out = d;
    return PS_DISPLAY_OK;
}

ps_display_status ps_display_bind(ps_display *d, uint64_t epoch) {
    ps_display_status status = check_owner(d);
    if (status != PS_DISPLAY_OK) return status;
    if (d->busy) return PS_DISPLAY_BUSY;
    if (d->retired) return PS_DISPLAY_RETIRED;
    if (epoch == 0) return PS_DISPLAY_INVALID;
    if (epoch <= d->epoch) return PS_DISPLAY_STALE;
    d->epoch = epoch;
    d->revision = 0;
    d->needs_snapshot = 1;
    return PS_DISPLAY_OK;
}

ps_display_status ps_display_submit(ps_display *d, const ps_display_batch *b) {
    ps_display_status status = check_owner(d);
    if (status != PS_DISPLAY_OK) return status;
    if (d->busy) return PS_DISPLAY_BUSY;
    if (d->retired) return PS_DISPLAY_RETIRED;
    if (b == NULL || (b->byte_count != 0 && b->utf8 == NULL) ||
        (b->operation < PS_DISPLAY_REPLACE_WINDOW || b->operation > PS_DISPLAY_APPEND_RUNS))
        return PS_DISPLAY_INVALID;
    if (b->byte_count > d->max_bytes) return PS_DISPLAY_LIMIT;
    if (d->epoch == 0 || b->surface_generation != d->generation || b->content_epoch != d->epoch)
        return PS_DISPLAY_STALE;
    if (b->base_revision != d->revision || b->base_revision == UINT64_MAX ||
        b->next_revision != b->base_revision + 1 ||
        (d->needs_snapshot && b->operation != PS_DISPLAY_REPLACE_WINDOW && b->operation != PS_DISPLAY_REPLACE_RUNS))
        return PS_DISPLAY_RESET_REQUIRED;
    if (!safe_text(b->utf8, b->byte_count)) return PS_DISPLAY_UNSAFE_TEXT;
    /* Snapshot before entering foreign code. The callback cannot alias the
     * original mutable native/FFI buffer. The sink borrows this owned copy.
     */
    uint8_t *owned = NULL;
    if (b->byte_count != 0) {
        owned = malloc(b->byte_count);
        if (owned == NULL) return PS_DISPLAY_NO_MEMORY;
        memcpy(owned, b->utf8, b->byte_count);
    }
    const size_t count = b->byte_count;
    const uint64_t next = b->next_revision;
    d->busy = 1;
    int failed = d->sink(d->context, b->operation, owned, count);
    d->busy = 0;
    free(owned);
    if (failed) return PS_DISPLAY_SINK_FAILED;
    d->revision = next;
    d->needs_snapshot = 0;
    if (d->batches != UINT64_MAX) d->batches++;
    d->bytes = count > UINT64_MAX - d->bytes ? UINT64_MAX : d->bytes + count;
    return PS_DISPLAY_OK;
}

ps_display_status ps_display_read(const ps_display *d, ps_display_snapshot *out) {
    ps_display_status status = check_owner(d);
    if (status != PS_DISPLAY_OK) return status;
    if (out == NULL) return PS_DISPLAY_INVALID;
    *out = (ps_display_snapshot){d->epoch, d->revision, d->batches, d->bytes, d->retired};
    return PS_DISPLAY_OK;
}

ps_display_status ps_display_retire(ps_display *d) {
    ps_display_status status = check_owner(d);
    if (status != PS_DISPLAY_OK) return status;
    if (d->busy) return PS_DISPLAY_BUSY;
    d->retired = 1;
    d->sink = NULL;
    d->context = NULL;
    return PS_DISPLAY_OK;
}

ps_display_status ps_display_destroy(ps_display *d) {
    ps_display_status status = ps_display_retire(d);
    if (status != PS_DISPLAY_OK) return status;
    free(d);
    return PS_DISPLAY_OK;
}

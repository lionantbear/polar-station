#include "pcfs_pack_plan.h"
#include <limits.h>
#include <stdlib.h>
#include <string.h>

static int entry_cmp(const void *a, const void *b) {
    const struct pcfs_pack_entry_v1 *x = a, *y = b;
    int c = memcmp(x->sha256, y->sha256, 32);
    if (c) return c;
    if (x->size_bytes != y->size_bytes) return x->size_bytes < y->size_bytes ? -1 : 1;
    return x->source_token < y->source_token ? -1 : x->source_token != y->source_token;
}
int32_t pcfs_pack_plan(const struct pcfs_pack_options_v1 *o,
    const struct pcfs_pack_entry_v1 *entries, size_t count,
    struct pcfs_pack_entry_v1 **out, size_t *n_out, uint64_t *bytes_out) {
    struct pcfs_pack_entry_v1 *sorted;
    size_t n = 0;
    uint64_t total = 0;
    if (!out || !n_out || !bytes_out) return PCFS_INVALID;
    *out = NULL; *n_out = 0; *bytes_out = 0;
    if (!o || o->struct_size != sizeof(*o) || (!entries && count)) return PCFS_INVALID;
    if (o->abi_version != PCFS_ABI_V1 || (o->profile != PCFS_EROFS_PROFILE_V1 && o->profile != PCFS_EROFS_PROFILE_NATIVE_V1 && o->profile != PCFS_EROFS_PROFILE_BUILD_V1) ||
        o->compression > PCFS_EROFS_COMPRESSION_LZ4) return PCFS_UNSUPPORTED;
    if (!o->max_objects || o->max_objects > PCFS_EROFS_MAX_OBJECTS ||
        count > o->max_objects || count > SIZE_MAX / sizeof(*sorted) ||
        o->max_input_bytes > INT64_MAX || o->max_output_bytes > INT64_MAX ||
        o->max_output_bytes < 4096) return PCFS_LIMIT;
    if (!count) return PCFS_OK; /* A valid, empty filesystem pack. */
    sorted = malloc(count * sizeof(*sorted));
    if (!sorted) return PCFS_IO;
    memcpy(sorted, entries, count * sizeof(*sorted));
    qsort(sorted, count, sizeof(*sorted), entry_cmp);
    for (size_t i = 0; i < count; ++i) {
        if (n && !memcmp(sorted[n-1].sha256, sorted[i].sha256, 32)) {
            if (sorted[n-1].size_bytes != sorted[i].size_bytes) {
                free(sorted); return PCFS_CORRUPT;
            }
            continue;
        }
        if (sorted[i].size_bytes > o->max_input_bytes - total) {
            free(sorted); return PCFS_LIMIT;
        }
        total += sorted[i].size_bytes;
        sorted[n++] = sorted[i];
    }
    *out = sorted; *n_out = n; *bytes_out = total;
    return PCFS_OK;
}

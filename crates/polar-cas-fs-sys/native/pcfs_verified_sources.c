#define _GNU_SOURCE
#define _FILE_OFFSET_BITS 64
#include "pcfs_verified_sources_private.h"
#include "liberofs_sha256.h"
#include <stdbool.h>
#include <limits.h>
#include <stdlib.h>
#include <string.h>

struct original_source { uint64_t token, block_start; };
struct pcfs_verified_sources {
    struct pcfs_sources_v1 original;
    struct original_source *items;
    struct pcfs_pack_entry_v1 *entries;
    uint8_t *digests, *cache;
    size_t count, block_bytes, active_index;
    uint64_t cache_block;
    size_t cache_length;
    struct pcfs_source *active;
    struct pcfs_verified_stats_v1 stats;
    int32_t failure;
    bool busy, cache_valid;
};
static int32_t poison(struct pcfs_verified_sources *s, int32_t rc) {
    if (!s->failure) s->failure = rc;
    return s->failure;
}
static int32_t ready(struct pcfs_verified_sources *s) {
    if (s->failure) return s->failure;
    if (s->original.is_cancelled && s->original.is_cancelled(s->original.context))
        return poison(s, PCFS_CANCELLED);
    return PCFS_OK;
}
static int32_t close_active(struct pcfs_verified_sources *s) {
    int32_t rc = PCFS_OK;
    if (s->active) {
        rc = pcfs_source_check(s->active);
        pcfs_source_destroy(s->active); s->active = NULL;
    }
    s->cache_valid = false;
    return rc ? poison(s, rc) : s->failure;
}
static int32_t open_active(struct pcfs_verified_sources *s, size_t index) {
    int32_t rc;
    if (s->active && s->active_index == index) return PCFS_OK;
    rc = close_active(s); if (rc) return rc;
    rc = pcfs_source_open(&s->original, s->items[index].token,
        s->entries[index].size_bytes, &s->active);
    if (rc) return poison(s, rc);
    s->active_index = index;
    ++s->stats.source_opens; s->stats.peak_open_sources = 1;
    return PCFS_OK;
}
static int32_t increase(uint64_t *n, uint64_t v) {
    if (v > UINT64_MAX - *n) return PCFS_LIMIT;
    *n += v; return PCFS_OK;
}
void pcfs_verified_sources_destroy(struct pcfs_verified_sources *s) {
    if (!s) return;
    (void)close_active(s);
    free(s->items); free(s->entries); free(s->digests); free(s->cache); free(s);
}
int32_t pcfs_verified_sources_identify(const struct pcfs_verify_options_v1 *o,
    const struct pcfs_sources_v1 *provider, const struct pcfs_source_spec_v1 *specs,
    size_t count, struct pcfs_verified_sources **out) {
    uint64_t total = 0, blocks = 0, index_bytes;
    struct pcfs_verified_sources *s = NULL;
    int32_t rc = PCFS_OK;
    if (!out) return PCFS_INVALID;
    *out = NULL;
    if (!o || o->struct_size != sizeof(*o) || !provider ||
        provider->struct_size != sizeof(*provider) || (!specs && count) || o->reserved)
        return PCFS_INVALID;
    if (o->abi_version != PCFS_ABI_V1 || provider->abi_version != PCFS_ABI_V1)
        return PCFS_UNSUPPORTED;
    if (count && !provider->read_at && !provider->open_fd) return PCFS_INVALID;
    if (o->block_bytes < 4096 || o->block_bytes > PCFS_IO_MAX_TRANSFER ||
        (o->block_bytes & (o->block_bytes - 1))) return PCFS_INVALID;
    if (!o->max_sources || o->max_sources > PCFS_EROFS_MAX_OBJECTS ||
        count > o->max_sources || o->max_input_bytes > INT64_MAX ||
        count > SIZE_MAX / (sizeof(struct original_source) + sizeof(struct pcfs_pack_entry_v1)))
        return PCFS_LIMIT;
    index_bytes = count * (sizeof(struct original_source) + sizeof(struct pcfs_pack_entry_v1));
    for (size_t i = 0; i < count; ++i) {
        if (specs[i].size_bytes > o->max_input_bytes - total) return PCFS_LIMIT;
        total += specs[i].size_bytes;
        uint64_t n = specs[i].size_bytes / o->block_bytes + !!(specs[i].size_bytes % o->block_bytes);
        if (increase(&blocks, n)) return PCFS_LIMIT;
    }
    if (blocks > (UINT64_MAX - index_bytes) / 32) return PCFS_LIMIT;
    index_bytes += blocks * 32;
    if (index_bytes > o->max_index_bytes || index_bytes > SIZE_MAX) return PCFS_LIMIT;
    s = calloc(1, sizeof(*s)); if (!s) return PCFS_IO;
    s->original = *provider; s->count = count; s->block_bytes = o->block_bytes;
    s->stats.index_bytes = index_bytes; s->stats.cache_bytes = o->block_bytes;
    s->items = calloc(count ? count : 1, sizeof(*s->items));
    s->entries = calloc(count ? count : 1, sizeof(*s->entries));
    s->digests = malloc(blocks ? (size_t)blocks * 32 : 1);
    s->cache = malloc(o->block_bytes);
    if (!s->items || !s->entries || !s->digests || !s->cache) { rc = PCFS_IO; goto fail; }
    blocks = 0;
    for (size_t i = 0; i < count; ++i) {
        struct sha256_state hash;
        uint64_t size = specs[i].size_bytes;
        s->items[i] = (struct original_source){specs[i].source_token, blocks};
        s->entries[i].size_bytes = size; s->entries[i].source_token = i;
        rc = ready(s); if (!rc) rc = open_active(s, i); if (rc) goto fail;
        erofs_sha256_init(&hash);
        for (uint64_t off = 0; off < size;) {
            size_t n = size - off < s->block_bytes ? (size_t)(size - off) : s->block_bytes;
            rc = pcfs_source_pread_exact(s->active, off, s->cache, n);
            if (rc) goto fail;
            if (erofs_sha256_process(&hash, s->cache, (unsigned long)n)) { rc = PCFS_IO; goto fail; }
            erofs_sha256(s->cache, (unsigned long)n, s->digests + (size_t)blocks * 32);
            ++blocks; off += n; s->stats.identity_bytes += n;
        }
        if (erofs_sha256_done(&hash, s->entries[i].sha256)) { rc = PCFS_IO; goto fail; }
        rc = close_active(s); if (rc) goto fail;
    }
    rc = ready(s); if (rc) goto fail;
    *out = s; return PCFS_OK;
fail:
    pcfs_verified_sources_destroy(s); return rc;
}
int32_t pcfs_verified_sources_identity(const struct pcfs_verified_sources *s,
    size_t index, struct pcfs_source_identity_v1 *out) {
    if (!out) return PCFS_INVALID;
    memset(out, 0, sizeof(*out));
    if (!s || index >= s->count) return PCFS_INVALID;
    if (s->failure) return s->failure;
    memcpy(out->sha256, s->entries[index].sha256, 32);
    out->size_bytes = s->entries[index].size_bytes; return PCFS_OK;
}
int32_t pcfs_verified_sources_pread(struct pcfs_verified_sources *s, size_t index,
    uint64_t offset, uint8_t *buffer, size_t capacity, size_t *read_out) {
    int32_t rc;
    if (!read_out) return PCFS_INVALID;
    *read_out = 0;
    if (!s || index >= s->count || (!buffer && capacity)) return PCFS_INVALID;
    rc = ready(s); if (rc) return rc;
    uint64_t size = s->entries[index].size_bytes;
    if (offset >= size || !capacity) return PCFS_OK;
    rc = open_active(s, index); if (rc) return rc;
    /* A cached verified block is already an owned snapshot of exactly the
     * identified bytes. Mutating external data cannot alter that buffer. */
    uint64_t block = offset / s->block_bytes, start = block * s->block_bytes;
    if (!s->cache_valid || s->cache_block != block) {
        uint8_t hash[32];
        size_t n = size - start < s->block_bytes ? (size_t)(size - start) : s->block_bytes;
        s->cache_valid = false;
        rc = pcfs_source_pread_exact(s->active, start, s->cache, n);
        if (rc) return poison(s, rc);
        if (increase(&s->stats.encoding_source_bytes, n)) return poison(s, PCFS_LIMIT);
        erofs_sha256(s->cache, (unsigned long)n, hash);
        if (memcmp(hash, s->digests + (size_t)(s->items[index].block_start + block) * 32, 32))
            return poison(s, PCFS_CORRUPT);
        rc = pcfs_source_check(s->active); if (rc) return poison(s, rc);
        ++s->stats.verified_blocks;
        s->cache_block = block; s->cache_length = n; s->cache_valid = true;
    } else {
        rc = pcfs_source_check(s->active); if (rc) return poison(s, rc);
        ++s->stats.cache_hits;
    }
    size_t in_block = (size_t)(offset - start), n = s->cache_length - in_block;
    if (n > capacity) n = capacity;
    if (increase(&s->stats.delivered_bytes, n)) return poison(s, PCFS_LIMIT);
    memcpy(buffer, s->cache + in_block, n); *read_out = n; return PCFS_OK;
}
int32_t pcfs_verified_sources_stats(const struct pcfs_verified_sources *s,
    struct pcfs_verified_stats_v1 *out) {
    if (!out) return PCFS_INVALID;
    memset(out, 0, sizeof(*out)); if (!s) return PCFS_INVALID;
    *out = s->stats; return PCFS_OK;
}
const struct pcfs_pack_entry_v1 *pcfs_verified_entries(const struct pcfs_verified_sources *s, size_t *count) {
    *count = s->count; return s->entries;
}
static int32_t gate_read(void *ctx, uint64_t token, uint64_t off, uint8_t *buf, size_t cap, size_t *got) {
    if (token > SIZE_MAX) return PCFS_INVALID;
    return pcfs_verified_sources_pread(ctx, (size_t)token, off, buf, cap, got);
}
static int32_t gate_cancel(void *ctx) { return ready(ctx) != PCFS_OK; }
int32_t pcfs_verified_begin(struct pcfs_verified_sources *s, struct pcfs_sources_v1 *out) {
    if (!s || !out || s->busy) return PCFS_INVALID;
    int32_t rc = ready(s); if (!rc) rc = close_active(s); if (rc) return rc;
    *out = (struct pcfs_sources_v1){PCFS_ABI_V1, sizeof(*out), s, NULL, gate_read, gate_cancel};
    s->busy = true; return PCFS_OK;
}
int32_t pcfs_verified_end(struct pcfs_verified_sources *s) {
    if (!s || !s->busy) return PCFS_INVALID;
    int32_t rc = close_active(s); if (!rc) rc = ready(s);
    s->busy = false; return rc;
}

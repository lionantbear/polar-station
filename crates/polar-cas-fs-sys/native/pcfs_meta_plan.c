#define _GNU_SOURCE
#include "pcfs_meta_plan.h"
#include <limits.h>
#include <stdlib.h>
#include <string.h>

int32_t pcfs_meta_validate_path(const char *path) {
    if (!path) return PCFS_INVALID;
    size_t len = strnlen(path, PCFS_META_PATH_MAX + 1u);
    if (!len || len > PCFS_META_PATH_MAX || path[0] == '/' || path[len-1] == '/') return PCFS_INVALID;
    const char *s = path;
    for (const char *p = s;; ++p) if (*p == '/' || !*p) {
        size_t n = (size_t)(p - s);
        if (!n || n > 255 || (n == 1 && s[0] == '.') ||
            (n == 2 && s[0] == '.' && s[1] == '.')) return PCFS_INVALID;
        if (!*p) break;
        s = p + 1;
    }
    return PCFS_OK;
}
static int compare(const void *a, const void *b) {
    const struct pcfs_meta_planned *x = a, *y = b;
    return strcmp(x->entry->path, y->entry->path);
}
static int compare_key(const void *a, const void *b) {
    const struct pcfs_meta_planned *y = b;
    return strcmp(a, y->entry->path);
}
int32_t pcfs_meta_plan(const struct pcfs_meta_options_v1 *o,
    const struct pcfs_meta_entry_v1 *entries, size_t count, size_t npacks,
    struct pcfs_meta_planned **out) {
    if (!out) return PCFS_INVALID;
    *out = NULL;
    if (!o || o->struct_size != sizeof(*o) || (!entries && count)) return PCFS_INVALID;
    if (o->abi_version != PCFS_ABI_V1 || (o->profile != PCFS_NATIVE_META_PROFILE_V1 &&
        o->profile != PCFS_HYBRID_META_PROFILE_V2)) return PCFS_UNSUPPORTED;
    if (o->root_mode > 0777) return PCFS_UNSUPPORTED;
    if (!o->max_entries || o->max_entries > PCFS_EROFS_MAX_OBJECTS || count > o->max_entries ||
        count > SIZE_MAX / sizeof(struct pcfs_meta_planned) || npacks > PCFS_META_MAX_PACKS ||
        o->max_output_bytes < 4096 || o->max_output_bytes > INT64_MAX ||
        !o->max_path_bytes || o->max_path_bytes > SIZE_MAX ||
        o->max_content_bytes > INT64_MAX) return PCFS_LIMIT;
    struct pcfs_meta_planned *plan = calloc(count ? count : 1, sizeof(*plan));
    if (!plan) return PCFS_IO;
    uint64_t bytes = 0, content_bytes = 0;
    int32_t rc = PCFS_OK;
    for (size_t i = 0; i < count; ++i) {
        const struct pcfs_meta_entry_v1 *e = entries + i;
        rc = pcfs_meta_validate_path(e->path);
        if (rc) goto fail;
        size_t pathlen = strlen(e->path) + 1;
        if (pathlen > o->max_path_bytes - bytes) { rc = PCFS_LIMIT; goto fail; }
        bytes += pathlen;
        if (e->mode > 0777 || e->reserved) { rc = PCFS_UNSUPPORTED; goto fail; }
        if (e->kind == PCFS_META_FILE || e->kind == PCFS_META_REDIRECT) {
            if (e->kind == PCFS_META_REDIRECT && o->profile != PCFS_HYBRID_META_PROFILE_V2) {
                rc = PCFS_UNSUPPORTED; goto fail;
            }
            if ((e->kind == PCFS_META_FILE ? e->pack_index >= npacks : e->pack_index != 0) ||
                e->symlink_target || e->size_bytes > INT64_MAX) {
                rc = PCFS_INVALID; goto fail;
            }
            if (e->size_bytes > o->max_content_bytes - content_bytes) { rc = PCFS_LIMIT; goto fail; }
            content_bytes += e->size_bytes;
        } else if (e->kind == PCFS_META_SYMLINK) {
            size_t n = e->symlink_target ? strnlen(e->symlink_target, PCFS_META_PATH_MAX + 1u) : 0;
            if (!n || n > PCFS_META_PATH_MAX || e->mode != 0777 || e->size_bytes || e->pack_index) {
                rc = PCFS_INVALID; goto fail;
            }
            if (n + 1 > o->max_path_bytes - bytes) { rc = PCFS_LIMIT; goto fail; }
            bytes += n + 1;
        } else if (e->kind == PCFS_META_DIRECTORY) {
            if (e->symlink_target || e->size_bytes || e->pack_index) { rc = PCFS_INVALID; goto fail; }
        } else { rc = PCFS_UNSUPPORTED; goto fail; }
        if (e->kind != PCFS_META_FILE && e->kind != PCFS_META_REDIRECT) {
            static const uint8_t empty[32];
            if (memcmp(e->sha256, empty, 32)) { rc = PCFS_INVALID; goto fail; }
        }
        plan[i].entry = e;
        plan[i].parent = SIZE_MAX;
    }
    qsort(plan, count, sizeof(*plan), compare);
    for (size_t i = 0; i < count; ++i) {
        const char *path = plan[i].entry->path, *slash = strrchr(path, '/');
        if (i && !strcmp(path, plan[i-1].entry->path)) { rc = PCFS_INVALID; goto fail; }
        if (slash) {
            char parent[PCFS_META_PATH_MAX + 1u];
            size_t n = (size_t)(slash - path);
            memcpy(parent, path, n); parent[n] = 0;
            struct pcfs_meta_planned *found = bsearch(parent, plan, i, sizeof(*plan), compare_key);
            if (!found || found->entry->kind != PCFS_META_DIRECTORY) { rc = PCFS_INVALID; goto fail; }
            plan[i].parent = (size_t)(found - plan);
        }
    }
    *out = plan;
    return PCFS_OK;
fail:
    free(plan);
    return rc;
}

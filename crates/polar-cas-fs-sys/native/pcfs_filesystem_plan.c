#define _GNU_SOURCE
#include "pcfs_filesystem_plan.h"
#include <limits.h>
#include <stdlib.h>
#include <string.h>

static int32_t copy_span(const char *in, uint64_t len, char **out, uint64_t *budget) {
    *out = NULL;
    if (!in || !len || len > PCFS_META_PATH_MAX) return PCFS_INVALID;
    if (len + 1 > *budget) return PCFS_LIMIT;
    if (memchr(in, 0, (size_t)len)) return PCFS_INVALID;
    char *text = malloc((size_t)len + 1);
    if (!text) return PCFS_IO;
    memcpy(text, in, (size_t)len); text[len] = 0;
    *budget -= len + 1; *out = text;
    return PCFS_OK;
}
void pcfs_filesystem_plan_destroy(struct pcfs_filesystem_plan *p) {
    if (!p) return;
    if (p->entries) for (size_t i = 0; i < p->count; ++i) {
        free((void *)p->entries[i].path);
        free((void *)p->entries[i].symlink_target);
    }
    free(p->entries); free(p->ordered); free(p->sources);
    memset(p, 0, sizeof(*p));
}
int32_t pcfs_filesystem_plan(const struct pcfs_filesystem_options_v1 *o,
    const struct pcfs_filesystem_entry_v1 *in, size_t count,
    struct pcfs_filesystem_plan *p) {
    if (!p) return PCFS_INVALID;
    memset(p, 0, sizeof(*p));
    if (!o || o->struct_size != sizeof(*o) || (!in && count)) return PCFS_INVALID;
    if (o->abi_version != PCFS_ABI_V1 || o->compression > PCFS_EROFS_COMPRESSION_LZ4 ||
        o->root_mode > 0777) return PCFS_UNSUPPORTED;
    if (!o->max_entries || o->max_entries > PCFS_EROFS_MAX_OBJECTS || count > o->max_entries ||
        count > SIZE_MAX / sizeof(*p->entries) || count > SIZE_MAX / sizeof(*p->sources) ||
        o->max_output_bytes < 4096 || o->max_output_bytes > INT64_MAX ||
        o->max_input_bytes > INT64_MAX || !o->max_path_bytes || o->max_path_bytes > SIZE_MAX)
        return PCFS_LIMIT;
    p->metadata = (struct pcfs_meta_options_v1){
        .abi_version = PCFS_ABI_V1, .struct_size = sizeof(p->metadata),
        .profile = PCFS_NATIVE_META_PROFILE_V1, .root_mode = o->root_mode,
        .uid = o->uid, .gid = o->gid, .max_output_bytes = o->max_output_bytes,
        .max_entries = o->max_entries, .max_path_bytes = o->max_path_bytes,
        .max_content_bytes = o->max_input_bytes,
    };
    p->count = count;
    p->entries = calloc(count ? count : 1, sizeof(*p->entries));
    p->sources = calloc(count ? count : 1, sizeof(*p->sources));
    int32_t rc = PCFS_IO;
    if (!p->entries || !p->sources) goto fail;
    uint64_t budget = o->max_path_bytes;
    for (size_t i = 0; i < count; ++i) {
        const struct pcfs_filesystem_entry_v1 *e = in + i;
        struct pcfs_meta_entry_v1 *dst = p->entries + i;
        if (e->struct_size != sizeof(*e) || e->abi_version != PCFS_ABI_V1 || e->reserved) {
            rc = PCFS_INVALID; goto fail;
        }
        if (e->kind != PCFS_META_DIRECTORY && e->kind != PCFS_META_FILE &&
            e->kind != PCFS_META_SYMLINK) { rc = PCFS_UNSUPPORTED; goto fail; }
        char *path = NULL;
        rc = copy_span(e->path, e->path_bytes, &path, &budget);
        if (rc) goto fail;
        dst->path = path;
        if (e->kind == PCFS_META_SYMLINK) {
            char *target = NULL;
            rc = copy_span(e->symlink_target, e->symlink_bytes, &target, &budget);
            if (rc) goto fail;
            dst->symlink_target = target;
        } else if (e->symlink_target || e->symlink_bytes) { rc = PCFS_INVALID; goto fail; }
        if (e->kind != PCFS_META_FILE && e->source_token) { rc = PCFS_INVALID; goto fail; }
        dst->kind = e->kind; dst->mode = e->mode; dst->size_bytes = e->size_bytes;
        memcpy(dst->sha256, e->sha256, 32);
        if (e->kind == PCFS_META_FILE) {
            if (e->size_bytes > o->max_input_bytes - p->input_bytes) { rc = PCFS_LIMIT; goto fail; }
            p->input_bytes += e->size_bytes; ++p->files;
            p->sources[i].size_bytes = e->size_bytes;
            p->sources[i].source_token = e->source_token;
            memcpy(p->sources[i].sha256, e->sha256, 32);
        }
    }
    /* Reuse canonical paths, exact explicit parents, duplicate rejection, and
     * file/directory/link field checks. The dummy pack slot never becomes a
     * device: this builder writes ALL payload into its own output image. */
    rc = pcfs_meta_plan(&p->metadata, p->entries, count, 1, &p->ordered);
    if (!rc) return PCFS_OK;
fail:
    pcfs_filesystem_plan_destroy(p); return rc;
}

int32_t pcfs_filesystem_validate_v1(const struct pcfs_filesystem_options_v1 *options,
    const struct pcfs_filesystem_entry_v1 *entries, size_t count) {
    struct pcfs_filesystem_plan plan;
    int32_t rc = pcfs_filesystem_plan(options, entries, count, &plan);
    if (!rc) pcfs_filesystem_plan_destroy(&plan);
    return rc;
}

#define _GNU_SOURCE
#define _FILE_OFFSET_BITS 64
#include "pcfs_erofs_private.h"
#include "pcfs_pack_plan.h"
#include "pcfs_erofs_job.h"
#include "pcfs_verified_sources_private.h"
#include "pcfs_filesystem_plan.h"
#include <inttypes.h>
#include "erofs/importer.h"
#include "erofs/inode.h"
#include "erofs/xattr.h"
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/uio.h>
#include <unistd.h>

struct build_context;
struct input_vf {
    struct erofs_vfile vf; /* first: ops recover this wrapper, never vf.fd */
    struct build_context *job;
    const struct pcfs_pack_entry_v1 *entry;
    struct pcfs_source *source;
    uint64_t pos;
    const uint8_t *memory;
    size_t memory_size;
};
struct build_context {
    struct pcfs_sources_v1 sources;
    struct pcfs_native_job *io;
    struct input_vf *inputs;
    struct erofs_inode **nodes;
    size_t node_count, node_capacity, input_count;
    struct input_vf summary_input;
    const char *summary;
    size_t summary_size;
};
static int fail_job(struct build_context *b, int32_t rc) {
    return pcfs_job_fail(b->io, rc);
}
static int cancelled(struct build_context *b) { return pcfs_job_cancelled(b->io); }
static int ensure_input(struct input_vf *v) {
    if (cancelled(v->job)) return -EIO;
    if (v->source || v->memory) return 0;
    int32_t rc = pcfs_source_open(&v->job->sources, v->entry->source_token,
                                 v->entry->size_bytes, &v->source);
    return rc ? fail_job(v->job, rc) : 0;
}
static ssize_t input_pread(struct erofs_vfile *vf, void *buf, size_t len, u64 off) {
    struct input_vf *v = (void *)vf;
    if (ensure_input(v)) return -EIO;
    uint64_t size = v->memory ? v->memory_size : v->entry->size_bytes;
    if (off >= size || !len) return 0;
    if ((uint64_t)len > size - off) len = (size_t)(size - off);
    if (v->memory) { memcpy(buf, v->memory + off, len); return (ssize_t)len; }
    if (len > SSIZE_MAX) return fail_job(v->job, PCFS_LIMIT);
    int32_t rc = pcfs_source_pread_exact(v->source, off, buf, len);
    return rc ? fail_job(v->job, rc) : (ssize_t)len;
}
static ssize_t input_read(struct erofs_vfile *vf, void *buf, size_t len) {
    struct input_vf *v = (void *)vf;
    ssize_t got = input_pread(vf, buf, len, v->pos);
    if (got > 0) v->pos += (uint64_t)got;
    return got;
}
static off_t input_seek(struct erofs_vfile *vf, u64 off, int whence) {
    struct input_vf *v = (void *)vf;
    uint64_t pos;
    if (cancelled(v->job)) return -EIO;
    if (whence == SEEK_SET) pos = off;
    else if (whence == SEEK_CUR && off <= INT64_MAX - v->pos) pos = v->pos + off;
    else if (whence == SEEK_END && off == 0) pos = v->memory ? v->memory_size : v->entry->size_bytes;
    else return -EINVAL;
    if (pos > INT64_MAX) return -EOVERFLOW;
    v->pos = pos; return (off_t)pos;
}
static int input_stat(struct erofs_vfile *vf, struct stat *st) {
    struct input_vf *v = (void *)vf;
    if (ensure_input(v)) return -EIO;
    int32_t rc = v->memory ? PCFS_OK : pcfs_source_check(v->source);
    if (rc) return fail_job(v->job, rc);
    memset(st, 0, sizeof(*st));
    st->st_mode = S_IFREG | 0444;
    st->st_size = (off_t)(v->memory ? v->memory_size : v->entry->size_bytes);
    st->st_blksize = 4096;
    return 0;
}
static void input_close(struct erofs_vfile *vf) {
    struct input_vf *v = (void *)vf;
    if (v->source) {
        int32_t rc = pcfs_source_check(v->source);
        if (rc) (void)fail_job(v->job, rc);
        pcfs_source_destroy(v->source); v->source = NULL;
    }
}
static struct erofs_vfops input_ops = {
    .pread = input_pread, .read = input_read, .lseek = input_seek,
    .fstat = input_stat, .close = input_close,
};
static int hold_node(struct build_context *b, struct erofs_inode *inode) {
    if (b->node_count == b->node_capacity) return -ENOMEM;
    b->nodes[b->node_count++] = erofs_igrab(inode);
    return 0;
}
static struct erofs_inode *add_node_typed(struct build_context *b, struct erofs_sb_info *sb,
    struct erofs_inode *parent, const char *name, const char *path, uint32_t mode) {
    struct erofs_inode *n = erofs_new_inode(sb);
    struct erofs_dentry *d;
    if (IS_ERR(n)) return n;
    n->i_srcpath = strdup(path);
    if (!n->i_srcpath) { erofs_iput(n); return ERR_PTR(-ENOMEM); }
    n->i_mode = mode;
    n->i_uid = 0; n->i_gid = 0; n->i_mtime = 0; n->i_mtime_nsec = 0;
    n->i_nlink = S_ISDIR(mode) ? 2 : 1;
    n->i_parent = parent;
    n->i_ino[1] = b->node_count + 1;
    d = erofs_d_alloc(parent, name);
    if (IS_ERR(d)) { erofs_iput(n); return ERR_CAST(d); }
    d->inode = n; d->type = erofs_mode_to_ftype(n->i_mode);
    if (hold_node(b, n)) return ERR_PTR(-ENOMEM);
    return n;
}
static struct erofs_inode *add_node(struct build_context *b, struct erofs_sb_info *sb,
    struct erofs_inode *parent, const char *name, const char *path, bool directory) {
    return add_node_typed(b, sb, parent, name, path,
                         directory ? (S_IFDIR | 0555) : (S_IFREG | 0444));
}
/* Every node has a job-owned pin. On an error the upstream traversal can leave
 * temporary traversal/dentry references. After all buffer managers have been
 * destroyed, only this private build graph can refer to these nodes. Resetting
 * those abandoned references is safe here; this MUST NOT be used on shared or
 * reader inodes. It also releases tails not attached to an abortable buffer. */
static void discard_nodes(struct build_context *b) {
    for (size_t i = 0; i < b->node_count; ++i) {
        struct erofs_inode *n = b->nodes[i];
        free(n->idata); n->idata = NULL;
        erofs_atomic_set(&n->i_count, 1);
        erofs_iput(n);
    }
    free(b->nodes); b->nodes = NULL;
}
static int make_tree(struct build_context *b, struct erofs_importer *im,
                     const struct pcfs_pack_entry_v1 *entries, size_t count) {
    static const char hex[] = "0123456789abcdef";
    struct erofs_inode *objects, *sha, *shard = NULL;
    unsigned int last_shard = 256;
    struct erofs_sb_info *sb = im->sbi;
    im->root = erofs_make_empty_root_inode(im, sb);
    if (IS_ERR(im->root)) { int err = PTR_ERR(im->root); im->root = NULL; return err; }
    im->root->i_mode = S_IFDIR | 0555;
    if (hold_node(b, im->root)) return -ENOMEM;
    objects = add_node(b, sb, im->root, "objects", "/objects", true);
    if (IS_ERR(objects)) return PTR_ERR(objects);
    sha = add_node(b, sb, objects, "sha256", "/objects/sha256", true);
    if (IS_ERR(sha)) return PTR_ERR(sha);
    for (size_t i = 0; i < count; ++i) {
        char path[PCFS_SHA256_PATH_CAPACITY], shardname[3], shardpath[20];
        struct erofs_inode *n;
        if (cancelled(b)) return -ECANCELED;
        if (last_shard != entries[i].sha256[0]) {
            last_shard = entries[i].sha256[0];
            shardname[0] = hex[last_shard >> 4]; shardname[1] = hex[last_shard & 15]; shardname[2] = 0;
            snprintf(shardpath, sizeof(shardpath), "/objects/sha256/%s", shardname);
            shard = add_node(b, sb, sha, shardname, shardpath, true);
            if (IS_ERR(shard)) return PTR_ERR(shard);
        }
        if (pcfs_sha256_path(entries[i].sha256, path, sizeof(path))) return -EINVAL;
        n = add_node(b, sb, shard, path + strlen("objects/sha256/xx/"), path, false);
        if (IS_ERR(n)) return PTR_ERR(n);
        n->i_size = entries[i].size_bytes;
        b->inputs[i] = (struct input_vf){
            .vf = { .ops = &input_ops, .fd = -1 }, .job = b, .entry = entries + i,
        };
        n->pcfs_vf = &b->inputs[i].vf;
    }
    if (b->summary) {
        struct erofs_inode *dir = add_node(b, sb, im->root, ".polar", "/.polar", true);
        if (IS_ERR(dir)) return PTR_ERR(dir);
        struct erofs_inode *file = add_node(b, sb, dir, "pack-build.json", "/.polar/pack-build.json", false);
        if (IS_ERR(file)) return PTR_ERR(file);
        file->i_size = b->summary_size;
        b->summary_input = (struct input_vf){ .vf = { .ops = &input_ops, .fd = -1 },
            .job = b, .memory = (const uint8_t *)b->summary, .memory_size = b->summary_size };
        file->pcfs_vf = &b->summary_input.vf;
    }
    return 0;
}
/* Ordinary-path graph for the new standalone filesystem profile. Source data
 * still enters the SAME pcfs_write_file/importer adapter as hash-path packs. */
static int make_filesystem_tree(struct build_context *b, struct erofs_importer *im,
    const struct pcfs_filesystem_plan *plan) {
    const struct pcfs_meta_options_v1 *o = &plan->metadata;
    im->root = erofs_make_empty_root_inode(im, im->sbi);
    if (IS_ERR(im->root)) { int err = PTR_ERR(im->root); im->root = NULL; return err; }
    im->root->i_mode = S_IFDIR | o->root_mode;
    im->root->i_uid = o->uid; im->root->i_gid = o->gid;
    if (hold_node(b, im->root)) return -ENOMEM;
    for (size_t i = 0; i < plan->count; ++i) {
        if (cancelled(b)) return -ECANCELED;
        const struct pcfs_meta_planned *slot = plan->ordered + i;
        const struct pcfs_meta_entry_v1 *e = slot->entry;
        size_t original = (size_t)(e - plan->entries);
        struct erofs_inode *parent = slot->parent == SIZE_MAX ? im->root : b->nodes[slot->parent + 1];
        const char *name = strrchr(e->path, '/'); name = name ? name + 1 : e->path;
        uint32_t mode = (e->kind == PCFS_META_DIRECTORY ? S_IFDIR :
                         e->kind == PCFS_META_FILE ? S_IFREG : S_IFLNK) | e->mode;
        struct erofs_inode *n = add_node_typed(b, im->sbi, parent, name, e->path, mode);
        if (IS_ERR(n)) return PTR_ERR(n);
        n->i_uid = o->uid; n->i_gid = o->gid;
        if (e->kind == PCFS_META_SYMLINK) {
            n->i_link = strdup(e->symlink_target);
            if (!n->i_link) return -ENOMEM;
            n->i_size = strlen(e->symlink_target);
        } else if (e->kind == PCFS_META_FILE) {
            n->i_size = e->size_bytes;
            b->inputs[i] = (struct input_vf){ .vf = { .ops = &input_ops, .fd = -1 },
                .job = b, .entry = plan->sources + original };
            n->pcfs_vf = &b->inputs[i].vf;
        }
    }
    return 0;
}
static int32_t verify_filesystem_tree(struct pcfs_image *image,
    const struct pcfs_filesystem_plan *plan, const struct pcfs_sources_v1 *sources,
    bool audit_payload) {
    const struct pcfs_meta_options_v1 *o = &plan->metadata;
    struct erofs_inode root = { .sbi = &image->sbi, .nid = image->sbi.root_nid };
    if (erofs_read_inode_from_disk(&root) || !S_ISDIR(root.i_mode) ||
        (root.i_mode & 07777) != o->root_mode || root.i_uid != o->uid || root.i_gid != o->gid)
        return PCFS_CORRUPT;
    uint64_t total = 0;
    int32_t rc = pcfs_native_meta_count_entries(image, plan->count + 1, &total);
    if (rc) return rc;
    if (total != plan->count + 1) return PCFS_CORRUPT;
    for (size_t i = 0; i < plan->count; ++i) {
        const struct pcfs_meta_entry_v1 *e = plan->ordered[i].entry;
        if (sources->is_cancelled && sources->is_cancelled(sources->context)) return PCFS_CANCELLED;
        struct pcfs_meta_stat_v1 st;
        rc = pcfs_native_meta_stat(image, e->path, &st);
        if (rc) return rc;
        if (st.kind != e->kind || st.mode != e->mode || st.uid != o->uid || st.gid != o->gid)
            return PCFS_CORRUPT;
        if (e->kind == PCFS_META_FILE) {
            struct pcfs_member *member = NULL;
            rc = pcfs_image_path_member(image, e->path, e->sha256, e->size_bytes, &member);
            if (!rc && audit_payload)
                rc = pcfs_verify_member_cancellable(member, sources->is_cancelled, sources->context);
            pcfs_member_destroy(member);
            if (rc) return rc;
        } else if (e->kind == PCFS_META_SYMLINK) {
            char target[PCFS_META_PATH_MAX + 1]; size_t length = 0;
            rc = pcfs_native_meta_readlink(image, e->path, target, sizeof(target), &length);
            if (rc) return rc;
            if (length != strlen(e->symlink_target) || memcmp(target, e->symlink_target, length))
                return PCFS_CORRUPT;
        }
    }
    return pcfs_image_unchanged(image);
}
/* Fixed-width, domain-separated inventory commitment. This is not a Merkle
 * inclusion proof and not a copy of the manifest. Values match the actual
 * sorted unique plan sent to make_tree(). */
static void be64(uint8_t out[8], uint64_t x) {
    for (size_t i = 0; i < 8; ++i) out[7-i] = (uint8_t)(x >> (8*i));
}
static int32_t make_summary(const struct pcfs_pack_entry_v1 *entries, size_t count,
    uint64_t total, uint8_t digest[32], char *out, size_t capacity) {
    static const uint8_t domain[] = "polar.cas.members.v1"; /* includes NUL */
    static const char hex[] = "0123456789abcdef";
    struct sha256_state hash; uint8_t number[8]; char text[65];
    erofs_sha256_init(&hash);
    if (erofs_sha256_process(&hash, domain, sizeof(domain))) return PCFS_IO;
    be64(number, count);
    if (erofs_sha256_process(&hash, number, sizeof(number))) return PCFS_IO;
    for (size_t i = 0; i < count; ++i) {
        be64(number, entries[i].size_bytes);
        if (erofs_sha256_process(&hash, entries[i].sha256, 32) ||
            erofs_sha256_process(&hash, number, sizeof(number))) return PCFS_IO;
    }
    if (erofs_sha256_done(&hash, digest)) return PCFS_IO;
    for (size_t i = 0; i < 32; ++i) { text[2*i] = hex[digest[i] >> 4]; text[2*i+1] = hex[digest[i]&15]; }
    text[64] = 0;
    /* Only fixed ASCII schema values and integers are serialized here. No
     * caller string, host path, timestamp, T/M or self-P hash is interpolated.
     * Keys and separators match the existing sorted canonical JSON contract. */
    int n = snprintf(out, capacity,
        "{\"builder_fingerprint\":\"%s;direct-input-v1;block-sha256-v1\","
        "\"member_bytes\":%" PRIu64 ",\"member_count\":%zu,"
        "\"member_index\":\"erofs-directory-v1\","
        "\"member_path_layout\":\"objects/sha256/prefix2/fullhex\","
        "\"members_digest\":\"sha256:%s\","
        "\"pack_profile\":\"native-compatible-proof-v1\","
        "\"schema\":\"polar.cas.pack-build.v1\"}",
        pcfs_erofs_fingerprint(), total, count, text);
    return n < 0 || (size_t)n >= capacity ? PCFS_LIMIT : PCFS_OK;
}
static int32_t build_impl(const struct pcfs_pack_options_v1 *o,
    const struct pcfs_sources_v1 *sources, const struct pcfs_pack_entry_v1 *entries,
    size_t count, int32_t output_fd, struct pcfs_pack_result_v1 *result,
    struct pcfs_pack_build_result_v2 *proof,
    const struct pcfs_filesystem_plan *filesystem, bool audit_filesystem) {
    struct pcfs_pack_entry_v1 *sorted = NULL;
    struct pcfs_pack_result_v1 completed = {0};
    struct erofs_sb_info sb = {0};
    struct erofs_importer_params params;
    struct erofs_importer im = { .sbi = &sb, .params = &params };
    struct z_erofs_paramset compressors[2] = {{0}, {0}};
    struct build_context b = {0};
    struct pcfs_image *image = NULL;
    bool locked = false, entered = false, importer_started = false;
    size_t unique = 0;
    int32_t rc;
    int err;
    u32 crc;
    char summary[1024];
    if (!result) return PCFS_INVALID;
    memset(result, 0, sizeof(*result));
    if (filesystem) {
        unique = filesystem->count;
        completed.input_bytes = filesystem->input_bytes;
    } else {
        rc = pcfs_pack_plan(o, entries, count, &sorted, &unique, &completed.input_bytes);
        if (rc) return rc;
    }
    if (!sources || sources->abi_version != PCFS_ABI_V1 || sources->struct_size != sizeof(*sources) ||
        ((filesystem ? filesystem->files : unique) && !sources->open_fd && !sources->read_at)) { rc = PCFS_INVALID; goto done; }
    b.sources = *sources;
    if (proof) {
        rc = make_summary(sorted, unique, completed.input_bytes,
                          proof->members_digest, summary, sizeof(summary));
        if (rc) goto done;
        b.summary = summary; b.summary_size = strlen(summary);
    }
    rc = pcfs_job_new(output_fd, o->max_output_bytes, sources->is_cancelled,
                      sources->context, &b.io);
    if (rc) goto done;
    b.node_capacity = filesystem ? unique + 1 : unique + 261;
    b.nodes = calloc(b.node_capacity, sizeof(*b.nodes));
    b.inputs = calloc(unique ? unique : 1, sizeof(*b.inputs));
    b.input_count = unique;
    if (!b.nodes || !b.inputs) { rc = PCFS_IO; goto done; }
    rc = pcfs_native_lock(); if (rc) goto done;
    locked = true;
    rc = pcfs_native_build_enter(); if (rc) goto done;
    entered = true;
    erofs_importer_preset(&params);
    params.fixed_uid = filesystem ? filesystem->metadata.uid : 0;
    params.fixed_gid = filesystem ? filesystem->metadata.gid : 0; params.build_time = 0;
    params.ignore_mtime = true; params.no_xattrs = true;
    params.no_datainline = (o->profile != PCFS_EROFS_PROFILE_V1);
    params.pclusterblks_def = 1; params.pclusterblks_max = 1;
    params.dedupe = EROFS_DEDUPE_FORCE_OFF;
    /* Always supply a terminated list, including raw/no-compressor builds. */
    params.z_paramsets = compressors;
    if (o->compression == PCFS_EROFS_COMPRESSION_LZ4) {
        compressors[0].alg = "lz4"; compressors[0].clevel = -1;
        erofs_sb_set_lz4_0padding(&sb);
    }
    /* Use the supported vfile payload for both the image and metadata buffers. */
    pcfs_job_attach(b.io, &sb.bdev);
    err = erofs_mkfs_format_fs(&sb, 12, 0, false);
    if (err) { rc = pcfs_from_errno(err); goto done; }
    importer_started = true;
    err = erofs_importer_init(&im);
    if (err) { rc = pcfs_from_errno(err); goto done; }
    err = filesystem ? make_filesystem_tree(&b, &im, filesystem) : make_tree(&b, &im, sorted, unique);
    if (!err) err = erofs_importer_load_tree(&im, true, false);
    if (!err) err = erofs_importer_flush_all(&im);
    if (!err) err = erofs_writesb(&sb);
    if (!err) err = erofs_flush_all_devices(&sb);
    if (!err) err = erofs_enable_sb_chksum(&sb, &crc);
    if (err) { rc = pcfs_job_status(b.io) ? pcfs_job_status(b.io) : pcfs_from_errno(err); goto done; }
    if (pcfs_job_status(b.io)) { rc = pcfs_job_status(b.io); goto done; }
    rc = pcfs_job_finish(b.io, &completed.size_bytes, completed.sha256);
    if (rc) goto done;
    if (filesystem) {
        rc = pcfs_open_verified_image(pcfs_job_fd(b.io), completed.size_bytes, &image);
        if (rc) goto done;
        image->projection = true; /* enable bounded path namespace checks */
        rc = verify_filesystem_tree(image, filesystem, sources, audit_filesystem);
        if (rc) goto done;
    } else if (!proof) {
    rc = pcfs_open_verified_image(pcfs_job_fd(b.io), completed.size_bytes, &image);
    if (rc) goto done;
    for (size_t i = 0; i < unique; ++i) {
        struct pcfs_member *member = NULL;
        if (cancelled(&b)) { rc = pcfs_job_status(b.io); goto done; }
        rc = pcfs_image_member(image, sorted[i].sha256, sorted[i].size_bytes, &member);
        if (!rc) rc = pcfs_verify_member_cancellable(member, sources->is_cancelled, sources->context);
        pcfs_member_destroy(member);
        if (rc == PCFS_NOT_FOUND) rc = PCFS_CORRUPT;
        if (rc) goto done;
    }
    } /* legacy caller claims still require the independent decoded audit */
    completed.unique_objects = filesystem ? filesystem->files : unique;
    *result = completed;
    rc = PCFS_OK;
done:
    if (image) pcfs_image_destroy(image);
    if (b.inputs) for (size_t i = 0; i < b.input_count; ++i) input_close(&b.inputs[i].vf);
    if (importer_started) erofs_importer_exit(&im);
    if (entered) {
        erofs_put_super(&sb); /* aborts outstanding buffer refs BEFORE graph disposal */
        discard_nodes(&b);
        pcfs_native_build_leave();
    } else free(b.nodes);
    if (locked) pcfs_native_unlock();
    free(b.inputs); free(sorted);
    if (!rc && b.io && pcfs_job_status(b.io)) rc = pcfs_job_status(b.io);
    pcfs_job_destroy(b.io);
    if (rc) memset(result, 0, sizeof(*result));
    return rc;
}

int32_t pcfs_erofs_build(const struct pcfs_pack_options_v1 *o,
    const struct pcfs_sources_v1 *sources, const struct pcfs_pack_entry_v1 *entries,
    size_t count, int32_t output_fd, struct pcfs_pack_result_v1 *result) {
    /* A caller's public digest claims are never evidence for the new profile. */
    if (o && o->struct_size == sizeof(*o) && o->profile == PCFS_EROFS_PROFILE_BUILD_V1) {
        if (result) memset(result, 0, sizeof(*result));
        return PCFS_UNSUPPORTED;
    }
    return build_impl(o, sources, entries, count, output_fd, result, NULL, NULL, true);
}
static int32_t build_verified_entries(const struct pcfs_pack_options_v1 *o,
    struct pcfs_verified_sources *verified, const struct pcfs_pack_entry_v1 *entries,
    size_t count, int32_t output_fd, struct pcfs_pack_build_result_v2 *result) {
    struct pcfs_sources_v1 provider;
    struct pcfs_verified_stats_v1 before = {0}, after = {0};
    struct pcfs_pack_build_result_v2 completed = {0};
    if (!result) return PCFS_INVALID;
    memset(result, 0, sizeof(*result));
    if (!o || o->struct_size != sizeof(*o) || !verified) return PCFS_INVALID;
    if (o->profile != PCFS_EROFS_PROFILE_BUILD_V1) return PCFS_UNSUPPORTED;
    int32_t rc = pcfs_verified_begin(verified, &provider);
    if (rc) return rc;
    (void)pcfs_verified_sources_stats(verified, &before);
    rc = build_impl(o, &provider, entries, count, output_fd, &completed.pack, &completed, NULL, true);
    int32_t final = pcfs_verified_end(verified);
    if (!rc) rc = final;
    if (rc) return rc;
    (void)pcfs_verified_sources_stats(verified, &after);
    completed.identity_bytes = after.identity_bytes;
    completed.encoding_source_bytes = after.encoding_source_bytes - before.encoding_source_bytes;
    completed.delivered_bytes = after.delivered_bytes - before.delivered_bytes;
    completed.verified_blocks = after.verified_blocks - before.verified_blocks;
    completed.cache_hits = after.cache_hits - before.cache_hits;
    *result = completed; return PCFS_OK;
}

int32_t pcfs_erofs_build_verified(const struct pcfs_pack_options_v1 *o,
    struct pcfs_verified_sources *verified, int32_t output_fd,
    struct pcfs_pack_build_result_v2 *result) {
    if (!result) return PCFS_INVALID;
    memset(result, 0, sizeof(*result));
    if (!verified) return PCFS_INVALID;
    size_t count = 0;
    const struct pcfs_pack_entry_v1 *entries = pcfs_verified_entries(verified, &count);
    return build_verified_entries(o, verified, entries, count, output_fd, result);
}
int32_t pcfs_erofs_build_verified_selected(const struct pcfs_pack_options_v1 *o,
    struct pcfs_verified_sources *verified, const size_t *indices, size_t count,
    int32_t output_fd, struct pcfs_pack_build_result_v2 *result) {
    if (!result) return PCFS_INVALID;
    memset(result, 0, sizeof(*result));
    if (!o || o->struct_size != sizeof(*o) || !verified || (!indices && count)) return PCFS_INVALID;
    if (count > PCFS_EROFS_MAX_OBJECTS || count > o->max_objects ||
        count > SIZE_MAX / sizeof(struct pcfs_pack_entry_v1)) return PCFS_LIMIT;
    size_t total = 0;
    const struct pcfs_pack_entry_v1 *all = pcfs_verified_entries(verified, &total);
    for (size_t i = 0; i < count; ++i) if (indices[i] >= total) return PCFS_INVALID;
    struct pcfs_pack_entry_v1 *selected = count ? malloc(count * sizeof(*selected)) : NULL;
    if (count && !selected) return PCFS_IO;
    for (size_t i = 0; i < count; ++i) selected[i] = all[indices[i]];
    int32_t rc = build_verified_entries(o, verified, selected, count, output_fd, result);
    free(selected); return rc;
}

const char *pcfs_filesystem_fingerprint_v1(void) {
    return "polar-standalone-filesystem-v1;explicit-paths;sorted;uid-gid;mtime=0;source-gate-v1";
}
static int32_t build_filesystem_impl(const struct pcfs_filesystem_options_v1 *options,
    const struct pcfs_sources_v1 *sources, const struct pcfs_filesystem_entry_v1 *entries,
    size_t count, int32_t output_fd, struct pcfs_filesystem_result_v1 *result,
    bool audit_payload) {
    if (!result) return PCFS_INVALID;
    memset(result, 0, sizeof(*result));
    struct pcfs_filesystem_plan plan;
    int32_t rc = pcfs_filesystem_plan(options, entries, count, &plan);
    if (rc) return rc;
    struct pcfs_pack_options_v1 o = { PCFS_ABI_V1, sizeof(o), PCFS_EROFS_PROFILE_V1,
        options->compression, options->max_output_bytes, options->max_input_bytes, options->max_entries };
    struct pcfs_pack_result_v1 built = {0};
    rc = build_impl(&o, sources, NULL, 0, output_fd, &built, NULL, &plan, audit_payload);
    if (!rc) {
        memcpy(result->sha256, built.sha256, 32);
        result->size_bytes = built.size_bytes;
        result->entries = plan.count;
        result->files = plan.files;
        result->input_bytes = built.input_bytes;
    }
    pcfs_filesystem_plan_destroy(&plan);
    return rc;
}
int32_t pcfs_erofs_build_tree_v1(const struct pcfs_filesystem_options_v1 *options,
    const struct pcfs_sources_v1 *sources, const struct pcfs_filesystem_entry_v1 *entries,
    size_t count, int32_t output_fd, struct pcfs_filesystem_result_v1 *result) {
    return build_filesystem_impl(options, sources, entries, count, output_fd, result, true);
}
int32_t pcfs_erofs_build_tree_verified_v1(const struct pcfs_filesystem_options_v1 *options,
    struct pcfs_verified_sources *verified, const struct pcfs_filesystem_entry_v1 *entries,
    size_t count, int32_t output_fd, struct pcfs_filesystem_result_v1 *result) {
    if (!result) return PCFS_INVALID;
    memset(result, 0, sizeof(*result));
    if (!verified || (!entries && count)) return PCFS_INVALID;
    /* Validate budgets BEFORE looking at attacker-supplied entry counts. */
    struct pcfs_filesystem_plan plan;
    int32_t rc = pcfs_filesystem_plan(options, entries, count, &plan);
    if (rc) return rc;
    size_t total = 0;
    const struct pcfs_pack_entry_v1 *known = pcfs_verified_entries(verified, &total);
    for (size_t i = 0; i < count; ++i) if (entries[i].kind == PCFS_META_FILE) {
        uint64_t token = entries[i].source_token;
        if (token >= total || entries[i].size_bytes != known[token].size_bytes ||
            memcmp(entries[i].sha256, known[token].sha256, 32)) {
            pcfs_filesystem_plan_destroy(&plan); return PCFS_CORRUPT;
        }
    }
    pcfs_filesystem_plan_destroy(&plan);
    struct pcfs_sources_v1 provider;
    rc = pcfs_verified_begin(verified, &provider);
    if (rc) return rc;
    rc = build_filesystem_impl(options, &provider, entries, count, output_fd, result, false);
    int32_t finish = pcfs_verified_end(verified);
    if (!rc) rc = finish;
    if (rc) memset(result, 0, sizeof(*result));
    return rc;
}

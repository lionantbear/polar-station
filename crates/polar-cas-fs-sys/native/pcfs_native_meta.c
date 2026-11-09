#define _GNU_SOURCE
#define _FILE_OFFSET_BITS 64
#include "pcfs_native_meta.h"
#include "pcfs_meta_plan.h"
#include "pcfs_erofs_job.h"
#include "erofs/importer.h"
#include "erofs/inode.h"
#include "erofs/xattr.h"
#include <errno.h>
#include <limits.h>
#include <stdlib.h>
#include <string.h>

struct meta_graph {
    struct erofs_inode **nodes; /* root + one per explicit entry */
    size_t count;
};
static void graph_destroy(struct meta_graph *g) {
    /* Upstream may leave traversal refs after cancellation. All buffer managers
     * are already aborted, so these job-private nodes have no external users. */
    for (size_t i = 0; i < g->count; ++i) {
        struct erofs_inode *n = g->nodes[i];
        free(n->idata); n->idata = NULL;
        erofs_atomic_set(&n->i_count, 1);
        erofs_iput(n);
    }
    free(g->nodes);
}
static int32_t validate_packs(struct pcfs_image *const *packs, size_t count) {
    if ((!packs && count) || count > PCFS_META_MAX_PACKS) return PCFS_INVALID;
    for (size_t i = 0; i < count; ++i) {
        int32_t rc = pcfs_check_native_pack(packs[i]);
        if (rc) return rc;
        for (size_t j = 0; j < i; ++j)
            if (!memcmp(packs[i]->digest, packs[j]->digest, 32)) return PCFS_INVALID;
    }
    return PCFS_OK;
}
static int device_table(struct erofs_sb_info *sb, struct pcfs_image *const *packs, size_t count) {
    int err = erofs_mkfs_init_devices(sb, (unsigned int)count);
    if (err) return err;
    static const char hex[] = "0123456789abcdef";
    for (size_t i = 0; i < count; ++i) {
        sb->devs[i].blocks = packs[i]->size / 4096;
        /* Standard 64-byte device tag binds the exact ContentRef to this slot.
         * EROFS's kernel does not authenticate tags; the host adapter must. */
        for (size_t j = 0; j < 32; ++j) {
            sb->devs[i].tag[2*j] = (uint8_t)hex[packs[i]->digest[j] >> 4];
            sb->devs[i].tag[2*j+1] = (uint8_t)hex[packs[i]->digest[j] & 15];
        }
        /* bmgr stays NULL: existing external devices are NEVER written. */
    }
    return 0;
}
static int32_t load_file(struct erofs_sb_info *dst, struct pcfs_image *pack,
    struct erofs_inode *inode, const struct pcfs_meta_entry_v1 *e,
    int32_t (*cancel)(void *), void *ctx, bool audit_payload) {
    struct pcfs_member *member = NULL;
    int32_t rc = pcfs_image_member(pack, e->sha256, e->size_bytes, &member);
    if (!rc) rc = pcfs_check_external_inode(pack, &member->inode);
    if (!rc && audit_payload) rc = pcfs_verify_member_cancellable(member, cancel, ctx);
    if (!rc) {
        inode->sbi = &pack->sbi;
        inode->nid = member->inode.nid;
        rc = pcfs_from_errno(erofs_read_inode_from_disk(inode));
    }
    if (!rc) {
        /* Physical mapping exists only within the adapter and output EROFS.
         * Source image's global dev field is NEVER changed. */
        inode->dev = e->pack_index + 1;
        rc = pcfs_from_errno(pcfs_erofs_rebuild_file(dst, inode));
    }
    pcfs_member_destroy(member);
    inode->sbi = dst;
    inode->nid = EROFS_NID_UNALLOCATED;
    return rc;
}
static int32_t redirect_inode(struct erofs_inode *n, const struct pcfs_meta_entry_v1 *e) {
    char target[PCFS_SHA256_PATH_CAPACITY + 1]; target[0] = '/';
    int32_t rc = pcfs_sha256_path(e->sha256, target + 1, sizeof(target) - 1);
    if (rc) return rc;
    if (!e->size_bytes) {
        uint8_t empty[32]; erofs_sha256((const uint8_t *)"", 0, empty);
        if (memcmp(empty, e->sha256, 32)) return PCFS_CORRUPT;
    }
    n->i_size = e->size_bytes;
    n->datalayout = EROFS_INODE_FLAT_PLAIN;
    n->u.i_blkaddr = EROFS_NULL_ADDR; /* no payload; never read as content */
    int err = erofs_vfs_setxattr(n, "trusted.overlay.metacopy", "", 0);
    if (!err) err = erofs_vfs_setxattr(n, "trusted.overlay.redirect", target, strlen(target));
    return pcfs_from_errno(err);
}
static int32_t build_graph(struct erofs_importer *im, struct meta_graph *g,
    const struct pcfs_meta_options_v1 *o, struct pcfs_image *const *packs,
    const struct pcfs_meta_planned *plan, size_t count, struct pcfs_native_job *job,
    int32_t (*cancel)(void *), void *ctx, bool audit_payload) {
    struct erofs_inode *root = erofs_make_empty_root_inode(im, im->sbi);
    if (IS_ERR(root)) return pcfs_from_errno(PTR_ERR(root));
    im->root = root;
    root->i_mode = S_IFDIR | o->root_mode;
    g->nodes[g->count++] = erofs_igrab(root);
    for (size_t i = 0; i < count; ++i) {
        if (pcfs_job_cancelled(job)) return pcfs_job_status(job);
        const struct pcfs_meta_entry_v1 *e = plan[i].entry;
        struct erofs_inode *parent = plan[i].parent == SIZE_MAX ? root : g->nodes[plan[i].parent + 1];
        struct erofs_inode *n = erofs_new_inode(im->sbi);
        if (IS_ERR(n)) return pcfs_from_errno(PTR_ERR(n));
        g->nodes[g->count++] = erofs_igrab(n);
        int32_t rc = PCFS_OK;
        if (e->kind == PCFS_META_FILE) rc = load_file(im->sbi, packs[e->pack_index], n, e, cancel, ctx, audit_payload);
        else if (e->kind == PCFS_META_REDIRECT) rc = redirect_inode(n, e);
        if (rc) return rc;
        n->i_srcpath = strdup(e->path);
        if (!n->i_srcpath) return PCFS_IO;
        n->i_parent = parent;
        n->i_uid = o->uid; n->i_gid = o->gid;
        n->i_mtime = 0; n->i_mtime_nsec = 0;
        n->i_nlink = e->kind == PCFS_META_DIRECTORY ? 2 : 1;
        n->i_mode = (e->kind == PCFS_META_DIRECTORY ? S_IFDIR :
                      (e->kind == PCFS_META_FILE || e->kind == PCFS_META_REDIRECT) ? S_IFREG : S_IFLNK) | e->mode;
        /* Do not derive hardlinks from byte identity: executable and ordinary
         * files may deliberately share payload but have distinct metadata. */
        n->i_ino[0] = 0;
        n->i_ino[1] = i + 2;
        if (e->kind == PCFS_META_SYMLINK) {
            n->i_link = strdup(e->symlink_target);
            if (!n->i_link) return PCFS_IO;
            n->i_size = strlen(e->symlink_target);
        }
        const char *name = strrchr(e->path, '/');
        name = name ? name + 1 : e->path;
        struct erofs_dentry *d = erofs_d_alloc(parent, name);
        if (IS_ERR(d)) return pcfs_from_errno(PTR_ERR(d));
        d->inode = n; d->type = erofs_mode_to_ftype(n->i_mode);
    }
    return PCFS_OK;
}
static int32_t verify_graph(struct pcfs_image *image, const struct pcfs_meta_options_v1 *o,
    const struct pcfs_meta_planned *plan, size_t count,
    int32_t (*cancel)(void *), void *ctx, uint64_t *files, bool audit_payload) {
    struct erofs_inode root = { .sbi = &image->sbi, .nid = image->sbi.root_nid };
    int32_t rc = pcfs_from_errno(erofs_read_inode_from_disk(&root));
    if (rc) return rc;
    if (!S_ISDIR(root.i_mode) || (root.i_mode & 07777) != o->root_mode ||
        root.i_uid != o->uid || root.i_gid != o->gid) return PCFS_CORRUPT;
    for (size_t i = 0; i < count; ++i) {
        if (cancel && cancel(ctx)) return PCFS_CANCELLED;
        const struct pcfs_meta_entry_v1 *e = plan[i].entry;
        struct pcfs_meta_stat_v1 st;
        rc = pcfs_native_meta_stat(image, e->path, &st);
        if (rc) return rc;
        if (st.kind != e->kind || st.mode != e->mode || st.uid != o->uid || st.gid != o->gid)
            return PCFS_CORRUPT;
        if (e->kind == PCFS_META_FILE) {
            struct pcfs_member *m = NULL;
            rc = pcfs_native_meta_member(image, e->path, e->sha256, e->size_bytes, &m);
            if (!rc && audit_payload) rc = pcfs_verify_member_cancellable(m, cancel, ctx);
            pcfs_member_destroy(m);
            if (rc) return rc;
            ++*files;
        } else if (e->kind == PCFS_META_REDIRECT) {
            uint8_t digest[32]; uint64_t size = 0;
            rc = pcfs_hybrid_meta_redirect(image, e->path, digest, &size);
            if (rc) return rc;
            if (size != e->size_bytes || memcmp(digest, e->sha256, 32)) return PCFS_CORRUPT;
            ++*files;
        } else if (e->kind == PCFS_META_SYMLINK) {
            char link[PCFS_META_PATH_MAX + 1u];
            size_t len = 0;
            rc = pcfs_native_meta_readlink(image, e->path, link, sizeof(link), &len);
            if (rc) return rc;
            if (len != strlen(e->symlink_target) || memcmp(link, e->symlink_target, len)) return PCFS_CORRUPT;
        }
    }
    return pcfs_image_unchanged(image);
}
static int32_t meta_build(const struct pcfs_meta_options_v1 *o,
    struct pcfs_image *const *packs, size_t npacks, const struct pcfs_meta_entry_v1 *entries,
    size_t count, int32_t fd, int32_t (*cancel)(void *), void *ctx, struct pcfs_meta_result_v1 *result, bool audit_payload) {
    struct pcfs_meta_planned *plan = NULL;
    struct pcfs_native_job *job = NULL;
    struct pcfs_meta_result_v1 completed = {0};
    struct erofs_sb_info sb = {0};
    struct erofs_importer_params params;
    struct erofs_importer im = { .sbi = &sb, .params = &params };
    struct z_erofs_paramset compressors[2] = {{0}, {0}};
    struct meta_graph graph = {0};
    struct pcfs_image *image = NULL;
    bool locked = false, entered = false, importer_started = false;
    int32_t rc;
    int err;
    u32 crc;
    if (!result) return PCFS_INVALID;
    memset(result, 0, sizeof(*result));
    if (cancel && cancel(ctx)) return PCFS_CANCELLED;
    rc = pcfs_meta_plan(o, entries, count, npacks, &plan);
    if (rc) return rc;
    rc = pcfs_native_lock(); if (rc) goto done;
    locked = true;
    rc = validate_packs(packs, npacks); if (rc) goto done;
    uint64_t address_blocks = (o->max_output_bytes + 4095) / 4096;
    for (size_t i = 0; i < npacks; ++i) {
        uint64_t blocks = packs[i]->size / 4096;
        if (address_blocks > UINT32_MAX || blocks > UINT32_MAX - address_blocks) {
            rc = PCFS_LIMIT; goto done;
        }
        address_blocks += blocks;
    }
    rc = pcfs_native_build_enter(); if (rc) goto done;
    entered = true;
    rc = pcfs_job_new(fd, o->max_output_bytes, cancel, ctx, &job);
    if (rc) goto done;
    graph.nodes = calloc(count + 1, sizeof(*graph.nodes));
    if (!graph.nodes) { rc = PCFS_IO; goto done; }
    pcfs_job_attach(job, &sb.bdev);
    erofs_importer_preset(&params);
    params.fixed_uid = o->uid; params.fixed_gid = o->gid; params.build_time = 0;
    params.ignore_mtime = true; params.no_xattrs = true; params.no_datainline = true;
    params.pclusterblks_def = 1; params.pclusterblks_max = 1;
    params.dedupe = EROFS_DEDUPE_FORCE_OFF;
    params.z_paramsets = compressors;
    for (size_t i = 0; i < npacks; ++i) if (packs[i]->sbi.available_compr_algs) {
        compressors[0].alg = "lz4"; compressors[0].clevel = -1;
        erofs_sb_set_lz4_0padding(&sb);
        break;
    }
    err = erofs_mkfs_format_fs(&sb, 12, 0, false);
    if (!err) err = device_table(&sb, packs, npacks);
    if (err) { rc = pcfs_from_errno(err); goto done; }
    importer_started = true;
    err = erofs_importer_init(&im);
    if (err) { rc = pcfs_from_errno(err); goto done; }
    rc = build_graph(&im, &graph, o, packs, plan, count, job, cancel, ctx, audit_payload);
    if (rc) goto done;
    err = erofs_importer_load_tree(&im, true, false);
    if (!err) err = erofs_importer_flush_all(&im);
    if (!err) err = erofs_writesb(&sb);
    if (!err) err = erofs_flush_all_devices(&sb);
    if (!err) err = erofs_enable_sb_chksum(&sb, &crc);
    if (err) { rc = pcfs_job_status(job) ? pcfs_job_status(job) : pcfs_from_errno(err); goto done; }
    rc = pcfs_job_finish(job, &completed.size_bytes, completed.sha256);
    if (rc) goto done;
    rc = pcfs_open_verified_image_devices(pcfs_job_fd(job), completed.size_bytes, packs, npacks, &image);
    if (rc) goto done;
    image->projection = true;
    image->hybrid = o->profile == PCFS_HYBRID_META_PROFILE_V2;
    memcpy(image->digest, completed.sha256, 32);
    rc = verify_graph(image, o, plan, count, cancel, ctx, &completed.files, audit_payload);
    if (rc) goto done;
    completed.entries = count; completed.dependencies = npacks;
    *result = completed;
done:
    pcfs_image_destroy(image);
    if (importer_started) erofs_importer_exit(&im);
    if (entered) {
        erofs_put_super(&sb);
        graph_destroy(&graph);
        pcfs_native_build_leave();
    }
    if (locked) pcfs_native_unlock();
    pcfs_job_destroy(job);
    free(plan);
    if (rc) memset(result, 0, sizeof(*result));
    return rc;
}
const char *pcfs_native_meta_fingerprint(void) {
    return "erofs-utils@1c230a485f25837cb51d6fc79332f30758d9006c+polar-native-meta-v1.1;4k;raw-lz4;noinline;ordered-sha256-devices";
}

const char *pcfs_hybrid_meta_fingerprint(void) {
    return "erofs-utils@1c230a485f25837cb51d6fc79332f30758d9006c+polar-hybrid-v2;native-v1;overlay-metacopy-empty;sha256-path-v1";
}

/* Trusted caller MUST have committed verified membership evidence. An image
 * hash alone is not that evidence. Default/import API keeps full audit. */
int32_t pcfs_meta_build_from_trusted_members(const struct pcfs_meta_options_v1 *o,
    struct pcfs_image *const *packs, size_t npacks, const struct pcfs_meta_entry_v1 *entries,
    size_t count, int32_t fd, int32_t (*cancel)(void *), void *ctx, struct pcfs_meta_result_v1 *result) {
    return meta_build(o, packs, npacks, entries, count, fd, cancel, ctx, result, false);
}
int32_t pcfs_native_meta_build(const struct pcfs_meta_options_v1 *o,
    struct pcfs_image *const *packs, size_t npacks, const struct pcfs_meta_entry_v1 *entries,
    size_t count, int32_t fd, int32_t (*cancel)(void *), void *ctx, struct pcfs_meta_result_v1 *result) {
    return meta_build(o, packs, npacks, entries, count, fd, cancel, ctx, result, true);
}

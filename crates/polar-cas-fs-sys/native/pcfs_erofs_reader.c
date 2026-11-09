#define _GNU_SOURCE
#define _FILE_OFFSET_BITS 64
#include "pcfs_erofs_private.h"
#include "pcfs_verified_sources.h"
#include "pcfs_native_meta.h"
#include "pcfs_meta_plan.h"
#include "erofs/config.h"
#include "erofs/dir.h"
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <pthread.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#ifdef EROFS_MT_ENABLED
#error "Polar vfile v1 requires --disable-multithreading upstream build"
#endif
#ifdef __USE_OPENSSL_SHA256
#error "Polar vfile v1 requires --without-openssl (internal SHA256 state)"
#endif

static pthread_once_t init_once = PTHREAD_ONCE_INIT;
static pthread_mutex_t engine_lock;
static int init_error;
static _Thread_local bool building;
static struct {
    _Atomic uint64_t whole_image_hash_calls;
    _Atomic uint64_t whole_image_hash_bytes;
    _Atomic uint64_t image_open_hash_calls;
    _Atomic uint64_t image_open_hash_bytes;
    _Atomic uint64_t member_verify_calls;
    _Atomic uint64_t member_verify_bytes;
    _Atomic uint64_t member_read_calls;
    _Atomic uint64_t member_read_bytes;
    _Atomic uint64_t image_pread_calls;
    _Atomic uint64_t image_pread_bytes;
} io_counters;
static void init_engine(void) {
    pthread_mutexattr_t attr;
    init_error = pthread_mutexattr_init(&attr);
    if (init_error) return;
    init_error = pthread_mutexattr_settype(&attr, PTHREAD_MUTEX_RECURSIVE);
    if (!init_error) init_error = pthread_mutex_init(&engine_lock, &attr);
    pthread_mutexattr_destroy(&attr);
    if (init_error) return;
    init_error = liberofs_global_init();
    /* No per-job global_exit: other image handles can still be alive. */
    cfg.c_showprogress = false;
    cfg.c_dbg_lvl = 0;
}
static int32_t native_init(void) {
    return pthread_once(&init_once, init_engine) || init_error ? PCFS_IO : PCFS_OK;
}
int32_t pcfs_native_lock(void) {
    if (native_init()) return PCFS_IO;
    return pthread_mutex_lock(&engine_lock) ? PCFS_IO : PCFS_OK;
}
void pcfs_native_unlock(void) { (void)pthread_mutex_unlock(&engine_lock); }
int32_t pcfs_io_counters_read(struct pcfs_io_counters_v1 *out) {
    if (!out) return PCFS_INVALID;
    out->whole_image_hash_calls = atomic_load_explicit(&io_counters.whole_image_hash_calls, memory_order_relaxed);
    out->whole_image_hash_bytes = atomic_load_explicit(&io_counters.whole_image_hash_bytes, memory_order_relaxed);
    out->image_open_hash_calls = atomic_load_explicit(&io_counters.image_open_hash_calls, memory_order_relaxed);
    out->image_open_hash_bytes = atomic_load_explicit(&io_counters.image_open_hash_bytes, memory_order_relaxed);
    out->member_verify_calls = atomic_load_explicit(&io_counters.member_verify_calls, memory_order_relaxed);
    out->member_verify_bytes = atomic_load_explicit(&io_counters.member_verify_bytes, memory_order_relaxed);
    out->member_read_calls = atomic_load_explicit(&io_counters.member_read_calls, memory_order_relaxed);
    out->member_read_bytes = atomic_load_explicit(&io_counters.member_read_bytes, memory_order_relaxed);
    out->image_pread_calls = atomic_load_explicit(&io_counters.image_pread_calls, memory_order_relaxed);
    out->image_pread_bytes = atomic_load_explicit(&io_counters.image_pread_bytes, memory_order_relaxed);
    return PCFS_OK;
}
/* Existing liberofs vfile hook: count actual main-device reads without changing
 * upstream or adding a second parser. Counters do not serialize independent I/O. */
static ssize_t image_pread(struct erofs_vfile *vf, void *buf, size_t len, u64 pos) {
    struct pcfs_image *im = (void *)((char *)vf - offsetof(struct pcfs_image, sbi.bdev));
    if (pos > im->size || len > im->size - pos) return -EIO;
    size_t done = 0;
    while (done < len) {
        ssize_t n = pread(vf->fd, (char *)buf + done, len - done, (off_t)(pos + done));
        atomic_fetch_add_explicit(&io_counters.image_pread_calls, 1, memory_order_relaxed);
        if (n < 0) { if (errno == EINTR) continue; return -errno; }
        if (!n) break;
        atomic_fetch_add_explicit(&io_counters.image_pread_bytes, (uint64_t)n, memory_order_relaxed);
        done += (size_t)n;
    }
    return (ssize_t)done;
}
static int image_fstat(struct erofs_vfile *vf, struct stat *st) { return fstat(vf->fd, st); }
static void image_close(struct erofs_vfile *vf) { (void)vf; /* pcfs_image owns fd */ }
static struct erofs_vfops image_read_ops = {
    .pread = image_pread, .fstat = image_fstat, .close = image_close,
};
int32_t pcfs_native_build_enter(void) {
    if (building) return PCFS_INVALID;
    building = true; return PCFS_OK;
}
void pcfs_native_build_leave(void) { building = false; }
int32_t pcfs_from_errno(int error) {
    if (!error) return PCFS_OK;
    if (error > 0) error = -error;
    switch (-error) {
    case EINVAL: return PCFS_INVALID;
    case EOPNOTSUPP: case ENOSYS: return PCFS_UNSUPPORTED;
    case EFBIG: case ENOSPC: case EOVERFLOW: return PCFS_LIMIT;
    case ECANCELED: return PCFS_CANCELLED;
    case ENOENT: return PCFS_NOT_FOUND;
    case ENOTDIR: case EBADMSG: case EUCLEAN: return PCFS_CORRUPT;
    default: return PCFS_IO;
    }
}
bool pcfs_same_stat(const struct stat *a, const struct stat *b) {
    return a->st_dev == b->st_dev && a->st_ino == b->st_ino &&
        a->st_size == b->st_size && a->st_mtim.tv_sec == b->st_mtim.tv_sec &&
        a->st_mtim.tv_nsec == b->st_mtim.tv_nsec &&
        a->st_ctim.tv_sec == b->st_ctim.tv_sec && a->st_ctim.tv_nsec == b->st_ctim.tv_nsec;
}
static int32_t hash_fd_counted(int fd, uint64_t size, uint8_t digest[32],
    int32_t (*cancel)(void *), void *context, uint64_t *read_bytes) {
    uint8_t buffer[65536];
    struct sha256_state hash;
    uint64_t pos = 0;
    atomic_fetch_add_explicit(&io_counters.whole_image_hash_calls, 1, memory_order_relaxed);
    erofs_sha256_init(&hash);
    while (pos < size) {
        if (cancel && cancel(context)) return PCFS_CANCELLED;
        size_t n = sizeof(buffer);
        ssize_t got;
        if ((uint64_t)n > size - pos) n = (size_t)(size - pos);
        do { got = pread(fd, buffer, n, (off_t)pos); } while (got < 0 && errno == EINTR);
        if (got <= 0) return PCFS_IO;
        atomic_fetch_add_explicit(&io_counters.whole_image_hash_bytes, (uint64_t)got, memory_order_relaxed);
        if (read_bytes) *read_bytes += (uint64_t)got;
        if (erofs_sha256_process(&hash, buffer, (unsigned long)got)) return PCFS_IO;
        pos += (uint64_t)got;
    }
    return erofs_sha256_done(&hash, digest) ? PCFS_IO : PCFS_OK;
}
int32_t pcfs_hash_fd_cancellable(int fd, uint64_t size, uint8_t digest[32],
    int32_t (*cancel)(void *), void *context) {
    return hash_fd_counted(fd, size, digest, cancel, context, NULL);
}
int32_t pcfs_hash_fd(int fd, uint64_t size, uint8_t digest[32]) {
    return pcfs_hash_fd_cancellable(fd, size, digest, NULL, NULL);
}
int32_t pcfs_image_unchanged(struct pcfs_image *im) {
    struct stat now;
    if (fstat(im->fd, &now)) return PCFS_IO;
    if (!pcfs_same_stat(&im->initial, &now)) return PCFS_CORRUPT;
    for (size_t i = 0; i < im->backing_count; ++i) {
        int32_t rc = pcfs_image_unchanged(im->backings[i]);
        if (rc) return rc;
    }
    return PCFS_OK;
}
static int32_t retain_image(struct pcfs_image *im) {
    size_t refs = atomic_load_explicit(&im->refs, memory_order_relaxed);
    do {
        if (refs == SIZE_MAX) return PCFS_LIMIT;
    } while (!atomic_compare_exchange_weak_explicit(&im->refs, &refs, refs + 1,
                 memory_order_relaxed, memory_order_relaxed));
    return PCFS_OK;
}
static void release_image(struct pcfs_image *im) {
    if (!im || atomic_fetch_sub_explicit(&im->refs, 1, memory_order_acq_rel) != 1) return;
    erofs_put_super(&im->sbi);
    close(im->fd);
    for (size_t i = 0; i < im->backing_count; ++i) release_image(im->backings[i]);
    free(im->backings);
    pthread_mutex_destroy(&im->lookup_lock);
    free(im);
}
int32_t pcfs_open_verified_image_devices(int fd, uint64_t size,
    struct pcfs_image *const *packs, size_t count, struct pcfs_image **out) {
    if ((!packs && count) || count > PCFS_META_MAX_PACKS) return PCFS_INVALID;
    struct pcfs_image *im = calloc(1, sizeof(*im));
    int err;
    if (!im) return PCFS_IO;
    if (pthread_mutex_init(&im->lookup_lock, NULL)) { free(im); return PCFS_IO; }
    im->fd = fcntl(fd, F_DUPFD_CLOEXEC, 3);
    if (im->fd < 0) { pthread_mutex_destroy(&im->lookup_lock); free(im); return PCFS_IO; }
    atomic_init(&im->refs, 1); im->size = size;
    im->sbi.bdev.fd = im->fd;
    im->sbi.bdev.ops = &image_read_ops;
    im->sbi.devsz = size;
    if (fstat(im->fd, &im->initial)) { release_image(im); return PCFS_IO; }
    err = erofs_read_superblock(&im->sbi);
    if (err) { release_image(im); return pcfs_from_errno(err); }
    /* v1 is a self-contained 4KiB raw/LZ4 pack. Do not silently resolve external
     * devices, fragments, 48-bit layouts or compressed metadata on this path. */
    if (im->sbi.blkszbits != 12 || im->sbi.extra_devices != count ||
        erofs_sb_has_48bit(&im->sbi) || erofs_sb_has_metabox(&im->sbi) ||
        erofs_sb_has_fragments(&im->sbi) ||
        (im->sbi.available_compr_algs & ~(1u << Z_EROFS_COMPRESSION_LZ4))) {
        release_image(im); return PCFS_UNSUPPORTED;
    }
    if (erofs_sb_has_sb_chksum(&im->sbi)) {
        err = erofs_superblock_csum_verify(&im->sbi);
        if (err) { release_image(im); return PCFS_CORRUPT; }
    }
    if (im->sbi.dif0.blocks > UINT64_MAX / 4096 || im->sbi.dif0.blocks * 4096 != size) {
        release_image(im); return PCFS_CORRUPT;
    }
    im->projection = count != 0;
    if (count) {
        im->backings = calloc(count, sizeof(*im->backings));
        if (!im->backings) { release_image(im); return PCFS_IO; }
    }
    for (size_t i = 0; i < count; ++i) {
        if (!packs[i]) { release_image(im); return PCFS_INVALID; }
        int32_t rc = pcfs_check_native_pack(packs[i]);
        if (rc) { release_image(im); return rc; }
        for (size_t j = 0; j < i; ++j) if (!memcmp(packs[i]->digest, packs[j]->digest, 32)) {
            release_image(im); return PCFS_INVALID;
        }
        static const char hex[] = "0123456789abcdef";
        char tag[64];
        for (size_t j = 0; j < 32; ++j) {
            tag[j*2] = hex[packs[i]->digest[j] >> 4];
            tag[j*2+1] = hex[packs[i]->digest[j] & 15];
        }
        if (memcmp(im->sbi.devs[i].tag, tag, sizeof(tag)) ||
            im->sbi.devs[i].blocks != packs[i]->size / 4096) {
            release_image(im); return PCFS_CORRUPT;
        }
        if (retain_image(packs[i])) { release_image(im); return PCFS_LIMIT; }
        im->backings[im->backing_count++] = packs[i];
        /* Borrow the FD under the retained image ref; never blob_closeall(). */
        im->sbi.blobfd[i] = (unsigned int)packs[i]->fd;
    }
    im->sbi.nblobs = (unsigned int)count;
    struct erofs_inode root = { .sbi = &im->sbi, .nid = im->sbi.root_nid };
    err = erofs_read_inode_from_disk(&root);
    if (err || !S_ISDIR(root.i_mode)) { release_image(im); return PCFS_CORRUPT; }
    *out = im; return PCFS_OK;
}
int32_t pcfs_open_verified_image(int fd, uint64_t size, struct pcfs_image **out) {
    return pcfs_open_verified_image_devices(fd, size, NULL, 0, out);
}
static int32_t open_image(int32_t fd, const uint8_t expected[32], uint64_t size,
    struct pcfs_image *const *packs, size_t count, struct pcfs_image **out, bool hash_image) {
    struct stat initial, after;
    uint8_t digest[32];
    int flags, owned = -1;
    int32_t rc;
    if (!out) return PCFS_INVALID;
    *out = NULL;
    if (!expected || fd < 0 || size < 4096 || size > INT64_MAX) return PCFS_INVALID;
    flags = fcntl(fd, F_GETFL);
    if (flags < 0 || fstat(fd, &initial)) return PCFS_IO;
    if ((flags & O_ACCMODE) != O_RDONLY || !S_ISREG(initial.st_mode) ||
        initial.st_size < 0 || (uint64_t)initial.st_size != size) return PCFS_INVALID;
    owned = fcntl(fd, F_DUPFD_CLOEXEC, 3);
    if (owned < 0) return PCFS_IO;
    rc = native_init();
    if (rc != PCFS_OK) { close(owned); return rc; }
    if (hash_image) {
        uint64_t read_bytes = 0;
        atomic_fetch_add_explicit(&io_counters.image_open_hash_calls, 1, memory_order_relaxed);
        rc = hash_fd_counted(owned, size, digest, NULL, NULL, &read_bytes);
        atomic_fetch_add_explicit(&io_counters.image_open_hash_bytes, read_bytes, memory_order_relaxed);
        if (!rc && memcmp(expected, digest, 32)) rc = PCFS_CORRUPT;
    }
    if (!rc && (fstat(owned, &after) || !pcfs_same_stat(&initial, &after))) rc = PCFS_CORRUPT;
    if (!rc) rc = pcfs_open_verified_image_devices(owned, size, packs, count, out);
    if (!rc) rc = pcfs_image_unchanged(*out);
    if (rc && *out) { release_image(*out); *out = NULL; }
    if (!rc) memcpy((*out)->digest, expected, 32);
    close(owned); return rc;
}
int32_t pcfs_image_open(int32_t fd, const uint8_t expected[32], uint64_t size,
                        struct pcfs_image **out) {
    return open_image(fd, expected, size, NULL, 0, out, true);
}
int32_t pcfs_image_open_managed(int32_t fd, const uint8_t expected[32], uint64_t size,
    struct pcfs_image **out) {
    return open_image(fd, expected, size, NULL, 0, out, false);
}
int32_t pcfs_native_meta_open(int32_t fd, const uint8_t expected[32], uint64_t size,
    struct pcfs_image *const *packs, size_t count, struct pcfs_image **out) {
    int32_t rc = open_image(fd, expected, size, packs, count, out, true);
    if (!rc) (*out)->projection = true;
    return rc;
}
int32_t pcfs_filesystem_open_managed(int32_t fd, const uint8_t expected[32],
    uint64_t size, struct pcfs_image **out) {
    int32_t rc = open_image(fd, expected, size, NULL, 0, out, false);
    if (!rc) (*out)->projection = true;
    return rc;
}
int32_t pcfs_hybrid_meta_open(int32_t fd, const uint8_t expected[32], uint64_t size,
    struct pcfs_image *const *packs, size_t count, struct pcfs_image **out) {
    int32_t rc = pcfs_native_meta_open(fd, expected, size, packs, count, out);
    if (!rc) (*out)->hybrid = true;
    return rc;
}
int32_t pcfs_hybrid_meta_open_managed(int32_t fd, const uint8_t expected[32], uint64_t size,
    struct pcfs_image *const *packs, size_t count, struct pcfs_image **out) {
    int32_t rc = open_image(fd, expected, size, packs, count, out, false);
    if (!rc) { (*out)->projection = true; (*out)->hybrid = true; }
    return rc;
}
void pcfs_image_destroy(struct pcfs_image *im) {
    release_image(im);
}
static struct pcfs_member *new_member(void) {
    struct pcfs_member *m = calloc(1, sizeof(*m));
    if (m && pthread_mutex_init(&m->inode_lock, NULL)) { free(m); return NULL; }
    return m;
}
static void free_member(struct pcfs_member *m) {
    if (!m) return;
    pthread_mutex_destroy(&m->inode_lock);
    free(m);
}
/* Upstream ilookup doesn't follow links, but also doesn't validate each parent
 * is a directory. Check prefixes before allowing an untrusted pathname. */
static int32_t lookup_relative(struct pcfs_image *im, const char *path, struct erofs_inode *inode) {
    int32_t rc = pcfs_meta_validate_path(path);
    if (rc) return rc;
    char prefix[PCFS_META_PATH_MAX + 1u];
    memcpy(prefix, path, strlen(path) + 1);
    for (char *p = prefix; *p; ++p) if (*p == '/') {
        struct erofs_inode parent = { .sbi = &im->sbi };
        *p = 0;
        rc = pcfs_from_errno(erofs_ilookup(prefix, &parent));
        *p = '/';
        if (rc) return rc;
        if (!S_ISDIR(parent.i_mode)) return PCFS_NOT_FOUND;
    }
    inode->sbi = &im->sbi;
    return pcfs_from_errno(erofs_ilookup(path, inode));
}
/* Complete xattr whitelist: no ACL/capability, extra marker or path escape.
 * Empty metacopy is the OverlayFS format without an optional verity digest.
 * Actual provider identity/verity is verified before Host activation. */
static int32_t parse_meta_inode_kind(struct pcfs_image *im, struct erofs_inode *n,
    uint32_t *kind, uint8_t digest[32]) {
    *kind = S_ISDIR(n->i_mode) ? PCFS_META_DIRECTORY :
        S_ISREG(n->i_mode) ? PCFS_META_FILE : S_ISLNK(n->i_mode) ? PCFS_META_SYMLINK : 0;
    if (!*kind || n->i_mtime || n->i_mtime_nsec) return PCFS_UNSUPPORTED;
    if (!n->xattr_isize) return PCFS_OK;
    if (!im->hybrid || !S_ISREG(n->i_mode)) return PCFS_UNSUPPORTED;
    char names[128]; int len = erofs_listxattr(n, names, sizeof(names));
    if (len < 0 || len > (int)sizeof(names)) return PCFS_CORRUPT;
    unsigned int found = 0;
    for (size_t off = 0; off < (size_t)len;) {
        size_t size = strnlen(names + off, (size_t)len - off);
        if (!size || size == (size_t)len - off) return PCFS_CORRUPT;
        unsigned int bit;
        if (!strcmp(names + off, "trusted.overlay.metacopy")) bit = 1;
        else if (!strcmp(names + off, "trusted.overlay.redirect")) bit = 2;
        else return PCFS_UNSUPPORTED;
        if (found & bit) return PCFS_CORRUPT;
        found |= bit; off += size + 1;
    }
    if (found != 3 || erofs_getxattr(n, "trusted.overlay.metacopy", names, sizeof(names)) != 0)
        return PCFS_CORRUPT;
    char target[PCFS_SHA256_PATH_CAPACITY + 1];
    len = erofs_getxattr(n, "trusted.overlay.redirect", target, sizeof(target));
    if (len != PCFS_SHA256_PATH_CAPACITY) return PCFS_CORRUPT;
    target[len] = 0;
    static const char prefix[] = "/objects/sha256/";
    if (memcmp(target, prefix, sizeof(prefix)-1) || target[18] != '/') return PCFS_CORRUPT;
    const char *hex = target + 19;
    if (target[16] != hex[0] || target[17] != hex[1]) return PCFS_CORRUPT;
    for (size_t i=0;i<32;++i) {
        uint8_t value=0;
        for (size_t j=0;j<2;++j) {
            char c=hex[2*i+j];
            if (!((c>='0'&&c<='9')||(c>='a'&&c<='f'))) return PCFS_CORRUPT;
            value=(uint8_t)((value<<4)|(c<='9'?c-'0':c-'a'+10));
        }
        digest[i]=value;
    }
    if (n->datalayout != EROFS_INODE_FLAT_PLAIN || n->u.i_blkaddr != EROFS_NULL_ADDR)
        return PCFS_CORRUPT;
    if (!n->i_size) {
        uint8_t empty[32]; erofs_sha256((const uint8_t *)"",0,empty);
        if (memcmp(digest,empty,32)) return PCFS_CORRUPT;
    }
    *kind=PCFS_META_REDIRECT; return PCFS_OK;
}
/* The upstream xattr reader lazily allocates a shared-index array, even for
 * zero shared entries. These inspection inodes are short-lived: release that
 * cache on every success/error path, not only when an Image is destroyed. */
static int32_t meta_inode_kind(struct pcfs_image *im, struct erofs_inode *n,
    uint32_t *kind, uint8_t digest[32]) {
    int32_t rc = parse_meta_inode_kind(im, n, kind, digest);
    free(n->xattr_shared_xattrs);
    n->xattr_shared_xattrs = NULL;
    n->xattr_shared_count = 0;
    erofs_atomic_set(&n->flags, erofs_atomic_read(&n->flags) & ~EROFS_I_EA_INITED);
    return rc;
}
int32_t pcfs_image_path_member(struct pcfs_image *im, const char *path,
    const uint8_t digest[32], uint64_t size, struct pcfs_member **out) {
    struct pcfs_member *m;
    int32_t rc;
    if (!out) return PCFS_INVALID;
    *out = NULL;
    if (!im || !digest || size > INT64_MAX) return PCFS_INVALID;
    rc = native_init(); if (rc) return rc;
    rc = pcfs_image_unchanged(im); if (rc) goto done;
    m = new_member();
    if (!m) { rc = PCFS_IO; goto done; }
    m->inode.sbi = &im->sbi;
    rc = lookup_relative(im, path, &m->inode);
    if (!rc && (!S_ISREG(m->inode.i_mode) || m->inode.i_size != size)) rc = PCFS_CORRUPT;
    if (!rc && im->projection) {
        uint32_t kind; uint8_t key[32]; rc = meta_inode_kind(im, &m->inode, &kind, key);
        if (!rc && kind == PCFS_META_REDIRECT) rc = PCFS_UNSUPPORTED;
    }
    if (rc) { free_member(m); goto done; }
    if (retain_image(im)) { free_member(m); rc = PCFS_LIMIT; goto done; }
    m->image = im;
    memcpy(m->digest, digest, 32); *out = m;
done:
     return rc;
}
int32_t pcfs_image_member(struct pcfs_image *im, const uint8_t digest[32],
                         uint64_t size, struct pcfs_member **out) {
    char path[PCFS_SHA256_PATH_CAPACITY];
    if (!out) return PCFS_INVALID;
    *out = NULL;
    if (!im || im->projection) return PCFS_INVALID;
    int32_t rc = pcfs_sha256_path(digest, path, sizeof(path));
    return rc ? rc : pcfs_image_path_member(im, path, digest, size, out);
}
int32_t pcfs_image_member_by_digest(struct pcfs_image *im, const uint8_t digest[32],
    uint64_t *size_out, struct pcfs_member **out) {
    if (out) *out = NULL;
    if (size_out) *size_out = 0;
    if (!out || !size_out || !im || !digest || im->projection) return PCFS_INVALID;
    char path[PCFS_SHA256_PATH_CAPACITY];
    int32_t rc = pcfs_sha256_path(digest, path, sizeof(path));
    if (rc) return rc;
    rc = native_init();
    if (rc) return rc;
    struct pcfs_member *m = NULL;
    rc = pcfs_image_unchanged(im);
    if (!rc) {
        m = new_member();
        if (!m) rc = PCFS_IO;
    }
    if (!rc) rc = lookup_relative(im, path, &m->inode);
    if (!rc && !S_ISREG(m->inode.i_mode)) rc = PCFS_CORRUPT;
    if (!rc && m->inode.i_size > INT64_MAX) rc = PCFS_LIMIT;
    if (!rc) rc = pcfs_image_unchanged(im);
    if (!rc) rc = retain_image(im);
    if (!rc) {
        m->image = im;
        memcpy(m->digest, digest, sizeof(m->digest));
        *size_out = m->inode.i_size;
        *out = m;
    } else free_member(m);

    return rc;
}
int32_t pcfs_native_meta_member(struct pcfs_image *im, const char *path,
    const uint8_t digest[32], uint64_t size, struct pcfs_member **out) {
    if (!out) return PCFS_INVALID;
    *out = NULL;
    if (!im || !im->projection) return PCFS_INVALID;
    return pcfs_image_path_member(im, path, digest, size, out);
}
/* Reuse upstream lookup and inode decoding. Only cache successful directory
 * type checks (one flag plus a 256-bit bitmap per image), never payload or another hash->inode index.
 * The existing image-stamp check precedes every group and every actual read. */
static int32_t lookup_cas_member(struct pcfs_image *im, const uint8_t digest[32],
    struct erofs_inode *inode) {
    struct erofs_inode parent = { .sbi = &im->sbi };
    int32_t rc;
    if (!im->cas_parents_checked) {
        rc = pcfs_from_errno(erofs_ilookup("objects", &parent));
        if (rc) return rc;
        if (!S_ISDIR(parent.i_mode)) return PCFS_NOT_FOUND;
        rc = pcfs_from_errno(erofs_ilookup("objects/sha256", &parent));
        if (rc) return rc;
        if (!S_ISDIR(parent.i_mode)) return PCFS_NOT_FOUND;
        im->cas_parents_checked = true;
    }
    char path[PCFS_SHA256_PATH_CAPACITY];
    rc = pcfs_sha256_path(digest, path, sizeof(path));
    if (rc) return rc;
    uint8_t mask = (uint8_t)(1u << (digest[0] & 7));
    if (!(im->cas_prefix_checked[digest[0] >> 3] & mask)) {
        path[17] = 0;
        rc = pcfs_from_errno(erofs_ilookup(path, &parent));
        path[17] = '/';
        if (rc) return rc;
        if (!S_ISDIR(parent.i_mode)) return PCFS_NOT_FOUND;
        im->cas_prefix_checked[digest[0] >> 3] |= mask;
    }
    inode->sbi = &im->sbi;
    return pcfs_from_errno(erofs_ilookup(path, inode));
}
int32_t pcfs_image_members(struct pcfs_image *im, const struct pcfs_pack_member_v1 *rows,
    size_t count, int32_t *statuses, struct pcfs_member **out) {
    if (count > 1024 || (!rows && count) || (!statuses && count) || (!out && count)) return PCFS_INVALID;
    for (size_t i=0; i<count; ++i) { statuses[i]=PCFS_INVALID; out[i]=NULL; }
    if (!im || im->projection) return PCFS_INVALID;
    if (!count) return pcfs_image_unchanged(im);
    int32_t rc = pthread_mutex_lock(&im->lookup_lock) ? PCFS_IO : PCFS_OK; if (rc) return rc;
    rc=pcfs_image_unchanged(im);
    if (rc) { pthread_mutex_unlock(&im->lookup_lock); return rc; }
    for (size_t i=0; i<count; ++i) {
        struct pcfs_member *m=NULL;
        int32_t status=PCFS_OK;
        if (rows[i].size_bytes > INT64_MAX) status=PCFS_INVALID;
        else {
            m=new_member();
            if (!m) status=PCFS_IO;
            else status=lookup_cas_member(im, rows[i].sha256, &m->inode);
        }
        if (!status && (!S_ISREG(m->inode.i_mode) || m->inode.i_size != rows[i].size_bytes)) status=PCFS_CORRUPT;
        if (!status) status=retain_image(im);
        if (status) free_member(m);
        else { m->image=im; memcpy(m->digest, rows[i].sha256, 32); out[i]=m; }
        statuses[i]=status;
    }
    pthread_mutex_unlock(&im->lookup_lock); return PCFS_OK;
}
/* Copy parsed metadata only. Each handle owns its lazy inode state and cursor
 * at the Rust boundary. No path traversal, body read or whole-image check. */
int32_t pcfs_member_clone(struct pcfs_member *source, struct pcfs_member **out) {
    if (!out) return PCFS_INVALID;
    *out = NULL;
    if (!source) return PCFS_INVALID;
    struct pcfs_member *m = new_member();
    if (!m) return PCFS_IO;
    bool lazy_inode = erofs_inode_is_data_compressed(source->inode.datalayout);
    if (lazy_inode && pthread_mutex_lock(&source->inode_lock)) { free_member(m); return PCFS_IO; }
    int32_t rc = PCFS_OK;
    /* Supported reader inodes have no owned heap fields. Never alias a future
     * parser extension's owning pointers by silently memcpy'ing them. */
    if (source->inode.xattr_shared_xattrs || source->inode.idata || source->inode.compressmeta)
        rc = PCFS_UNSUPPORTED;
    if (!rc) rc = retain_image(source->image);
    if (!rc) {
        m->image = source->image;
        m->inode = source->inode;
        memcpy(m->digest, source->digest, sizeof(m->digest));
        *out = m;
    }
    if (lazy_inode) pthread_mutex_unlock(&source->inode_lock);
    if (rc) free_member(m);
    return rc;
}
int32_t pcfs_member_pread(struct pcfs_member *m, uint64_t off, uint8_t *buffer,
                         size_t cap, size_t *read_out) {
    struct erofs_vfile vf;
    size_t n;
    int32_t rc;
    if (!read_out) return PCFS_INVALID;
    *read_out = 0;
    if (!m || (!buffer && cap)) return PCFS_INVALID;
    bool lazy_inode = erofs_inode_is_data_compressed(m->inode.datalayout);
    if (lazy_inode && pthread_mutex_lock(&m->inode_lock)) return PCFS_IO;
    atomic_fetch_add_explicit(&io_counters.member_read_calls, 1, memory_order_relaxed);
    rc = pcfs_image_unchanged(m->image); if (rc) goto done;
    if (!cap || off >= m->inode.i_size) goto done;
    n = cap > PCFS_IO_MAX_TRANSFER ? PCFS_IO_MAX_TRANSFER : cap;
    if ((uint64_t)n > m->inode.i_size - off) n = (size_t)(m->inode.i_size - off);
    rc = pcfs_from_errno(erofs_iopen(&vf, &m->inode));
    if (!rc) rc = pcfs_from_errno(erofs_pread(&vf, buffer, n, off));
    if (!rc) rc = pcfs_image_unchanged(m->image);
    if (!rc) { *read_out = n; atomic_fetch_add_explicit(&io_counters.member_read_bytes, n, memory_order_relaxed); }
done:
    if (lazy_inode) pthread_mutex_unlock(&m->inode_lock);
    return rc;
}
int32_t pcfs_verify_member_cancellable(struct pcfs_member *m,
    int32_t (*cancel)(void *), void *context) {
    uint8_t buffer[65536], digest[32];
    struct sha256_state hash;
    uint64_t pos = 0;
    int32_t rc;
    if (!m) return PCFS_INVALID;
    atomic_fetch_add_explicit(&io_counters.member_verify_calls, 1, memory_order_relaxed);
    erofs_sha256_init(&hash);
    while (pos < m->inode.i_size) {
        if (cancel && cancel(context)) { rc = PCFS_CANCELLED; goto done; }
        size_t got;
        rc = pcfs_member_pread(m, pos, buffer, sizeof(buffer), &got);
        if (rc || !got) { if (!rc) rc = PCFS_CORRUPT; goto done; }
        atomic_fetch_add_explicit(&io_counters.member_verify_bytes, got, memory_order_relaxed);
        if (erofs_sha256_process(&hash, buffer, (unsigned long)got)) { rc = PCFS_IO; goto done; }
        pos += got;
    }
    rc = pcfs_image_unchanged(m->image);
    if (!rc && erofs_sha256_done(&hash, digest)) rc = PCFS_IO;
    if (!rc && memcmp(digest, m->digest, 32)) rc = PCFS_CORRUPT;
done:
    return rc;
}
int32_t pcfs_member_verify(struct pcfs_member *m) {
    return pcfs_verify_member_cancellable(m, NULL, NULL);
}
void pcfs_member_destroy(struct pcfs_member *m) {
    if (!m) return;
    release_image(m->image); free_member(m);
}
const char *pcfs_erofs_fingerprint(void) {
    /* Stable encoder identity: the importer is still serial. Read-side cache
     * and locking changes must not change generated Pack bytes or identities. */
    return "erofs-utils@1c230a485f25837cb51d6fc79332f30758d9006c+polar-vfile-v2;4k;raw-lz4;serial";
}

int32_t pcfs_check_native_pack(struct pcfs_image *im) {
    if (!im || im->projection || im->backing_count) return PCFS_UNSUPPORTED;
    int32_t rc = pcfs_image_unchanged(im);
    if (rc) return rc;
    /* This is an explicit whitelist, not "kernel supports it so it must work".
     * A legacy image can be admitted member-by-member when placement permits. */
    const uint32_t allowed = EROFS_FEATURE_INCOMPAT_LZ4_0PADDING |
                             EROFS_FEATURE_INCOMPAT_COMPR_CFGS;
    /* COMPR_CFGS and BIG_PCLUSTER share the same feature bit. Check the
     * decoded compression config too; a flag mask alone cannot enforce v1. */
    if (im->sbi.blkszbits != 12 || im->sbi.extra_devices ||
        im->sbi.lz4.max_pclusterblks > 1 ||
        (im->sbi.feature_incompat & ~allowed) ||
        (im->sbi.available_compr_algs & ~(1u << Z_EROFS_COMPRESSION_LZ4))) return PCFS_UNSUPPORTED;
    return PCFS_OK;
}
int32_t pcfs_check_external_inode(struct pcfs_image *im, struct erofs_inode *inode) {
    int32_t rc = pcfs_check_native_pack(im);
    if (rc) return rc;
    if (!S_ISREG(inode->i_mode)) return PCFS_CORRUPT;
    if (inode->xattr_isize) return PCFS_UNSUPPORTED;
    if (!inode->i_size) return PCFS_OK;
    if (inode->datalayout != EROFS_INODE_FLAT_PLAIN &&
        !is_inode_layout_compression(inode)) return PCFS_UNSUPPORTED;
    uint64_t pos = 0;
    while (pos < inode->i_size) {
        struct erofs_map_blocks map = { .buf = __EROFS_BUF_INITIALIZER, .m_la = pos };
        int err = erofs_map_blocks(inode, &map, EROFS_GET_BLOCKS_FIEMAP);
        if (err) return pcfs_from_errno(err);
        if (!(map.m_flags & EROFS_MAP_MAPPED) ||
            (map.m_flags & (EROFS_MAP_META | __EROFS_MAP_FRAGMENT | EROFS_MAP_PARTIAL_REF)) ||
            map.m_deviceid || (map.m_pa & 4095)) return PCFS_UNSUPPORTED;
        if (map.m_la > pos || map.m_llen > UINT64_MAX - map.m_la ||
            map.m_la + map.m_llen <= pos || map.m_pa > im->size ||
            !map.m_plen || map.m_plen > im->size - map.m_pa) return PCFS_CORRUPT;
        if (is_inode_layout_compression(inode) &&
            map.m_algorithmformat < Z_EROFS_COMPRESSION_MAX &&
            map.m_algorithmformat != Z_EROFS_COMPRESSION_LZ4) return PCFS_UNSUPPORTED;
        pos = map.m_la + map.m_llen;
    }
    return pcfs_image_unchanged(im);
}
int32_t pcfs_image_check_native_member(struct pcfs_image *im, const uint8_t digest[32], uint64_t size) {
    struct pcfs_member *m = NULL;
    int32_t rc = pcfs_native_lock();
    if (rc) return rc;
    rc = pcfs_image_member(im, digest, size, &m);
    if (!rc) rc = pcfs_check_external_inode(im, &m->inode);
    pcfs_member_destroy(m);
    pcfs_native_unlock();
    return rc;
}
int32_t pcfs_native_meta_stat(struct pcfs_image *im, const char *path, struct pcfs_meta_stat_v1 *out) {
    if (!out) return PCFS_INVALID;
    memset(out, 0, sizeof(*out));
    if (!im || !im->projection) return PCFS_INVALID;
    int32_t rc = pcfs_native_lock();
    if (rc) return rc;
    struct erofs_inode inode = {0};
    rc = pcfs_image_unchanged(im);
    if (!rc && path && !*path) {
        inode.sbi = &im->sbi; inode.nid = im->sbi.root_nid;
        rc = pcfs_from_errno(erofs_read_inode_from_disk(&inode));
    } else if (!rc) rc = lookup_relative(im, path, &inode);
    /* Native profile v1 has fixed epoch and no xattrs. An imported image
     * must not smuggle ACLs/capabilities while matching only mode bits. */
    uint32_t kind=0; uint8_t key[32];
    if (!rc) rc=meta_inode_kind(im,&inode,&kind,key);
    if (!rc) {
        out->size_bytes = inode.i_size;
        out->mode = inode.i_mode & 07777;
        out->uid = inode.i_uid; out->gid = inode.i_gid;
        out->kind = kind;
        if (!out->kind) rc = PCFS_UNSUPPORTED;
    }
    if (!rc) rc = pcfs_image_unchanged(im);
    if (rc) memset(out, 0, sizeof(*out));
    pcfs_native_unlock();
    return rc;
}
int32_t pcfs_native_meta_readlink(struct pcfs_image *im, const char *path,
    char *buf, size_t cap, size_t *length) {
    if (!length) return PCFS_INVALID;
    *length = 0;
    if (!im || !im->projection || (!buf && cap)) return PCFS_INVALID;
    int32_t rc = pcfs_native_lock();
    if (rc) return rc;
    struct erofs_inode inode = {0};
    struct erofs_vfile vf;
    rc = pcfs_image_unchanged(im);
    if (!rc) rc = lookup_relative(im, path, &inode);
    if (!rc && !S_ISLNK(inode.i_mode)) rc = PCFS_INVALID;
    if (!rc && (inode.i_size > cap || inode.i_size > PCFS_META_PATH_MAX)) rc = PCFS_LIMIT;
    if (!rc) rc = pcfs_from_errno(erofs_iopen(&vf, &inode));
    if (!rc) rc = pcfs_from_errno(erofs_pread(&vf, buf, inode.i_size, 0));
    if (!rc) rc = pcfs_image_unchanged(im);
    if (!rc) *length = (size_t)inode.i_size;
    pcfs_native_unlock();
    return rc;
}

/* Bounded namespace traversal for restore verification. A superblock inode
 * count is NOT a path count (hardlinks); never use it as a membership proof. */
struct count_state {
    erofs_nid_t *queue;
    size_t tail, capacity;
    uint64_t count, maximum;
};
struct count_context { struct erofs_dir_context dir; struct count_state *state; };
static int count_dirent(struct erofs_dir_context *base) {
    struct count_state *s = ((struct count_context *)base)->state;
    if (base->dot_dotdot) return 0;
    if (s->count >= s->maximum) return -EFBIG;
    ++s->count;
    if (base->de_ftype != EROFS_FT_DIR) return 0;
    if (s->tail == s->capacity) {
        size_t next = s->capacity * 2;
        if (next > s->maximum) next = (size_t)s->maximum;
        if (next <= s->tail) return -EFBIG;
        void *p = realloc(s->queue, next * sizeof(*s->queue));
        if (!p) return -ENOMEM;
        s->queue = p; s->capacity = next;
    }
    s->queue[s->tail++] = base->de_nid;
    return 0;
}
int32_t pcfs_native_meta_count_entries(struct pcfs_image *im,
    uint64_t maximum, uint64_t *out) {
    if (!out) return PCFS_INVALID;
    *out = 0;
    if (!im || !im->projection) return PCFS_INVALID;
    if (!maximum || maximum > PCFS_EROFS_MAX_OBJECTS + 1u) return PCFS_LIMIT;
    int32_t rc = pcfs_native_lock();
    if (rc) return rc;
    struct count_state s = {.count = 1, .maximum = maximum, .tail = 1,
        .capacity = (size_t)(maximum < 128 ? maximum : 128)};
    s.queue = calloc(s.capacity, sizeof(*s.queue));
    if (!s.queue) { rc = PCFS_IO; goto done; }
    s.queue[0] = im->sbi.root_nid;
    rc = pcfs_image_unchanged(im);
    for (size_t head = 0; !rc && head < s.tail; ++head) {
        struct erofs_inode inode = {.sbi = &im->sbi, .nid = s.queue[head]};
        rc = pcfs_from_errno(erofs_read_inode_from_disk(&inode));
        if (rc) break;
        if (!S_ISDIR(inode.i_mode)) { rc = PCFS_CORRUPT; break; }
        struct count_context ctx = {.dir = {.dir = &inode, .cb = count_dirent}, .state = &s};
        int err = erofs_iterate_dir(&ctx.dir, true);
        rc = err == -EFBIG ? PCFS_LIMIT : pcfs_from_errno(err);
    }
    if (!rc) rc = pcfs_image_unchanged(im);
    if (!rc) *out = s.count;
done:
    free(s.queue);
    pcfs_native_unlock();
    return rc;
}

int32_t pcfs_hybrid_meta_redirect(struct pcfs_image *im, const char *path,
    uint8_t sha256[32], uint64_t *size) {
    if (sha256) memset(sha256,0,32);
    if (size) *size=0;
    if(!im || !im->projection || !im->hybrid || !sha256 || !size) return PCFS_INVALID;
    int32_t rc=pcfs_native_lock(); if(rc) return rc;
    struct erofs_inode n={0}; uint32_t kind=0; uint8_t key[32]={0};
    rc=pcfs_image_unchanged(im);
    if(!rc) rc=lookup_relative(im,path,&n);
    if(!rc) rc=meta_inode_kind(im,&n,&kind,key);
    if(!rc && kind!=PCFS_META_REDIRECT) rc=PCFS_INVALID;
    if(!rc) rc=pcfs_image_unchanged(im);
    if(!rc){memcpy(sha256,key,32);*size=n.i_size;}
    pcfs_native_unlock();return rc;
}

int32_t pcfs_native_meta_source_pack(struct pcfs_image *im, const char *path, uint32_t *out) {
    if (out) *out = UINT32_MAX;
    if (!im || !im->projection || !out) return PCFS_INVALID;
    int32_t rc = pcfs_native_lock();
    if (rc) return rc;
    struct erofs_inode n = {0}; uint32_t kind = 0, source = UINT32_MAX; uint8_t key[32];
    rc = pcfs_image_unchanged(im);
    if (!rc) rc = lookup_relative(im, path, &n);
    if (!rc) rc = meta_inode_kind(im, &n, &kind, key);
    if (!rc && kind != PCFS_META_FILE) rc = PCFS_UNSUPPORTED;
    uint64_t pos = 0;
    while (!rc && pos < n.i_size) {
        struct erofs_map_blocks map = { .buf=__EROFS_BUF_INITIALIZER, .m_la=pos };
        rc = pcfs_from_errno(erofs_map_blocks(&n, &map, EROFS_GET_BLOCKS_FIEMAP));
        struct erofs_map_dev dev = { .m_deviceid=map.m_deviceid, .m_pa=map.m_pa };
        if (!rc) rc = pcfs_from_errno(erofs_map_dev(&im->sbi, &dev));
        if (!rc && (!map.m_llen || !(map.m_flags & EROFS_MAP_MAPPED) ||
            dev.m_deviceid == 0 || dev.m_deviceid > im->backing_count ||
            map.m_la > UINT64_MAX-map.m_llen || map.m_la+map.m_llen <= pos)) rc = PCFS_CORRUPT;
        if (!rc) {
            uint32_t current = dev.m_deviceid-1;
            if (source != UINT32_MAX && source != current) rc = PCFS_CORRUPT;
            source = current; pos = map.m_la+map.m_llen;
        }
    }
    if (!rc) rc = pcfs_image_unchanged(im);
    /* Compression metadata may be allocated by map traversal. */
    free(n.compressmeta);
    if (!rc) *out = source;
    pcfs_native_unlock(); return rc;
}

/* A bounded metadata record, not an API for arbitrary paths or payloads. */
int32_t pcfs_pack_summary_read(struct pcfs_image *im, char *buffer,
    size_t capacity, size_t *length) {
    if (!length) return PCFS_INVALID;
    *length = 0;
    if (!im || (!buffer && capacity)) return PCFS_INVALID;
    int32_t rc = pcfs_native_lock(); if (rc) return rc;
    struct erofs_inode node = {0}; struct erofs_vfile vf;
    char bytes[1024];
    rc = pcfs_check_native_pack(im);
    if (!rc) rc = lookup_relative(im, ".polar/pack-build.json", &node);
    if (!rc && (!S_ISREG(node.i_mode) || node.xattr_isize ||
                node.i_size == 0 || node.i_size > sizeof(bytes))) rc = PCFS_CORRUPT;
    if (!rc && node.i_size > capacity) rc = PCFS_LIMIT;
    if (!rc) rc = pcfs_from_errno(erofs_iopen(&vf, &node));
    if (!rc) rc = pcfs_from_errno(erofs_pread(&vf, bytes, node.i_size, 0));
    if (!rc) rc = pcfs_image_unchanged(im);
    if (!rc) { memcpy(buffer, bytes, node.i_size); *length = (size_t)node.i_size; }
    free(node.xattr_shared_xattrs);
    pcfs_native_unlock(); return rc;
}

/* Bounded complete enumeration of the canonical member namespace. Reuse the
 * pinned liberofs directory parser; no second on-disk format parser. */
struct pack_walk {
    struct erofs_dir_context dir;
    struct pcfs_image *image;
    struct pcfs_pack_member_v1 *out;
    size_t capacity, *count;
    int32_t (*cancel)(void *);
    void *context;
    int shard;
};
static int hex_digit(char c) {
    return c >= '0' && c <= '9' ? c - '0' : c >= 'a' && c <= 'f' ? c - 'a' + 10 : -1;
}
static int pack_walk_entry(struct erofs_dir_context *dir) {
    struct pack_walk *walk = (void *)dir;
    if (walk->cancel && walk->cancel(walk->context)) return -ECANCELED;
    if (dir->dot_dotdot) return 0;
    struct erofs_inode inode = { .sbi = &walk->image->sbi, .nid = dir->de_nid };
    int err = erofs_read_inode_from_disk(&inode);
    if (err) return err;
    if (walk->shard < 0) {
        if (dir->de_namelen != 2 || !S_ISDIR(inode.i_mode)) return -EUCLEAN;
        int hi = hex_digit(dir->dname[0]), lo = hex_digit(dir->dname[1]);
        if (hi < 0 || lo < 0) return -EUCLEAN;
        struct pack_walk child = *walk;
        child.shard = hi * 16 + lo;
        child.dir = (struct erofs_dir_context){ .dir = &inode, .cb = pack_walk_entry };
        return erofs_iterate_dir(&child.dir, false);
    }
    if (dir->de_namelen != 64 || !S_ISREG(inode.i_mode)) return -EUCLEAN;
    if (*walk->count == walk->capacity || inode.i_size > INT64_MAX) return -EFBIG;
    struct pcfs_pack_member_v1 *row = &walk->out[*walk->count];
    for (size_t n = 0; n < 32; ++n) {
        int hi = hex_digit(dir->dname[2*n]), lo = hex_digit(dir->dname[2*n+1]);
        if (hi < 0 || lo < 0) return -EUCLEAN;
        row->sha256[n] = (uint8_t)(hi * 16 + lo);
    }
    if (row->sha256[0] != walk->shard) return -EUCLEAN;
    row->size_bytes = inode.i_size;
    ++*walk->count;
    return 0;
}
int32_t pcfs_pack_members(struct pcfs_image *im, struct pcfs_pack_member_v1 *out,
    size_t capacity, size_t *count, int32_t (*cancel)(void *), void *context) {
    if (count) *count = 0;
    if (!im || im->projection || !count || (!out && capacity) || capacity > 1000000)
        return PCFS_INVALID;
    int32_t rc = pcfs_native_lock(); if (rc) return rc;
    rc = pcfs_image_unchanged(im);
    if (!rc) rc = pcfs_check_native_pack(im);
    struct erofs_inode inode = {0};
    if (!rc) rc = lookup_relative(im, "objects/sha256", &inode);
    if (!rc && !S_ISDIR(inode.i_mode)) rc = PCFS_CORRUPT;
    if (!rc) {
        struct pack_walk walk = { .dir = { .dir = &inode, .cb = pack_walk_entry },
            .image = im, .out = out, .capacity = capacity, .count = count,
            .cancel = cancel, .context = context, .shard = -1 };
        rc = pcfs_from_errno(erofs_iterate_dir(&walk.dir, false));
    }
    if (!rc) rc = pcfs_image_unchanged(im);
    if (rc) *count = 0;
    pcfs_native_unlock(); return rc;
}

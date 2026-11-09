#ifndef PCFS_EROFS_PRIVATE_H
#define PCFS_EROFS_PRIVATE_H
#ifndef _GNU_SOURCE
#define _GNU_SOURCE
#endif
#include "pcfs_erofs.h"
#include "erofs/internal.h"
#include "erofs/io.h"
#include "liberofs_sha256.h"
#include <stdbool.h>
#include <stdatomic.h>
#include <pthread.h>
#include <sys/stat.h>

/* Encoder and legacy inspection serialization. Read-only image open/member
 * operations use immutable per-image state, atomic refs and object-local locks.
 * Global cfg is initialized once and never modified afterwards. */
int32_t pcfs_native_lock(void);
void pcfs_native_unlock(void);
int32_t pcfs_native_build_enter(void);
void pcfs_native_build_leave(void);
int32_t pcfs_from_errno(int error);
int32_t pcfs_hash_fd(int fd, uint64_t size, uint8_t digest[32]);
int32_t pcfs_hash_fd_cancellable(int fd, uint64_t size, uint8_t digest[32],
    int32_t (*cancel)(void *), void *context);
int32_t pcfs_verify_member_cancellable(struct pcfs_member *,
    int32_t (*cancel)(void *), void *context);
bool pcfs_same_stat(const struct stat *, const struct stat *);

struct pcfs_image {
    struct erofs_sb_info sbi;
    struct stat initial;
    uint64_t size;
    uint8_t digest[32];
    struct pcfs_image **backings;
    size_t backing_count;
    bool projection;
    bool hybrid;
    bool cas_parents_checked;
    uint8_t cas_prefix_checked[32];
    pthread_mutex_t lookup_lock;
    atomic_size_t refs;
    int fd;
};
struct pcfs_member {
    struct pcfs_image *image;
    struct erofs_inode inode;
    uint8_t digest[32];
    /* Only this handle's lazy LZ4 inode fields; never a cross-Pack read lock. */
    pthread_mutex_t inode_lock;
};
int32_t pcfs_image_unchanged(struct pcfs_image *);
int32_t pcfs_check_native_pack(struct pcfs_image *);
int32_t pcfs_check_external_inode(struct pcfs_image *, struct erofs_inode *);
int32_t pcfs_image_path_member(struct pcfs_image *, const char *,
    const uint8_t digest[32], uint64_t size, struct pcfs_member **);
int pcfs_erofs_rebuild_file(struct erofs_sb_info *, struct erofs_inode *);
/* Global initialization must have completed. Exact bytes were just hashed or
 * supplied under an admitted managed/immutable image ownership contract.
 * The new image is private until fully initialized; retained backings use atomic refs. */
int32_t pcfs_open_verified_image_devices(int fd, uint64_t size,
    struct pcfs_image *const *packs, size_t count, struct pcfs_image **out);
int32_t pcfs_open_verified_image(int fd, uint64_t size, struct pcfs_image **out);
#endif

#define _GNU_SOURCE
#include "pcfs_generated_seal.h"
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

static int sync_retry(int fd) {
    int r;
    do { r = fsync(fd); } while (r < 0 && errno == EINTR);
    return r;
}
static int chmod_retry(int fd, mode_t mode) {
    int r;
    do { r = fchmod(fd, mode); } while (r < 0 && errno == EINTR);
    return r;
}
static int same_inode(const struct stat *a, const struct stat *b) {
    return S_ISREG(a->st_mode) && S_ISREG(b->st_mode) &&
        a->st_dev == b->st_dev && a->st_ino == b->st_ino &&
        a->st_size == b->st_size && a->st_uid == b->st_uid &&
        a->st_gid == b->st_gid && a->st_mode == b->st_mode &&
        a->st_nlink == 1 && b->st_nlink == 1 &&
        a->st_mtim.tv_sec == b->st_mtim.tv_sec &&
        a->st_mtim.tv_nsec == b->st_mtim.tv_nsec &&
        a->st_ctim.tv_sec == b->st_ctim.tv_sec &&
        a->st_ctim.tv_nsec == b->st_ctim.tv_nsec;
}
int32_t pcfs_generated_seal(int32_t writer_fd, int32_t directory_fd,
                            const char *leaf, uint64_t expected_size,
                            int32_t *read_only_fd) {
    struct stat before, named, after, directory;
    int flags, readonly = -1;
    int32_t status = PCFS_IO;
    if (!read_only_fd) return PCFS_INVALID;
    *read_only_fd = -1;
    if (!leaf || !*leaf || strnlen(leaf, NAME_MAX + 1u) > NAME_MAX ||
        strchr(leaf, '/') || !strcmp(leaf, ".") || !strcmp(leaf, "..") ||
        expected_size > INT64_MAX) return PCFS_INVALID;
    flags = fcntl(writer_fd, F_GETFL);
    if (flags < 0) return PCFS_IO;
    if ((flags & O_ACCMODE) != O_RDWR || (flags & O_APPEND)) return PCFS_INVALID;
    if (fstat(directory_fd, &directory) || fstat(writer_fd, &before)) return PCFS_IO;
    if (!S_ISDIR(directory.st_mode) || !S_ISREG(before.st_mode) ||
        before.st_uid != geteuid() || before.st_nlink != 1 ||
        before.st_size < 0 || (uint64_t)before.st_size != expected_size ||
        (before.st_mode & 07777) != 0600) return PCFS_CORRUPT;
    readonly = openat(directory_fd, leaf, O_RDONLY | O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK);
    if (readonly < 0) return errno == ENOENT || errno == ELOOP ? PCFS_CORRUPT : PCFS_IO;
    if (fstat(readonly, &named)) goto done;
    if (!same_inode(&before, &named)) { status = PCFS_CORRUPT; goto done; }
    if (chmod_retry(writer_fd, 0444) || sync_retry(writer_fd) || sync_retry(directory_fd)) goto done;
    if (fstat(writer_fd, &after) || fstat(readonly, &named)) goto done;
    if (!same_inode(&after, &named) || after.st_dev != before.st_dev ||
        after.st_ino != before.st_ino || after.st_size != before.st_size ||
        after.st_mtim.tv_sec != before.st_mtim.tv_sec ||
        after.st_mtim.tv_nsec != before.st_mtim.tv_nsec ||
        (after.st_mode & 07777) != 0444) { status = PCFS_CORRUPT; goto done; }
    /* Recheck that the name still names our inode, not only the opened FD. */
    if (fstatat(directory_fd, leaf, &named, AT_SYMLINK_NOFOLLOW)) goto done;
    if (!same_inode(&after, &named)) { status = PCFS_CORRUPT; goto done; }
    *read_only_fd = readonly;
    readonly = -1;
    status = PCFS_OK;
done:
    if (readonly >= 0) close(readonly);
    return status;
}

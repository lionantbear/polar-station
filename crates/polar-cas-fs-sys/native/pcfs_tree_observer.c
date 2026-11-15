#define _GNU_SOURCE
#include "pcfs_tree_observer.h"
#include "pcfs_verity.h"
#include <linux/fs.h>
#include <sys/ioctl.h>
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <stdbool.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#define PCFS_TREE_PATH_LIMIT 4096u
#define PCFS_TREE_ENTRY_LIMIT 1000000u

struct observed_entry {
    char *path, *target;
    struct stat stamp;
    uint64_t token;
    uint32_t kind;
};
struct pcfs_observed_tree {
    int root, poison, locked;
    uint64_t locked_mount;
    struct pcfs_tree_options_v1 limits;
    struct pcfs_tree_cancel_v1 cancel;
    struct pcfs_tree_stats_v1 stats;
    struct observed_entry *entries;
    size_t count, capacity;
    size_t *sources;
};
static int locked_inode(struct pcfs_observed_tree *, int fd,
    struct stat *, uint32_t kind, uint32_t enable,
    struct pcfs_tree_lock_report_v1 *report);
static int io_status(void) {
    return errno == ENOMEM ? PCFS_LIMIT :
        (errno == ENOENT || errno == ENOTDIR || errno == ELOOP ? PCFS_CORRUPT : PCFS_IO);
}
static int fail(struct pcfs_observed_tree *tree, int status) {
    if (status && !tree->poison) tree->poison = status;
    return status;
}
static int cancelled(struct pcfs_observed_tree *tree) {
    if (tree->poison) return tree->poison;
    if (tree->cancel.is_cancelled && tree->cancel.is_cancelled(tree->cancel.context))
        return fail(tree, PCFS_CANCELLED);
    return PCFS_OK;
}
static bool same_stamp(const struct stat *a, const struct stat *b) {
    return a->st_dev == b->st_dev && a->st_ino == b->st_ino &&
        a->st_mode == b->st_mode && a->st_uid == b->st_uid && a->st_gid == b->st_gid &&
        a->st_size == b->st_size && a->st_mtim.tv_sec == b->st_mtim.tv_sec &&
        a->st_mtim.tv_nsec == b->st_mtim.tv_nsec &&
        a->st_ctim.tv_sec == b->st_ctim.tv_sec && a->st_ctim.tv_nsec == b->st_ctim.tv_nsec;
}
static int open_dir(int parent, const char *name) {
    int fd;
    do { fd = openat(parent, name, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC); }
    while (fd < 0 && errno == EINTR);
    return fd;
}
static int at_stat(int parent, const char *name, struct stat *out) {
    int rc;
    do { rc = fstatat(parent, name, out, AT_SYMLINK_NOFOLLOW); } while (rc < 0 && errno == EINTR);
    return rc;
}
static int fd_stat(int fd, struct stat *out) {
    int rc;
    do { rc = fstat(fd, out); } while (rc < 0 && errno == EINTR);
    return rc;
}
static int read_target(int parent, const char *name, char **out) {
    char buffer[PCFS_TREE_PATH_LIMIT + 1];
    ssize_t n;
    *out = NULL;
    do { n = readlinkat(parent, name, buffer, sizeof(buffer)); } while (n < 0 && errno == EINTR);
    if (n < 0) return io_status();
    if (!n || n >= (ssize_t)PCFS_TREE_PATH_LIMIT) return PCFS_LIMIT;
    buffer[n] = '\0';
    *out = strdup(buffer);
    return *out ? PCFS_OK : PCFS_LIMIT;
}
static int append(struct pcfs_observed_tree *tree, char *path, char *target,
                  const struct stat *stamp, uint32_t kind) {
    size_t bytes = strlen(path) + 1 + (target ? strlen(target) + 1 : 0);
    /* This function takes ownership ONLY on success. All limits are checked
     * before growing the node vector or publishing a node. */
    if (tree->count >= tree->limits.max_entries ||
        bytes > tree->limits.max_path_bytes - tree->stats.path_bytes)
        return PCFS_LIMIT;
    if (kind == PCFS_TREE_FILE && (stamp->st_size < 0 ||
        tree->stats.file_count >= tree->limits.max_sources ||
        (uint64_t)stamp->st_size > tree->limits.max_input_bytes - tree->stats.input_bytes))
        return PCFS_LIMIT;
    if (tree->count == tree->capacity) {
        size_t cap = tree->capacity ? tree->capacity * 2 : 64;
        if (cap > tree->limits.max_entries) cap = (size_t)tree->limits.max_entries;
        void *nodes = realloc(tree->entries, cap * sizeof(*tree->entries));
        if (!nodes) return PCFS_LIMIT;
        tree->entries = nodes; tree->capacity = cap;
    }
    tree->entries[tree->count++] = (struct observed_entry){
        .path = path, .target = target, .stamp = *stamp,
        .token = PCFS_TREE_NO_SOURCE, .kind = kind,
    };
    tree->stats.entry_count++;
    tree->stats.path_bytes += bytes;
    if (kind == PCFS_TREE_FILE) {
        tree->stats.file_count++;
        tree->stats.input_bytes += (uint64_t)stamp->st_size;
    } else if (kind == PCFS_TREE_DIRECTORY) tree->stats.directory_count++;
    else tree->stats.symlink_count++;
    return PCFS_OK;
}
static int join_path(const char *parent, const char *name, char **out) {
    size_t a = strlen(parent), b = strlen(name);
    size_t total = a + (a ? 1 : 0) + b;
    *out = NULL;
    if (!b || b > 255 || total >= PCFS_TREE_PATH_LIMIT) return PCFS_LIMIT;
    char *path = malloc(total + 1);
    if (!path) return PCFS_LIMIT;
    if (a) { memcpy(path, parent, a); path[a] = '/'; }
    memcpy(path + a + (a ? 1 : 0), name, b + 1);
    *out = path;
    return PCFS_OK;
}
/* The recursion is limited to 128, and every directory FD is part of the
 * explicit max_directory_fds budget. File FDs are never opened by the walk. */
static int walk(struct pcfs_observed_tree *tree, int owned_fd, const char *path,
                const struct stat *expected, uint32_t depth, uint32_t active) {
    int status = cancelled(tree);
    struct stat before, after;
    if (status) { close(owned_fd); return status; }
    if (active > tree->limits.max_directory_fds || depth > tree->limits.max_depth) {
        close(owned_fd); return PCFS_LIMIT;
    }
    if (fd_stat(owned_fd, &before)) { status = io_status(); close(owned_fd); return status; }
    if (!same_stamp(&before, expected)) { close(owned_fd); return PCFS_CORRUPT; }
    DIR *stream = fdopendir(owned_fd);
    if (!stream) { status = io_status(); close(owned_fd); return status; }
    tree->stats.directory_walks++;
    if (active > tree->stats.peak_directory_fds) tree->stats.peak_directory_fds = active;
    while (!status) {
        if ((status = cancelled(tree))) break;
        errno = 0;
        struct dirent *d = readdir(stream);
        if (!d) { if (errno) status = io_status(); break; }
        if (!strcmp(d->d_name, ".") || !strcmp(d->d_name, "..")) continue;
        struct stat stamp, check;
        char *child_path = NULL, *target = NULL;
        uint32_t kind;
        if (depth >= tree->limits.max_depth) { status = PCFS_LIMIT; break; }
        if (at_stat(dirfd(stream), d->d_name, &stamp)) { status = io_status(); break; }
        if (S_ISDIR(stamp.st_mode)) kind = PCFS_TREE_DIRECTORY;
        else if (S_ISREG(stamp.st_mode)) kind = PCFS_TREE_FILE;
        else if (S_ISLNK(stamp.st_mode)) kind = PCFS_TREE_SYMLINK;
        else { status = PCFS_UNSUPPORTED; break; }
        status = join_path(path, d->d_name, &child_path);
        if (!status && kind == PCFS_TREE_SYMLINK) {
            status = read_target(dirfd(stream), d->d_name, &target);
            if (!status && at_stat(dirfd(stream), d->d_name, &check)) status = io_status();
            if (!status && !same_stamp(&stamp, &check)) status = PCFS_CORRUPT;
        }
        if (!status) status = append(tree, child_path, target, &stamp, kind);
        if (status) { free(child_path); free(target); break; }
        if (kind == PCFS_TREE_DIRECTORY) {
            if (active >= tree->limits.max_directory_fds) { status = PCFS_LIMIT; break; }
            int child = open_dir(dirfd(stream), d->d_name);
            if (child < 0) { status = io_status(); break; }
            status = walk(tree, child, child_path, &stamp, depth + 1, active + 1);
        }
    }
    if (!status && fd_stat(dirfd(stream), &after)) status = io_status();
    if (!status && !same_stamp(&before, &after)) status = PCFS_CORRUPT;
    if (closedir(stream) && !status) status = PCFS_IO;
    return status;
}
static int cmp_entry(const void *a, const void *b) {
    return strcmp(((const struct observed_entry *)a)->path, ((const struct observed_entry *)b)->path);
}
static const struct observed_entry *lookup(const struct pcfs_observed_tree *tree, const char *path) {
    struct observed_entry key = { .path = (char *)path };
    return bsearch(&key, tree->entries, tree->count, sizeof(*tree->entries), cmp_entry);
}
static int root_ok(struct pcfs_observed_tree *tree) {
    struct stat actual;
    if (fd_stat(tree->root, &actual)) return fail(tree, io_status());
    if (!same_stamp(&actual, &tree->entries[0].stamp)) return fail(tree, PCFS_CORRUPT);
    if (tree->locked) {
        struct pcfs_tree_lock_report_v1 report = {0};
        return fail(tree, locked_inode(tree, tree->root, &actual, PCFS_TREE_DIRECTORY, 0, &report));
    }
    return PCFS_OK;
}
/* Return a NEW parent fd. Every traversed directory is opened NOFOLLOW and
 * compared against its observed identity. It cannot be swapped for a symlink
 * or another directory between observation and reopening a regular source. */
static int parent_fd(struct pcfs_observed_tree *tree, const char *path,
                     int *out, const char **basename) {
    char prefix[PCFS_TREE_PATH_LIMIT];
    const char *slash, *start = path;
    int current, status = cancelled(tree);
    *out = -1;
    if (status || (status = root_ok(tree))) return status;
    current = open_dir(tree->root, ".");
    if (current < 0) return fail(tree, io_status());
    while ((slash = strchr(start, '/'))) {
        size_t end = (size_t)(slash - path), n = (size_t)(slash - start);
        char name[256];
        if (!n || n > 255) { status = PCFS_INVALID; break; }
        memcpy(name, start, n); name[n] = '\0';
        memcpy(prefix, path, end); prefix[end] = '\0';
        const struct observed_entry *entry = lookup(tree, prefix);
        if (!entry || entry->kind != PCFS_TREE_DIRECTORY) { status = PCFS_CORRUPT; break; }
        int next = open_dir(current, name);
        if (next < 0) { status = io_status(); break; }
        close(current); current = next;
        struct stat actual;
        if (fd_stat(current, &actual)) { status = io_status(); break; }
        if (!same_stamp(&actual, &entry->stamp)) { status = PCFS_CORRUPT; break; }
        if (tree->locked) {
            struct pcfs_tree_lock_report_v1 report = {0};
            status = locked_inode(tree, current, &actual, PCFS_TREE_DIRECTORY, 0, &report);
            if (status) break;
        }
        if ((status = cancelled(tree))) break;
        start = slash + 1;
    }
    if (status) { close(current); return fail(tree, status); }
    *out = current; *basename = start;
    return PCFS_OK;
}
int32_t pcfs_tree_observe(int32_t fd, const struct pcfs_tree_options_v1 *options,
    const struct pcfs_tree_cancel_v1 *cancel, struct pcfs_observed_tree **out) {
    if (!out) return PCFS_INVALID;
    *out = NULL;
    if (!options || options->abi_version != PCFS_ABI_V1 ||
        options->struct_size != sizeof(*options) || !options->max_entries ||
        options->max_entries > PCFS_TREE_ENTRY_LIMIT ||
        options->max_sources > PCFS_TREE_ENTRY_LIMIT || options->max_depth > 128 ||
        options->max_directory_fds < 3 || options->max_directory_fds > 1024 ||
        options->max_input_bytes > INT64_MAX || !options->max_path_bytes ||
        options->max_path_bytes > SIZE_MAX || fd < 0) return PCFS_INVALID;
    struct pcfs_observed_tree *tree = calloc(1, sizeof(*tree));
    if (!tree) return PCFS_LIMIT;
    tree->root = -1; tree->limits = *options;
    if (cancel) tree->cancel = *cancel;
    int status = cancelled(tree);
    struct stat stamp;
    if (status) goto done;
    tree->root = open_dir(fd, ".");
    if (tree->root < 0) { status = io_status(); goto done; }
    if (fd_stat(tree->root, &stamp)) { status = io_status(); goto done; }
    char *root_path = strdup("");
    if (!root_path) { status = PCFS_LIMIT; goto done; }
    status = append(tree, root_path, NULL, &stamp, PCFS_TREE_DIRECTORY);
    if (status) { free(root_path); goto done; }
    int walking = open_dir(tree->root, ".");
    if (walking < 0) { status = io_status(); goto done; }
    status = walk(tree, walking, "", &stamp, 0, 2);
    if (status) goto done;
    qsort(tree->entries, tree->count, sizeof(*tree->entries), cmp_entry);
    if (tree->stats.file_count) {
        tree->sources = calloc((size_t)tree->stats.file_count, sizeof(*tree->sources));
        if (!tree->sources) { status = PCFS_LIMIT; goto done; }
    }
    size_t token = 0;
    for (size_t i = 0; i < tree->count; ++i) {
        if (i && !strcmp(tree->entries[i-1].path, tree->entries[i].path)) {
            status = PCFS_CORRUPT; goto done;
        }
        if (tree->entries[i].kind == PCFS_TREE_FILE) {
            tree->entries[i].token = token; tree->sources[token++] = i;
        }
    }
    status = pcfs_tree_revalidate(tree);
done:
    if (status) { pcfs_tree_destroy(tree); return status; }
    *out = tree;
    return PCFS_OK;
}
int32_t pcfs_tree_entry(const struct pcfs_observed_tree *tree, size_t index,
                       struct pcfs_tree_entry_v1 *out) {
    if (out) memset(out, 0, sizeof(*out));
    if (!tree || !out || index >= tree->count) return PCFS_INVALID;
    if (tree->poison) return tree->poison;
    const struct observed_entry *e = tree->entries + index;
    *out = (struct pcfs_tree_entry_v1){ .path = e->path, .symlink_target = e->target,
        .size_bytes = e->kind == PCFS_TREE_FILE ? (uint64_t)e->stamp.st_size : 0,
        .source_token = e->token, .kind = e->kind, .mode = e->stamp.st_mode };
    return PCFS_OK;
}
int32_t pcfs_tree_stats(const struct pcfs_observed_tree *tree, struct pcfs_tree_stats_v1 *out) {
    if (!tree || !out) return PCFS_INVALID;
    *out = tree->stats;
    return PCFS_OK;
}
int32_t pcfs_tree_source_node(const struct pcfs_observed_tree *tree, uint64_t token, size_t *out) {
    if (!tree || !out || token >= tree->stats.file_count) return PCFS_INVALID;
    if (tree->poison) return tree->poison;
    *out = tree->sources[token];
    return PCFS_OK;
}
int32_t pcfs_tree_open_source(struct pcfs_observed_tree *tree, uint64_t token, int32_t *out) {
    if (!out) return PCFS_INVALID;
    *out = -1;
    if (!tree || token >= tree->stats.file_count) return PCFS_INVALID;
    if (cancelled(tree)) return tree->poison;
    const struct observed_entry *e = &tree->entries[tree->sources[token]];
    int parent = -1, fd, status;
    const char *name = NULL;
    status = parent_fd(tree, e->path, &parent, &name);
    if (status) return status;
    do { fd = openat(parent, name, O_RDONLY | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC); }
    while (fd < 0 && errno == EINTR);
    int saved_errno = errno;
    close(parent); errno = saved_errno;
    if (fd < 0) return fail(tree, io_status());
    struct stat actual;
    if (fd_stat(fd, &actual)) status = io_status();
    else if (!S_ISREG(actual.st_mode) || !same_stamp(&actual, &e->stamp)) status = PCFS_CORRUPT;
    if (status) { close(fd); return fail(tree, status); }
    if (tree->locked) {
        struct pcfs_tree_lock_report_v1 report = {0};
        status = locked_inode(tree, fd, &actual, PCFS_TREE_FILE, 0, &report);
        if (status) { close(fd); return fail(tree, status); }
    }
    tree->stats.source_opens++;
    *out = fd;
    return PCFS_OK;
}
int32_t pcfs_tree_revalidate(struct pcfs_observed_tree *tree) {
    if (!tree) return PCFS_INVALID;
    int status = cancelled(tree);
    if (status || (status = root_ok(tree))) return status;
    for (size_t i = 1; i < tree->count; ++i) {
        if ((status = cancelled(tree))) return status;
        const struct observed_entry *e = tree->entries + i;
        int parent = -1;
        const char *name = NULL;
        struct stat actual;
        status = parent_fd(tree, e->path, &parent, &name);
        if (status) return status;
        if (at_stat(parent, name, &actual)) status = io_status();
        else if (!same_stamp(&actual, &e->stamp)) status = PCFS_CORRUPT;
        if (!status && e->kind == PCFS_TREE_SYMLINK) {
            char *target = NULL;
            status = read_target(parent, name, &target);
            if (!status && strcmp(e->target, target)) status = PCFS_CORRUPT;
            free(target);
        }
        if (!status && tree->locked && e->kind != PCFS_TREE_SYMLINK) {
            int flags = O_RDONLY | O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK;
            if (e->kind == PCFS_TREE_DIRECTORY) flags |= O_DIRECTORY;
            int child = openat(parent, name, flags);
            if (child < 0) status = io_status();
            else {
                struct stat pinned;
                struct pcfs_tree_lock_report_v1 report = {0};
                if (fd_stat(child, &pinned)) status = io_status();
                else if (!same_stamp(&actual, &pinned)) status = PCFS_CORRUPT;
                else status = locked_inode(tree, child, &pinned, e->kind, 0, &report);
                close(child);
            }
        }
        close(parent);
        if (status) return fail(tree, status);
    }
    return root_ok(tree);
}
static int32_t provider_open(void *context, uint64_t token, int32_t *out) {
    return pcfs_tree_open_source(context, token, out);
}
static int32_t provider_cancel(void *context) { return cancelled(context) != PCFS_OK; }
int32_t pcfs_tree_source_provider(struct pcfs_observed_tree *tree, struct pcfs_sources_v1 *out) {
    if (!tree || !out) return PCFS_INVALID;
    if (tree->poison) return tree->poison;
    *out = (struct pcfs_sources_v1){ .abi_version = PCFS_ABI_V1,
        .struct_size = sizeof(*out), .context = tree,
        .open_fd = provider_open, .is_cancelled = provider_cancel };
    return PCFS_OK;
}
void pcfs_tree_destroy(struct pcfs_observed_tree *tree) {
    if (!tree) return;
    if (tree->root >= 0) close(tree->root);
    for (size_t i = 0; i < tree->count; ++i) {
        free(tree->entries[i].path); free(tree->entries[i].target);
    }
    free(tree->entries); free(tree->sources); free(tree);
}


/* Setting mode bits is NOT a sealing operation. Files use the existing kernel
 * fs-verity contract (which excludes writable FDs/mmaps during enable), then
 * immutable metadata; directory immutable flags protect the namespace. This
 * backend deliberately fails when that substrate is unavailable. */
static int lock_error(int error, struct pcfs_tree_lock_report_v1 *r) {
    r->os_error = error;
    if (error == ENOTTY || error == EOPNOTSUPP || error == ENOSYS)
        return PCFS_UNSUPPORTED;
    if (error == EPERM || error == EACCES || error == EROFS)
        return PCFS_VERITY_DENIED;
    if (error == EBUSY || error == ETXTBSY) return PCFS_VERITY_BUSY;
    return error == ENOMEM ? PCFS_LIMIT : PCFS_IO;
}
static int mount_identity(int fd, uint64_t *mount_id) {
    struct statx stx = {0};
    if (statx(fd, "", AT_EMPTY_PATH | AT_SYMLINK_NOFOLLOW,
              STATX_MNT_ID, &stx)) return errno;
    if (!(stx.stx_mask & STATX_MNT_ID)) return EOPNOTSUPP;
    *mount_id = stx.stx_mnt_id;
    return 0;
}
static int inode_flags(int fd, unsigned long request, int *flags) {
    int rc;
    do { rc = ioctl(fd, request, flags); } while (rc < 0 && errno == EINTR);
    return rc;
}
static int locked_inode(struct pcfs_observed_tree *tree, int fd,
    struct stat *stamp, uint32_t kind, uint32_t enable,
    struct pcfs_tree_lock_report_v1 *report) {
    uint64_t mount_id = 0;
    int error = mount_identity(fd, &mount_id), flags = 0;
    if (error) return lock_error(error, report);
    if (mount_id != tree->locked_mount) return PCFS_UNSUPPORTED;
    /* External aliases defeat directory ownership transfer. Reject them rather
     * than modifying another user's inode. Symlinks are checked separately. */
    if (kind != PCFS_TREE_DIRECTORY && stamp->st_nlink != 1)
        return PCFS_UNSUPPORTED;
    if (inode_flags(fd, FS_IOC_GETFLAGS, &flags)) return lock_error(errno, report);
    if (!(flags & FS_IMMUTABLE_FL) && !enable) return PCFS_TREE_LOCK_REQUIRED;
    if (kind == PCFS_TREE_FILE) {
        struct pcfs_verity_digest_v1 digest;
        int32_t os_error = 0;
        /* Already-immutable but non-verity files are never unlocked for enable. */
        int status = pcfs_verity_require(fd, 0, &digest, &os_error);
        if (status == PCFS_VERITY_NOT_ENABLED && enable &&
            !(flags & FS_IMMUTABLE_FL)) {
            status = pcfs_verity_require(fd, 1, &digest, &os_error);
            if (!status) {
                ++report->changed_nodes;
                /* ext4's successful enable removes blocks beyond EOF and can
                 * advance mtime as well as ctime. Capture that exact boundary;
                 * identity, payload size, ownership, mode and links must not
                 * change. Later mtime drift is still rejected by the caller. */
                struct stat after_verity;
                if (fd_stat(fd, &after_verity)) return lock_error(errno, report);
                struct stat compatible = after_verity;
                compatible.st_mtim = stamp->st_mtim;
                compatible.st_ctim = stamp->st_ctim;
                if (!same_stamp(stamp, &compatible) ||
                    after_verity.st_nlink != stamp->st_nlink)
                    return PCFS_CORRUPT;
                *stamp = after_verity;
            }
        }
        if (status) { report->os_error = os_error; return status; }
    }
    if (!(flags & FS_IMMUTABLE_FL)) {
        flags |= FS_IMMUTABLE_FL;
        if (inode_flags(fd, FS_IOC_SETFLAGS, &flags)) return lock_error(errno, report);
        ++report->changed_nodes;
        if (inode_flags(fd, FS_IOC_GETFLAGS, &flags)) return lock_error(errno, report);
        if (!(flags & FS_IMMUTABLE_FL)) return PCFS_TREE_LOCK_REQUIRED;
        if (fsync(fd)) return lock_error(errno, report);
    }
    ++report->checked_nodes;
    return PCFS_OK;
}
int32_t pcfs_tree_lock_v1(struct pcfs_observed_tree *tree, uint32_t enable,
    struct pcfs_tree_lock_report_v1 *report) {
    if (report) memset(report, 0, sizeof(*report));
    if (!tree || !report || enable > 1) return PCFS_INVALID;
    if (tree->poison) return tree->poison;
    int status = cancelled(tree);
    if (status) return status;
    status = pcfs_tree_revalidate(tree);
    if (status) return status;
    int error = mount_identity(tree->root, &tree->locked_mount);
    if (error) return fail(tree, lock_error(error, report));
    /* The array is lexicographically sorted: a directory is sealed before any
     * descendant is opened. Existing observation stamps detect intervening edits.
     * Do not set tree->locked until the entire scope has been sealed. */
    for (size_t i = 0; i < tree->count; ++i) {
        if ((status = cancelled(tree))) return status;
        struct observed_entry *e = tree->entries + i;
        int fd = -1, parent = -1;
        const char *name = NULL;
        struct stat before, after;
        if (!i) fd = open_dir(tree->root, ".");
        else {
            status = parent_fd(tree, e->path, &parent, &name);
            if (status) return status;
            int flags = O_NOFOLLOW | O_CLOEXEC;
            if (e->kind == PCFS_TREE_SYMLINK) flags |= O_PATH;
            else flags |= O_RDONLY | O_NONBLOCK;
            if (e->kind == PCFS_TREE_DIRECTORY) flags |= O_DIRECTORY;
            do { fd = openat(parent, name, flags); } while (fd < 0 && errno == EINTR);
            int saved = errno; close(parent); errno = saved;
        }
        if (fd < 0) return fail(tree, lock_error(errno, report));
        if (fd_stat(fd, &before)) status = lock_error(errno, report);
        else if (!same_stamp(&before, &e->stamp)) status = PCFS_CORRUPT;
        else if (e->kind == PCFS_TREE_SYMLINK) {
            uint64_t mnt = 0;
            error = mount_identity(fd, &mnt);
            if (error) status = lock_error(error, report);
            else if (mnt != tree->locked_mount || before.st_nlink != 1)
                status = PCFS_UNSUPPORTED;
            else ++report->checked_nodes;
        } else status = locked_inode(tree, fd, &before, e->kind, enable, report);
        if (!status && fd_stat(fd, &after)) status = lock_error(errno, report);
        if (!status) {
            /* A newly enabled file's baseline was captured immediately after
             * fs-verity enable. From that boundary, only immutable-flag ctime
             * may differ; size, mode, owner, mtime and identity stay strict. */
            struct stat compatible = after;
            compatible.st_ctim = before.st_ctim;
            if (!same_stamp(&before, &compatible)) status = PCFS_CORRUPT;
            else e->stamp = after;
        }
        close(fd);
        if (status) return fail(tree, status);
    }
    tree->locked = 1;
    return pcfs_tree_revalidate(tree);
}


/* Export a currently checked seal without re-hashing file bytes. This reuses
 * parent_fd/locked_inode, including one-mount, alias and immutable checks. */
int32_t pcfs_tree_seal_entry_v1(struct pcfs_observed_tree *tree, size_t index,
    struct pcfs_tree_seal_entry_v1 *out) {
    if (out) memset(out, 0, sizeof(*out));
    if (!tree || !out || index >= tree->count) return PCFS_INVALID;
    if (tree->poison) return tree->poison;
    if (!tree->locked) return PCFS_TREE_LOCK_REQUIRED;
    int status = cancelled(tree), fd = -1, parent = -1;
    if (status) return status;
    struct observed_entry *e = tree->entries + index;
    const char *name = NULL;
    if (!index) fd = open_dir(tree->root, ".");
    else {
        status = parent_fd(tree, e->path, &parent, &name);
        if (status) return status;
        int flags = O_NOFOLLOW | O_CLOEXEC;
        flags |= e->kind == PCFS_TREE_SYMLINK ? O_PATH : O_RDONLY | O_NONBLOCK;
        if (e->kind == PCFS_TREE_DIRECTORY) flags |= O_DIRECTORY;
        do { fd = openat(parent, name, flags); } while (fd < 0 && errno == EINTR);
        int saved = errno; close(parent); errno = saved;
    }
    if (fd < 0) return fail(tree, io_status());
    struct stat st;
    struct pcfs_tree_lock_report_v1 report = {0};
    if (fd_stat(fd, &st)) status = io_status();
    else if (!same_stamp(&st, &e->stamp)) status = PCFS_CORRUPT;
    else if (e->kind == PCFS_TREE_SYMLINK) {
        uint64_t mnt = 0;
        if (mount_identity(fd, &mnt)) status = PCFS_IO;
        else if (mnt != tree->locked_mount || st.st_nlink != 1) status = PCFS_CORRUPT;
    } else status = locked_inode(tree, fd, &st, e->kind, 0, &report);
    if (!status && e->kind == PCFS_TREE_FILE) {
        int32_t os_error = 0;
        status = pcfs_verity_require(fd, 0, &out->verity, &os_error);
    }
    if (!status && (st.st_size < 0 || st.st_mtim.tv_nsec < 0 ||
        st.st_mtim.tv_nsec >= 1000000000 || st.st_ctim.tv_nsec < 0 ||
        st.st_ctim.tv_nsec >= 1000000000)) status = PCFS_CORRUPT;
    if (!status) {
        out->device = (uint64_t)st.st_dev;
        out->inode = (uint64_t)st.st_ino;
        out->link_count = (uint64_t)st.st_nlink;
        out->size_bytes = (uint64_t)st.st_size;
        out->mtime_seconds = (int64_t)st.st_mtim.tv_sec;
        out->ctime_seconds = (int64_t)st.st_ctim.tv_sec;
        out->mtime_nanos = (uint32_t)st.st_mtim.tv_nsec;
        out->ctime_nanos = (uint32_t)st.st_ctim.tv_nsec;
        out->mode = (uint32_t)st.st_mode;
        out->uid = (uint32_t)st.st_uid;
        out->gid = (uint32_t)st.st_gid;
        out->kind = e->kind;
    }
    close(fd);
    if (status) memset(out, 0, sizeof(*out));
    return status ? fail(tree, status) : PCFS_OK;
}

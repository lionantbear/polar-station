#define _GNU_SOURCE
#define _FILE_OFFSET_BITS 64
#include "pcfs_io.h"
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <stdbool.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

_Static_assert(sizeof(off_t) >= 8, "64-bit positional I/O is required");
struct pcfs_source {
    struct pcfs_sources_v1 callbacks;
    uint64_t token, size;
    int fd;
    struct stat initial;
};
struct pcfs_output {
    int fd;
    uint64_t max_bytes;
    int32_t failure;
    bool finished;
};
static bool valid_status(int32_t rc) {
    return rc <= PCFS_OK && rc >= PCFS_NOT_FOUND;
}
static int32_t source_cancelled(const struct pcfs_sources_v1 *c) {
    return c->is_cancelled && c->is_cancelled(c->context) ? PCFS_CANCELLED : PCFS_OK;
}
static int32_t output_poison(struct pcfs_output *o, int32_t rc) {
    if (o && !o->failure) o->failure = rc;
    return rc;
}
static int32_t output_ready(const struct pcfs_output *o) {
    if (!o) return PCFS_INVALID;
    if (o->failure) return o->failure;
    return o->finished ? PCFS_INVALID : PCFS_OK;
}
int32_t pcfs_source_open(const struct pcfs_sources_v1 *c, uint64_t token,
                         uint64_t size, struct pcfs_source **out) {
    struct pcfs_source *s;
    int32_t rc, transferred = -1;
    if (!out) return PCFS_INVALID;
    *out = NULL;
    if (!c || c->struct_size != sizeof(*c)) return PCFS_INVALID;
    if (c->abi_version != PCFS_ABI_V1) return PCFS_UNSUPPORTED;
    if (size > INT64_MAX) return PCFS_LIMIT;
    rc = source_cancelled(c);
    if (rc != PCFS_OK) return rc;
    s = calloc(1, sizeof(*s));
    if (!s) return PCFS_IO;
    s->fd = -1;
    s->callbacks = *c;
    s->size = size;
    s->token = token;
    rc = c->open_fd ? c->open_fd(c->context, token, &transferred) : PCFS_NO_FD;
    if (rc == PCFS_OK) {
        int flags;
        if (transferred < 0) { rc = PCFS_INVALID; goto fail; }
        s->fd = transferred;
        flags = fcntl(s->fd, F_GETFL);
        if (flags < 0 || fstat(s->fd, &s->initial) < 0) { rc = PCFS_IO; goto fail; }
        if ((flags & O_ACCMODE) != O_RDONLY || !S_ISREG(s->initial.st_mode)
#ifdef O_PATH
            || (flags & O_PATH)
#endif
        ) { rc = PCFS_INVALID; goto fail; }
        if (s->initial.st_size < 0 || (uint64_t)s->initial.st_size != size) {
            rc = PCFS_CORRUPT; goto fail;
        }
        if (fcntl(s->fd, F_SETFD, FD_CLOEXEC) < 0) { rc = PCFS_IO; goto fail; }
    } else {
        /* A callback assigning an FD has transferred it. Do not leak it even
         * when the callback also violates its status contract. */
        if (transferred != -1) {
            s->fd = transferred; rc = PCFS_INVALID; goto fail;
        }
        if (rc != PCFS_NO_FD) {
            if (!valid_status(rc)) rc = PCFS_INVALID;
            goto fail;
        }
        if (!c->read_at) { rc = PCFS_UNSUPPORTED; goto fail; }
    }
    *out = s;
    return PCFS_OK;
fail:
    pcfs_source_destroy(s);
    return rc;
}
int32_t pcfs_source_pread(struct pcfs_source *s, uint64_t offset,
                          uint8_t *buf, size_t capacity, size_t *read_out) {
    size_t count;
    int32_t rc;
    if (!read_out) return PCFS_INVALID;
    *read_out = 0;
    if (!s || (!buf && capacity)) return PCFS_INVALID;
    rc = source_cancelled(&s->callbacks);
    if (rc != PCFS_OK) return rc;
    if (offset >= s->size || !capacity) return PCFS_OK;
    count = capacity;
    if (count > PCFS_IO_MAX_TRANSFER) count = PCFS_IO_MAX_TRANSFER;
    if ((uint64_t)count > s->size - offset) count = (size_t)(s->size - offset);
    if (s->fd >= 0) {
        ssize_t got;
        do { got = pread(s->fd, buf, count, (off_t)offset); }
        while (got < 0 && errno == EINTR);
        if (got < 0) return PCFS_IO;
        *read_out = (size_t)got;
        return PCFS_OK;
    }
    size_t got = 0;
    rc = s->callbacks.read_at(s->callbacks.context, s->token, offset, buf, count, &got);
    if (!valid_status(rc) || got > count) return PCFS_INVALID;
    if (rc != PCFS_OK) return rc;
    *read_out = got;
    return PCFS_OK;
}
int32_t pcfs_source_pread_exact(struct pcfs_source *s, uint64_t offset,
                                uint8_t *buf, size_t length) {
    size_t done = 0;
    if (!s || (!buf && length)) return PCFS_INVALID;
    if (offset > s->size || (uint64_t)length > s->size - offset) return PCFS_CORRUPT;
    while (done < length) {
        size_t got;
        int32_t rc = pcfs_source_pread(s, offset + done, buf + done, length - done, &got);
        if (rc != PCFS_OK) return rc;
        if (!got) return PCFS_CORRUPT;
        done += got;
    }
    return source_cancelled(&s->callbacks);
}
int32_t pcfs_source_check(struct pcfs_source *s) {
    struct stat now;
    int32_t rc;
    if (!s) return PCFS_INVALID;
    rc = source_cancelled(&s->callbacks);
    if (rc != PCFS_OK) return rc;
    if (s->fd < 0) return PCFS_OK;
    if (fstat(s->fd, &now) < 0) return PCFS_IO;
    if (now.st_dev != s->initial.st_dev || now.st_ino != s->initial.st_ino ||
        now.st_size != s->initial.st_size ||
        now.st_mtim.tv_sec != s->initial.st_mtim.tv_sec ||
        now.st_mtim.tv_nsec != s->initial.st_mtim.tv_nsec ||
        now.st_ctim.tv_sec != s->initial.st_ctim.tv_sec ||
        now.st_ctim.tv_nsec != s->initial.st_ctim.tv_nsec) return PCFS_CORRUPT;
    return PCFS_OK;
}
void pcfs_source_destroy(struct pcfs_source *s) {
    if (!s) return;
    /* Do not retry close after EINTR: the descriptor may already be released. */
    if (s->fd >= 0) (void)close(s->fd);
    free(s);
}
int32_t pcfs_output_new(int32_t fd, uint64_t limit, struct pcfs_output **out) {
    struct stat st;
    struct pcfs_output *o;
    int flags;
    if (!out) return PCFS_INVALID;
    *out = NULL;
    if (fd < 0) return PCFS_INVALID;
    if (limit > INT64_MAX) return PCFS_LIMIT;
    flags = fcntl(fd, F_GETFL);
    if (flags < 0 || fstat(fd, &st) < 0) return PCFS_IO;
    if ((flags & O_ACCMODE) != O_RDWR || (flags & O_APPEND) ||
        !S_ISREG(st.st_mode) || st.st_size != 0) return PCFS_INVALID;
    o = calloc(1, sizeof(*o));
    if (!o) return PCFS_IO;
    o->fd = fcntl(fd, F_DUPFD_CLOEXEC, 3);
    if (o->fd < 0) { free(o); return PCFS_IO; }
    o->max_bytes = limit;
    *out = o;
    return PCFS_OK;
}
int32_t pcfs_output_pwrite(struct pcfs_output *o, uint64_t offset,
                           const uint8_t *buf, size_t length) {
    size_t done = 0;
    int32_t rc = output_ready(o);
    if (rc != PCFS_OK) return rc;
    if (!buf && length) return output_poison(o, PCFS_INVALID);
    if (offset > o->max_bytes || (uint64_t)length > o->max_bytes - offset)
        return output_poison(o, PCFS_LIMIT);
    /* Linux pwrite on an O_APPEND open-file-description does not honor the
     * offset. Recheck because a duplicate shares file-status flags. */
    int flags = fcntl(o->fd, F_GETFL);
    if (flags < 0) return output_poison(o, PCFS_IO);
    if (flags & O_APPEND) return output_poison(o, PCFS_INVALID);
    while (done < length) {
        size_t count = length - done;
        ssize_t wrote;
        if (count > PCFS_IO_MAX_TRANSFER) count = PCFS_IO_MAX_TRANSFER;
        do { wrote = pwrite(o->fd, buf + done, count, (off_t)(offset + done)); }
        while (wrote < 0 && errno == EINTR);
        if (wrote <= 0) return output_poison(o, PCFS_IO);
        done += (size_t)wrote;
    }
    return PCFS_OK;
}
int32_t pcfs_output_truncate(struct pcfs_output *o, uint64_t size) {
    int32_t rc = output_ready(o);
    if (rc != PCFS_OK) return rc;
    if (size > o->max_bytes) return output_poison(o, PCFS_LIMIT);
    int result;
    do { result = ftruncate(o->fd, (off_t)size); }
    while (result < 0 && errno == EINTR);
    if (result < 0) return output_poison(o, PCFS_IO);
    return PCFS_OK;
}
int32_t pcfs_output_finish(struct pcfs_output *o, uint64_t *size) {
    struct stat st;
    int32_t rc;
    if (!size) return output_poison(o, PCFS_INVALID);
    *size = 0;
    rc = output_ready(o);
    if (rc != PCFS_OK) return rc;
    if (fstat(o->fd, &st) < 0 || st.st_size < 0) return output_poison(o, PCFS_IO);
    if ((uint64_t)st.st_size > o->max_bytes) return output_poison(o, PCFS_LIMIT);
    int result;
    do { result = fsync(o->fd); } while (result < 0 && errno == EINTR);
    if (result < 0) return output_poison(o, PCFS_IO);
    o->finished = true;
    *size = (uint64_t)st.st_size;
    return PCFS_OK;
}
void pcfs_output_destroy(struct pcfs_output *o) {
    if (!o) return;
    (void)close(o->fd);
    free(o);
}
int32_t pcfs_sha256_path(const uint8_t sha[32], char *out, size_t capacity) {
    static const char hex[] = "0123456789abcdef";
    static const char prefix[] = "objects/sha256/";
    size_t n = sizeof(prefix) - 1;
    if (!sha || !out) return PCFS_INVALID;
    if (capacity < PCFS_SHA256_PATH_CAPACITY) return PCFS_LIMIT;
    memcpy(out, prefix, n);
    out[n++] = hex[sha[0] >> 4];
    out[n++] = hex[sha[0] & 15];
    out[n++] = '/';
    for (size_t i = 0; i < 32; ++i) {
        out[n++] = hex[sha[i] >> 4]; out[n++] = hex[sha[i] & 15];
    }
    out[n] = '\0';
    return PCFS_OK;
}

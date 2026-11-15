#define _GNU_SOURCE
#define _FILE_OFFSET_BITS 64
#include "pcfs_erofs_job.h"
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <stdlib.h>
#include <string.h>
#include <sys/uio.h>
#include <unistd.h>

struct pcfs_native_job {
    struct pcfs_output *output;
    int fd;
    int32_t failure;
    int32_t (*cancel)(void *);
    void *context;
    uint64_t pos;
};
static struct pcfs_native_job *job_of(struct erofs_vfile *vf) {
    struct pcfs_native_job *job;
    memcpy(&job, vf->payload, sizeof(job));
    return job;
}
int pcfs_job_fail(struct pcfs_native_job *job, int32_t status) {
    if (!job->failure) job->failure = status;
    return -EIO;
}
int pcfs_job_cancelled(struct pcfs_native_job *job) {
    if (job->failure) return -EIO;
    if (job->cancel && job->cancel(job->context)) return pcfs_job_fail(job, PCFS_CANCELLED);
    return 0;
}
int32_t pcfs_job_status(const struct pcfs_native_job *job) { return job->failure; }
int pcfs_job_fd(const struct pcfs_native_job *job) { return job->fd; }
static ssize_t out_pwrite(struct erofs_vfile *vf, const void *buf, u64 pos, size_t len) {
    struct pcfs_native_job *job = job_of(vf);
    if (pcfs_job_cancelled(job)) return -EIO;
    if (len > SSIZE_MAX) return pcfs_job_fail(job, PCFS_LIMIT);
    int32_t rc = pcfs_output_pwrite(job->output, pos, buf, len);
    return rc ? pcfs_job_fail(job, rc) : (ssize_t)len;
}
static ssize_t out_pwritev(struct erofs_vfile *vf, const struct iovec *iov, int count, u64 pos) {
    ssize_t total = 0;
    if (count < 0 || (!iov && count)) return -EINVAL;
    for (int i = 0; i < count; ++i) {
        if (iov[i].iov_len > (size_t)(SSIZE_MAX - total) || pos > UINT64_MAX - iov[i].iov_len)
            return pcfs_job_fail(job_of(vf), PCFS_LIMIT);
        ssize_t n = out_pwrite(vf, iov[i].iov_base, pos, iov[i].iov_len);
        if (n < 0) return n;
        total += n; pos += (uint64_t)n;
    }
    return total;
}
static ssize_t out_pread(struct erofs_vfile *vf, void *buf, size_t len, u64 pos) {
    struct pcfs_native_job *job = job_of(vf);
    ssize_t n;
    if (pcfs_job_cancelled(job)) return -EIO;
    if (pos > INT64_MAX || len > SSIZE_MAX) return pcfs_job_fail(job, PCFS_LIMIT);
    do { n = pread(job->fd, buf, len, (off_t)pos); } while (n < 0 && errno == EINTR);
    return n < 0 ? pcfs_job_fail(job, PCFS_IO) : n;
}
static int out_truncate(struct erofs_vfile *vf, u64 size) {
    struct pcfs_native_job *job = job_of(vf);
    if (pcfs_job_cancelled(job)) return -EIO;
    int32_t rc = pcfs_output_truncate(job->output, size);
    return rc ? pcfs_job_fail(job, rc) : 0;
}
static int out_stat(struct erofs_vfile *vf, struct stat *st) {
    struct pcfs_native_job *job = job_of(vf);
    return fstat(job->fd, st) ? pcfs_job_fail(job, PCFS_IO) : 0;
}
static int out_sync(struct erofs_vfile *vf) {
    struct pcfs_native_job *job = job_of(vf);
    int ret;
    do { ret = fsync(job->fd); } while (ret < 0 && errno == EINTR);
    return ret ? pcfs_job_fail(job, PCFS_IO) : 0;
}
static int out_allocate(struct erofs_vfile *vf, u64 pos, u64 len, bool pad) {
    uint8_t zero[4096] = {0};
    struct stat st;
    if (pos > INT64_MAX || len > INT64_MAX - pos) return pcfs_job_fail(job_of(vf), PCFS_LIMIT);
    if (out_stat(vf, &st)) return -EIO;
    if ((uint64_t)st.st_size < pos + len && out_truncate(vf, pos + len)) return -EIO;
    if (pad) while (len) {
        size_t n = len > sizeof(zero) ? sizeof(zero) : (size_t)len;
        if (out_pwrite(vf, zero, pos, n) != (ssize_t)n) return -EIO;
        pos += n; len -= n;
    }
    return 0;
}
static ssize_t out_read(struct erofs_vfile *vf, void *buf, size_t len) {
    struct pcfs_native_job *job = job_of(vf);
    ssize_t n = out_pread(vf, buf, len, job->pos);
    if (n > 0) job->pos += (uint64_t)n;
    return n;
}
static ssize_t out_write(struct erofs_vfile *vf, void *buf, size_t len) {
    struct pcfs_native_job *job = job_of(vf);
    ssize_t n = out_pwrite(vf, buf, job->pos, len);
    if (n > 0) job->pos += (uint64_t)n;
    return n;
}
static off_t out_seek(struct erofs_vfile *vf, u64 pos, int whence) {
    if (whence != SEEK_SET || pos > INT64_MAX) return -EINVAL;
    job_of(vf)->pos = pos;
    return (off_t)pos;
}
static int out_copy(struct erofs_vfile *out, off_t pos, struct erofs_vfile *in,
                    unsigned int len, bool noseek) {
    uint8_t buf[65536];
    (void)noseek; /* input streams maintain their own cursor */
    if (pos < 0) return -EINVAL;
    while (len) {
        size_t n = len > sizeof(buf) ? sizeof(buf) : len;
        ssize_t got = erofs_io_read(in, buf, n);
        if (got <= 0 || (size_t)got > n) return -EIO;
        if (out_pwrite(out, buf, (u64)pos, (size_t)got) != got) return -EIO;
        pos += got; len -= (unsigned int)got;
    }
    return 0;
}
static void out_close(struct erofs_vfile *vf) { (void)vf; }
static struct erofs_vfops output_ops = {
    .pread = out_pread, .pwrite = out_pwrite, .pwritev = out_pwritev,
    .fstat = out_stat, .ftruncate = out_truncate, .fsync = out_sync,
    .fallocate = out_allocate, .read = out_read, .write = out_write,
    .lseek = out_seek, .xcopy = out_copy, .close = out_close,
};
void pcfs_job_attach(struct pcfs_native_job *job, struct erofs_vfile *vf) {
    memset(vf, 0, sizeof(*vf));
    vf->ops = &output_ops;
    memcpy(vf->payload, &job, sizeof(job));
}
int32_t pcfs_job_new(int fd, uint64_t limit, int32_t (*cancel)(void *),
                    void *context, struct pcfs_native_job **out) {
    if (!out) return PCFS_INVALID;
    *out = NULL;
    struct pcfs_native_job *job = calloc(1, sizeof(*job));
    if (!job) return PCFS_IO;
    job->fd = -1; job->cancel = cancel; job->context = context;
    int32_t rc = pcfs_output_new(fd, limit, &job->output);
    if (!rc) {
        job->fd = fcntl(fd, F_DUPFD_CLOEXEC, 3);
        if (job->fd < 0) rc = PCFS_IO;
    }
    if (!rc && pcfs_job_cancelled(job)) rc = job->failure;
    if (rc) { pcfs_job_destroy(job); return rc; }
    *out = job;
    return PCFS_OK;
}
int32_t pcfs_job_finish(struct pcfs_native_job *job, uint64_t *size, uint8_t digest[32]) {
    if (!job || !size || !digest) return PCFS_INVALID;
    if (pcfs_job_cancelled(job)) return job->failure;
    int32_t rc = pcfs_output_finish(job->output, size);
    if (!rc) rc = pcfs_hash_fd_cancellable(job->fd, *size, digest, job->cancel, job->context);
    return rc;
}
void pcfs_job_destroy(struct pcfs_native_job *job) {
    if (!job) return;
    pcfs_output_destroy(job->output);
    if (job->fd >= 0) close(job->fd);
    free(job);
}

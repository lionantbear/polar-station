#define _GNU_SOURCE
#include "pcfs_verity.h"
#include <errno.h>
#include <fcntl.h>
#include <linux/fsverity.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/stat.h>
#include <unistd.h>

static int do_ioctl(int fd, unsigned long request, void *arg)
{
    int ret;
    do { ret = ioctl(fd, request, arg); } while (ret < 0 && errno == EINTR);
    return ret;
}
static int32_t failure(int error, int32_t *os_error)
{
    if (os_error) *os_error = error;
    switch (error) {
    case ENODATA: return PCFS_VERITY_NOT_ENABLED;
    case ENOTTY: case EOPNOTSUPP: case ENOSYS: case ENOPKG:
        return PCFS_UNSUPPORTED;
    case EACCES: case EPERM: case EROFS: return PCFS_VERITY_DENIED;
    case EBUSY: case ETXTBSY: return PCFS_VERITY_BUSY;
    case EFBIG: case EOVERFLOW: return PCFS_LIMIT;
    default: return PCFS_IO;
    }
}
static int measure(int fd, struct pcfs_verity_digest_v1 *out)
{
    /* malloc guarantees alignment; flexible-array access follows the UAPI. */
    struct fsverity_digest *d = calloc(1, sizeof(*d) + 64);
    int ret, saved;
    if (!d) return ENOMEM;
    d->digest_size = 64;
    ret = do_ioctl(fd, FS_IOC_MEASURE_VERITY, d);
    saved = errno;
    if (ret < 0) { free(d); return saved; }
    if ((d->digest_algorithm != FS_VERITY_HASH_ALG_SHA256 || d->digest_size != 32) &&
        (d->digest_algorithm != FS_VERITY_HASH_ALG_SHA512 || d->digest_size != 64)) {
        free(d); return EOVERFLOW;
    }
    out->algorithm = d->digest_algorithm;
    out->digest_size = d->digest_size;
    memcpy(out->digest, d->digest, d->digest_size);
    free(d);
    return 0;
}
int32_t pcfs_verity_require(int32_t fd, uint32_t enable_if_missing,
    struct pcfs_verity_digest_v1 *out, int32_t *os_error)
{
    struct stat st;
    struct fsverity_enable_arg arg = {0};
    int flags, err;
    if (os_error) *os_error = 0;
    if (out) memset(out, 0, sizeof(*out));
    if (!out || fd < 0 || enable_if_missing > 1) return PCFS_INVALID;
    do { flags = fcntl(fd, F_GETFL); } while (flags < 0 && errno == EINTR);
    if (flags < 0) return failure(errno, os_error);
    if ((flags & O_ACCMODE) != O_RDONLY || (flags & O_PATH)) return PCFS_INVALID;
    do { err = fstat(fd, &st); } while (err < 0 && errno == EINTR);
    if (err < 0) return failure(errno, os_error);
    if (!S_ISREG(st.st_mode)) return PCFS_INVALID;
    err = measure(fd, out);
    if (!err) return PCFS_OK;
    if (err != ENODATA || !enable_if_missing) return failure(err, os_error);
    arg.version = 1;
    arg.hash_algorithm = FS_VERITY_HASH_ALG_SHA256;
    arg.block_size = 4096;
    if (do_ioctl(fd, FS_IOC_ENABLE_VERITY, &arg) < 0 && errno != EEXIST)
        return failure(errno, os_error);
    /* EEXIST may mean another authorized enabler won the race. In either
     * case measure again; success of enable is not proof of measurement. */
    err = measure(fd, out);
    return err ? failure(err, os_error) : PCFS_OK;
}

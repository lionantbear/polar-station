#ifndef POLAR_CAS_FS_IO_V1_H
#define POLAR_CAS_FS_IO_V1_H
/* Polar-owned positional I/O adapter. This is not an EROFS implementation.
 * Callback context remains caller-owned until source_destroy. No callback may
 * unwind, await, perform network resolution, or retain a passed buffer pointer.
 */
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif

#define PCFS_ABI_V1 1u
#define PCFS_NO_FD 1
#define PCFS_IO_MAX_TRANSFER (1024u * 1024u)
#define PCFS_SHA256_PATH_CAPACITY 83u

enum pcfs_status {
    PCFS_OK = 0, PCFS_INVALID = -1, PCFS_IO = -2,
    PCFS_CANCELLED = -3, PCFS_LIMIT = -4, PCFS_UNSUPPORTED = -5,
    PCFS_CORRUPT = -6, PCFS_NOT_FOUND = -7
};

struct pcfs_sources_v1 {
    uint32_t abi_version;
    uint32_t struct_size;
    void *context;
    /* OK transfers one newly-owned readonly regular-file descriptor to C.
     * NO_FD selects read_at. All other returns must leave fd_out at -1. */
    int32_t (*open_fd)(void *, uint64_t token, int32_t *fd_out);
    /* Short reads are allowed. Zero is EOF, NOT sparse data/implicit zeros. */
    int32_t (*read_at)(void *, uint64_t token, uint64_t offset,
                       uint8_t *buffer, size_t capacity, size_t *read_out);
    /* Return 0 to continue, nonzero to cancel. Optional. */
    int32_t (*is_cancelled)(void *);
};

struct pcfs_source;
struct pcfs_output;

/* The provider context and its immutability pin must outlive the handle.
 * FD metadata checks detect some mutations, but do NOT prove content identity. */
int32_t pcfs_source_open(const struct pcfs_sources_v1 *, uint64_t token,
                         uint64_t declared_size, struct pcfs_source **out);
int32_t pcfs_source_pread(struct pcfs_source *, uint64_t offset,
                          uint8_t *buffer, size_t capacity, size_t *read_out);
int32_t pcfs_source_pread_exact(struct pcfs_source *, uint64_t offset,
                                uint8_t *buffer, size_t length);
/* For FD input, compare size/device/inode/mtime/ctime against initial metadata.
 * Callback input remains protected by the caller's prepared-source pin. */
int32_t pcfs_source_check(struct pcfs_source *);
void pcfs_source_destroy(struct pcfs_source *);

/* Duplicates an empty regular O_RDWR descriptor (not O_APPEND). The output is
 * private to one job; caller must exclude other writers. Original FD remains
 * caller-owned. No filesystem path is used or opened by this adapter. */
int32_t pcfs_output_new(int32_t fd, uint64_t max_bytes, struct pcfs_output **out);
int32_t pcfs_output_pwrite(struct pcfs_output *, uint64_t offset,
                           const uint8_t *buffer, size_t length);
int32_t pcfs_output_truncate(struct pcfs_output *, uint64_t size);
/* Flushes and seals this HANDLE, not the underlying inode. Native image/member
 * verification, final hash and CAS registration happen in later layers. */
int32_t pcfs_output_finish(struct pcfs_output *, uint64_t *final_size);
void pcfs_output_destroy(struct pcfs_output *);

/* Canonical relative path: objects/sha256/xx/<64 lower-case hex characters>.
 * No business filename or second manifest; needs 83 bytes including NUL. */
int32_t pcfs_sha256_path(const uint8_t sha256[32], char *out, size_t capacity);
#ifdef __cplusplus
}
#endif
#endif

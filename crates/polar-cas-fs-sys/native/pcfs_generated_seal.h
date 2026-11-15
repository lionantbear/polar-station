#ifndef POLAR_CAS_GENERATED_SEAL_V1_H
#define POLAR_CAS_GENERATED_SEAL_V1_H
#include "pcfs_io.h"
#ifdef __cplusplus
extern "C" {
#endif
/* Bind a finished PRIVATE staging inode to its directory entry without
 * copying or hashing bytes. Requires a single-link, owner-private 0600 regular
 * file, opened O_RDWR and not O_APPEND. leaf is one component under directory_fd.
 * Success chmods this inode 0444, syncs it and the directory, and transfers one
 * new O_RDONLY FD. Never closes writer_fd: caller MUST close all writable FDs
 * before exposing the result. Private directory ownership excludes other
 * writers and name changes; chmod does not revoke other already-open writers.
 * This verifies inode/name/size binding, NOT bytes, fs-verity, or CAS admission.
 * Only a trusted builder may associate its computed digest with this result.
 * On error read_only_fd is -1; a late error may leave private staging read-only.
 */
int32_t pcfs_generated_seal(int32_t writer_fd, int32_t directory_fd,
                            const char *leaf, uint64_t expected_size,
                            int32_t *read_only_fd);
#ifdef __cplusplus
}
#endif
#endif

#ifndef PCFS_MOUNT_H
#define PCFS_MOUNT_H
#include "pcfs_verity.h"
#ifdef __cplusplus
extern "C" {
#endif
/* Return an OWNED detached mount fd. This does not attach over any path and
 * does not separately mount any P. Borrow all input fds for this synchronous
 * call. They must already have been bound to exact {M, ordered P} identities
 * by the trusted Host verifier. Verity is required again, never enabled here.
 * Caller retains all image leases until the detached mount fd is closed.
 * allow_exec is 0/1; readonly,nosuid,nodev always remain enabled.
 * Success is NOT a published path, reboot-persistent mount, or GC receipt.
 */
int32_t pcfs_native_mount_detached(int32_t meta_fd, const int32_t *pack_fds,
    size_t pack_count, uint32_t allow_exec, int32_t *mount_fd, int32_t *os_error);
#ifdef __cplusplus
}
#endif
#endif

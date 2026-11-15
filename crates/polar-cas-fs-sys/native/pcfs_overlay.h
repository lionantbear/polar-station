#ifndef PCFS_OVERLAY_H
#define PCFS_OVERLAY_H
#include "pcfs_mount.h"
#ifdef __cplusplus
extern "C" {
#endif
#define PCFS_OVERLAY_MAX_DATADIRS 128u
/* Unsafe low-level mount primitive: caller already verified the Hybrid Tree,
 * native M/P and EVERY required redirect in the ordered immutable providers.
 * Readonly FD alone does NOT prove a directory namespace is frozen.
 * All input directory FDs/leases are borrowed. Return one OWNED detached mount
 * FD; this does not publish a path or provide reboot/cross-process GC safety.
 * upper_fd/work_fd must both be -1 (readonly) or exclusive directories on the
 * same filesystem. Linux FD-based layer configuration is required; no pathname
 * fallback and no shell command. nodedev/nosuid always, exec is explicit. */
int32_t pcfs_overlay_mount_detached(int32_t meta_root_fd,const int32_t *data_fds,
    size_t data_count,int32_t upper_fd,int32_t work_fd,uint32_t allow_exec,
    int32_t *mount_fd,int32_t *os_error);
/* Pin-preserving hardlink from a verified file FD into a private generation.
 * Does not chmod the source. The caller proved digest/size AFTER fs-verity and
 * retains a content lease; this primitive checks verity and safe path layout.
 * Cross-filesystem linkage fails explicitly, never silently copies or symlinks.
 * generation root must be caller-owned/private and not yet a mounted lower. */
int32_t pcfs_frozen_link_content(int32_t generation_root_fd,int32_t source_fd,
    const uint8_t sha256[32],uint64_t size_bytes,int32_t *os_error);
/* Set readonly on the SAME overlay mount; open writers fail EBUSY. syncfs
 * completes before success. Does not unseal lower or manipulate raw upper. */
int32_t pcfs_overlay_readonly_v1(int32_t mount_fd, int32_t *os_error);
#ifdef __cplusplus
}
#endif
#endif

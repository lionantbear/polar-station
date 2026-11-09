#ifndef PCFS_VERITY_H
#define PCFS_VERITY_H
#include "pcfs_io.h"
#ifdef __cplusplus
extern "C" {
#endif
#define PCFS_VERITY_NOT_ENABLED (-8)
#define PCFS_VERITY_DENIED (-9)
#define PCFS_VERITY_BUSY (-10)
/* This is the Linux fs-verity file digest, NOT a content SHA256. */
struct pcfs_verity_digest_v1 {
    uint32_t algorithm;
    uint32_t digest_size;
    uint8_t digest[64];
};
/* Borrow an O_RDONLY regular fd, never reopen a pathname. enable_if_missing
 * is explicitly 0 or 1. No fallback to hash-only. On failure out is zeroed
 * and os_error is the preserved errno, if supplied. Success only measures
 * verity: the caller MUST still bind actual bytes to the expected ContentRef.
 * Enabling is irreversible on this inode and requires owner authorization.
 * No signature/keyring changes, mount, filesystem formatting or implicit copy.
 */
int32_t pcfs_verity_require(int32_t fd, uint32_t enable_if_missing,
    struct pcfs_verity_digest_v1 *out, int32_t *os_error);
#ifdef __cplusplus
}
#endif
#endif

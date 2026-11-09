#ifndef POLAR_CAS_FILESYSTEM_PACK_V1_H
#define POLAR_CAS_FILESYSTEM_PACK_V1_H
#include "pcfs_native_meta.h"
#include "pcfs_verified_sources.h"
#ifdef __cplusplus
extern "C" {
#endif

/* Standalone ordinary-path filesystem, not the existing hash-path CAS pack,
 * and not M referencing external P. Existing ABI/profile numbers are unchanged.
 * No root entry: root metadata belongs to options. No mounts or source paths. */
struct pcfs_filesystem_options_v1 {
    uint32_t abi_version, struct_size, compression, root_mode, uid, gid;
    uint64_t max_output_bytes, max_input_bytes, max_entries, max_path_bytes;
};
struct pcfs_filesystem_entry_v1 {
    uint32_t abi_version, struct_size;
    const char *path;
    uint64_t path_bytes;                 /* exact span, no embedded NUL */
    const char *symlink_target;
    uint64_t symlink_bytes;
    uint8_t sha256[32];                   /* FILE only */
    uint64_t size_bytes, source_token;    /* FILE only */
    uint32_t kind, mode, reserved;
};
struct pcfs_filesystem_result_v1 {
    uint8_t sha256[32];
    uint64_t size_bytes, entries, files, input_bytes;
};

/* Sources must stay valid for this synchronous call. Regular-file expected
 * SHA/length are audited against decoded output (raw claims are not trusted).
 * Result is zero on ALL failure paths. A partially written fd must be discarded.
 * Success does not publish or seal the inode, or admit it to Polar CAS. */
int32_t pcfs_erofs_build_tree_v1(const struct pcfs_filesystem_options_v1 *,
    const struct pcfs_sources_v1 *, const struct pcfs_filesystem_entry_v1 *,
    size_t count, int32_t output_fd, struct pcfs_filesystem_result_v1 *);

/* Reuses the opaque identified-source gate: source_token is its source index.
 * Expected SHA/length must match its identities, and every encoding read is
 * checked by that gate. No duplicate full decoded payload audit. Not a public
 * skip-verification flag; namespace/output checks remain compulsory. */
int32_t pcfs_erofs_build_tree_verified_v1(const struct pcfs_filesystem_options_v1 *,
    struct pcfs_verified_sources *, const struct pcfs_filesystem_entry_v1 *,
    size_t count, int32_t output_fd, struct pcfs_filesystem_result_v1 *);

/* Structural/budget checks only, without opening any input or output. */
int32_t pcfs_filesystem_validate_v1(const struct pcfs_filesystem_options_v1 *,
    const struct pcfs_filesystem_entry_v1 *, size_t count);
const char *pcfs_filesystem_fingerprint_v1(void);
#ifdef __cplusplus
}
#endif
#endif

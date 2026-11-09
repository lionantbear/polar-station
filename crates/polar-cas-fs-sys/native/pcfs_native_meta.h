#ifndef PCFS_NATIVE_META_H
#define PCFS_NATIVE_META_H
#include "pcfs_erofs.h"
#ifdef __cplusplus
extern "C" {
#endif

#define PCFS_NATIVE_META_PROFILE_V1 1u
#define PCFS_HYBRID_META_PROFILE_V2 2u
/* Regular file with OverlayFS metacopy markers, not a new disk inode type. */
#define PCFS_META_REDIRECT 4u
#define PCFS_META_MAX_PACKS 64u
#define PCFS_META_DIRECTORY 1u
#define PCFS_META_FILE 2u
#define PCFS_META_SYMLINK 3u
#define PCFS_META_PATH_MAX 4095u

struct pcfs_meta_options_v1 {
    uint32_t abi_version, struct_size, profile, root_mode;
    uint32_t uid, gid;
    uint64_t max_output_bytes, max_entries, max_path_bytes, max_content_bytes;
};
/* Explicit logical Tree entries. No root entry: root attributes are options.
 * Parents must be explicit DIRECTORY entries. Input order is irrelevant.
 * path is canonical, root-relative, UTF-8 supplied by the Rust Tree adapter.
 * FILE refers to an ordered pack slot + whole content digest, NOT an offset.
 * Symlinks are stored as data; never followed during compilation/verification.
 * mode only holds permission bits (0000..0777); setuid/devices/xattrs are not
 * supported by this bounded v1 adapter. No implicit same-content hardlinks. */
struct pcfs_meta_entry_v1 {
    const char *path;
    const char *symlink_target;
    uint8_t sha256[32];
    uint64_t size_bytes;
    uint32_t kind, pack_index, mode, reserved;
};
struct pcfs_meta_result_v1 {
    uint8_t sha256[32];
    uint64_t size_bytes, entries, files, dependencies;
};
/* Internal layout checking without exposing blocks/extents in Polar's model.
 * Checks global profile + the selected member's physical placement. */
int32_t pcfs_image_check_native_member(struct pcfs_image *, const uint8_t sha256[32],
                                       uint64_t size_bytes);
/* All packs must be previously verified immutable standalone images. Pointers
 * are borrowed synchronously; no path or network callback is used. On failure
 * result is zero and output MUST be discarded. On success all output metadata
 * and decoded files have been checked using the external device order.
 * The caller persists {TreeRef, MetaRef, ordered PackRefs, profile, compiler}.
 * Changing a pack's bytes/order requires recompilation. */
int32_t pcfs_native_meta_build(const struct pcfs_meta_options_v1 *,
    struct pcfs_image *const *packs, size_t pack_count,
    const struct pcfs_meta_entry_v1 *, size_t count, int32_t output_fd,
    int32_t (*is_cancelled)(void *), void *cancel_context,
    struct pcfs_meta_result_v1 *result);
/* Trusted construction/catalog adapter ONLY, not an untrusted skip flag.
 * Requires admitted PackRefs and verified membership from the source-gated
 * builder or explicit import audit. Checks metadata/layout and namespace
 * without decoding/re-hashing member payloads. */
int32_t pcfs_meta_build_from_trusted_members(const struct pcfs_meta_options_v1 *,
    struct pcfs_image *const *packs, size_t pack_count,
    const struct pcfs_meta_entry_v1 *, size_t count, int32_t output_fd,
    int32_t (*is_cancelled)(void *), void *cancel_context,
    struct pcfs_meta_result_v1 *result);
/* Verify M's whole content digest and each device-table slot's full pack digest.
 * Each supplied pack stays pinned by this image; they are NEVER mounted here.
 * Swapped, missing, duplicated or repacked dependencies are rejected. */
int32_t pcfs_native_meta_open(int32_t fd, const uint8_t sha256[32], uint64_t size,
    struct pcfs_image *const *packs, size_t pack_count, struct pcfs_image **out);
/* path is not followed through a symlink. The expected SHA/size are supplied by
 * the authoritative Polar Tree. Returned member pins M and its pack closure. */
int32_t pcfs_native_meta_member(struct pcfs_image *, const char *path,
    const uint8_t sha256[32], uint64_t size, struct pcfs_member **out);
struct pcfs_meta_stat_v1 {
    uint64_t size_bytes;
    uint32_t kind, mode, uid, gid;
};
int32_t pcfs_native_meta_stat(struct pcfs_image *, const char *, struct pcfs_meta_stat_v1 *);
int32_t pcfs_native_meta_readlink(struct pcfs_image *, const char *,
    char *buffer, size_t capacity, size_t *length);
/* Includes root; bounded to maximum entries, refuses cycles by budget. */
int32_t pcfs_native_meta_count_entries(struct pcfs_image *, uint64_t maximum, uint64_t *out);
/* Hybrid handles whitelist exactly metacopy(empty) + canonical redirect.
 * Native v1 handles continue rejecting these marker inodes. */
int32_t pcfs_hybrid_meta_open(int32_t fd, const uint8_t sha256[32], uint64_t size,
    struct pcfs_image *const *packs, size_t count, struct pcfs_image **out);
/* Exact just-generated/managed M only; same metadata checks without a second
 * image hash. Caller retains the trusted immutable output and pack closure. */
/* Only an exact admitted immutable lease may call this. Zero external devices;
 * metadata validation, no new blob/member digest evidence. */
int32_t pcfs_filesystem_open_managed(int32_t fd, const uint8_t sha256[32],
    uint64_t size, struct pcfs_image **out);
int32_t pcfs_hybrid_meta_open_managed(int32_t fd, const uint8_t sha256[32], uint64_t size,
    struct pcfs_image *const *packs, size_t count, struct pcfs_image **out);
int32_t pcfs_hybrid_meta_redirect(struct pcfs_image *, const char *path,
    uint8_t sha256[32], uint64_t *size_bytes);
const char *pcfs_native_meta_fingerprint(void);
/* Adapter-only native source inspection. UINT32_MAX for an empty regular
 * native file (no extents). Reject redirects or mixed/inline payload mappings.
 * Never serialize the returned slot into the Polar Catalog. */
int32_t pcfs_native_meta_source_pack(struct pcfs_image *, const char *path, uint32_t *pack_index);
const char *pcfs_hybrid_meta_fingerprint(void);
#ifdef __cplusplus
}
#endif
#endif

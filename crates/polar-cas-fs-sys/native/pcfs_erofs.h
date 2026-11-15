#ifndef POLAR_CAS_FS_EROFS_V1_H
#define POLAR_CAS_FS_EROFS_V1_H
#include "pcfs_io.h"
#ifdef __cplusplus
extern "C" {
#endif

#define PCFS_EROFS_PROFILE_V1 1u /* legacy, may inline regular payload */
#define PCFS_EROFS_PROFILE_NATIVE_V1 2u /* externally referenceable raw/LZ4 */
#define PCFS_EROFS_PROFILE_BUILD_V1 3u /* native layout + construction summary; gated v2 API only */
#define PCFS_EROFS_COMPRESSION_NONE 0u
#define PCFS_EROFS_COMPRESSION_LZ4 1u
#define PCFS_EROFS_MAX_OBJECTS 1000000u

struct pcfs_pack_options_v1 {
    uint32_t abi_version, struct_size;
    uint32_t profile, compression;
    uint64_t max_output_bytes, max_input_bytes, max_objects;
};
struct pcfs_pack_entry_v1 {
    uint8_t sha256[32];
    uint64_t size_bytes, source_token;
};
struct pcfs_pack_result_v1 {
    uint8_t sha256[32];
    uint64_t size_bytes, unique_objects, input_bytes;
};

/* Synchronous, one job. No borrowed pointer escapes the call. open_fd/read_at
 * are called on demand, NOT once for all entries before writing. Entries are
 * sorted/deduplicated by digest; conflicting lengths are rejected. The output
 * must be an empty, exclusively owned regular O_RDWR file. On ANY failure it
 * is not a CAS object and must be discarded. Result is zeroed on failure.
 * Success means every decoded member AND final image SHA256 were verified;
 * publication, filesystem immutability and Catalog commit are NOT performed.
 * Do not re-enter a build from a provider callback. Native reads may re-enter.
 */
int32_t pcfs_erofs_build(const struct pcfs_pack_options_v1 *,
    const struct pcfs_sources_v1 *, const struct pcfs_pack_entry_v1 *,
    size_t count, int32_t output_fd, struct pcfs_pack_result_v1 *result);

struct pcfs_image;
struct pcfs_member;
/* Always checks the whole image digest and size before parsing. A read-only FD
 * is duplicated and pinned. Caller must retain the immutable backing lease;
 * readonly is not proof that another FD/process cannot mutate the inode. */
int32_t pcfs_image_open(int32_t fd, const uint8_t expected_sha256[32],
                        uint64_t expected_size, struct pcfs_image **out);
/* Trusted ownership adapter ONLY: the caller already admitted/verified exact
 * bytes and retains their immutable managed-store or sealed-snapshot lease. The expected digest is the
 * admitted identity, not a new proof. Checks FD, size, format and metadata;
 * does not hash/decode the entire pack. Never use for untrusted imports. */
int32_t pcfs_image_open_managed(int32_t fd, const uint8_t expected_sha256[32],
    uint64_t expected_size, struct pcfs_image **out);
/* Process-wide cumulative diagnostic counters, updated atomically. A snapshot
 * is field-wise; exact deltas require observing completion of all measured work.
 * image_pread counts successful main-device pread bytes (metadata and selected
 * payload); external-device reads and OS readahead are not included. Whole
 * hashes include finalized outputs; open hashes distinguish strict reopen. */
struct pcfs_io_counters_v1 {
    uint64_t whole_image_hash_calls, whole_image_hash_bytes;
    uint64_t image_open_hash_calls, image_open_hash_bytes;
    uint64_t member_verify_calls, member_verify_bytes;
    uint64_t member_read_calls, member_read_bytes;
    uint64_t image_pread_calls, image_pread_bytes;
};
int32_t pcfs_io_counters_read(struct pcfs_io_counters_v1 *out);
void pcfs_image_destroy(struct pcfs_image *);
/* The member keeps the image alive, even after image_destroy. Only canonical
 * hash paths and regular files are exposed. Not a general path traversal API. */
int32_t pcfs_image_member(struct pcfs_image *, const uint8_t sha256[32],
                         uint64_t expected_size, struct pcfs_member **out);
/* SHA-only lookup: length is read from the native inode, not a second index.
 * This does not verify the decoded payload: consume with a content verifier.
 * All supplied outputs are zeroed on failure. */
int32_t pcfs_image_member_by_digest(struct pcfs_image *, const uint8_t sha256[32],
    uint64_t *size_out, struct pcfs_member **out);
/* Lists canonical members, without claiming their payload hashes are valid. */
struct pcfs_pack_member_v1 { uint8_t sha256[32]; uint64_t size_bytes; };
/* At most 1024 already-admitted members; no payload/extent audit. Global
 * failure leaves all handles NULL. Item statuses own independent successes.
 * Canonical parent directory types are checked once per immutable image. */
int32_t pcfs_image_members(struct pcfs_image *, const struct pcfs_pack_member_v1 *,
    size_t count, int32_t *statuses, struct pcfs_member **out);
int32_t pcfs_pack_members(struct pcfs_image *, struct pcfs_pack_member_v1 *,
    size_t capacity, size_t *count, int32_t (*cancel)(void *), void *context);
/* Clone parsed metadata into an independent handle; no file I/O. The caller
 * retains the immutable backing contract, just as for the original handle. */
int32_t pcfs_member_clone(struct pcfs_member *, struct pcfs_member **out);
/* Owned handles can move between threads. References are atomic; mutable
 * directory checks and lazy member inode fields use object-local mutexes.
 * Independent images/members never take the encoder's process-wide lock.
 * A handle must not be destroyed concurrently with a call using that handle. */
int32_t pcfs_member_pread(struct pcfs_member *, uint64_t offset,
                         uint8_t *buffer, size_t capacity, size_t *read_out);
int32_t pcfs_member_verify(struct pcfs_member *);
int32_t pcfs_verify_member_cancellable(struct pcfs_member *, int32_t (*cancel)(void *), void *context);
void pcfs_member_destroy(struct pcfs_member *);
const char *pcfs_erofs_fingerprint(void);

#ifdef __cplusplus
}
#endif
#endif

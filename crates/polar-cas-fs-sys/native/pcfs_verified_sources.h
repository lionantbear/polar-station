#ifndef POLAR_PCFS_VERIFIED_SOURCES_V1_H
#define POLAR_PCFS_VERIFIED_SOURCES_V1_H
#include "pcfs_erofs.h"
#ifdef __cplusplus
extern "C" {
#endif

/* Ephemeral construction input, NOT a CAS admission or a persisted chunk store.
 * Pass A identifies the original source. Pass B authenticates whole source
 * blocks BEFORE any byte is handed to the encoder. Random access, rereads and
 * compression fallback do not corrupt a sequential hash accumulator.
 *
 * All calls are synchronous and single-owner. The original callbacks/context
 * must remain alive until destroy; a callback must not re-enter this handle.
 * Only one original input is open at a time. No payload scratch file is made.
 */
struct pcfs_verified_sources;
struct pcfs_source_spec_v1 { uint64_t source_token, size_bytes; };
struct pcfs_verify_options_v1 {
    uint32_t abi_version, struct_size;
    uint32_t block_bytes, reserved; /* power of two, 4096 .. 1 MiB */
    uint64_t max_sources, max_input_bytes, max_index_bytes;
};
struct pcfs_source_identity_v1 { uint8_t sha256[32]; uint64_t size_bytes; };
struct pcfs_verified_stats_v1 {
    uint64_t identity_bytes, encoding_source_bytes, delivered_bytes;
    uint64_t verified_blocks, cache_hits, source_opens, peak_open_sources;
    uint64_t index_bytes, cache_bytes;
};
int32_t pcfs_verified_sources_identify(const struct pcfs_verify_options_v1 *,
    const struct pcfs_sources_v1 *, const struct pcfs_source_spec_v1 *,
    size_t count, struct pcfs_verified_sources **out);
int32_t pcfs_verified_sources_identity(const struct pcfs_verified_sources *,
    size_t index, struct pcfs_source_identity_v1 *out);
/* Short reads at a block boundary are deliberate. On any failure, read_out is
 * zero and the caller's buffer is NOT changed. Authentication failure poisons
 * the source set; restoring the source bytes does not clear the failure. */
int32_t pcfs_verified_sources_pread(struct pcfs_verified_sources *, size_t index,
    uint64_t offset, uint8_t *buffer, size_t capacity, size_t *read_out);
int32_t pcfs_verified_sources_stats(const struct pcfs_verified_sources *,
    struct pcfs_verified_stats_v1 *out);
void pcfs_verified_sources_destroy(struct pcfs_verified_sources *);

/* Read at most 1024 bytes of the embedded self-description. The image handle
 * authenticates the image identity, NOT the producer's semantic claims. This
 * helper does not convert an unknown pack into trusted construction evidence.
 */
int32_t pcfs_pack_summary_read(struct pcfs_image *, char *buffer,
    size_t capacity, size_t *length);

/* Explicit safe build entry, not a skip_verify flag. Only an opaque identified
 * source set can enter it. Success authenticates bytes supplied to the trusted
 * upstream encoder, writes the embedded summary, then hashes the final image.
 * It does NOT decode/re-hash output members, publish CAS replicas, authenticate
 * another producer, or prove an arbitrary codec free of defects. Legacy
 * pcfs_erofs_build keeps its original full decoded-member audit contract.
 */
struct pcfs_pack_build_result_v2 {
    struct pcfs_pack_result_v1 pack;
    uint8_t members_digest[32];
    uint64_t identity_bytes, encoding_source_bytes, delivered_bytes;
    uint64_t verified_blocks, cache_hits;
};
int32_t pcfs_erofs_build_verified(const struct pcfs_pack_options_v1 *,
    struct pcfs_verified_sources *, int32_t output_fd,
    struct pcfs_pack_build_result_v2 *result);
/* The runtime can identify once, query CAS, then pack only the selected new
 * source indices. Indices cannot change digests or sizes; duplicates dedupe.
 * A count of zero means an empty selection, not the entire input set. */
int32_t pcfs_erofs_build_verified_selected(const struct pcfs_pack_options_v1 *,
    struct pcfs_verified_sources *, const size_t *indices, size_t count,
    int32_t output_fd, struct pcfs_pack_build_result_v2 *result);
#ifdef __cplusplus
}
#endif
#endif

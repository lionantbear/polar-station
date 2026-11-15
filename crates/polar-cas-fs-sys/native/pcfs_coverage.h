#ifndef POLAR_CAS_FS_COVERAGE_H
#define POLAR_CAS_FS_COVERAGE_H
#include "pcfs_erofs.h"
#ifdef __cplusplus
extern "C" {
#endif
#define PCFS_COVERAGE_MAX_PROVIDERS 128u
struct pcfs_required_content_v1 { uint8_t sha256[32]; uint64_t size_bytes; };
struct pcfs_coverage_result_v1 { uint64_t covered, probes, verified_bytes; };
/* Requirements are unique, lexicographically sorted full SHA256 values.
 * Providers are previously verified standalone packs, not M or another device
 * graph. For every requirement all present copies are decoded and hash-checked,
 * including shadowed providers. Union coverage must be complete; unused and
 * duplicate image identities are rejected. No Catalog writes or kernel mounts.
 * max_probes bounds the product of counts; max_verify_bytes includes all present
 * copies (not just selected ones). On failure result is zero; selected[] is
 * reset to UINT32_MAX when counts are valid. Invalid/oversized counts are
 * rejected before any caller-sized array is touched.
 * selected[] has requirement_count entries and receives ephemeral provider
 * indices only on success. These are NOT EROFS device IDs or durable mappings.
 * Callers must retain immutable backing leases; handles are synchronously
 * borrowed and cannot be concurrently destroyed. Cancellation callbacks must
 * not perform IO, Catalog work, or destroy/re-enter the input handles. */
int32_t pcfs_pack_verify_coverage(struct pcfs_image *const *providers,
    size_t provider_count, const struct pcfs_required_content_v1 *requirements,
    size_t requirement_count, uint64_t max_probes, uint64_t max_verify_bytes,
    int32_t (*is_cancelled)(void *), void *cancel_context,
    uint32_t *selected, struct pcfs_coverage_result_v1 *result);
#ifdef __cplusplus
}
#endif
#endif

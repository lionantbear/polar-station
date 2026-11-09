#include "pcfs_coverage.h"
#include "pcfs_erofs_private.h"
#include <stdlib.h>
#include <string.h>
#include <limits.h>

int32_t pcfs_pack_verify_coverage(struct pcfs_image *const *providers,
    size_t provider_count, const struct pcfs_required_content_v1 *requirements,
    size_t count, uint64_t max_probes, uint64_t max_verify_bytes,
    int32_t (*is_cancelled)(void *), void *context,
    uint32_t *selected, struct pcfs_coverage_result_v1 *result) {
    if (result) memset(result, 0, sizeof(*result));
    /* Validate count before touching an untrusted caller-sized output array. */
    if (count > PCFS_EROFS_MAX_OBJECTS || provider_count > PCFS_COVERAGE_MAX_PROVIDERS)
        return PCFS_LIMIT;
    if (selected) for (size_t i = 0; i < count; ++i) selected[i] = UINT32_MAX;
    if (!result || (count && (!requirements || !selected)) ||
        (provider_count && !providers)) return PCFS_INVALID;
    if (provider_count && !count) return PCFS_INVALID;
    if ((uint64_t)provider_count * count > max_probes) return PCFS_LIMIT;
    for (size_t i = 0; i < count; ++i) {
        if (requirements[i].size_bytes > INT64_MAX) return PCFS_LIMIT;
        if (i && memcmp(requirements[i-1].sha256, requirements[i].sha256, 32) >= 0)
            return PCFS_INVALID;
    }
    if (is_cancelled && is_cancelled(context)) return PCFS_CANCELLED;
    if (!count) return PCFS_OK;
    if (!provider_count) return PCFS_NOT_FOUND;
    uint32_t *choices = malloc(count * sizeof(*choices));
    if (!choices) return PCFS_IO;
    bool used[PCFS_COVERAGE_MAX_PROVIDERS] = {false};
    struct pcfs_coverage_result_v1 progress = {0};
    int32_t rc = pcfs_native_lock();
    if (rc) { free(choices); return rc; }
    for (size_t j = 0; j < provider_count; ++j) {
        if (!providers[j]) { rc = PCFS_INVALID; goto done; }
        if (providers[j]->projection || providers[j]->backing_count) {
            rc = PCFS_UNSUPPORTED; goto done;
        }
        rc = pcfs_image_unchanged(providers[j]);
        if (rc) goto done;
        for (size_t k = 0; k < j; ++k) {
            if (!memcmp(providers[k]->digest, providers[j]->digest, 32)) {
                rc = PCFS_INVALID; goto done;
            }
        }
    }
    for (size_t i = 0; i < count; ++i) {
        choices[i] = UINT32_MAX;
        for (size_t j = 0; j < provider_count; ++j) {
            if (is_cancelled && is_cancelled(context)) { rc = PCFS_CANCELLED; goto done; }
            struct pcfs_member *member = NULL;
            ++progress.probes;
            rc = pcfs_image_member(providers[j], requirements[i].sha256,
                                   requirements[i].size_bytes, &member);
            if (rc == PCFS_NOT_FOUND) { rc = PCFS_OK; continue; }
            if (rc) goto done;
            if (requirements[i].size_bytes > max_verify_bytes - progress.verified_bytes) {
                pcfs_member_destroy(member); rc = PCFS_LIMIT; goto done;
            }
            rc = pcfs_verify_member_cancellable(member, is_cancelled, context);
            pcfs_member_destroy(member);
            if (rc) goto done;
            progress.verified_bytes += requirements[i].size_bytes;
            used[j] = true;
            if (choices[i] == UINT32_MAX) choices[i] = (uint32_t)j;
        }
        if (choices[i] == UINT32_MAX) { rc = PCFS_NOT_FOUND; goto done; }
        ++progress.covered;
    }
    for (size_t j = 0; j < provider_count; ++j) {
        if (!used[j]) { rc = PCFS_INVALID; goto done; }
        rc = pcfs_image_unchanged(providers[j]);
        if (rc) goto done;
    }
    if (is_cancelled && is_cancelled(context)) { rc = PCFS_CANCELLED; goto done; }
    memcpy(selected, choices, count * sizeof(*choices));
    *result = progress;
done:
    pcfs_native_unlock();
    free(choices);
    return rc;
}

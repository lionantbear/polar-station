#ifndef POLAR_PCFS_PACK_PLAN_H
#define POLAR_PCFS_PACK_PLAN_H
#include "pcfs_erofs.h"
/* Pure validation/planning: no EROFS dependency and no content I/O. */
int32_t pcfs_pack_plan(const struct pcfs_pack_options_v1 *,
    const struct pcfs_pack_entry_v1 *, size_t count,
    struct pcfs_pack_entry_v1 **sorted_unique, size_t *unique_count,
    uint64_t *total_bytes);
#endif

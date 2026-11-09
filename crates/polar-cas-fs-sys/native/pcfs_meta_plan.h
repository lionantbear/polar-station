#ifndef PCFS_META_PLAN_H
#define PCFS_META_PLAN_H
#include "pcfs_native_meta.h"
struct pcfs_meta_planned {
    const struct pcfs_meta_entry_v1 *entry;
    size_t parent; /* index in sorted plan, SIZE_MAX for root */
};
int32_t pcfs_meta_plan(const struct pcfs_meta_options_v1 *,
    const struct pcfs_meta_entry_v1 *, size_t count, size_t pack_count,
    struct pcfs_meta_planned **out);
int32_t pcfs_meta_validate_path(const char *path);
#endif

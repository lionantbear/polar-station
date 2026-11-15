#ifndef POLAR_CAS_FILESYSTEM_PLAN_H
#define POLAR_CAS_FILESYSTEM_PLAN_H
#include "pcfs_filesystem_pack.h"
#include "pcfs_meta_plan.h"
struct pcfs_filesystem_plan {
    struct pcfs_meta_options_v1 metadata;
    struct pcfs_meta_entry_v1 *entries;       /* owns copied path/target strings */
    struct pcfs_meta_planned *ordered;       /* same existing path/parent rules */
    struct pcfs_pack_entry_v1 *sources;      /* indexed by original entry */
    size_t count, files;
    uint64_t input_bytes;
};
int32_t pcfs_filesystem_plan(const struct pcfs_filesystem_options_v1 *,
    const struct pcfs_filesystem_entry_v1 *, size_t,
    struct pcfs_filesystem_plan *);
void pcfs_filesystem_plan_destroy(struct pcfs_filesystem_plan *);
#endif

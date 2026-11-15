#ifndef PCFS_VERIFIED_SOURCES_PRIVATE_H
#define PCFS_VERIFIED_SOURCES_PRIVATE_H
#include "pcfs_verified_sources.h"
/* Internal borrowed plan, only while the opaque set remains alive. */
const struct pcfs_pack_entry_v1 *pcfs_verified_entries(
    const struct pcfs_verified_sources *, size_t *count);
int32_t pcfs_verified_begin(struct pcfs_verified_sources *, struct pcfs_sources_v1 *);
int32_t pcfs_verified_end(struct pcfs_verified_sources *);
#endif

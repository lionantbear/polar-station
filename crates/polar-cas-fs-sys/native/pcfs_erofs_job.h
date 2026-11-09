#ifndef PCFS_EROFS_JOB_H
#define PCFS_EROFS_JOB_H
#include "pcfs_erofs_private.h"

/* Shared bounded seekable output for packs and native metadata. Synchronous;
 * it never owns callbacks beyond the call, never publishes or mutates a pack. */
struct pcfs_native_job;
int32_t pcfs_job_new(int fd, uint64_t limit, int32_t (*cancel)(void *),
                    void *context, struct pcfs_native_job **out);
void pcfs_job_attach(struct pcfs_native_job *, struct erofs_vfile *);
int pcfs_job_cancelled(struct pcfs_native_job *);
int pcfs_job_fail(struct pcfs_native_job *, int32_t status);
int32_t pcfs_job_status(const struct pcfs_native_job *);
int pcfs_job_fd(const struct pcfs_native_job *);
int32_t pcfs_job_finish(struct pcfs_native_job *, uint64_t *size, uint8_t digest[32]);
void pcfs_job_destroy(struct pcfs_native_job *);
#endif

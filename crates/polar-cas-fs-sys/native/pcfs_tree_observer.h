#ifndef POLAR_CAS_LOCAL_TREE_OBSERVER_V1_H
#define POLAR_CAS_LOCAL_TREE_OBSERVER_V1_H
#include "pcfs_io.h"
#include "pcfs_verity.h"
#ifdef __cplusplus
extern "C" {
#endif

#define PCFS_TREE_DIRECTORY 1u
#define PCFS_TREE_FILE 2u
#define PCFS_TREE_SYMLINK 3u
#define PCFS_TREE_NO_SOURCE UINT64_MAX

struct pcfs_tree_options_v1 {
    uint32_t abi_version, struct_size;
    uint32_t max_depth, max_directory_fds;
    uint64_t max_entries, max_sources, max_input_bytes, max_path_bytes;
};
/* Cancellation context is borrowed until tree_destroy. Never unwind/await. */
struct pcfs_tree_cancel_v1 {
    void *context;
    int32_t (*is_cancelled)(void *);
};
struct pcfs_tree_entry_v1 {
    /* Borrowed UTF-8-unchecked POSIX names, valid until tree_destroy. The Rust
     * Tree adapter performs its existing portable name/symlink validation. */
    const char *path, *symlink_target;
    uint64_t size_bytes, source_token;
    uint32_t kind, mode;
};
struct pcfs_tree_stats_v1 {
    uint64_t entry_count, file_count, directory_count, symlink_count;
    uint64_t input_bytes, path_bytes, directory_walks, source_opens;
    uint32_t peak_directory_fds, reserved;
};
struct pcfs_observed_tree;

/* Duplicates the directory capability. Never resolves an ambient absolute path.
 * No payload reads, writes, hardlinks or CAS registration are performed here. */
int32_t pcfs_tree_observe(int32_t root_fd,
    const struct pcfs_tree_options_v1 *, const struct pcfs_tree_cancel_v1 *,
    struct pcfs_observed_tree **out);
int32_t pcfs_tree_entry(const struct pcfs_observed_tree *, size_t index,
    struct pcfs_tree_entry_v1 *out);
int32_t pcfs_tree_stats(const struct pcfs_observed_tree *, struct pcfs_tree_stats_v1 *out);
/* Transfer one NEW readonly FD to caller; bounds and all ancestor identities are
 * checked. Use these FDs with existing VerifiedReadAt, not as digest evidence. */
int32_t pcfs_tree_open_source(struct pcfs_observed_tree *, uint64_t token, int32_t *out);
int32_t pcfs_tree_source_node(const struct pcfs_observed_tree *, uint64_t token,
    size_t *entry_index);
/* Checks observed namespace metadata (no second readdir); this is not an
 * atomic snapshot or payload proof. Errors permanently poison this handle. */
int32_t pcfs_tree_revalidate(struct pcfs_observed_tree *);
/* Provider context borrows tree. Destroy IdentifiedSources before the tree. */
int32_t pcfs_tree_source_provider(struct pcfs_observed_tree *, struct pcfs_sources_v1 *out);
/* Strict immutable-directory backend. Reuses this bounded observation.
 * REQUIRE never mutates; ENABLE requires explicit ownership handoff and
 * authority to set immutable directory flags. Regular files must be verity
 * protected AND immutable. No chmod/flock/readonly-fd fallback.
 * All nodes must be on the same mount; non-directory hardlinks are rejected.
 * Failure may leave a sealed prefix (including irreversible file verity);
 * the caller must preserve it for explicit recovery, NEVER auto-unlock.
 * report is always initialized. A successful call is not content admission. */
#define PCFS_TREE_LOCK_REQUIRED (-11)
struct pcfs_tree_lock_report_v1 {
    uint64_t checked_nodes, changed_nodes;
    int32_t os_error;
    uint32_t reserved;
};
int32_t pcfs_tree_lock_v1(struct pcfs_observed_tree *, uint32_t enable,
    struct pcfs_tree_lock_report_v1 *report);
/* Persistable *observation*, not a capability or CAS-content proof. Fields are
 * explicit ABI types; do not serialize the C struct or include padding in hashes.
 * Requires an entirely locked tree and a current identity check. No payload IO.
 * mount IDs are deliberately not persisted: they change across mounts/boots. */
struct pcfs_tree_seal_entry_v1 {
    uint64_t device, inode, link_count, size_bytes;
    int64_t mtime_seconds, ctime_seconds;
    uint32_t kind, mode, uid, gid, mtime_nanos, ctime_nanos;
    struct pcfs_verity_digest_v1 verity;
};
int32_t pcfs_tree_seal_entry_v1(struct pcfs_observed_tree *, size_t index,
    struct pcfs_tree_seal_entry_v1 *out);
void pcfs_tree_destroy(struct pcfs_observed_tree *);
#ifdef __cplusplus
}
#endif
#endif

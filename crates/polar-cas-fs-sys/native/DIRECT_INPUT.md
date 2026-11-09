# Direct packed input: verified construction (development)

This is the native P2.3 / P2.4 slice of the existing Direct Tree Ingest plan,
not the completed `Cas::ingest_tree_erofs` operation. No new phase numbering.
The implementation links the pinned vendored liberofs through the existing
C shim; it does not invoke a runtime codec executable or generate a tar.

## Input and ownership

`pcfs_verified_sources_identify` takes size/token descriptors, not caller-supplied
ContentRefs. It opens each original source on demand through the existing
`pcfs_sources_v1` provider. One input is open at a time. The caller retains the
provider/context until `pcfs_verified_sources_destroy` and must not re-enter it.
The API is synchronous and single-owner; use a blocking worker, not an async
reactor thread. It identifies bytes, NOT an atomic snapshot of an entire source
namespace. Source enumeration and its race policy still belong to the observer.

Identity pass: compute the whole-file SHA256 and temporary fixed-block digests.
Encoder pass: load a full source block into an owned scratch buffer, verify it,
then deliver bytes. Short block-boundary reads are deliberate. Repeated reads,
random reads and raw fallback do not feed a misleading sequential hash over the
read-call sequence. Cache fills are authenticated; cache hits reuse owned bytes.
Errors/cancellation poison the set and do not return a successful construction.
A changed input requires fresh identification, not clearing the error flag.

The temporary block index is bounded operation memory, not persistent CAS,
not a content-defined chunking engine, and not a second replica table. Current
implementation is memory-only: exceeding the configured budget fails explicitly;
there is no scratch spill implementation yet. Cache budget is one block (4 KiB
to 1 MiB), in addition to the explicit index budget. Size/token arrays and
encoder data structures also consume memory; index statistics are not total RSS.

## Build API

1. Identify external inputs once; inspect their returned identities.
2. A runtime may then query its Catalog and choose which input indices are new.
3. Call `pcfs_erofs_build_verified_selected` for that subset, or
   `pcfs_erofs_build_verified` for all inputs. Zero selected indices means empty.
4. The encoder consumes authenticated reads and writes one staging P.
5. Embedded metadata is finalized; the final image hash is computed after all
   backpatches. This unavoidable hash of P is not a member-decode pass.
6. Caller must still seal/admit the exact P, then register member replicas.

The new profile is `PCFS_EROFS_PROFILE_BUILD_V1` (value 3): raw or LZ4, native
compatible 4 KiB blocks, no regular-file inline data, no fragments/ztail.
An old caller supplying arbitrary SHA claims cannot request a skip-deep flag:
`pcfs_erofs_build` rejects profile 3 and retains its existing decoded audit.
Only the opaque identified-source capability can enter the new path.

No loose payload object or scratch payload copy is created by this builder.
The caller owns the output FD. Success is NOT CAS admission, fs-verity enablement,
a signed producer attestation, or proof that a codec has no defects. The trust
boundary includes this shim and the pinned encoder. Legacy runtime paths have
NOT yet been migrated to use this capability.

## Embedded self-description

P contains standard member paths `objects/sha256/<prefix2>/<fullhex>` and a small
`/.polar/pack-build.json` regular file. The summary is canonical compact JSON
with fixed keys in sorted order. It excludes its own final image hash, target
Tree and Meta hash (no circular content dependency). It contains no host path,
wall clock timestamp or user-controlled builder string.

Fields: `builder_fingerprint`, `member_bytes`, `member_count`, `member_index`,
`member_path_layout`, `members_digest`, `pack_profile`, `schema`.

The member index remains the EROFS directory plus inode sizes; a second complete
member list is not embedded. The summary itself is excluded from member counts.

Membership digest is SHA256 of:

```
b"polar.cas.members.v1\0"
+ unique_count as unsigned 64-bit big endian
+ each sorted unique (raw_sha256[32] + size as unsigned 64-bit big endian)
```

This is a roster commitment, NOT a Merkle inclusion proof and not a proof that
an unknown producer's compressed payload matches its claims. Reading
`pcfs_pack_summary_read` authenticates neither the producer nor semantic claims.
Remote hash-only acceptance still needs a trusted binding to the build evidence.
An explicitly requested deep audit uses the retained member verifier.

## Test entry points

From the existing root workspace (Python 3.11+, C build dependencies installed):

```
python3 crates/polar-cas-fs-sys/tests/run_direct_input.py \
  --build-dir /absolute/private/build --out /absolute/fresh/results \
  --objects 100000 --fsck /absolute/fsck.erofs

CC=clang python3 crates/polar-cas-fs-sys/tests/run_direct_input.py \
  --sanitize --build-dir /absolute/private/asan-build \
  --out /absolute/fresh/asan-results --objects 100000 \
  --fsck /absolute/fsck.erofs
```

Outputs are outside the source workspace. No runtime path depends on fsck;
it is an independent test oracle only. The runner saves every process status,
log hash, exact native source hashes and output identities. It explicitly states
what it does not test. The synthetic 100k test uses on-demand source callbacks;
a separate 64-file fixture uses real external files under FD limit 32.

The linker wraps the existing decoded verifier. New builds assert zero calls,
whereas a legacy-build fixture asserts a positive call, checking that the
instrumentation itself works. Explicit post-build audit then independently checks
members. That audit is not hidden in construction.

## Rust surface (not yet compiled in this environment)

`IdentifiedSources`, `IdentityLimits`, `DirectPackOptions`, `PackConstruction`
and `Image::pack_build_summary` are thin wrappers. The callback context lives
until native destruction; callback panics are contained. Source capabilities are
!Send/!Sync; construct/use/drop inside one blocking worker. Run the existing
workspace tests including `polar-cas-fs` test `direct_input` with feature `erofs`
on the reviewed Rust runner before using these wrappers. No alternative Cargo
workspace or hand-written replacement for shared domain types was introduced.

## Measured constraints and remaining work

100,000 small synthetic sources: 8,748,464 payload bytes, 420,544,512 image bytes,
9,600,000 index bytes and 65,536 cache bytes. Original source reads during encoding
were 8,748,464 bytes; 17,496,928 bytes were delivered because raw fallback rereads
cached verified data. This demonstrates correctness and bounds, NOT good small-
file compression: noinline incurs approximately one block per tiny object. Keep
this profile limitation visible; do not silently relax native mapping support.

Pending: source-tree observer, full public ingest APIs, generated-file admission,
owned P layout reuse for M, Catalog transaction/recovery, normal transfer/runtime
trust integration, kernel mounting, consumer migration and all Rust acceptance.
No payload GC is enabled by this change.

## Managed image reads and compilation (development)

`pcfs_image_open_managed` is an additive trusted-ownership adapter entrypoint.
The caller supplies an exact admitted image identity and retains its immutable
managed-store, generated-file or already verified snapshot lease. It checks a
readonly regular FD, exact size, supported superblock/layout, checksum and root
metadata without a whole-image hash. It does not turn a path, JSON summary or
arbitrary caller digest into admission evidence. `pcfs_image_open` remains the
strict full-image verification API for untrusted input or explicit audits.

Rust `Image::open_leased` consumes the identity contract of `CasImageLease` and
retains the owner through image/member/metadata closures. Managed replicas have
no 256 MiB snapshot cap and no snapshot copy. Imported snapshots retain that
explicit budget and establish their identity once when the snapshot is made;
opening the resulting typed lease does not hash the same immutable bytes again.
Member trust is separate from image identity: an imported pack's unknown member
claims still take the decoded audit path. Managed blob ownership alone grants
no member trust: `with_admitted_members` binds an existing VerifiedPackedMembers
witness to the exact image and selected digest/size pairs; a subset witness
never authorizes other members. Catalog-admitted or locally generated members compile from inode/layout metadata only, using the
existing `pcfs_meta_build_from_trusted_members` API. The fresh M is hashed once
at finalize, then inspected through `pcfs_hybrid_meta_open_managed` rather than
hashed again. Public Image-only compilation and explicit verify remain strict.

The managed-store contract excludes hostile same-owner/administrator mutation.
Metadata freshness detects ordinary replacement/drift, not byte corruption
hidden before a new lease is captured. Explicit whole-image/deep audits remain
available. No change is made to `PreparedImage`, fs-verity or mount authority.

`pcfs_io_counters_read` / Rust `native_io_counters` return cumulative process
counters under the existing native engine lock. They measure whole-image hash
calls/bytes (including finalized outputs), strict-open hash calls/bytes, decoded
member verification calls/bytes, member-read calls/returned bytes, and actual
main-device pread calls/bytes. Main-device preads include metadata and selected
payload, exclude direct external-device reads and OS readahead; member-read
counters include logical reads through external devices. Use isolated-process
before/after deltas, not resets. Instrumentation adds integer increments, not
extra scans or per-read lock acquisitions.

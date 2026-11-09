# Public source profile

This export is prepared for a new public repository, not a binary release.
The source of development remains PolarInfra. Never import its Git history,
private configuration or Polar Hear hardware cryptography into this repository.
Keep upstream pins and attribution. Future public imports retain public history.

The public desktop profile marks iPhone Companion unavailable. Desktop launch
and the supplied Linux service bind the existing Host transport to loopback
without mDNS advertisement. Local IPC and desktop functions remain unchanged.
This does not remove the Host protocol or claim native runtime verification.

First-party AGPL-3.0 is owner-approved; existing upstream licenses remain.
Complete dependency license evidence, native builds and installation checks
remain publication gates. No unsigned binary is represented as notarized.

Reviewed test-only Rust items and local testkit are removed using hash-guarded
AST-derived patches. Production mixed conditions such as any(test, feature)
remain intact. Vendored upstream code is not rewritten merely to remove its
inline tests. Both baseline and cleaned Linux default workspace cargo check
passed. This does not substitute for macOS/native GUI or runtime validation.

Export with --normalize-lock after populating the approved Cargo dependency
cache. This prunes the copied monorepo lock offline and rejects any new package
identity or changed checksum. Cleanup refuses source drift; re-review patches
for the next monorepo revision rather than bypassing the guard.

## Native source cleanup

The public profile also removes reviewed first-party Zig test declarations,
standalone benchmarks and experimental test programs, native overlay test
injection, and the macOS DEBUG fixture mode/resources. Production overlay
rules, Host-backed control-center behavior, and user-facing diagnostic features
(such as microphone/provider connection tests) are retained. Upstream legal and
provenance files are retained separately from first-party test material.

`public_native_cleanup.json` stores hashes and minimal edit ranges for these
reviewed changes. It is a release transformation manifest, not a test suite.
Reapplying the cleanup is idempotent; changed source fails closed for review.
Snapshot verification rejects first-party Zig test declarations and retired
Linux Python frontend paths under either old or renamed source directories.

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

Test-only Rust items are removed using current-source syn/cfg-expr syntax analysis.
Zig test declarations use the pinned upstream AST. Dedicated test files are excluded
by declared paths and conventions. Local testkit dependencies are edited structurally. Production mixed conditions such as any(test, feature)
remain intact. Vendored upstream code is not rewritten merely to remove its
inline tests. The current cloud PolarInfra working tree is the sole source of
product code; historical public snapshots are not merged back. Cargo, native GUI,
runtime and installation checks must be run for each newly exported input identity.
No historical Cargo or macOS result validates this working-tree export.

Every CLI export normalizes the staged lock offline using the exact pinned
Rust/Cargo versions already installed. The compatibility --normalize-lock flag
is optional because this check is always on. Normalization rejects new package
identities, changed checksums or added dependency edges, then verifies locked
Cargo metadata against the normalized graph. Failed normalization records a
rejection and cannot produce a valid source manifest. The private source lock
is never changed. This is not a compile, test, native runtime or release result.
Special public adaptations use exact local context and reject relevant structural
changes. Ordinary production edits do not require per-file cleanup hash updates.

## Native source cleanup

The public profile also removes reviewed first-party Zig test declarations,
standalone benchmarks and experimental test programs, native overlay test
injection, and the macOS DEBUG fixture mode/resources. Production overlay
rules, Host-backed control-center behavior, and user-facing diagnostic features
(such as microphone/provider connection tests) are retained. Upstream legal and
provenance files are retained separately from first-party test material.

`public_native_cleanup.json` stores bounded contextual rules for nonstandard
fixture hooks and public adaptations; standard test declarations are parsed dynamically. It is a release transformation manifest, not a test suite.
Reapplying the cleanup is idempotent; changed source fails closed for review.
Snapshot verification rejects first-party Zig test declarations and retired
Linux Python frontend paths under either old or renamed source directories.

The additional source-hygiene gate checks Rust, Zig, Swift, Python, C/C++,
JavaScript and GDScript test declarations/frameworks, plus test declarations
added by upstream overlay patches. It runs during export as well as verification.
It does not indiscriminately remove `test` strings: provider/microphone tests,
query metrics, configured runtime mock providers and upstream patch context
remain functional product or provenance material. Unused identity fixtures,
Adam write-fault injection and development sample-view packages are excluded.

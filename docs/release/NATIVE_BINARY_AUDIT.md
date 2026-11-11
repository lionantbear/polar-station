# Native binary evidence and remaining release gates

Historical runs below belong to their stated public-source commits, not the
current PolarInfra working-tree export. This document is retained as workflow
and provenance context. No native build or installation result for the current
`PUBLIC_SOURCE_MANIFEST.json` input identity is implied.

The native validation workflow never authorizes a binary upload. A complete
build, DMG, valid ad-hoc signature and 15-second process survival are useful
checks, not license approval or interactive product validation.

## Exact evidence collected

- Each copied Graphviz executable/library records its original resolved path,
  original SHA-256 and post-relocation/ad-hoc-signing SHA-256. Collection checks
  the original against the actual installed Homebrew formula/version file.
  Renamed `dot` → `dot.bin` is matched by that source path, not its basename.
- A separate receipt records all Mach-O hashes before final signing and after
  copying the verified signed app. The collector checks complete membership,
  source commit, graphics receipt hash, pre-sign hashes against the retained
  pre-sign snapshot and final hashes against the final BOM. This is an observed build chain, not proof that arbitrary
  changed bytes are semantically equivalent.
- Cargo compiler artifacts are source-bound and the report bytes are hashed
  in provenance. Records from both Host and graphics builds are required. Locked
  target metadata is retained separately. Neither proves that every compiled/resolved crate was linked into the app.
- Exact source archive research is stored in
  `licenses/native-source-evidence.json`. The referenced UTF-8 source/license
  texts retain their original byte hashes. Research is associated with a copied
  library only after formula version, source archive checksum and library name
  match; it does not automatically approve attribution.
- UTF-8 evidence retains its original byte hash. Legacy legal text with unknown
  encoding is preserved losslessly in a UTF-8 base64 envelope, with both the
  envelope and original-byte hashes checked. The collector does not guess the
  encoding or treat the envelope as reviewed license text.
- The temporary DMG copy must have the exact audited Mach-O file membership and
  hashes before launch. Its report binds those observations to the source commit
  and the DMG hash/size. This checks package identity, not interactive behavior.

## Verified source distinctions

The archives in the research manifest were checked against the SHA-256 source
checksums recorded by the prior Homebrew build evidence:

- gettext 1.0: the `gettext-runtime/intl` implementation declares
  LGPL-2.1-or-later. The formula-wide GPL/LGPL aggregate is not an adequate
  attribution for `libintl.8.dylib`.
- GLib 2.90.1: inspected GLib, GModule and GThread source headers declare
  LGPL-2.1-or-later. This is not a complete file-level composition audit.
- Libtool 2.6.2: `libltdl/ltdl.c` has a Lesser GPL version-2-or-later declaration
  and a file-level Libtool exception; `COPYING.LIB` contains LGPL 2.1. Neither
  the formula's GPL label nor that exception should be applied blindly to the
  entire `libltdl.7.dylib`.

- Graphviz 16.1.0: the checksum-verified root COPYING and sampled core/pathplan
  headers specify EPL-2.0, while the captured Homebrew formula says EPL-1.0.
  The latter is not an approved per-binary label. Additional Random kit,
  Mersenne Twister, ellipse, rbtree and Bison-skeleton exception notices also
  require composition review; do not flatten these into the root license.
- PCRE2 10.49: LICENCE.md declares BSD-3-Clause WITH PCRE2-exception and separate
  BSD-2-Clause licensing for SLJIT. Exact JIT/Unicode configuration and source
  notices must be checked before preparing the binary's notice set.
- GTS 0.7.6 (Homebrew revision 4): README describes Library GPL scope and
  explicitly identifies predicates.c as public domain. tribox3.c has separate
  authorship and is listed in the library build recipe, but its header has no
  file-local grant. The root scope, revision-4 patches and exact bottle inputs
  need provenance review, not an unsupported incompatibility conclusion.

For LGPL components, release preparation still needs applicable notices/license
texts, corresponding sources and build/patch instructions via a compliant
distribution method, plus a validated replacement/relink procedure compatible
with macOS signing. These obligations are tracked, not marked satisfied.

## Fail-closed behavior

`macos_binary_inventory.py` requires both binary and reviewed license-text
hashes. Existing schema-1 attribution maps without `license_text_sha256` remain
blocked. It rejects unsafe dependency paths, non-Mach-O targets and incomplete
scans. Universal-binary slices and ambiguous nested executable paths are blocked
until an architecture-aware executable dependency graph is implemented.

`write_macos_release_gate.py` checks report schemas, exact component sets/hashes,
retained text hashes, Cargo evidence, source revision and limited launch outcome.
Missing/malformed evidence or collection gaps fail the evidence step. It always
sets `release_ready` and `binary_upload_authorized_by_this_workflow` to false.
Only reports are uploaded, never the app or DMG.

Still open: full Ghostty/Godot/Cargo/Sparkle static and resource composition;
artifact-specific notices installed before signing; exact corresponding source
and relink obligations; reproducible Homebrew inputs; clean-Mac terminal,
graphics, permissions, upgrade and removal tests. No current report clears
these requirements.

The production evidence validator can be rerun on the retained reports and
original local DMG:

```sh
python3 automation/polar_station/write_macos_release_gate.py \
  --evidence /path/to/station-evidence \
  --source-commit FULL_BUILD_COMMIT \
  --dmg /path/to/Polar-Station-unnotarized.dmg
```

No test code is included in the clean public source export. Validation reports
and any regression results must identify the source revision they exercised.
A fresh native run is required after changing collection or gate logic.

Historical run [42715696012](https://github.com/lionantbear/polar-station/actions/runs/42715696012)
built and signed the app, created the DMG, and passed a temporary copy/signature
and 15-second process check on commit `07879c271041c58eae18f7ea38722f95c45d54f1`.
Its report archive SHA-256 was
`4d524610c299ce492065fb8d2e17d5f1dabaeedaa07f68d5e7b95555c722a74e`.
It recorded 26 Mach-O components, 1,501 Cargo compiler records and 2,466 retained
text records; all retained hashes and BOM/provenance hashes matched. That older
run lacks final-signing and installed-copy identity receipts and retains 16
copy-to-signing hash gaps. It does not validate this newer implementation or
clear any outstanding license, source-distribution or interactive release gate.

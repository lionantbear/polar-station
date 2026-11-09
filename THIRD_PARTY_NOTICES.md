# Third-party notices and verification status

Owner-designated first-party Polar Station code is AGPL-3.0-only. Existing
MIT-licensed local components retain their stated MIT terms. This does **not** relicense third-party
files, adapted algorithms, patches' upstream portions, or dependencies.
No upstream Git history is rewritten by this source export.

## Exact revision texts obtained

`licenses/pinned-evidence.json` records upstream URLs, exact commits, byte counts,
SHA-256 hashes and unavailable candidate filenames. Files marked `fetched` are
stored unchanged under `licenses/pinned/`. A 404 for an alternative filename does
not establish the absence of all other legal notices.

| Component | Exact revision | Texts bundled | Terms observed |
|---|---|---|---|
| Ghostty | `332b2aefc6e72d363aa93ab6ecfc86eeeeb5ed28` | `licenses/pinned/ghostty/LICENSE` | MIT |
| Godot | `ed1daf0bf001b61586d9930840f2f1394092c079` | `licenses/pinned/godot/LICENSE.txt`, `COPYRIGHT.txt` | MIT engine; third-party terms in COPYRIGHT |
| grammers | `4ca2263e5678d2e85414aa8c2f469c1341d3a143` | `licenses/pinned/grammers/LICENSE-MIT`, `LICENSE-APACHE` | MIT / Apache-2.0; individual package metadata still required |
| nucleo | `4253de9faabb4e5c6d81d946a5e35a90f87347ee` | `licenses/pinned/nucleo/LICENSE` | MPL-2.0 |
| Codex fork | `7eea594e1264e37e08f029633f2f476f03fbda6c` | `licenses/pinned/codex/LICENSE`, `NOTICE` | Apache-2.0; NOTICE includes Ratatui attribution |
| Microsoft mxc | `6cd3d58f05d3447e67109cfb75e042803b843ca4` | `licenses/pinned/mxc/LICENSE.md` | MIT |
| MCP Rust SDK | `4048ac40e958fb1a7d614617a2ae8d26666ee2fa` | `licenses/pinned/rust-sdk/LICENSE` | Mixed Apache-2.0/MIT transition; documentation CC-BY-4.0 |
| tokio-tungstenite fork | `132f5b39c862e3a970f731d709608b3e6276d5f6` | `licenses/pinned/tokio-tungstenite/LICENSE` | MIT |
| tungstenite fork | `9200079d3b54a1ff51072e24d81fd354f085156f` | `licenses/pinned/tungstenite-rs/LICENSE-MIT`, `LICENSE-APACHE` | MIT / Apache-2.0 |

The existing Godot files in `apps/polar-graphics-native/licenses/` match the
exact pinned LICENSE and COPYRIGHT **byte-for-byte**. This establishes those
texts' provenance, not coverage of a binary that has not been built.

The MCP license explicitly says contributions without relicensing consent remain
MIT; it must not be flattened to Apache-2.0. If its documentation is redistributed,
its CC-BY-4.0 requirements also apply.

## Source already included in the export

- Native force-layout adapts d3-force-3d at
  `263b3bfc37578e541a7d7e59dbb32d026d66fa38` and d3-octree at
  `28beff983b94757c90b003c51c2c8a053ddd63be`. Preserve the MIT texts,
  copyright attribution, `NOTICE.md` and `PROVENANCE.json` under
  `crates/polar-force-layout-core/upstream/`, even if development oracle tests
  are excluded. The derived Rust implementation is not wholly original work.
- godot-rust/gdext 0.4.5 is MPL-2.0, not Apache-2.0. Its text is retained at
  `crates/polar-force-layout-core/upstream/LICENSE-godot-rust-MPL-2.0.txt`.
  Source availability and any modifications to covered files need distribution
  review; linking does not automatically relicense all first-party files.
- The Godot gohud-wheel addon retains its own MIT text under
  `apps/polar-ideacave-godot/licenses/`.
- EROFS source retains `COPYING` and `LICENSES/` under
  `crates/polar-cas-fs-sys/native/vendor/erofs-utils/`. Its file SPDX inventory
  includes dual GPL/MIT, dual BSD/GPL, MIT and Unlicense. Do not describe all
  vendored files as Apache-2.0. `lib/liberofs_base64.h` contains declarations
  without its own SPDX header; the root COPYING describes the lib/include
  licensing pattern, but this exception remains explicitly flagged for review.
- Vendored `grammers-tl-gen` retains its MIT and Apache texts.

For source-only publication versus executable-distribution obligations, see
`SOURCE_DISTRIBUTION.md`. The binary gates below are not a statement that
referencing dependencies in Cargo.lock distributes their implementations.

## Not yet verified

This is **not a completed dependency or binary license clearance**.

- Locked all-features Cargo metadata now covers 1,363 packages: 56 local,
  1,168 registry and 139 Git packages. Portable metadata claims and retained
  legal texts are in `licenses/cargo-evidence.json`; no local filesystem paths
  are included. Of 1,307 external packages, 1,285 have collected legal texts;
  22 remain without complete collected texts and are explicitly reported.
- Every one of the 1,168 registry archive SHA-256 hashes matches `Cargo.lock`
  (`licenses/cargo-archive-integrity.json`). For 21 MPL registry packages,
  632 source files match their original archives byte-for-byte. The two Git
  MPL packages remain separately identified by pin. These checks do not prove
  native binary composition or distribution compliance.
- Legacy Cargo `MIT/Apache-2.0`-style slash alternatives are normalized to OR
  while retaining the original value. CDLA-Permissive-2.0 and the exact LLVM
  exception were inspected; their texts remain bundled. The 23 MPL packages
  retain an explicit source-availability/distribution review gate.
- Re-run locked Cargo metadata for the actual supported target/features when
  dependencies change. Root Git licenses do not replace every package's notices.
- Graphviz is an unpinned external installation in the source build. Determine
  the actual version and applicable license from the built artifact. No generic
  current Graphviz license is substituted for that missing evidence.
- Inventory every copied Graphviz library/plugin and Ghostty/Godot native
  dependency, and match license texts to the actual binary BOM.
- MPL/EPL/source-availability obligations and mixed-license notices remain
  release gates. No prebuilt binary is declared cleared by this document.

`automation/polar_station/license_audit.py` validates collected file hashes and
reports unresolved items. `release_ready` remains false until the full checks
and actual-build evidence are supplied and reviewed.

## Reproducing source evidence

```sh
cargo metadata --locked --all-features --format-version=1 > /tmp/cargo-metadata.json
python3 automation/polar_station/collect_cargo_licenses.py --metadata /tmp/cargo-metadata.json --output licenses
python3 automation/polar_station/fetch_cargo_legal_texts.py --source . --metadata /tmp/cargo-metadata.json
python3 automation/polar_station/verify_cargo_archives.py --source . --metadata /tmp/cargo-metadata.json --crate-cache /absolute/path/to/cargo/registry/cache/index-directory
python3 automation/polar_station/license_audit.py --source . --cargo-metadata /tmp/cargo-metadata.json --summary-only
```

The fetch step reads only exact VCS revisions found in downloaded package
metadata. The audit can use captured claims without a machine-local metadata
file, matching package identity to the current lock. Neither mode overrides
unresolved notices or actual binary checks.

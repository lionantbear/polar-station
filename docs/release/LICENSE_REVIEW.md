> Historical input report supplied with the archive48 release kit. Its paths, counts,
> missing-tool claims and license-selection status describe that earlier run, not
> the current prepared tree. First-party AGPL-3.0 is now owner-approved; see
> the current root README, source manifest, and rerun audit output for current gates.
> The full original archive48 was not available to this preparation run.

# Polar Station archive48 — license and provenance assessment

Date: 2026-11-09. Scope: the **exported Station source closure**, not all of the
private PolarInfra monorepo. This is a source and policy inventory, **not a legal
clearance or release certificate**. Do not distribute binaries using this report
alone.

## 1. Source closure and what is actually known

The deterministic exporter selects four Station Cargo roots, their recursively
reachable local crates and patch sources, plus explicitly selected native
build inputs. `Cargo.lock` expands their over-approximated transitive closure to:

| Dependency class | Packages | Verification state |
| --- | ---: | --- |
| Local path crates (Station and required shared components) | 59 | Sources present; 55 first-party manifests lack an explicit license pending root policy |
| crates.io packages | 1,234 | Exact versions/checksums recorded; license metadata and texts not yet collected |
| Cargo packages from pinned Git dependencies | 139 | Seven distinct pinned repositories cataloged; individual package license metadata and exact commit texts not yet verified |
| **Total reachable Cargo packages** | **1,432** | **NOT LICENSE-CLEARED** |

The monorepo-wide `Cargo.lock` has more packages; **1,432 is the Station
dependency graph**, not a claim about all PolarInfra modules. Because Cargo.lock
has no package `license` field, it would be false to call 1,373 external
packages permissive without running `cargo metadata --locked`, reading their
actual license-file entries, and inspecting redistributed source/binaries.

`license_audit.py` produces one JSON record per reachable Cargo package with
version, package source, checksum (where present), manifest location, SPDX
claim, metadata status and review decision. It currently returns **exit code
2 (blocked)**. Cargo `license = "MIT OR Apache-2.0"` is a license-choice expression;
`license = "MIT AND GPL-3.0-only"` requires both license conditions. The
automated SPDX evaluator treats unknown terms or missing metadata as blocked.
It is an independent preflight, not a replacement for cargo-deny or legal review.

## 2. Identified source and native licenses

| Component | Source-derived license | Actual archive48 evidence | Remaining release action |
| --- | --- | --- | --- |
| Ghostty macOS/Linux upstream | MIT | Both `upstream.lock.json` files pin tag v1.3.1 and SHA `332b2ae...` | Retrieve full LICENSE from **that exact SHA**, plus its Zig/C/native dependencies and bundle their license texts. |
| Godot Engine | MIT plus distinct third-party notices | `apps/polar-graphics-native/licenses/Godot-LICENSE.txt` **and** the 98 KiB `Godot-COPYRIGHT.txt` are **already in the source archive and clean export** | Verify those exact bytes match Godot pin `ed1daf0...`; distribute both in the final app and check third-party selection. |
| godot-rust/gdext | MPL-2.0 | `crates/polar-force-layout-core/upstream/LICENSE-godot-rust-MPL-2.0.txt`, `NOTICE.md` | Retain MPL license and notices, inspect changes to covered source files and any source-offer obligations; adapter currently claims unchanged upstream. |
| Graphviz | Current releases EPL-2.0; historic builds EPL-1.0 | `graphics_macos.py` dynamically packages `dot`, plugins and recursively discovered linked dylibs; **no pinned Homebrew package version or license payload** | Pin exact Graphviz version and every copied dylib. Capture correct EPL license text and full attributed notices for every bundled non-system library. |
| EROFS (`liberofs`) | Vendored `COPYING` says much of `lib/` and `include/` dual GPL-2.0-or-later **OR MIT**; some files different | Vendored `COPYING`, `LICENSES`, `native/upstream.lock.json` and source headers | Prefer the MIT option where applicable; confirm compiled object/source-file set, one no-SPDX-header file's inherited terms and separate BSD/Unlicense notices. Do **not** label the whole library GPL-only. |
| d3-force-3d adaptation | MIT | Full upstream `LICENSE`, `NOTICE.md`, `PROVENANCE.json`, pinned commit `263b3bf...` | Preserve attribution to the original source and clarify adaptation in public notices. |
| d3-octree adaptation | MIT | Full upstream `LICENSE`, `NOTICE.md`, `PROVENANCE.json`, pinned commit `28beff9...` | Preserve attribution to the original source and clarify adaptation in public notices. |
| Nucleo (Git crate source) | MPL-2.0 upstream | Exact Git SHA pinned in Cargo.lock; catalog records upstream license | Fetch and inspect exact revision; capture text and any modified file obligations. |

The EROFS C header pass examined 99 `.c/.cc/.cpp/.h/.inc` files within the
export: 87 had `GPL-2.0+ OR MIT`, 8 had `MIT`, 2 had
`BSD-2-Clause OR GPL-2.0`, 1 had `Unlicense`, and 1 had **no per-file SPDX
header**. These are source declarations; a reliable compiled-file inventory
and full license-text review remain necessary. Avoid treating `GPL OR MIT` as
`GPL AND MIT`.

## 3. Pinned Git dependencies requiring exact-license checks

These **seven repositories** contribute 139 Cargo packages in aggregate.
`license_policy.json` holds the complete 40-character pins and evidence URLs:

| Repository | License claim from upstream, not a pinned-SHA clearance |
| --- | --- |
| `codeberg.org/Lonami/grammers.git` | MIT OR Apache-2.0 claim; verify exact SHA and the locally patched `grammers-tl-gen` crate. |
| `github.com/helix-editor/nucleo.git` | MPL-2.0; inspect exact revision. |
| `github.com/leoricshcn/codex.git` | OpenAI upstream Apache-2.0; verify **fork** exact source and notices. |
| `github.com/microsoft/mxc` | MIT; verify exact revision. |
| `github.com/modelcontextprotocol/rust-sdk` | License history includes mixed terms; inspect exact revision/file scopes, not just current README. |
| `github.com/openai-oss-forks/tokio-tungstenite` | Upstream MIT; verify **fork** exact revision. |
| `github.com/openai-oss-forks/tungstenite-rs` | Upstream MIT OR Apache-2.0; verify **fork** exact revision. |

Exact references and official evidence sources are encoded in
`automation/polar_station/license_policy.json`; source-level claims are **not
assertions that the executable is free from proprietary code or backdoors**.

## 4. Release gate: evidence must follow the actual artifact

1. **Snapshot:** verify all source paths, proprietary exclusion rules, patch
   dependencies, pin integrity and SHA-256 manifest; a source integrity pass
   is not a license pass.
2. **Cargo:** on both `ubuntu-latest` and macOS runners, run
   `cargo metadata --locked --format-version 1 --all-features` to saved JSON,
   and feed both files to `license_audit.py --cargo-metadata`. Execute
   `cargo deny check licenses` with reviewed exceptions and `cargo about`
   to collect exact third-party license/copyright texts. `cargo metadata`
   alone cannot certify redistributed native binary obligations.
3. **Upstream:** fetch Ghostty/Godot/seven pinned Git repositories by immutable
   SHA, verify repository identity and copy licenses/NOTICE/COPYRIGHT from those
   *actual source snapshots*, never from an arbitrary latest branch.
4. **Native binary BOM:** on the real macOS `.app`, run
   `macos_binary_inventory.py --app 'Polar Station.app' --output macos-bom.json`
   to enumerate Mach-O executables, frameworks/dylibs and every non-system
   dynamic edge (`otool -L`, `LC_RPATH`, SHA-256), including recursively copied
   Homebrew Graphviz libraries. Supply `--attributions mapping.json` with
   exact binary hashes, supplier/source, SPDX license and a real in-bundle
   `license_text_path`. The script blocks missing map entries or unresolved
   dylibs. It **does not inspect non-Mach-O assets**, and its output still
   requires separate independent source-to-binary and notice verification.
   On Linux enumerate ELF `.so` dependencies and package supplier licenses.
5. **Generated notices:** package a readable Third-party Licenses file inside
   the app/DMG and in GitHub Releases; ensure each redistributed third-party
   component is covered by exact license text, notices and its license-specific
   source availability requirements.
6. **Security/provenance:** scan source and dependency manifests; record
   build commit, public source SHA, binary digest and reproducible workflow.
   Readable source reduces opacity but does not itself prove absence of a
   backdoor, nor that the downloaded binary exactly corresponds to source.
7. **Approval:** block promotion until root project LICENSE is chosen, every
   unknown or copyleft exception reviewed, all exact notices exist, and the
   build/installation/security gates pass.

Current source audit is deliberately **fail-closed**. An operator cannot
silently mark `PUBLIC_SOURCE_MANIFEST.json` as `accepted` to bypass the checker:
the release audit reruns the license auditor. However the current prototype
does not yet implement source-to-binary NOTICE collection end-to-end or prove
Mac installation; it always reports those pending gates. The Mac BOM tool is
a real audit collector with unit tests for link/rpath resolution, but is
**not native-Mac-executed** in this Linux assessment.

## 5. Credible public distribution without Apple fees

- Keep the developer's main machine on stable macOS 26 for baseline testing;
  add CI tests against standard `macos-26` and `xcode-27` (macOS 27 preview)
  where available. Do not assume the preview image is a stable release gate.
- A **public** GitHub Actions repository's standard hosted macOS minutes are
  free under GitHub's public-repo policy; larger runners and private repos have
  different billing. Pin GitHub action versions and toolchains before release.
- An ad-hoc-signed `.app` distributed in an **UNNOTARIZED** DMG does not require
  Apple Developer Program membership or its annual fee. Gatekeeper may warn;
  disclose manual Open Anyway instructions and never instruct disabling system
  security globally. Without Developer ID notarization, true effortless
  double-click installation cannot be guaranteed.
- `package_macos_free.sh` packages an **already built, correctly ad-hoc-signed**
  `.app`; the *existing native builder still requires Apple Development signing*
  and private checkout assumptions. The helper is not a working end-to-end
  release pipeline yet.

## 6. Selected authoritative primary references

- Ghostty upstream license: https://github.com/ghostty-org/ghostty/blob/main/LICENSE
- Godot license and copyright: https://github.com/godotengine/godot/blob/master/LICENSE.txt and https://github.com/godotengine/godot/blob/master/COPYRIGHT.txt
- godot-rust/gdext v0.4.5: https://github.com/godot-rust/gdext/tree/v0.4.5
- Graphviz license: https://graphviz.org/license/
- erofs-utils project: https://github.com/erofs/erofs-utils
- d3-force-3d: https://github.com/vasturiano/d3-force-3d
- d3-octree: https://github.com/vasturiano/d3-octree
- cargo-deny: https://github.com/EmbarkStudios/cargo-deny
- cargo-about: https://github.com/EmbarkStudios/cargo-about
- GitHub-hosted standard runner policy: https://docs.github.com/actions/reference/runners/github-hosted-runners
- GitHub macOS runner image catalog: https://github.com/actions/runner-images

This document records the scope and remaining work honestly. It does not
certify every dependency without its actual license text and release binary.

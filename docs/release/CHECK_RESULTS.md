> Historical input report supplied with the archive48 release kit. Its paths, counts,
> missing-tool claims and license-selection status describe that earlier run, not
> the current prepared tree. First-party AGPL-3.0 is now owner-approved; see
> the current root README, source manifest, and rerun audit output for current gates.
> The full original archive48 was not available to this preparation run.

# Archive48 source release — static verification record

Environment: Linux executor, Python 3.11+; no Cargo, Zig, macOS runner or Apple certificate available. Source baseline fully extracted at `/mnt/data/polarinfrav1-archive48`.

## Implemented checks

| Command/check | Result |
|---|---|
| `python3 -m unittest discover -s automation/tests -p 'test_*.py' -v` | **PASS — 15 tests** |
| `python3 -m py_compile automation/polar_station/{public_release,license_audit,macos_binary_inventory}.py` | **PASS** |
| `sh -n automation/polar_station/package_macos_free.sh` | **PASS** |
| Real archive48 `plan` | **PASS — 59 crates; includes transitive local Git patch `grammers-tl-gen`** |
| Real archive48 `export` | **PASS — 1,248 exported file hashes; ~18 MiB source tree** |
| Real export `verify` | **PASS — source integrity only** |
| Real export `audit` | **BLOCKED — release-readiness and licensing** |
| License policy + audit | **PASS as fail-closed tool — 1,432 packages inventoried; 1,373 external license claims not verifiable offline** |
| Native EROFS source-header audit | **PASS inventory — 99 C headers/sources surveyed; one without a per-file SPDX header needs review** |
| Native Godot texts | **FOUND — both LICENSE.txt and COPYRIGHT.txt in clean export; exact pinned-byte comparison pending** |
| Mach-O BOM scanner | **PASS — unit tests on `otool` link/rpath parsing and bundle-boundary resolution; macOS runtime not available** |
| `cargo metadata --locked` / Cargo build | **NOT RUN — no Cargo** |
| macOS native build / DMG / first launch | **NOT RUN — no macOS** |
| Linux native build / installer | **NOT RUN — no Rust build toolchain** |
| Complete secret scan and full third-party licensing clearance | **NOT RUN — pending CI/review** |

## Major current blockers

1. Test-only `polar-storage-testkit` still appears in the dev-dependency closure; 207 Rust `.rs` source files contain inline `#[cfg(test)]` sections.
2. No project root `LICENSE`. The audited Cargo dependency graph includes 1,234 crates.io packages and 139 Git packages across seven repositories; no exact resolver license metadata was available. Vendored EROFS licenses need compiled-file and inherited-header review.
3. Three Mac icon/status assets are absent from archive48.
4. Companion is unavailable but native Mac launcher has an advertised `0.0.0.0:47820` listener.
5. Existing Mac builder requires Apple Development identity, private user checkout, local Zig/SDK, and `git rev-parse` on a Git-history-free source tree.
   Mac graphics build also patches its shared Godot checkout in place, requiring a disposable pinned source tree for public CI.
6. Graphviz's recursive macOS dylib license inventory is unverified; Ghostty's exact pinned upstream text and seven Git forks/repositories are not fetched.
7. No native build, binary provenance, Gatekeeper installation, or real DMG validation.

This artifact is a **release workflow prototype**; zero external repositories or releases were created, modified, or published.

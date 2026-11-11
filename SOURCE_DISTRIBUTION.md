# Source publication and later binary distribution

This repository publishes source, not prebuilt application binaries. The first-party AGPL-3.0 grant does not relicense third-party files. Preserve their copyright notices, license headers, full legal texts and the notices referenced by `THIRD_PARTY_NOTICES.md`.

## MPL dependencies

Cargo.lock pins the resolved dependencies. `licenses/cargo-evidence.json` provides their exact registry/source identities and collected legal texts. `licenses/mpl-source-inventory.json` verifies 21 registry MPL packages (632 files) against the original checksum-verified Cargo archives; it also identifies the two pinned nucleo Git packages. This is evidence for the downloaded dependencies, not a claim that every future build will use unchanged source.

The source repository references these Cargo dependencies; it does not vendor their entire source. MPL does not require relicensing separately authored application files merely because they use MPL libraries. If publishing modified MPL-covered files, preserve their MPL coverage and make the modified covered source available under MPL. For any executable distribution containing MPL-covered code, supply recipients a clear, durable means to obtain the corresponding covered source, including actual modifications, at no more than distribution cost, and tell them how. An exact-source archive alongside the binary is preferable to relying only on third-party hosting. Preserve license/attribution notices and do not restrict recipients' MPL rights. These executable-distribution steps remain a release gate; they are not a reason to call this source-only repository a binary release.

## Vendored EROFS source

The tree `crates/polar-cas-fs-sys/native/vendor/erofs-utils` is actually redistributed here. Keep its COPYING, LICENSES, per-file notices, pinned `native/upstream.lock.json` and `native/patches/0001-polar-vfile.patch`. The patch records the Polar changes and is published as source; do not describe those changes as unmodified upstream.

The inspected 99 C/header files comprise 87 `GPL-2.0+ OR MIT`, 8 MIT, 2 BSD-2-Clause OR GPL, 1 Unlicense, and one header without an SPDX identifier. There is no GPL-only C/header file in this exported subset. For explicitly dual-licensed files, the permissive MIT or BSD branch is available; retain the associated notices and disclaimers in source and, when applicable, binary accompanying materials. `lib/xxhash.c` includes the full BSD notice. Do not infer that root AGPL-3.0 overrides these terms.

`lib/liberofs_base64.h` has no file-level identifier. Upstream COPYING says most lib/include files are dual-licensed and all other files MIT unless otherwise stated, which supports inherited permissive treatment, but the wording is not an explicit file-specific grant. The file is a short declarations-only header. This uncertainty is recorded rather than silently assigning a new license; obtain upstream clarification if a file-specific legal determination is required.

## What still blocks a binary release, not source inspection

Historical macOS CI evidence for a separately identified public-source commit is described in `docs/release/NATIVE_BINARY_AUDIT.md`. It does not validate this PolarInfra working-tree export. Both native builds for this new input identity, complete static-library/resource composition, exact reviewed attribution, corresponding-source/relink compliance and interactive clean-machine behavior remain unverified. A binary release must satisfy those gates for the actual bytes it ships. See `docs/release/NATIVE_BINARY_AUDIT.md` for source-bound evidence and its limits. This source review is not binary certification or legal advice.

Some Cargo packages declare their licenses but do not ship license text or an exact resolvable legal-text location. They are listed as missing in the evidence instead of substituting arbitrary current-branch texts. These dependency-source gaps remain relevant before vendoring those packages or distributing binaries containing them; a lockfile reference alone does not redistribute their implementation.

# polar-station

Polar Station

Native macOS/Linux agent workstation, extracted from PolarInfra.

## Status

**Source-only preview. Not a binary release.** Linux workspace Cargo checks pass;
complete macOS/Linux GUI packages and installation have not been verified. First-party Polar code is licensed under [AGPL-3.0](LICENSE), as approved by
the owner. Third-party components retain their original licenses. Do not
distribute this staging tree as a fully license-cleared release.

The iPhone Companion screen is **Unavailable** in this public profile. Supplied
macOS and Linux desktop launchers bind Host to loopback without mDNS advertising.
The underlying Host transport still exists; this is not a claim that all network
features have been removed. Native behavior has not yet been exercised.

## Source and upstreams

Development continues in PolarInfra. This repository is intended to start with a
new public import, without private development history; future public imports
must preserve this repository's history. Polar Hear private hardware cryptography
is not included. Upstream projects remain pinned references with their original
license and attribution material, not rewritten upstream histories.

Read [source vs. binary distribution obligations](SOURCE_DISTRIBUTION.md),
[third-party notices](THIRD_PARTY_NOTICES.md),
[license review](docs/release/LICENSE_REVIEW.md), and
[public profile](docs/release/PUBLIC_PROFILE.md).

## Release tooling

Python 3.11+ tools live in `automation/polar_station/`:

- `public_release.py plan/export/verify/diff`: deterministic source selection,
  guarded public-profile adaptations, integrity hashes and snapshot comparisons.
- `public_release.py audit`: fail-closed readiness check. An integrity pass is
  not permission to publish or evidence of a successful native build.
- `license_audit.py`: exact Cargo/native license evidence inventory.
- `macos_binary_inventory.py`: actual Mach-O dependencies and attribution checks.
- `package_macos_free.sh`: package an already built signed `.app` into an explicitly
  **UNNOTARIZED** drag-to-Applications DMG. It does not build or notarize the app.

Before every subsequent export, install the current release tools and reviewed
build adaptations in the private monorepo. Export to a new directory, review the
snapshot diff, run native build/security/license checks, then import only the
approved public tree. Do not push the private monorepo or its `.git` directory.

Current remaining gates: exact resolved third-party license evidence;
native GUI builds and clean installation. Linux default workspace cargo check
has passed before and after first-party test-only-source cleanup. Vendored
upstream source remains intact.

The retired Linux Python/PyGObject frontend is excluded. The supported Linux frontend is the Zig module embedded in managed Ghostty; Python build and release utilities remain included.

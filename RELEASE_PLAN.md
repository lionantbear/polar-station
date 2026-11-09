# Installable release pipeline

## Current evidence

The source-build workflow has successfully compiled the Rust workspace on Linux
and macOS 15, and the macOS Swift libraries. That is not a native application
release. The macOS 26 native workflow now attempts the complete Godot/Ghostty app
and a local DMG, without publishing binaries. Read the actual run result.

## macOS Apple Silicon

1. Build the pinned native sources and all embedded graphics resources on macOS
   26. Record the tool/SDK versions and source revision.
2. Inventory every bundled executable/dylib with `macos_binary_inventory.py`;
   resolve only in-bundle or Apple system dependencies. Include hash-bound
   attribution and license texts for Graphviz and its copied dependencies,
   Ghostty, Godot, Cargo dependencies and application assets.
3. Keep the public Companion-disabled profile. Verify no new public listener.
4. Ad-hoc sign and verify the app, then use `package_macos_free.sh` to create and
   verify the DMG. No paid Apple developer account is required. This is not
   Developer ID signing or notarization; document the normal macOS Open Anyway
   flow, never disable Gatekeeper globally.
5. Install and launch on a clean Mac. Test terminal creation/input, embedded
   graphics, Host startup, permissions, upgrade and uninstall. Retain logs and
   explicitly list the tested macOS versions. A headless CI compile does not
   substitute for these tests.
6. Only after those gates, create a versioned GitHub Release containing the DMG,
   SHA256SUMS, dependency/license notices, exact source tag and known limitations.

## Linux x86_64

The missing standalone Rust build module and root-justfile dependency can be
fixed independently of product renaming. Full Ghostty/Godot native builds must
then run in a declared distro/sysroot with pinned Zig, SCons and resource tools.

The current installer consumes a development receipt with absolute artifact
paths. It is **not** a portable end-user package. Do not zip that receipt and call
it installable. First define a relocatable runtime tree, preserve its file hashes,
ship the required desktop icons and resources, and verify all ELF dependencies.
Then build a distro-specific `.deb` (explicit supported Ubuntu/Debian versions)
or a separately validated AppImage. Avoid promising universal Linux support.

Exercise install, launch, terminal/graphics, upgrade and removal in a clean VM
with no source checkout or build cache. Complete the same artifact-specific
license/notice and public-profile gates before publishing a versioned package.

## Release automation contract

Use separate validation and publishing workflows. Validation has read-only
repository permissions and can run without secrets. Publishing should run only
for reviewed version tags after artifact validation, use write permission only
in its final release job, and attach checksums and exact source/BOM provenance.
Failed or missing checks must block publication. Do not label an untested binary
stable merely because compilation succeeded.

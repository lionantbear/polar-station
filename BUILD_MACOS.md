# macOS source build (Apple Silicon)

This source comes only from the current PolarInfra working tree. Its export-only
builder adapters have Python/shell and command-construction regression evidence.
**Cargo compilation, native compilation, launch, signing, graphics and clean-machine
installation have not been validated for this exported input identity.**
Historical public-source build reports do not validate this tree.

## Required local inputs

The scripts do not install these dependencies:

- Apple Silicon Mac, Xcode and selected command-line developer tools.
- Rust toolchain from `rust-toolchain.toml`, including `aarch64-apple-darwin`.
- Zig **0.15.2**, gettext `msgfmt`, Python 3 and SCons **4.9.1**.
- Ghostty checkout at `332b2aefc6e72d363aa93ab6ecfc86eeeeb5ed28`.
- A Zig-compatible macOS SDK. The current script accepts
  `POLAR_STATION_MACOS_SDK` or the existing
  `/Library/Developer/CommandLineTools/SDKs/MacOSX15.sdk`; it does not discover
  a newer SDK automatically or claim compatibility with macOS 26/27 SDKs.
- Godot checkout at `ed1daf0bf001b61586d9930840f2f1394092c079`, located at
  `$POLAR_GRAPHICS_CACHE/godot`, and Godot editor **4.7.2**.
- Graphviz installation prefix, including its libraries and plugins.

The copied monorepo lock keeps its pinned package identities and checksums.
Two removed private-test dependency edges are hash-guarded export changes.
Full Cargo lock normalization remains a separate check: populate the approved
Cargo cache and use the exporter's `--normalize-lock` option offline. Do not
resolve an arbitrary newer dependency set to make a build pass.

## Build entrypoints

Example with explicit local paths, after lock validation:

```sh
export POLAR_STATION_GHOSTTY_CHECKOUT=/absolute/path/to/ghostty
export POLAR_STATION_ZIG=/absolute/path/to/zig-0.15.2
export POLAR_STATION_MACOS_SDK=/absolute/path/to/compatible/MacOSX15.sdk
export POLAR_GRAPHICS_CACHE=/absolute/path/to/graphics-cache
export POLAR_GRAPHICS_SCONS=/absolute/path/to/scons-4.9.1
export POLAR_GRAPHICS_EDITOR=/absolute/path/to/Godot.app/Contents/MacOS/Godot
export POLAR_GRAPHICS_GRAPHVIZ=/absolute/path/to/graphviz-prefix
export POLAR_STATION_CODESIGN_IDENTITY=-
bash automation/polar_station/build_macos_graphics.sh
bash automation/polar_station/build_macos.sh --configuration Release
```

Ghostty is built in a disposable clone of the pinned checkout. **The current
macOS graphics script patches `platform/macos/libgodot_macos.mm` in the supplied
Godot cache checkout in place.** Use a dedicated approved build input for that
script; it is not a read-only upstream source operation.

Rust outputs are constrained to this export's `target/`, regardless of an
inherited `CARGO_TARGET_DIR`. App outputs are under
`target/polar-station-macos/Release/`. Native receipts consume the Mono HEAD,
working-tree input digest and actual exported-file fingerprint from
`PUBLIC_SOURCE_MANIFEST.json`; a dirty export is never labeled a clean commit.

## Signing and distribution

`POLAR_STATION_CODESIGN_IDENTITY=-` explicitly selects local ad-hoc signing.
Without that choice, the current certificate-selection behavior remains.
Xcode intermediate automatic signing is disabled; final signing is explicit.
Ad-hoc signing is neither notarization nor a trusted publisher identity. A
separate local signing choice does not authorize uploading or distribution.
TCC/automation permissions and their persistence still need actual Mac tests.
Do not disable Gatekeeper globally.

The scripts do not enroll in an Apple program, create credentials or notarize.
Packaging with `package_macos_free.sh` is separate from compilation. Native
licensing, exact binary provenance, installation and interactive behavior must
be checked for the actual bytes before a release is claimed.

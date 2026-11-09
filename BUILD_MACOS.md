# macOS source build (Apple Silicon)

This build entry has portability fixes and Linux static contract tests only.
**A complete macOS compile, launch, signing, graphics, and DMG installation test
has not yet passed for this exported source. Do not label it a tested release.**

Required inputs, installed by the builder (these scripts do not install tools):

- Apple Silicon Mac, Xcode with command-line developer tools selected.
- Rust toolchain from `rust-toolchain.toml`, with `aarch64-apple-darwin` target.
- Zig **0.15.2**, gettext `msgfmt`, Python 3, SCons **4.9.1**.
- Ghostty checkout at `332b2aefc6e72d363aa93ab6ecfc86eeeeb5ed28`.
- Godot checkout at `ed1daf0bf001b61586d9930840f2f1394092c079`,
  and Godot editor **4.7.2**. These existing archive pins are preserved, not
  substituted with whatever the package manager currently provides.
- Graphviz installation prefix, including its libraries and plugins.

Example using explicit local input paths:

```sh
export POLAR_STATION_GHOSTTY_CHECKOUT=/absolute/path/to/ghostty
export POLAR_STATION_ZIG=/absolute/path/to/zig
export POLAR_GRAPHICS_GODOT_CHECKOUT=/absolute/path/to/godot
export POLAR_GRAPHICS_SCONS=/absolute/path/to/scons
export POLAR_GRAPHICS_EDITOR=/absolute/path/to/Godot.app/Contents/MacOS/Godot
export POLAR_GRAPHICS_GRAPHVIZ=/absolute/path/to/graphviz-prefix
bash automation/polar_station/build_macos_graphics.sh
bash automation/polar_station/build_macos.sh --configuration Release
```

The two upstream checkouts are cloned into disposable build directories. Patches
are not applied to your original checkout. The source revision and Ghostty source
archive digest are checked. Outputs are under `target/polar-station-macos/`.
Rust builds use this export's `target/`, not an inherited `CARGO_TARGET_DIR`.

The SDK is discovered with `xcrun --sdk macosx --show-sdk-path`. Override it with
`POLAR_STATION_MACOS_SDK` if the selected SDK is incompatible with pinned Zig.
Discovery is not a compatibility guarantee; macOS 26/27 builds remain unverified.

## Signing and distribution

The default is **ad-hoc signing** (`POLAR_STATION_CODESIGN_IDENTITY=-`), with
Xcode automatic signing disabled. No paid Apple account is needed for that mode.
It is **not notarization or Developer ID signing**. A downloaded app may require
explicit approval in System Settings > Privacy & Security. Do not disable
Gatekeeper globally. TCC/automation permissions and their persistence need actual
Mac testing; ad-hoc signing does not establish a trusted publisher identity.

To use an already configured signing identity, explicitly set
`POLAR_STATION_CODESIGN_IDENTITY`. This script does not enroll in an Apple program,
create credentials, or notarize anything. After the build succeeds, packaging an
unnotarized DMG is a separate step with `package_macos_free.sh` if included.
The native dependency license inventory and distribution gates must also pass.

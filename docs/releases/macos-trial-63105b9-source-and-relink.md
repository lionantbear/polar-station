# Polar Station Mac trial: source and rebuilding

This page supplies source and modification directions for the newly exported Apple Silicon trial built from **63105b965dee5bb6cedb88720382dd17d07aa516**. Downloadable release files are the DMG and SHA256SUMS. Full third-party license texts and the component index are inside the DMG's THIRD-PARTY-NOTICES folder. Upstream sources are obtained from their public repositories and official archives, not from extra release attachments.

The exact native build is [run 43373252716, attempt 1](https://github.com/lionantbear/polar-station/actions/runs/43373252716). Its build, signature, installer-copy launch and terminal-check results are recorded there. These checks do not establish clean-machine microphone permission, Codex authentication or full interactive GUI acceptance.

## Exact application and upstream sources

- [Complete Polar Station source, Cargo.lock, native modules and build scripts](https://github.com/lionantbear/polar-station/tree/63105b965dee5bb6cedb88720382dd17d07aa516). First-party code is AGPL-3.0; upstream files retain their own licenses.
- [Ghostty 1.3.1 source](https://github.com/ghostty-org/ghostty/tree/332b2aefc6e72d363aa93ab6ecfc86eeeeb5ed28).
- [Godot native source](https://github.com/godotengine/godot/tree/ed1daf0bf001b61586d9930840f2f1394092c079).
- [Sparkle 2.9.0 source and complete LICENSE](https://github.com/sparkle-project/Sparkle/tree/21d8df80440b1ca3b65fa82e40782f1e5a9e6ba2).

The application source's `apps/polar-station-macos/ghostty-overlay/ghostty-v1.3.1.patch`, `apply-overlay.sh`, `automation/polar_station/apply_display_overlay.py`, `native_action_overlay.py`, Ghostty module files and `StationDisplay.zig` contain the Ghostty integrations. The overlay binds the local Swift package path for your checkout. `automation/polar_station/build_macos_graphics.sh` applies the Godot macOS entry-point change from `OS_MacOS_NSApp` to `OS_MacOS_Headless` and includes the public custom modules. These scripts recreate the modified source from the upstream pins; generated headers/framework metadata are produced by the build.

Cargo's checked-in lockfile specifies exact registry versions, archive checksums and Git revisions. `cargo fetch --locked` retrieves them. A registry package NAME at VERSION is available at `https://static.crates.io/crates/NAME/NAME-VERSION.crate`; verify its SHA-256 against Cargo.lock. Preserve full Git workspaces, including sibling path dependencies and workspace manifests. The compiled Git dependency roots are:

- [Codex](https://github.com/leoricshcn/codex/tree/6203904da314b5dd5445dfdae2fa00581148f64d), workspace `codex-rs/`.
- [mxc](https://github.com/microsoft/mxc/tree/b21120168d70befe12356411423266bfef168446).
- [nucleo](https://github.com/helix-editor/nucleo/tree/4253de9faabb4e5c6d81d946a5e35a90f87347ee).
- [MCP Rust SDK](https://github.com/modelcontextprotocol/rust-sdk/tree/98a54789b80110ad210495c4ad845c89e1980471).
- [tokio-tungstenite](https://github.com/openai-oss-forks/tokio-tungstenite/tree/132f5b39c862e3a970f731d709608b3e6276d5f6).
- [tungstenite-rs](https://github.com/openai-oss-forks/tungstenite-rs/tree/9200079d3b54a1ff51072e24d81fd354f085156f).

Local/vendor Cargo packages are in the exact Polar Station source. The MPL-covered packages' source is available by these same exact lockfile locations: colored 3.1.1; gdextension-api 0.3.3; godot, godot-bindings, godot-cell, godot-codegen, godot-core, godot-ffi and godot-macros 0.4.5; option-ext 0.2.0; symphonia 0.6.0; symphonia-bundle-mp3, symphonia-format-isomp4, symphonia-format-mkv, symphonia-format-ogg and symphonia-format-riff 0.6.0; symphonia-common, symphonia-core and symphonia-metadata 0.6.1; webm 2.2.0; webm-sys 2.2.1; and nucleo/nucleo-matcher at the linked Git revision. The vendored libwebm notices remain applicable alongside MPL terms. For a modified Cargo dependency, retain its workspace and use Cargo's `[patch]` mechanism for its original registry/Git source, pointing to the package directory. Review intentional lockfile changes before rebuilding.

Ghostty's pinned `build.zig.zon` and `pkg/*/build.zig.zon` files specify its other Zig/native dependencies by URL and content hash. Zig resolves these normally over the network; this is not an offline vendor tree.

## Vendored bip0032 source and modification notices

The exact application source includes `bip0032 0.2.2`, its Apache-2.0 LICENSE,
original source identity and [Polar change record](https://github.com/lionantbear/polar-station/blob/63105b965dee5bb6cedb88720382dd17d07aa516/vendor/bip0032/POLAR-PATCHES.md).
The modified production files carry prominent per-file modification notices.
The original registry archive SHA-256 is `9552cc87e70ae1cb0d7af0c9710a3341b92950117ece2fd5b99eb3453a92ed0a`.
The DMG retains the modification record among its full notices.

## Graphviz and its supplied libraries

Use the exact source archives below. The Homebrew keg revision `_4` for GTS does not change the upstream 0.7.6 archive version.

| Component | Official source | SHA-256 |
|---|---|---|
| gettext 1.0 | https://ftpmirror.gnu.org/gettext/gettext-1.0.tar.gz | 85d99b79c981a404874c02e0342176cf75c7698e2b51fe41031cf6526d974f1a |
| GLib 2.90.1 | https://download.gnome.org/sources/glib/2.90/glib-2.90.1.tar.xz | 93c941aa17d5eb1d53fe838365f29a8b4e539c222a256d974ec8f30fc413e396 |
| libtool/libltdl 2.6.2 | https://ftpmirror.gnu.org/libtool/libtool-2.6.2.tar.xz | 2ef1067c16c97db930fd740cc9bc3d3ba9a583804ae5ac42cc3e8719e49e191e |
| Graphviz 16.1.0 | https://gitlab.com/api/v4/projects/4207231/packages/generic/graphviz-releases/16.1.0/graphviz-16.1.0.tar.xz | 0f661718f3e5268dc3bad0fb53ab646605d26db70f4080799aff1dc2f61783c3 |
| GTS 0.7.6 | https://downloads.sourceforge.net/project/gts/gts/0.7.6/gts-0.7.6.tar.gz | 059c3e13e3e3b796d775ec9f96abdce8f2b3b5144df8514eda0cc12e13e8b81e |
| PCRE2 10.49 | https://github.com/PCRE2Project/pcre2/releases/download/pcre2-10.49/pcre2-10.49.tar.bz2 | 53c156e1ba416a20da8e65395daa132da0d80e76910424caca3fcdae7831d384 |
| Static gettext 0.24 | https://deps.files.ghostty.org/gettext-0.24.tar.gz | c918503d593d70daf4844d175a13d816afacb667c06fba1ec9dcd5002c1518b7 |

The build recipes are at [Homebrew core ffc79088768f59d49d304f3ff00de576b4ea8f69](https://github.com/Homebrew/homebrew-core/tree/ffc79088768f59d49d304f3ff00de576b4ea8f69): `Formula/g/gettext.rb`, `Formula/g/glib.rb`, `Formula/g/graphviz.rb`, `Formula/g/gts.rb`, `Formula/lib/libtool.rb` and `Formula/p/pcre2.rb`. These include configure/Meson arguments and install transformations. Their source/build procedures match the supplied components; bottle metadata, conflict declarations and Homebrew network-policy metadata are not source patches.

GLib uses [hardcoded-paths.diff](https://github.com/Homebrew/homebrew-core/blob/ffc79088768f59d49d304f3ff00de576b4ea8f69/Patches/glib/hardcoded-paths.diff), SHA-256 `d846efd0bf62918350da94f850db33b0f8727fece9bfaf8164566e3094e80c97`, with `@@HOMEBREW_PREFIX@@` substitution and the recipe's `gio/meson.build`/pkg-config transformations. Its build resource [gobject-introspection 1.86.0](https://download.gnome.org/sources/gobject-introspection/1.86/gobject-introspection-1.86.0.tar.xz) has SHA-256 `920d1a3fcedeadc32acff95c2e203b319039dd4b4a08dd1a2dfd283d19c0b9ae`. GTS's inline recipe patch changes the example program's `pgm.h` include to `netpbm/pgm.h`; the recipe runs autoreconf and selects GNU17 C. No separate unpublished library patch is needed.

To rebuild or modify these libraries, use those source versions and the source-build steps in those recipes, including their patches/resources. A dedicated Homebrew build installation can use that exact core checkout, with `HOMEBREW_NO_INSTALL_FROM_API=1` and `HOMEBREW_NO_AUTO_UPDATE=1`, and `brew install --build-from-source` (or `brew reinstall --build-from-source` for already installed formulae). Build the listed dependencies before Graphviz rather than substituting newer bottles. For modifications, edit the extracted source or the corresponding recipe patch; retain its license notices. Set `POLAR_GRAPHICS_GRAPHVIZ` to your resulting Graphviz prefix. Exact historical bottle bytes are not necessary to modify and relink the application.

## Build the complete app

Use an Apple Silicon Mac, Xcode 26.3 and a Zig-compatible macOS 15 SDK, Zig 0.15.2, Rust with `aarch64-apple-darwin`, Python 3, SCons 4.9.1, gettext's msgfmt, the Graphviz prefix above, and Godot editor 4.7.2. The pinned [native workflow](https://github.com/lionantbear/polar-station/blob/63105b965dee5bb6cedb88720382dd17d07aa516/.github/workflows/macos-release-build.yml) contains exact ordinary-tool download URLs/checksums and setup commands. Obtain Xcode/Apple SDKs separately under Apple's terms; Apple's [developer downloads](https://developer.apple.com/download/all/) provide historical Xcode/Command Line Tools releases, including the Xcode 16 generation containing macOS 15 SDKs. Set `POLAR_STATION_MACOS_SDK` explicitly to your macOS 15 SDK; otherwise the main builder requires `/Library/Developer/CommandLineTools/SDKs/MacOSX15.sdk`. The main builder forwards `DEVELOPER_DIR` into its clean app-build environment; when unset it uses the active command-line developer-tools selection. Xcode 26.4+ is not interchangeable here because Zig 0.15.2 cannot parse its newer libSystem stubs.

```sh
git clone https://github.com/lionantbear/polar-station.git
cd polar-station
git checkout --detach 63105b965dee5bb6cedb88720382dd17d07aa516
cargo fetch --locked
rustup target add aarch64-apple-darwin
# Clone each upstream into a separate directory, then checkout its exact pin above.
export POLAR_STATION_GHOSTTY_CHECKOUT=/absolute/path/to/ghostty
export POLAR_GRAPHICS_GODOT_CHECKOUT=/absolute/path/to/godot
export DEVELOPER_DIR=/Applications/Xcode_26.3.app/Contents/Developer
export POLAR_STATION_MACOS_SDK=/absolute/path/to/compatible/MacOSX15.sdk
export POLAR_STATION_ZIG=/absolute/path/to/zig-0.15.2/zig
export POLAR_STATION_MSGFMT=/absolute/path/to/gettext/bin/msgfmt
export POLAR_GRAPHICS_SCONS=/absolute/path/to/scons-4.9.1
export POLAR_GRAPHICS_EDITOR=/absolute/path/to/Godot.app/Contents/MacOS/Godot
export POLAR_GRAPHICS_GRAPHVIZ=/absolute/path/to/graphviz-prefix
export POLAR_STATION_CODESIGN_IDENTITY=-
bash automation/polar_station/build_macos_graphics.sh
bash automation/polar_station/build_macos.sh --configuration Release
codesign --verify --deep --strict 'target/polar-station-macos/Release/Polar Station.app'
```

The two scripts perform the complete graphics, Rust, Zig, Swift/Xcode, resource-copy, bundle-relative load-path and ad-hoc signing steps. The result is `target/polar-station-macos/Release/Polar Station.app`. No publisher signing key, paid Apple account or notarization service is required for your modified build. Normal macOS approval may still be needed to open it; do not disable system security protections. The trial's limited launch test is not a claim of every GUI scenario or a bit-identical recipient rebuild.

## Modify and relink static gettext

Ghostty statically links gettext 0.24's libintl. Editing the upstream checkout alone will not work: the normal app script creates a clean disposable clone. In your application source copy, generate an alternate build script that installs your edited gettext after the overlay, preserving the rest of the complete app build:

```sh
# Download/extract gettext-0.24.tar.gz from the table, verify its SHA-256, and edit it.
export POLAR_STATION_GETTEXT_SOURCE=/absolute/path/to/edited-gettext-0.24
python3 - <<'PY'
from pathlib import Path
p = Path('automation/polar_station/build_macos.sh')
s = p.read_text()
marker = '"$overlay" "$build_source"\n'
assert s.count(marker) == 1
hook = '''python3 - "$build_source" "$POLAR_STATION_GETTEXT_SOURCE" <<'PY_GETTEXT'
from pathlib import Path
import re, shutil, sys
root, edited = map(Path, sys.argv[1:])
assert (edited / 'gettext-runtime/intl/gettext.c').is_file()
pkg = root / 'pkg/libintl'
shutil.copytree(edited, pkg / 'local-gettext', symlinks=True)
p = pkg / 'build.zig.zon'
s = p.read_text()
pattern = r'(?s)\\.gettext\\s*=\\s*\\.\\{.*?\\},'
s, count = re.subn(pattern, '.gettext = .{ .path = "local-gettext", .lazy = true },', s)
assert count == 1
p.write_text(s)
PY_GETTEXT
'''
Path('automation/polar_station/build_macos_with_local_gettext.sh').write_text(s.replace(marker, marker + hook))
PY
bash automation/polar_station/build_macos_with_local_gettext.sh --configuration Release
```

This selects your local library source, recompiles it and relinks the full application, including the native XCFramework and app. It does not require a proprietary object-file kit. Keep the entire gettext source layout. Do not call this an unmodified upstream build. For dynamically linked GLib/gettext/GTS/libltdl, rebuild a compatible Graphviz prefix as above and run both normal application build scripts; they copy the dependency closure, adjust install names and sign your modified result.

## License choices and modification rights

For this distribution, the LGPL-covered parts of static gettext 0.24, gettext 1.0 libintl, GLib/gmodule/gthread 2.90.1, GTS 0.7.6 and libltdl 2.6.2 are conveyed using the **LGPL version 3 option expressly offered by their version-2/2.1-or-later grants**. This is an exercise of the upstream grant, not a new license imposed on upstream or a conversion of the whole application to GPL. Original copyright notices, other per-file grants and libltdl's special exception remain preserved. The full original terms, LGPLv3 and GPLv3 are in the DMG.

Recipients may modify these libraries, rebuild/relink the application with those modifications, and reverse engineer the combined work for debugging them. There is no additional Polar Station restriction on those activities. The complete source and build route above provide Corresponding Application Code for LGPLv3 §4(d)(0); ad-hoc signing permits installation of your modified build without our keys. Preserve notices and comply with the applicable licenses when redistributing your changes. Graphviz's applicable source terms are EPL-2.0, and the listed MPL-covered packages retain MPL-2.0. Where half 1.8.3 and self_cell offer Apache-2.0 as an alternative, this distribution uses that offered alternative; retain their copyright notices.

Source is available now at the exact links above, with equivalent public download access. This uses AGPLv3/GPLv3 §6(d) and LGPLv3 §4(d)(0)'s online-source method, not a written offer for future delivery. The source directions must remain next to the binary download. Upstream hosting does not remove the distributor's applicable responsibility for source availability; this page does not add a separate service guarantee or a new written-offer commitment.

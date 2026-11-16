# Polar Station: source, rebuilding and library modifications

This page accompanies the Apple Silicon preview built from 46dc85650ef7aece591357a6de93e62faab39595. The full
application source, Cargo.lock, native modules, patches and build entries are
[available at that exact commit](https://github.com/lionantbear/polar-station/tree/46dc85650ef7aece591357a6de93e62faab39595).
The native build is [run 43810179030, attempt 1](https://github.com/lionantbear/polar-station/actions/runs/43810179030).
Downloadable release files are the DMG and SHA256SUMS. Complete original license
texts and the component index are in the DMG's THIRD-PARTY-NOTICES directory.

## Application and upstream source

Follow [BUILD_MACOS.md](https://github.com/lionantbear/polar-station/blob/46dc85650ef7aece591357a6de93e62faab39595/BUILD_MACOS.md)
for the single complete macOS build command. It builds graphics, Rust, Zig and
Swift/Xcode, copies resources and signs the result locally. Publisher keys,
notarization credentials, private automation and a release account are not
required. You can modify the source and rerun the same command.

The pinned upstream native sources are:

- [Ghostty](https://github.com/ghostty-org/ghostty/tree/332b2aefc6e72d363aa93ab6ecfc86eeeeb5ed28).
- [Godot](https://github.com/godotengine/godot/tree/ed1daf0bf001b61586d9930840f2f1394092c079).
- [Sparkle 2.9.0](https://github.com/sparkle-project/Sparkle/tree/21d8df80440b1ca3b65fa82e40782f1e5a9e6ba2).

Station's Ghostty patch is in `apps/polar-station-macos/ghostty-overlay/`.
`apps/polar-tmux-shared/ghostty/overlay.py` and `native_actions.py` apply renderer
and action changes; native Swift/Zig integration is beside the patch.
`build-macos.py` applies the Godot headless entry change and selects the custom
modules in `apps/polar-graphics-native/godot-modules`. All integration code is
in the application source linked above. Generated headers and framework
metadata are produced during the build.

Cargo.lock gives exact package versions, registry archive SHA-256 values and
Git revisions. `cargo fetch --locked` obtains those sources. A registry package
NAME at VERSION is downloadable as
`https://static.crates.io/crates/NAME/NAME-VERSION.crate`; compare its SHA-256
with Cargo.lock. Keep full Git workspaces, including sibling packages and their
workspace manifest. `licenses/cargo-evidence.json` and
`licenses/mpl-source-inventory.json` give collected notices and MPL source
identities. These routes also cover the MPL-covered Godot Rust bindings,
Symphonia components, webm, colored, option-ext and nucleo. For edited libraries,
use Cargo's `[patch]` entries for the original registry/Git source and point to
the local package directories. Review intentional lockfile changes.

Local and vendored packages are included in this exact application source.
Their original licenses and modification records remain applicable, including
`vendor/bip0032/POLAR-PATCHES.md` and the EROFS upstream lock and patch. Preserve
the per-file and full-text notices. Ghostty's `build.zig.zon` and
`pkg/*/build.zig.zon` specify other Zig/native source URLs and content hashes.
Zig fetches those over the network; the build does not require an offline vendor tree.

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

To rebuild or modify these libraries, use those source versions and the source-build steps in those recipes, including their patches/resources. A dedicated Homebrew build installation can use that exact core checkout, with `HOMEBREW_NO_INSTALL_FROM_API=1` and `HOMEBREW_NO_AUTO_UPDATE=1`, and `brew install --build-from-source` (or `brew reinstall --build-from-source` for already installed formulae). Build the listed dependencies before Graphviz rather than substituting newer bottles. For modifications, edit the extracted source or the corresponding recipe patch; retain its license notices. Pass your resulting Graphviz prefix to `build-macos.py --graphviz`. Exact historical bottle bytes are not necessary to modify and relink the application.

## Complete app and library relinking

Use the prerequisites and upstream checkouts in BUILD_MACOS.md. Xcode 26.3 was
selected for the preview; Zig 0.15.2 needs a compatible macOS 15 SDK. Xcode
26.4 and later use libSystem stubs that this Zig version cannot parse. Obtain
Apple SDKs/Xcode under Apple's terms from its developer downloads. The normal
command accepts an explicit `--sdk` path and respects `DEVELOPER_DIR`.

```sh
git clone https://github.com/lionantbear/polar-station.git
cd polar-station
git checkout --detach 46dc85650ef7aece591357a6de93e62faab39595
cargo fetch --locked
# Prepare the tools and pinned upstream checkouts described in BUILD_MACOS.md.
export DEVELOPER_DIR=/Applications/Xcode_26.3.app/Contents/Developer
python3 build-macos.py --ghostty ../ghostty --godot ../godot \
  --editor /Applications/Godot.app/Contents/MacOS/Godot \
  --graphviz /absolute/path/to/graphviz-prefix \
  --zig /absolute/path/to/zig-0.15.2/zig \
  --sdk /absolute/path/to/MacOSX15.sdk
```

The result is `target/macos/Release/Polar Station.app`. This rebuild path is for
modification and relinking; a bit-identical reproduction is not promised.
Normal macOS approval may be needed to open your locally signed app.

Ghostty statically links gettext 0.24's libintl. To modify it, obtain the
checksum-verified complete gettext archive from the table above, edit it, and
keep its full source layout. In your copy of `build-macos.py`, insert the
following directly after `apply_display(ghostty, 'macos')`, at the same indent:

```python
import re
edited = Path('/absolute/path/to/edited-gettext-0.24')
assert (edited / 'gettext-runtime/intl/gettext.c').is_file()
pkg = ghostty / 'pkg/libintl'
shutil.copytree(edited, pkg / 'local-gettext', symlinks=True)
manifest = pkg / 'build.zig.zon'
text, count = re.subn(r'(?s)\.gettext\s*=\s*\.\{.*?\},',
    '.gettext = .{ .path = "local-gettext", .lazy = true },', manifest.read_text())
assert count == 1
manifest.write_text(text)
```

Run the same full app build command. It recompiles the edited library and
relinks the XCFramework and complete app, then applies local ad-hoc signing.
Editing the supplied upstream checkout alone does not replace this step: the
entry builds from a disposable clone of its committed contents. For dynamically
linked GLib/gettext/GTS/libltdl, rebuild a compatible Graphviz prefix using the
source recipes above, pass that prefix to `--graphviz`, and rebuild the app.
The entry copies its dependency closure, adjusts bundle-relative library paths
and signs the modified result. No proprietary object-file kit is required.

## Bundled target tools

The application includes unmodified tmux 3.8 tool archives for local and SSH
targets. Their exact per-platform hashes and upstream release are recorded in
[`target_tools/tmux-3.8/manifest.json`](https://github.com/lionantbear/polar-station/blob/46dc85650ef7aece591357a6de93e62faab39595/crates/polar-station-runtime/src/services/target_tools/tmux-3.8/manifest.json).
The upstream build and release source is [tmux-builds v3.8](https://github.com/tmux/tmux-builds/releases/tag/v3.8).
The original provided notices for tmux, utf8proc/Unicode data, libevent, ncurses
and musl accompany the DMG unchanged. The archived binaries and their original
LICENSES archive are also part of the exact application source tree.

## License choices and modification rights

The LGPL-covered parts of static gettext 0.24, gettext 1.0 libintl,
GLib/gmodule/gthread 2.90.1, GTS 0.7.6 and libltdl 2.6.2 are conveyed using the
LGPL version 3 option offered by their version-2/2.1-or-later grants. Original
copyright notices, per-file grants and libltdl's exception remain preserved.
The full original terms, LGPLv3 and GPLv3 accompany the DMG. Graphviz retains
EPL-2.0 and MPL-covered packages retain MPL-2.0. Where half/self_cell offer
Apache-2.0 as an alternative, this distribution uses that offered alternative.

You may modify these libraries, rebuild/relink the combined application, and
reverse engineer it to debug those modifications. Polar Station adds no
restriction on those activities. The complete source/build route provides
Corresponding Application Code under LGPLv3 section 4(d)(0); ad-hoc signing
allows installation without our keys. Comply with the relevant licenses and
preserve notices when redistributing changes.

Source is available at the exact links above through the online-source method,
including AGPLv3/GPLv3 section 6(d) and LGPLv3 section 4(d)(0). This is not a
written offer of future delivery. These directions remain linked next to the
binary download; upstream hosting does not remove applicable distributor
source-availability obligations.

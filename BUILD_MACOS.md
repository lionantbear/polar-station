# Build on macOS

Apple Silicon and macOS 15 or later are required. Install Xcode, Rust (the
repository's `rust-toolchain.toml` selects the version), Zig 0.17.0, Python 3.11+,
rsync and Graphviz/gettext. The Zig build needs a compatible macOS 15 SDK; select the
SDK explicitly below. Xcode builds use your selected Xcode installation.

```sh
brew install graphviz gettext sccache
python3 -m venv .venv
.venv/bin/pip install scons==4.9.1
export PATH="$PWD/.venv/bin:$(brew --prefix gettext)/bin:$PATH"
rustup target add aarch64-apple-darwin
```

Prepare these upstream sources outside this repository. The build reads their
committed contents and applies Station changes in a managed build copy;
it does not alter either supplied checkout.

```sh
git clone https://github.com/ghostty-org/ghostty.git ../ghostty
git -C ../ghostty checkout 83fb95624b0439ee660c2679b43b38c8b63769cd
git clone https://github.com/godotengine/godot.git ../godot
git -C ../godot checkout ed1daf0bf001b61586d9930840f2f1394092c079
```

Install the [Godot 4.7.2 editor](https://github.com/godotengine/godot/releases/tag/4.7.2-stable).
Then, from this source directory:

```sh
python3 build-macos.py \
  --ghostty ../ghostty --godot ../godot \
  --editor /Applications/Godot.app/Contents/MacOS/Godot \
  --graphviz "$(brew --prefix graphviz)" \
  --sdk /Library/Developer/CommandLineTools/SDKs/MacOSX15.sdk
```

Use `--zig /path/to/zig` if it is not on PATH, `--jobs N` to limit parallel
compilation, and `--debug` for a debug build. Cargo and Godot compilation caches
remain under `target/` for subsequent builds. Output:
`target/macos/Release/Polar Station.app` (or `Debug`). You can modify this
repository and rerun the same command. ZIP downloads work too; this source
folder does not need `.git`, an export manifest, an account or a release service.

The entry shows Cargo, SCons, Zig and Xcode commands directly. Renderer overlay
code is in `apps/polar-tmux-shared/ghostty/overlay.py` and `native_actions.py`;
graphics resource assembly is in `apps/polar-graphics-native/resources*.py`.

The local app is ad-hoc signed, including its microphone permission declaration.
It is not notarized. The command builds an app; it does not install it, create an
installer, upload it or change system permissions.

If you already have a verified Station Godot dependency directory, add
`--godot-prebuilt /path/to/directory`. It must contain `libgodot.dylib` and
`manifest.json` matching this source's Godot pin, patch, modules, build options
and local toolchain. The entry checks the library hash, arm64 architecture and
required exports before reuse. The pinned Godot checkout is still required.
Without this option, the entry builds Godot from source as above.

Incremental rebuilds use fixed directories under
`target/polar-station/native/`. Keep these directories between builds: they
contain the managed Ghostty source, Zig compilation cache and native build
outputs (including Xcode DerivedData on macOS). The entry updates changed source
contents and removes deleted source files without rewriting unchanged inputs.
A platform lock prevents two builds from modifying the same managed tree.
Godot's SCons outputs remain in `target/deps/godot`; unchanged graphics resources
are reused after checking their input identity and completed bundle digest.

Rerun the same command after editing source. `CARGO_TARGET_DIR` selects an
alternative persistent Cargo output directory; its default is unchanged.
The console reports tool durations and Zig cache results. A fresh source
checkout or missing cache still builds from source with the same command.

If `sccache` is on PATH, the entry uses it for Rust compiler requests, including
switches between the Host and desktop feature sets. Its persistent cache is
shared across builds; an explicit `RUSTC_WRAPPER` takes precedence. This optional
tool accelerates rebuilding and is not required to compile the source.

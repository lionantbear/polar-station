# Build on Linux

The current frontend targets Linux x86_64 with Wayland. Install Rust (version in
`rust-toolchain.toml`), Zig 0.17.0, Python 3.11+, Git, a C/C++ compiler, binutils,
pkg-config, rsync, CMake 3.18+ and SCons 4.11.1. Godot needs its normal Linux/Vulkan development
headers, including native GTK4 and libepoxy development packages for the graphics
bridge. These use the host development SDK; the explicit sysroot below is for
the Ghostty frontend. Install SCons in your own Python environment:

```sh
python3 -m venv .venv
.venv/bin/pip install scons==4.11.1
export PATH="$PWD/.venv/bin:$PATH"
```

Prepare Ghostty and Godot checkouts at the same revisions listed in
[BUILD_MACOS.md](BUILD_MACOS.md), and install the
[Godot 4.7.2 editor](https://github.com/godotengine/godot/releases/tag/4.7.2-stable).
The build keeps managed copies and leaves those supplied checkouts intact.

Supply a GTK development sysroot containing GTK4, libadwaita, gettext,
Blueprint 0.20 or later, Wayland, fontconfig, HarfBuzz, FreeType, EGL, gtk4-layer-shell and epoxy, under `usr/bin`, `usr/include`
and `usr/lib/x86_64-linux-gnu`. Supply a Graphviz sysroot containing `dot`, its
libraries/plugins and notices under `usr/`. These are explicit development
inputs; no private checkout or infrastructure account is needed. This entry
preserves the controlled sysroot build contract; it does not install packages
or create a container. A source download alone does not provide these sysroots.

```sh
python3 build-linux.py \
  --ghostty ../ghostty --godot ../godot \
  --editor /path/to/Godot_v4.7.2-stable_linux.x86_64 \
  --sysroot /path/to/gtk-sysroot \
  --graphviz /path/to/graphviz-sysroot
```

Use `--zig /path/to/zig`, `--jobs N`, or `--debug` as needed. Output is
`target/linux/release/bin/polarstation` (or `debug`), accompanied by the matching
Host, graphics resources, terminal resources and GNOME shortcut files. Keep
that output directory together.
Cargo and Godot compilation caches remain under `target/` for subsequent builds.

The entry shows Cargo, SCons and Zig commands directly. Product renderer edits
and graphics resource assembly remain beside their native application sources.
Modify source and rerun the command; Git and the export manifest are not
required in the Station source folder. Build commands do not install the app,
change desktop settings or publish a release.

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

An installed `sccache` is used automatically for Rust compiler requests unless
`RUSTC_WRAPPER` is explicitly set. It is optional; source builds work without it.

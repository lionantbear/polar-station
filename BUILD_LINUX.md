# Build on Linux

The current frontend targets Linux x86_64 with Wayland. Install Rust (version in
`rust-toolchain.toml`), Zig 0.15.2, Python 3.11+, Git, a C/C++ compiler, binutils,
pkg-config, CMake 3.18+ and SCons 4.11.1. Godot needs its normal Linux/Vulkan development
headers. Install SCons in your own Python environment:

```sh
python3 -m venv .venv
.venv/bin/pip install scons==4.11.1
export PATH="$PWD/.venv/bin:$PATH"
```

Prepare Ghostty and Godot checkouts at the same revisions listed in
[BUILD_MACOS.md](BUILD_MACOS.md), and install the
[Godot 4.7.2 editor](https://github.com/godotengine/godot/releases/tag/4.7.2-stable).
The build uses disposable clones and leaves those supplied checkouts intact.

Supply a GTK development sysroot containing GTK4, libadwaita, gettext,
Blueprint, Wayland, fontconfig, HarfBuzz and epoxy, under `usr/bin`, `usr/include`
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

# Continuous integration

`Station source build` runs on main pushes, pull requests and manual dispatch.
It uses Linux (Ubuntu 24.04) and macOS 15 GitHub-hosted runners, with the
repository's pinned Rust toolchain and Cargo.lock. Both jobs execute
`cargo build --workspace --locked` (actual compilation/linking, not just
`cargo check`). The macOS job additionally executes the Swift package build
against `target/debug/libpolar_tmux.a`.

A green workflow means these source components compile. It does **not** mean
the complete native application or release package has been built or tested.
SwiftPM contains library targets, not the full Ghostty application.

## Full native-app build gaps

The published Linux export currently references an absent
`automation.polar_station.build_rust_runtime` module and an absent root
`justfile`; these must be repaired before its native build scripts are
self-contained. The Linux installer also requires icon inputs not included in
the export. Fixes are separate from this CI setup and from the legacy-name audit.

Both full GUI builds require the exact documented Ghostty/Godot revisions,
Zig, SCons, editor and Graphviz resources. macOS packaging uses ad-hoc signing;
this CI does not request Apple certificates or notarization. Companion remains
disabled by the public profile. CI neither publishes binaries nor changes the
source-only release's licensing or legal-release gates.

No secrets are required. Jobs have read-only repository permissions, a
60-minute timeout and two Cargo build workers. New runs supersede obsolete
runs on the same branch. Initial build results must be read from Actions;
adding this workflow is not itself evidence of a successful build.

## Native macOS 26 validation

`macOS 26 native app validation` is a separate manual workflow (also runs when
its workflow file changes on main). It attempts the real embedded Godot,
Ghostty/native app and ad-hoc signed DMG pipeline on Apple Silicon macOS 26.
Ghostty/Godot sources are commit-pinned; downloaded Zig and Godot editor
archives are SHA-256 checked. It deliberately does not upload binaries or
create a Release. Build success is distinct from fresh-machine install/launch,
graphics/TCC testing and exact bundled-license clearance. Homebrew Graphviz
versions are recorded but not yet locked as reproducible release inputs.

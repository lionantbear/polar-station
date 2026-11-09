# Polar Station for macOS

Polar Station is one managed Ghostty application. It is not a standalone
Swift launcher, panel app, supervisor, or ordinary Ghostty extension.

## Runtime boundary

- Managed Ghostty owns the terminal window, tabs, native Surface tree,
  fullscreen entry, menu item, status item, and in-window Control Center.
- The Control Center is a cached 200-390-point Layout Map, or a 421-611-point
  Sessions-plus-Map left sibling of the terminal. It has no `NSPanel`, Pin, or
  Keep Open state.
- Swift renders the Rust projection, owns macOS geometry, applies the smallest
  safe native Surface-tree change, and submits typed intents.
- The bundled release `polar-tmux` Host owns tmux, SSH, content binding,
  validation, persistence, and Resources. Managed Ghostty leases it as a child.
- Ordinary Ghostty is a different application and remains untouched.

The canonical requirements and implementation ledger are in
`docs/subprojects/polar-tmux/design/control-center-ui-spec.json`.

## Source layout

- `ghostty-module/`: code compiled inside managed Ghostty.
- `ghostty-overlay/`: pinned, reviewable upstream Ghostty patch and lock.
- `Sources/PolarTmuxHostClient/`: authenticated typed Host client.
- `Sources/PolarTmuxMacUI/`: projection-only SwiftUI components.
- `Tests/`: focused Swift and source-boundary tests.

`Package.swift` exists for the reusable client, UI, native Workspace model, and
their tests. It intentionally exposes no macOS application executable.

## Build and install

Use the repository commands; do not build the full Rust workspace or invent a
second Xcode project.

```bash
just polar-station-macos-build Release
just polar-station-macos-install \
  target/polar-station-macos/Release/Polar\ Station.app \
  /Applications/Polar\ Station.app
```

Run build, signing, installation and tests through the existing `macstation:0.0`
tmux execution context on the Mac; an occupied pane uses background `run-shell`.
The [deployment guide](../../docs/subprojects/polar-tmux/deployment.md#macos-standalone-build-signing-and-installation-context)
records the commands and the SSH/Keychain signing failure checks.
Source synchronization uses Git fetch and fast-forward only.
The application, Rust Host and client library are Apple Silicon arm64 only.

The build requires the pinned Zig 0.15.2 toolchain, Xcode, the
controlled macOS SDK, gettext `msgfmt`, a release Rust Host, and the configured
Apple signing identity. Local results are development evidence; formal
acceptance requires the reviewed GitLab job at the exact pushed commit.

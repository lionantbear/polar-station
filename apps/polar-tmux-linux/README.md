# Polar Station Linux

The product Linux frontend is a Polar-owned Zig module compiled into the
pinned managed Ghostty GTK/Wayland application. Polar Station opens as a
maximized normal Ghostty terminal. The one `app.polar-station` toggle is
reached through the native `Polar Station…` menu item, `Ctrl+Alt+P`, or the
white-bear button immediately after Ghostty's original New Tab control. The
GTK4/libadwaita Control Center is attached only when requested as a real left
child pane inside that active Ghostty window. The existing terminal remains
the adjacent right child, and no controller toplevel or second taskbar surface
is created.

The retired Python/PyGObject frontend is excluded from this public repository.
The managed Zig frontend is a thin client of the per-user Rust Host:

- the Host owns execution targets, tmux inspection, mutations, revisions, and
  typed attachment plans;
- the Zig module owns Linux presentation and managed Ghostty application
  integration;
- the app never executes tmux, builds shell text, polls, or requests terminal
  capture and private path/process fields.

Linux topology responses use the dedicated `sanitized_snapshot`,
`sanitized_sessions`, `sanitized_windows`, and `sanitized_panes` wire results.
The IPC client rejects the full desktop topology result family, and the model
requires the exact privacy-safe DTO keys.

The local IPC socket defaults to:

```text
$XDG_RUNTIME_DIR/polar-tmux/host.sock
```

`POLAR_TMUX_IPC_SOCKET` may override it for an isolated development fixture.
Frames are bounded one-MiB JSON values with a four-byte big-endian length.

## Current implemented surface

The managed Ghostty Zig module locally implements the current Phase 5B.5E
Workspace Control Center slice:

- terminal-first launch and one on-demand Control Center;
- real Local/Remote tmux session discovery through the Rust Host;
- Add to Tab/Remove from Tab assignment through revisioned typed intents;
- all-tab Host-projected Layout Map and Monitor/Focus controls;
- managed multi-tab/split/surface presentation;
- complete Workspace snapshots from Rust plus a Zig-local applied/native state
  map that computes the least disruptive valid Ghostty change;
- revision-matched logical receipts that never expose native window, Tab,
  Surface, pointer, or process-wrapper identity;
- application-scoped Focus shortcuts;
- projected lifecycle action plus Resources/iPhone auxiliary windows.

Workspace rename/reorder dialogs, session create/rename/kill, complete
confirmation UX, and the global Resources engine are not delivered by this
slice. The retired Python bootstrap is not launched, imported, packaged, or a
supported development path.

## Public source scope

The obsolete Python frontend, its UI resources, pip package, development launcher,
and tests are not exported. Python scripts under `automation/polar_station/`
are build/export/license utilities, not the retired UI, and remain necessary.
The compatible `polar-tmux-linux` source directory and Rust Host/IPC identifiers
remain unchanged. The public exporter excludes the retired paths and rejects
snapshots that reintroduce them.

Ghostty's `new-window-command` action accepts Ghostty CLI arguments rather than
the child command directly. The adapter therefore prepends Ghostty's fixed
`-e` delimiter to the exact Host-produced program/argv vector. No argument is
joined, reparsed, or passed through a shell.

The final adapter boundary accepts only `tmux [-L <safe-socket>]
attach-session -t $<digits>` or the corresponding fixed
`ssh -S <Host-owned-control-socket> -o ProxyCommand=/usr/bin/false -tt
<safe-destination> <remote-tmux-attach>` slave plan. Other tmux verbs,
caller-supplied SSH policy, and WSL plans are rejected before native execution.
The canonical socket grammar is
`[A-Za-z0-9][A-Za-z0-9._-]{0,95}` and matches the Host producer.

For a Host-forwarded `open_session`, the prepared plan arrives only in the
Host-owned top-level `attach_plan` field of `frontend_execute`. The Linux
handler never sends a nested Host request before acknowledging an action;
normal request responses apply the Host-returned completion snapshot.

## Managed Ghostty development build

The managed build uses the pinned Ghostty revision, pinned Zig 0.15.2, reviewed
sysroot, and explicit `-Dpolar-tmux=true -Dapp-runtime=gtk
-Dgtk-wayland=true -Dgtk-x11=false` options. It builds only the managed
Ghostty artifact; it is not the repository-wide `polar` build. The reviewed
default is `-Doptimize=ReleaseFast`; `Debug` is an explicit diagnostic override
and is not valid for installed performance or human-review evidence.

The managed build requests maximization before every managed Ghostty window is
mapped. This product guarantee does not depend on the caller remembering
`--maximize=true`; the desktop launcher retains that argument as a redundant
declaration of intent. Ordinary Ghostty builds keep upstream startup behavior.
Module initialization only connects to the Host. The menu item, shortcut, and
toolbar bear all invoke `app.polar-station`, which adds or removes one Control
Center child pane in the active window. The bear button is compiled into both
supported Ghostty title-bar styles directly after New Tab.

The reviewed repository entrypoint is:

```text
just polar-station-linux-build \
  /absolute/path/to/pinned-ghostty-checkout \
  /absolute/path/to/zig-0.15.2 \
  /absolute/path/to/reviewed-sysroot
```

The checkout is a read-only source at the pinned commit. The entrypoint records
its HEAD, full dirty status, and archive digest, materializes `git archive HEAD`
under this repository's ignored `target/polar-station/ghostty-builds/`, and
applies the overlay, copies the Zig module, and builds only in that non-Git
staging directory. It fails if the source checkout changes during the build.

The entrypoint also creates a stable generated header search-prefix view of the
reviewed sysroot which excludes only its root-level `freetype` and `ft2build.h`
compatibility paths. It deliberately omits a `lib` link so that no generated
prefix path enters the ELF `RUNPATH`; libraries continue to resolve through
the fixed reviewed sysroot and pkg-config contract. Those header paths would
otherwise shadow Ghostty's pinned vendored FreeType headers. Neither the source
checkout nor the actual sysroot is rewritten, and GTK/pkg-config inputs,
`freetype2`, tools, libraries, and shared data remain available.

The focused regression check is:

```text
just test-polar-station-linux-build
```

## Wayland implementation capture

Current Linux implementation evidence uses one compositor-approved ScreenCast
session rather than one Screenshot-Portal prompt per image:

```text
gjs -m apps/polar-tmux-linux/tools/wayland-capture.mjs
gjs -m apps/polar-tmux-linux/tools/wayland-capture.mjs --source=monitor
```

GNOME asks the human to select the managed Polar Station window once. After the
tool prints `READY`, commands on standard input capture the same approved
window without another picker or screenshot button:

```text
capture /absolute/path/to/UI-SHOT.png
status
quit
```

Each capture obtains a fresh PipeWire remote file descriptor for the persistent
Portal session and passes it as a distinct descriptor to a one-frame
GStreamer pipeline. The tool refuses to overwrite a file, rejects relative or
non-PNG paths and concurrent requests, decodes the output to reject and remove
visually flat frames, and closes the Portal session on `quit` or end of input.
Window capture remains the default; use the explicit monitor mode only when
the compositor returns a black GPU-rendered window stream. The initial GNOME
permission remains human-owned and is never synthesized.

## Packaging

- `data/com.polarealm.PolarStation.desktop`: desktop launcher;
- `data/com.polarealm.PolarStation.metainfo.xml`: AppStream metadata;
- `data/icon-source/`: the editable Polar product SVG derived from Android;
- `data/icons/`: generated 48/64/128/256/512-pixel PNGs for GNOME Shell, all
  using the exact geometry and palette from Android's
  `ic_launcher_background.xml` and `ic_launcher_foreground.xml`. The SVG is
  deliberately not installed into the hicolor runtime theme because GNOME
  Shell otherwise selects it for small taskbar sizes and renders an empty or
  black icon on the supported development desktop;
- `systemd/polar-tmux-host.service`: optional per-user Host unit.

The executable, GTK application ID, D-Bus identity, desktop filename,
`StartupWMClass`, and icon theme name converge on the public application
identity `com.polarealm.PolarStation`. Ordinary Ghostty keeps its own identity and
icon.

The user unit deliberately uses `KillMode=process` and does not use
`PrivateTmp` or `PrivateDevices`; otherwise a Host restart could hide or kill
the user's tmux server and PTYs. It listens on the reviewed Companion port and
opts into authenticated discovery with
`host --listen 0.0.0.0:47820 --advertise`; pairing and encrypted application
frames protect the non-loopback listener, while secrets remain in the platform
secure store and are not mDNS data.

An operator can add a reviewed SSH target registry with a systemd drop-in. The
drop-in must clear `ExecStart` before restating the fixed listener and adding
`--target-config /absolute/path/to/targets.json`; target destinations and
credentials never enter the frontend target picker.

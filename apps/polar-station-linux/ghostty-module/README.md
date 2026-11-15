# Polar Station Ghostty Zig module

This directory is the Polar-owned Linux frontend. A controlled build copies it
to `src/apprt/gtk/polar_tmux/` in a non-Git staging tree materialized from the
pinned read-only Ghostty source checkout.

The module is intentionally thin:

- preserve Ghostty's normal initial terminal window;
- register `app.polar-station` and expose it as `Polar Station…` in Ghostty's
  native menu;
- add or remove the Control Center pane only after that explicit action;
- construct one native Workspace table from display-safe Control Center facts;
- render the native GTK/libadwaita controller;
- apply each structural user intent to that private native state before Host
  persistence, then
  execute the least disruptive valid change against Ghostty's existing GTK
  `Application`, `Window`, `Tab`, `SplitTree`, and `Surface`;
- replay the authoritative Host projection only for startup, refresh, or
  rollback after a rejected operation.

It must not implement tmux access, SSH lifecycle, Session validation, content
execution, or Resource supervision. Those remain in the shared Rust crates and
Host. Native owns Workspace Tab/Pane order, Monitor/Focus layout, Focus-main,
desired Pane-to-Session binding, pointer identity, exact Ghostty-tree
realization, and the local minimum-change calculation; none of those physical
facts crosses IPC.

Pure protocol and state-machine files must remain testable with Zig alone.
Ghostty-dependent files are compiled and tested only inside the exact pinned
upstream checkout.

The current implementation contains:

- bounded length framing and typed Host frame classification;
- exact frontend identity/generation, Control Center revision, and
  generation-bound Pane-content correlation;
- process-lifetime channel, request, and private-socket identity across warm
  window replacement, plus retryable exact Pane-host retirement leases;
- GObject weak-lifetime tokens that distinguish destroyed and replacement
  windows even when GTK reuses the same raw pointer address;
- a reconnecting same-user Unix-socket client whose worker never mutates GTK;
- strict Host attach `program + argv` validation with no shell reparsing;
- first-Tab reuse using Ghostty's bootstrap tab and shell Surface;
- local logical-to-native bindings that are never serialized;
- in-place tab title, tab order, Surface reuse/creation, exact split-tree,
  selection, focus, and font-size realization;
- retirement of omitted Surfaces only after the new tree is installed;
- compositor-owned maximized Linux outer geometry, with Control Center changes
  limited to child allocation and managed fullscreen disabled;
- no active full-Workspace render request or receipt protocol;
- Native-created Tab and Pane UUIDs preserved by the Rust persistence mirror;
- local-first add, assignment, selection, activation, removal, rename, and
  Monitor/Focus changes with authoritative projection rollback;
- a native GTK/libadwaita child pane that renders the Host catalog/runtime
  projection, Workspace/tab editor, real target/session catalog, canonical
  Layout Map, Monitor/Focus controls, and revisioned Open, Apply, Repair,
  confirmed Rebuild, and Detach requests;
- application-scoped previous/next/sidebar-slot Focus shortcuts that submit
  shared typed commands while GTK owns physical focus;
- terminal-first shell lifecycle: startup creates no controller; the native
  action adds one cached child pane beside the terminal, the same action or
  Escape removes its allocation, and a later action reuses that pane.

The capability advertisement contains no Workspace-rendering capability.
Shared SSH ControlMaster lifecycle is Host-owned; a terminal Surface runs only
the Rust Pane-content host for its target and tmux Session. Terminal bytes do
not traverse Workspace projection IPC. The controller sees only configured
target IDs and sanitized session facts. Global Resources remain
Workspace-independent.

Run the pure module Development Tests with the pinned Zig toolchain:

```text
# Private monorepo test entrypoint omitted from the public source export.
```

The retired Python/PyGObject Phase 5 bootstrap remains only in the private
monorepo as migration evidence. It is excluded from this public source export
and does not participate in the product runtime.

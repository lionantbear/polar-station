# Polar Station pinned Ghostty overlay

This directory owns the small integration layer that compiles the Polar Station
Zig frontend into an exact official Ghostty GTK source tree.

The source authority is `upstream.lock.json`. The initial pin is Ghostty
`v1.3.1`, peeled commit
`332b2aefc6e72d363aa93ab6ecfc86eeeeb5ed28`, requiring Zig `0.15.2`.

The overlay contract is:

- apply only inside a disposable non-Git build staging directory materialized
  from the exact pinned commit;
- treat the controlled Ghostty checkout as a read-only `git archive` source;
- never write under `/home/chris/dev/reference/`;
- never modify the installed ordinary Ghostty;
- copy Polar source only to `src/apprt/gtk/polar_tmux/`;
- patch only the lockfile-allowlisted upstream files;
- require the explicit `-Dpolar-tmux=true` build feature;
- emit `polarstation` with application ID
  `com.polarealm.PolarStation`;
- preserve Ghostty's ordinary initial terminal window and add the on-demand
  Control Center as a real left pane beside that terminal through Ghostty's
  native main menu;
- leave a normal Ghostty build behavior free of Polar startup.

The detailed runtime, transaction, threading, and evidence design is
`docs/subprojects/polar-tmux/design/linux-ghostty-zig-adapter.md`.

The reviewed Linux build adapter under `automation/polar_station/` verifies the
controlled source identity, materializes a clean archive under the main
repository's ignored `target/`, applies `ghostty-v1.3.1.patch`, and copies the
Polar-owned module only into that staging tree. `apply-overlay.sh` remains a
manual helper for a separately created disposable source tree. A patch conflict
is a failed upgrade result, not permission to edit the controlled source or
silently expand the upstream patch.

The overlay has been locally compiled with the pinned Zig 0.15.2 toolchain and
Wayland enabled. This is Development Test evidence only; a reviewed GitLab
Build Job must repeat the exact pinned operation and publish its input and
artifact digests before the artifact is accepted.

`polar_tmux_linux/` is the PyGObject GTK4/libadwaita frontend package.

It stays outside the Cargo workspace because it is a native Python adapter over
the Rust Host IPC contract. Shared tmux behavior and protocol authority remain
in the Rust crates.

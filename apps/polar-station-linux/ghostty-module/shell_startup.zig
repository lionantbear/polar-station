//! Product-owned startup policy for the managed Linux Ghostty shell.
//!
//! The desktop entry may repeat `--maximize=true`, but the Polar Station
//! build must not depend on any particular caller preserving that argument.
//! Ordinary Ghostty retains its configuration-owned startup behavior.

/// Decide whether a Ghostty window requests maximization before it is mapped.
pub fn shouldMaximize(
    configured_maximize: bool,
    polar_station_build: bool,
) bool {
    return configured_maximize or polar_station_build;
}

/// Polar Station owns a maximized Wayland window, never a fullscreen one.
/// Ordinary Ghostty retains its configuration-owned fullscreen behavior.
pub fn shouldFullscreen(
    configured_fullscreen: bool,
    polar_station_build: bool,
) bool {
    return configured_fullscreen and !polar_station_build;
}

/// An early Control Center request must wait for Wayland's maximize configure.
///
/// Presenting the pane and changing the default size before that configure can
/// replace the compositor request and leave the first window unmaximized.
pub fn shouldDeferController(
    maximized: bool,
    fullscreen: bool,
    current_width: c_int,
    current_height: c_int,
    polar_station_build: bool,
) bool {
    return polar_station_build and
        (!maximized or fullscreen or current_width <= 1 or current_height <= 1);
}

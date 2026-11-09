#!/bin/bash
# Standalone Station graphics dependency; application installation remains separate.
set -euo pipefail
[ "$(uname -s)" = Linux ] && [ "$(uname -m)" = x86_64 ] || { echo "Linux x86_64 is required." >&2; exit 64; }
root=$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd)
CARGO_TARGET_DIR="$root/target"
export CARGO_TARGET_DIR
cache=${POLAR_GRAPHICS_CACHE:-"$HOME/.cache/polar-3d-lab"}
godot=${POLAR_GRAPHICS_GODOT_SOURCE:-"$cache/godot-4.7.2-gtk"}
pin=ed1daf0bf001b61586d9930840f2f1394092c079
[ "$(git -C "$godot" rev-parse HEAD)" = "$pin" ] || { echo "Godot source pin mismatch: $pin" >&2; exit 65; }
# Never compile into or modify the user's pinned source checkout.
source_checkout=$godot
build_source=$(mktemp -d "${TMPDIR:-/tmp}/polar-station-godot.XXXXXXXX")
cleanup() { rm -rf -- "$build_source"; }
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' HUP TERM
git clone --local --no-checkout "$source_checkout" "$build_source"
git -C "$build_source" checkout --detach "$pin"
godot=$build_source
scons=${POLAR_GRAPHICS_SCONS:-}
if [ -z "$scons" ] && [ -x "$cache/build-venv/bin/scons" ]; then scons="$cache/build-venv/bin/scons"; fi
if [ -z "$scons" ]; then scons=$(command -v scons || true); fi
[ -x "$scons" ] || { echo "Pinned SCons 4.11.1 is required." >&2; exit 66; }
editor=${POLAR_GRAPHICS_EDITOR:-"$cache/Godot_v4.7.2-stable_linux.x86_64"}
[ -x "$editor" ] || { echo "Pinned Godot 4.7.2 editor required at $editor" >&2; exit 67; }
"$editor" --version | grep -Eq '^4[.]7[.]2([.]|$)' || { echo "Godot editor must be version 4.7.2." >&2; exit 68; }
# graphics_resources expects a staged Debian-style tree, not an arbitrary prefix.
graphviz=${POLAR_GRAPHICS_GRAPHVIZ:-"$cache/graphviz"}
[ -x "$graphviz/usr/bin/dot" ] && [ -d "$graphviz/usr/lib/x86_64-linux-gnu" ] || {
    echo "Set POLAR_GRAPHICS_GRAPHVIZ to a staged tree containing usr/bin/dot and usr/lib/x86_64-linux-gnu." >&2; exit 69;
}
"$scons" --version | grep -q 'v4.11.1'
"$scons" -C "$godot" -j "${POLAR_GRAPHICS_JOBS:-8}" platform=linuxbsd arch=x86_64 \
    target=template_debug library_type=shared_library vulkan=no opengl3=yes \
    x11=no wayland=no debug_symbols=no optimize=speed disable_path_overrides=no \
    module_jolt_physics_enabled=no module_godot_physics_2d_enabled=no module_godot_physics_3d_enabled=no \
    custom_modules="$root/apps/polar-graphics-native/godot-modules"
cargo build --locked --release --manifest-path "$root/Cargo.toml" \
    --target x86_64-unknown-linux-gnu -p polar-graphics-godot
extension_target="$CARGO_TARGET_DIR/x86_64-unknown-linux-gnu/release"
python3 "$root/automation/polar_station/graphics_resources.py" "$root" --build \
    --output "${POLAR_STATION_GRAPHICS_OUTPUT:-$root/target/polar-station/linux-graphics}" \
    --godot-library "$godot/bin/libgodot.linuxbsd.template_debug.x86_64.so" \
    --godot-editor "$editor" \
    --extension "$extension_target/libpolar_graphics_godot.so" \
    --graphviz-root "$graphviz"

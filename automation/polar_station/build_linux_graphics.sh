#!/bin/sh
# Standalone Station graphics dependency; application installation remains separate.
set -eu
[ "$(uname -s)" = Linux ] && [ "$(uname -m)" = x86_64 ]
root=$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd)
cache=${POLAR_GRAPHICS_CACHE:-"$HOME/.cache/polar-3d-lab"}
godot=${POLAR_GRAPHICS_GODOT_SOURCE:-"$cache/godot-4.7.2-gtk"}
pin=ed1daf0bf001b61586d9930840f2f1394092c079
[ "$(git -C "$godot" rev-parse HEAD)" = "$pin" ] || { echo "Godot source pin mismatch: $pin" >&2; exit 65; }
scons=${POLAR_GRAPHICS_SCONS:-"$cache/build-venv/bin/scons"}
"$scons" --version | grep -q 'v4.11.1'
"$scons" -C "$godot" -j "${POLAR_GRAPHICS_JOBS:-8}" platform=linuxbsd arch=x86_64 \
    target=template_release library_type=shared_library vulkan=yes opengl3=yes \
    x11=no wayland=no debug_symbols=no optimize=speed disable_path_overrides=no \
    module_jolt_physics_enabled=no module_godot_physics_2d_enabled=no module_godot_physics_3d_enabled=no \
    custom_modules="$root/apps/polar-graphics-native/godot-modules"
extension_target=$(cd "$root" && python3 -m automation.polar_station.build_rust_runtime \
    --source-root "$root" --profile release --scope graphics --operation build)
python3 "$root/automation/polar_station/graphics_resources.py" "$root" --build \
    --output "${POLAR_STATION_GRAPHICS_OUTPUT:-$root/target/polar-station/linux-graphics}" \
    --godot-library "$godot/bin/libgodot.linuxbsd.template_release.x86_64.so" \
    --godot-editor "$cache/Godot_v4.7.2-stable_linux.x86_64" \
    --extension "$extension_target/libpolar_graphics_godot.so" \
    --graphviz-root "$cache/graphviz"

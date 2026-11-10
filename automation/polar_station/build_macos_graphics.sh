#!/bin/bash
# Standard Mac Station dependency build. Source pin and patch are explicit inputs.
set -euo pipefail
[ "$(uname -s)" = Darwin ] && [ "$(uname -m)" = arm64 ]
root=$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd)
CARGO_TARGET_DIR="$root/target"
export CARGO_TARGET_DIR
cache=${POLAR_GRAPHICS_CACHE:-"$HOME/.cache/polar-graphics"}
godot=${POLAR_GRAPHICS_GODOT_CHECKOUT:-"$cache/godot"}
pin=ed1daf0bf001b61586d9930840f2f1394092c079
[ "$(git -C "$godot" rev-parse HEAD)" = "$pin" ] || { echo "Godot checkout must be pinned to $pin" >&2; exit 65; }
# Apply the integration patch only to an owned disposable clone, not the
# user's pinned upstream checkout (which may contain unrelated changes).
source_checkout=$godot
build_source=$(mktemp -d "${TMPDIR:-/tmp}/polar-station-godot.XXXXXXXX")
cleanup() { rm -rf -- "$build_source"; }
trap cleanup EXIT HUP INT TERM
git clone --local --no-checkout "$source_checkout" "$build_source"
git -C "$build_source" checkout --detach "$pin"
godot=$build_source
# This library is called by an existing NSApplication. Reuse the upstream OS
# wrapper that does not install its own delegate/runloop; GPU display is separate.
python3 - "$godot/platform/macos/libgodot_macos.mm" <<'PY'
from pathlib import Path
import sys
p = Path(sys.argv[1])
s = p.read_text()
old = 'new OS_MacOS_NSApp('
new = 'new OS_MacOS_Headless('
if old in s:
    assert s.count(old) == 1
    p.write_text(s.replace(old, new))
else:
    assert s.count(new) == 1, 'unexpected pinned libgodot entry'
PY
scons=${POLAR_GRAPHICS_SCONS:-}
if [ -z "$scons" ] && [ -x "$cache/tools/bin/scons" ]; then scons="$cache/tools/bin/scons"; fi
if [ -z "$scons" ]; then scons=$(command -v scons || true); fi
[ -x "$scons" ] || { echo "Pinned SCons 4.9.1 is required at $scons" >&2; exit 66; }
"$scons" --version | grep -q 'v4.9.1'
"$scons" -C "$godot" -j "${POLAR_GRAPHICS_JOBS:-6}" platform=macos arch=arm64 \
    target=template_debug library_type=shared_library generate_bundle=no \
    vulkan=no metal=no angle=no opengl3=yes debug_symbols=no optimize=speed disable_path_overrides=no \
    module_jolt_physics_enabled=no module_godot_physics_2d_enabled=no module_godot_physics_3d_enabled=no \
    custom_modules="$root/apps/polar-graphics-native/godot-modules"
# Retain actual modified build source before the disposable clone is removed.
# Local files only: caller separately controls any later distribution.
if [ -n "${POLAR_STATION_NATIVE_SOURCE_EVIDENCE:-}" ]; then
    mkdir -p "$POLAR_STATION_NATIVE_SOURCE_EVIDENCE"
    python3 "$root/automation/polar_station/capture_native_source.py" \
        --tree "$build_source" --component godot \
        --source-commit "$(git -C "$root" rev-parse HEAD)" \
        --build-script "$root/automation/polar_station/build_macos_graphics.sh" \
        --output "$POLAR_STATION_NATIVE_SOURCE_EVIDENCE/godot-built-source.tar.gz"
fi

mkdir -p "$root/target/build-evidence"
cargo build --message-format=json-render-diagnostics --locked --release --manifest-path "$root/Cargo.toml" \
    --target aarch64-apple-darwin -p polar-graphics-godot | tee "$root/target/build-evidence/cargo-graphics.jsonl"
editor=${POLAR_GRAPHICS_EDITOR:-"$cache/Godot.app/Contents/MacOS/Godot"}
[ -x "$editor" ] || { echo "Pinned Godot 4.7.2 editor required at $editor" >&2; exit 67; }
"$editor" --version | grep -Eq '^4[.]7[.]2([.]|$)' || {
    echo "Godot editor must be version 4.7.2." >&2; exit 68;
}
graphviz=${POLAR_GRAPHICS_GRAPHVIZ:-}
if [ -z "$graphviz" ] && command -v brew >/dev/null 2>&1; then graphviz=$(brew --prefix graphviz); fi
[ -n "$graphviz" ] && [ -x "$graphviz/bin/dot" ] || {
    echo "Set POLAR_GRAPHICS_GRAPHVIZ to a Graphviz prefix containing bin/dot." >&2; exit 69;
}
output="$root/target/polar-station-macos/graphics"
python3 "$root/automation/polar_station/graphics_macos.py" \
    --source "$root" --godot "$godot/bin/libgodot.macos.template_debug.arm64.dylib" \
    --editor "$editor" --extension "$root/target/aarch64-apple-darwin/release/libpolar_graphics_godot.dylib" \
    --graphviz "$graphviz" --output "$output"

#!/bin/sh
set -eu

if [ "$#" -ne 1 ]; then
    echo "usage: apply-overlay.sh /absolute/path/to/clean-ghostty-checkout" >&2
    exit 64
fi

overlay_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH='' cd -- "$overlay_dir/../../.." && pwd)
source_dir=$(CDPATH='' cd -- "$1" && pwd -P)
expected_commit=332b2aefc6e72d363aa93ab6ecfc86eeeeb5ed28
expected_archive=401fde83af12b915d7ec86df59d8240a39a35f64bbb85b235315812275a8698f

actual_commit=$(git -C "$source_dir" rev-parse HEAD)
[ "$actual_commit" = "$expected_commit" ] || {
    echo "Ghostty commit mismatch: $actual_commit" >&2
    exit 65
}
[ -z "$(git -C "$source_dir" status --porcelain)" ] || {
    echo "Ghostty checkout must be clean before applying the macOS overlay" >&2
    exit 66
}
actual_archive=$(git -C "$source_dir" archive --format=tar HEAD | shasum -a 256 | awk '{print $1}')
[ "$actual_archive" = "$expected_archive" ] || {
    echo "Ghostty source archive digest mismatch" >&2
    exit 67
}

git -C "$source_dir" apply --check "$overlay_dir/ghostty-v1.3.1.patch"
git -C "$source_dir" apply "$overlay_dir/ghostty-v1.3.1.patch"
# Bind the pinned project patch to this export, not the private checkout layout.
python3 - "$source_dir" "$repo_root" <<'PY_PACKAGE'
from pathlib import Path
import json
import os
import sys
source, root = map(Path, sys.argv[1:])
package = root / "apps/polar-station-macos"
if not (package / "Package.swift").is_file():
    raise SystemExit("Station Swift package is missing")
project = source / "macos/Ghostty.xcodeproj/project.pbxproj"
text = project.read_text()
marker = "relativePath = ../../../polarinfrav1/apps/polar-station-macos;"
if text.count(marker) != 1:
    raise SystemExit("Unexpected pinned Station package reference")
relative = os.path.relpath(package, source / "macos")
project.write_text(text.replace(marker, "relativePath = " + json.dumps(relative) + ";"))
PY_PACKAGE
python3 "$repo_root/automation/polar_station/apply_display_overlay.py" \
    --source "$source_dir" --platform macos
install -d -m 0755 "$source_dir/macos/Sources/PolarStation"
install -m 0644 \
    "$repo_root/apps/polar-station-macos/ghostty-module/PolarStationRuntime.swift" \
    "$source_dir/macos/Sources/PolarStation/PolarStationRuntime.swift"
install -m 0644 \
    "$repo_root/apps/polar-station-macos/ghostty-module/PolarStationShortcuts.swift" \
    "$source_dir/macos/Sources/PolarStation/PolarStationShortcuts.swift"
install -m 0644 \
    "$repo_root/apps/polar-station-macos/ghostty-module/StationDictateKeyCapture.swift" \
    "$source_dir/macos/Sources/PolarStation/StationDictateKeyCapture.swift"
install -m 0644 \
    "$repo_root/apps/polar-station-macos/ghostty-module/PolarStationNativeExecutor.swift" \
    "$source_dir/macos/Sources/PolarStation/PolarStationNativeExecutor.swift"
install -m 0644 \
    "$repo_root/apps/polar-station-macos/ghostty-module/PolarStationNativeTabGroupCoordinator.swift" \
    "$source_dir/macos/Sources/PolarStation/PolarStationNativeTabGroupCoordinator.swift"
install -m 0644 \
    "$repo_root/apps/polar-station-macos/ghostty-module/PolarStationPaneSurfaceCoordinator.swift" \
    "$source_dir/macos/Sources/PolarStation/PolarStationPaneSurfaceCoordinator.swift"
install -m 0644 \
    "$repo_root/apps/polar-station-macos/ghostty-module/NativeTabGroupPresentationState.swift" \
    "$source_dir/macos/Sources/PolarStation/NativeTabGroupPresentationState.swift"
install -m 0644 \
    "$repo_root/apps/polar-station-macos/ghostty-module/ManagedApplicationPresentationState.swift" \
    "$source_dir/macos/Sources/PolarStation/ManagedApplicationPresentationState.swift"
install -m 0644 \
    "$repo_root/apps/polar-station-macos/ghostty-module/NativeSurfaceRetirementState.swift" \
    "$source_dir/macos/Sources/PolarStation/NativeSurfaceRetirementState.swift"
install -m 0644 \
    "$repo_root/apps/polar-station-macos/ghostty-module/NativeWorkspaceState.swift" \
    "$source_dir/macos/Sources/PolarStation/NativeWorkspaceState.swift"
install -m 0644 \
    "$repo_root/apps/polar-station-macos/ghostty-module/NativeWorkspaceLayout.swift" \
    "$source_dir/macos/Sources/PolarStation/NativeWorkspaceLayout.swift"
install -m 0644 \
    "$repo_root/apps/polar-station-macos/ghostty-module/PolarStationFileOpenPresentation.swift" \
    "$source_dir/macos/Sources/PolarStation/PolarStationFileOpenPresentation.swift"
install -m 0644 \
    "$repo_root/apps/polar-station-macos/ghostty-module/ControlCenterStore.swift" \
    "$source_dir/macos/Sources/PolarStation/ControlCenterStore.swift"
install -m 0644 \
    "$repo_root/apps/polar-station-macos/ghostty-module/ControlCenterWidthPreferences.swift" \
    "$source_dir/macos/Sources/PolarStation/ControlCenterWidthPreferences.swift"
install -m 0755 \
    "$repo_root/apps/polar-station-macos/ghostty-module/merge-static-libs.sh" \
    "$source_dir/macos/Sources/PolarStation/merge-static-libs.sh"
install -m 0644 \
    "$repo_root/apps/polar-station-macos/Resources/PolarStationStatusTemplate.png" \
    "$source_dir/macos/Sources/PolarStation/PolarStationStatusTemplate.png"
install -m 0644 \
    "$repo_root/apps/polar-station-macos/Resources/PolarStationStatusTemplate@2x.png" \
    "$source_dir/macos/Sources/PolarStation/PolarStationStatusTemplate@2x.png"
install -m 0644 \
    "$repo_root/apps/polar-station-macos/Resources/PolarStation.icns" \
    "$source_dir/macos/Sources/PolarStation/PolarStation.icns"

install -m 0644 \
    "$repo_root/apps/polar-station-macos/ghostty-module/PolarStationDisplaySurface.swift" \
    "$source_dir/macos/Sources/PolarStation/PolarStationDisplaySurface.swift"
install -m 0644 \
    "$repo_root/apps/polar-station-macos/ghostty-module/PolarStationLgpWorkspace.swift" \
    "$source_dir/macos/Sources/PolarStation/PolarStationLgpWorkspace.swift"
install -m 0644 \
    "$repo_root/apps/polar-station-macos/ghostty-module/PolarStationInputBar.swift" \
    "$source_dir/macos/Sources/PolarStation/PolarStationInputBar.swift"

printf '%s\n' "macOS Polar Station overlay applied to $actual_commit"

install -m 0644 \
    "$repo_root/apps/polar-station-macos/ghostty-module/PolarStationGraphicsView.swift" \
    "$source_dir/macos/Sources/PolarStation/PolarStationGraphicsView.swift"

install -m 0644 \
    "$repo_root/apps/polar-station-macos/ghostty-module/PolarStationDictateAudio.swift" \
    "$source_dir/macos/Sources/PolarStation/PolarStationDictateAudio.swift"

install -m 0644 \
    "$repo_root/apps/polar-station-macos/ghostty-module/PolarStationDictate.swift" \
    "$source_dir/macos/Sources/PolarStation/PolarStationDictate.swift"

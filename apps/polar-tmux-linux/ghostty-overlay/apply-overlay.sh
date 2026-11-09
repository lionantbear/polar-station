#!/bin/sh
set -eu

if [ "$#" -ne 1 ]; then
    echo "usage: apply-overlay.sh /absolute/path/to/clean-ghostty-checkout" >&2
    exit 64
fi

overlay_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH= cd -- "$overlay_dir/../../.." && pwd)
source_dir=$(realpath -- "$1")
reference_root=/home/chris/dev/reference
expected_commit=332b2aefc6e72d363aa93ab6ecfc86eeeeb5ed28
expected_archive=401fde83af12b915d7ec86df59d8240a39a35f64bbb85b235315812275a8698f

case "$source_dir/" in
    "$reference_root/"*)
        echo "refusing to modify a third-party reference checkout" >&2
        exit 65
        ;;
esac

actual_commit=$(git -C "$source_dir" rev-parse HEAD)
if [ "$actual_commit" != "$expected_commit" ]; then
    echo "Ghostty commit mismatch: expected $expected_commit, got $actual_commit" >&2
    exit 66
fi

if [ -n "$(git -C "$source_dir" status --porcelain)" ]; then
    echo "Ghostty checkout must be clean before applying the Polar overlay" >&2
    exit 67
fi

actual_archive=$(git -C "$source_dir" archive --format=tar HEAD | sha256sum | awk '{print $1}')
if [ "$actual_archive" != "$expected_archive" ]; then
    echo "Ghostty source archive digest mismatch" >&2
    exit 68
fi

git -C "$source_dir" apply --check "$overlay_dir/ghostty-v1.3.1.patch"
git -C "$source_dir" apply "$overlay_dir/ghostty-v1.3.1.patch"
python3 "$repo_root/automation/polar_station/apply_display_overlay.py" \
    --source "$source_dir" --platform linux
install -d -m 0755 "$source_dir/src/apprt/gtk/polar_tmux"
cp -R "$repo_root/apps/polar-tmux-linux/ghostty-module/." \
    "$source_dir/src/apprt/gtk/polar_tmux/"
install -m 0644 "$repo_root/apps/polar-tmux-shared/display/station_display.h" \
    "$source_dir/src/apprt/gtk/polar_tmux/station_display.h"

actual_files=$(git -C "$source_dir" diff --name-only | LC_ALL=C sort)
expected_files=$(printf '%s\n' \
    src/App.zig \
    src/Surface.zig \
    src/apprt/gtk/App.zig \
    src/apprt/gtk/Surface.zig \
    src/apprt/gtk/class/application.zig \
    src/apprt/gtk/class/split_tree.zig \
    src/apprt/gtk/class/surface.zig \
    src/apprt/gtk/class/window.zig \
    src/apprt/gtk/ui/1.5/window.blp \
    src/build/Config.zig \
    src/build/GhosttyExe.zig \
    src/build/GhosttyResources.zig \
    src/build_config.zig \
    src/config/Config.zig \
    src/main_ghostty.zig \
    src/renderer/State.zig \
    src/renderer/Thread.zig \
    src/renderer/generic.zig \
    src/renderer/link.zig \
    src/termio/Exec.zig \
    src/termio/Termio.zig \
    src/termio/backend.zig | LC_ALL=C sort)
if [ "$actual_files" != "$expected_files" ]; then
    echo "overlay modified a file outside the allowlist" >&2
    exit 69
fi

jq -n \
    --arg upstream_commit "$actual_commit" \
    --arg changed "$actual_files" \
    --arg module_destination "src/apprt/gtk/polar_tmux" \
    '{
        schema_version: 1,
        status: "applied",
        upstream_commit: $upstream_commit,
        feature: "polar-tmux",
        module_destination: $module_destination,
        // The enforced file list is emitted from the actual diff below.
        upstream_files_changed: ($changed | split("\n") | map(select(length > 0)))
    }'

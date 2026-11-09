#!/bin/sh
set -eu

usage() {
    echo "usage: install_linux.sh --receipt <Linux runtime-set v2 receipt> [--destination <path>] [--host-destination <path>]" >&2
    exit 64
}

receipt=
destination="${HOME}/.local/bin/polarstation"
host_destination="${HOME}/.local/bin/polar-tmux"
while [ "$#" -gt 0 ]; do
    case "$1" in
        --receipt)
            [ "$#" -ge 2 ] || usage
            receipt=$2
            shift 2
            ;;
        --destination)
            [ "$#" -ge 2 ] || usage
            destination=$2
            shift 2
            ;;
        --host-destination)
            [ "$#" -ge 2 ] || usage
            host_destination=$2
            shift 2
            ;;
        *)
            usage
            ;;
    esac
done

[ -n "$receipt" ] || usage
if [ "$(uname -s)" != "Linux" ]; then
    echo "Polar Station Linux installation must run on Linux." >&2
    exit 65
fi

case "$receipt" in
    /*) ;;
    *)
        echo "runtime-set receipt must be an absolute path: $receipt" >&2
        exit 66
        ;;
esac
if [ ! -f "$receipt" ] || [ -L "$receipt" ]; then
    echo "runtime-set receipt must be a regular file: $receipt" >&2
    exit 66
fi
receipt_parent=$(CDPATH='' cd -- "$(dirname -- "$receipt")" && pwd -P)
receipt="$receipt_parent/$(basename -- "$receipt")"
if ! jq -e '
    .schema_version == 2 and
    .status == "development-built" and
    .product == "Polar Station" and
    (.installable | type == "boolean") and
    (.runtime_set_id | type == "string" and length == 64) and
    (.polar_source_commit | type == "string" and length > 0) and
    (.polar_source_state_sha256 | type == "string" and length == 64) and
    (.upstream_commit | type == "string" and length > 0) and
    (.optimize | type == "string" and length > 0) and
    (.rust_profile | type == "string" and length > 0) and
    (.artifacts.managed_ghostty.path | type == "string" and startswith("/")) and
    (.artifacts.managed_ghostty.sha256 | type == "string" and length == 64) and
    (.artifacts.rust_host.path | type == "string" and startswith("/")) and
    (.artifacts.rust_host.sha256 | type == "string" and length == 64)
' "$receipt" >/dev/null; then
    echo "runtime-set receipt is invalid: $receipt" >&2
    exit 66
fi

installable=$(jq -r '.installable' "$receipt")
optimize=$(jq -r '.optimize' "$receipt")
rust_profile=$(jq -r '.rust_profile' "$receipt")
if [ "$installable" != true ] || [ "$optimize" != ReleaseFast ] || [ "$rust_profile" != release ]; then
    echo "runtime-set receipt is not installable; Linux installation requires ReleaseFast with a release Host." >&2
    exit 67
fi

artifact=$(jq -r '.artifacts.managed_ghostty.path' "$receipt")
expected_sha=$(jq -r '.artifacts.managed_ghostty.sha256' "$receipt")
host_artifact=$(jq -r '.artifacts.rust_host.path' "$receipt")
expected_host_sha=$(jq -r '.artifacts.rust_host.sha256' "$receipt")
runtime_set_id=$(jq -r '.runtime_set_id' "$receipt")
polar_source_commit=$(jq -r '.polar_source_commit' "$receipt")
polar_source_state_sha256=$(jq -r '.polar_source_state_sha256' "$receipt")
upstream_commit=$(jq -r '.upstream_commit' "$receipt")
themes_path=$(jq -er '.artifacts.themes.path' "$receipt")
themes_digest=$(jq -er '.artifacts.themes.sha256' "$receipt")
graphics_path=$(jq -r '.artifacts.graphics.path // empty' "$receipt")
graphics_digest=$(jq -r '.artifacts.graphics.sha256 // empty' "$receipt")
shortcuts_path=$(jq -r '.artifacts.gnome_shortcuts.path // empty' "$receipt")
shortcuts_digest=$(jq -r '.artifacts.gnome_shortcuts.sha256 // empty' "$receipt")
computed_runtime_set_id=$({ printf '%s\n' \
    "$polar_source_commit" \
    "$polar_source_state_sha256" \
    "$upstream_commit" \
    "$optimize" \
    "$rust_profile" \
    "$expected_sha" \
    "$expected_host_sha" \
    "$themes_digest";
    if [ -n "$shortcuts_digest" ]; then printf '%s\n' "$shortcuts_digest"; fi
    if [ -n "$graphics_digest" ]; then printf '%s\n' "$graphics_digest"; fi
    } | sha256sum | awk '{print $1}')
if [ "$computed_runtime_set_id" != "$runtime_set_id" ]; then
    echo "runtime-set receipt identity mismatch: $receipt" >&2
    exit 68
fi

validate_artifact() {
    candidate=$1
    label=$2
    if [ ! -f "$candidate" ] || [ ! -x "$candidate" ] || [ -L "$candidate" ]; then
        echo "The controlled $label artifact must be a regular executable: $candidate" >&2
        exit 66
    fi
}

validate_destination() {
    candidate=$1
    label=$2
    case "$candidate" in
        /*) ;;
        *)
            echo "$label destination must be an absolute path: $candidate" >&2
            exit 67
            ;;
    esac
    if [ -L "$candidate" ]; then
        echo "Refusing to replace a symlink destination: $candidate" >&2
        exit 70
    fi
    if [ -e "$candidate" ] && { [ ! -f "$candidate" ] || [ ! -x "$candidate" ]; }; then
        echo "Existing $label destination is not a regular executable: $candidate" >&2
        exit 71
    fi
}

validate_artifact "$artifact" "managed Ghostty"
validate_artifact "$host_artifact" "Rust Host"
validate_destination "$destination" "managed Ghostty"
validate_destination "$host_destination" "Rust Host"

artifact_parent=$(CDPATH='' cd -- "$(dirname -- "$artifact")" && pwd -P)
artifact="$artifact_parent/$(basename -- "$artifact")"
host_artifact_parent=$(CDPATH='' cd -- "$(dirname -- "$host_artifact")" && pwd -P)
host_artifact="$host_artifact_parent/$(basename -- "$host_artifact")"
destination_parent=$(CDPATH='' cd -- "$(dirname -- "$destination")" && pwd -P)
host_destination_parent=$(CDPATH='' cd -- "$(dirname -- "$host_destination")" && pwd -P)
destination="$destination_parent/$(basename -- "$destination")"
host_destination="$host_destination_parent/$(basename -- "$host_destination")"

if [ "$destination_parent" != "$host_destination_parent" ]; then
    echo "Managed Ghostty and Rust Host destinations must share one installation directory." >&2
    exit 68
fi
if [ "$artifact" = "$destination" ] || [ "$host_artifact" = "$host_destination" ]; then
    echo "Controlled artifacts and installed destinations must be different paths." >&2
    exit 68
fi
if [ ! -w "$destination_parent" ]; then
    echo "Destination is not writable; this entrypoint never escalates privileges: $destination_parent" >&2
    exit 69
fi

source_sha=$(sha256sum "$artifact" | awk '{print $1}')
host_source_sha=$(sha256sum "$host_artifact" | awk '{print $1}')
if [ "$source_sha" != "$expected_sha" ] || [ "$host_source_sha" != "$expected_host_sha" ]; then
    echo "runtime-set Artifact digest mismatch: $receipt" >&2
    exit 72
fi

# Desktop assets come only from this controlled checkout, never the upstream
# Ghostty install prefix (which carries a different application identity).
repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd -P)
desktop_assets="$repo_root/apps/polar-station-linux/data"
app_id=com.polarealm.PolarStation
data_home=${XDG_DATA_HOME:-"${HOME}/.local/share"}
case "$data_home" in
    /*) ;;
    *) echo "XDG_DATA_HOME must be an absolute path: $data_home" >&2; exit 76 ;;
esac
data_home=$(readlink -m -- "$data_home")
applications="$data_home/applications"
icon_theme="$data_home/icons/hicolor"
skip_restart=${POLAR_STATION_INSTALL_SKIP_SERVICE_RESTART_FOR_TESTS:-0}
if [ "$skip_restart" = "1" ]; then
    case "$destination_parent:$data_home" in
        /tmp/*:/tmp/*) ;;
        *)
            echo "The service-restart test escape requires temporary binary and desktop destinations." >&2
            exit 75
            ;;
    esac
fi

preflight_desktop_asset() {
    relative=$1
    target=$2
    if [ ! -f "$desktop_assets/$relative" ] || [ ! -s "$desktop_assets/$relative" ] ||
        [ ! -r "$desktop_assets/$relative" ] || [ -L "$desktop_assets/$relative" ]; then
        echo "Controlled desktop/icon asset is missing or invalid: $desktop_assets/$relative" >&2
        exit 76
    fi
    if [ -L "$target" ] || { [ -e "$target" ] && [ ! -f "$target" ]; }; then
        echo "Refusing non-regular desktop/icon destination: $target" >&2
        exit 76
    fi
    parent=$(dirname -- "$target")
    while [ "$parent" != / ]; do
        if [ -L "$parent" ] || { [ -e "$parent" ] && [ ! -d "$parent" ]; }; then
            echo "Invalid desktop/icon destination directory: $parent" >&2
            exit 76
        fi
        parent=$(dirname -- "$parent")
    done
}

preflight_desktop_asset "$app_id.desktop" "$applications/$app_id.desktop"
for size in 48 64 128 256 512; do
    relative="icons/hicolor/${size}x${size}/apps/$app_id.png"
    preflight_desktop_asset "$relative" "$data_home/$relative"
done
stage_root=$(mktemp -d "$destination_parent/.polar-station-install.XXXXXX")
stage="$stage_root/polarstation"
host_stage="$stage_root/polar-tmux"

cleanup_stage() {
    status=$?
    if [ -d "$stage_root" ]; then
        rm -rf -- "$stage_root"
    fi
    exit "$status"
}
trap cleanup_stage EXIT HUP INT TERM

# Stage every required asset and prepare all destination directories before
# replacing either executable. These are new assets, never previous-version backups.
install -m 0644 -- "$desktop_assets/$app_id.desktop" "$stage_root/$app_id.desktop"
mkdir -p -- "$applications"
for size in 48 64 128 256 512; do
    install -m 0644 -- "$desktop_assets/icons/hicolor/${size}x${size}/apps/$app_id.png" "$stage_root/$size.png"
    mkdir -p -- "$icon_theme/${size}x${size}/apps"
done

install -m 0755 -- "$artifact" "$stage"
install -m 0755 -- "$host_artifact" "$host_stage"
stage_sha=$(sha256sum "$stage" | awk '{print $1}')
host_stage_sha=$(sha256sum "$host_stage" | awk '{print $1}')
if [ "$stage_sha" != "$source_sha" ] || [ "$host_stage_sha" != "$host_source_sha" ]; then
    echo "Staged runtime digests do not match the controlled builds." >&2
    exit 72
fi

python3 "$(dirname -- "$0")/theme_resources.py" "$themes_path" --expected "$themes_digest" --destination "$data_home/polar-station/themes" >/dev/null
if [ -n "$graphics_path" ]; then
    python3 "$(dirname -- "$0")/graphics_resources.py" "$graphics_path" --expected "$graphics_digest" --destination "$(dirname -- "$destination")/graphics" >/dev/null
fi
if [ -n "$shortcuts_path" ]; then
    python3 "$(dirname -- "$0")/gnome_shortcuts.py" "$shortcuts_path" --expected "$shortcuts_digest" --destination "$data_home/gnome-shell/extensions/shortcuts@polarstation.polarealm.com"
fi
mv -- "$stage" "$destination"
mv -- "$host_stage" "$host_destination"
installed_sha=$(sha256sum "$destination" | awk '{print $1}')
installed_host_sha=$(sha256sum "$host_destination" | awk '{print $1}')
if [ "$installed_sha" != "$source_sha" ] ||
    [ "$installed_host_sha" != "$host_source_sha" ] ||
    [ ! -x "$destination" ] || [ ! -x "$host_destination" ]; then
    echo "Installed Polar Station runtime validation failed." >&2
    exit 74
fi

install -m 0644 -- "$stage_root/$app_id.desktop" "$applications/$app_id.desktop"
for size in 48 64 128 256 512; do
    install -m 0644 -- "$stage_root/$size.png" "$icon_theme/${size}x${size}/apps/$app_id.png"
done
if command -v update-desktop-database >/dev/null 2>&1; then
    if ! update-desktop-database "$applications"; then
        echo "Desktop cache update failed; installed files retained without rollback." >&2
        exit 77
    fi
else
    echo "update-desktop-database unavailable; desktop cache update skipped." >&2
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    # A per-user hicolor overlay need not contain its own index.theme.
    if ! gtk-update-icon-cache -f -t "$icon_theme"; then
        echo "Icon cache update failed; installed files retained without rollback." >&2
        exit 77
    fi
else
    echo "gtk-update-icon-cache unavailable; icon cache update skipped." >&2
fi

if [ "$skip_restart" != "1" ]; then
    systemctl --user restart polar-tmux-host.service
    systemctl --user is-active --quiet polar-tmux-host.service
fi

printf 'POLAR_STATION_INSTALLED_EXECUTABLE=%s\n' "$destination"
printf 'POLAR_STATION_INSTALLED_SHA256=%s\n' "$installed_sha"
printf 'POLAR_STATION_INSTALLED_HOST_EXECUTABLE=%s\n' "$host_destination"
printf 'POLAR_STATION_INSTALLED_HOST_SHA256=%s\n' "$installed_host_sha"
printf 'POLAR_STATION_INSTALLED_RUNTIME_SET_ID=%s\n' "$runtime_set_id"
printf 'POLAR_STATION_INSTALLED_RECEIPT=%s\n' "$receipt"
printf 'POLAR_STATION_INSTALLED_DESKTOP_ENTRY=%s\n' "$applications/$app_id.desktop"
printf 'POLAR_STATION_INSTALLED_ICON_THEME=%s\n' "$icon_theme"
printf 'POLAR_STATION_BACKUP_POLICY=none\n'

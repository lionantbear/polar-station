#!/bin/sh
set -eu

usage() {
    echo "usage: install_macos.sh --app <signed Polar Station.app> [--destination <path>]" >&2
    exit 64
}

source_app=
destination="/Applications/Polar Station.app"
while [ "$#" -gt 0 ]; do
    case "$1" in
        --app)
            [ "$#" -ge 2 ] || usage
            source_app=$2
            shift 2
            ;;
        --destination)
            [ "$#" -ge 2 ] || usage
            destination=$2
            shift 2
            ;;
        *)
            usage
            ;;
    esac
done

[ -n "$source_app" ] || usage
if [ "$(uname -s)" != "Darwin" ]; then
    echo "Polar Station macOS installation must run on macOS." >&2
    exit 65
fi
if [ ! -d "$source_app" ]; then
    echo "The source Polar Station app does not exist: $source_app" >&2
    exit 66
fi

source_parent=$(CDPATH='' cd -- "$(dirname -- "$source_app")" && pwd -P)
source_app="$source_parent/$(basename -- "$source_app")"
destination_parent=$(CDPATH='' cd -- "$(dirname -- "$destination")" && pwd -P)
destination="$destination_parent/$(basename -- "$destination")"
if [ "$source_app" = "$destination" ]; then
    echo "Source and destination must be different paths." >&2
    exit 67
fi
if [ ! -w "$destination_parent" ]; then
    echo "Destination is not writable; this entrypoint never escalates privileges: $destination_parent" >&2
    exit 68
fi

expected_app_identifier=com.polarealm.polar-station
expected_host_identifier=com.polarealm.polar-station.host
lsregister=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
[ -x "$lsregister" ] || {
    echo "LaunchServices registration tool is unavailable: $lsregister" >&2
    exit 69
}

validate_app() {
    candidate=$1
    host="$candidate/Contents/Helpers/polar-tmux"
    info="$candidate/Contents/Info.plist"
    if [ ! -x "$host" ] || [ ! -f "$info" ]; then
        echo "Polar Station app is missing its signed Host or Info.plist: $candidate" >&2
        return 1
    fi

    /usr/bin/codesign --verify --deep --strict "$candidate"
    /usr/bin/codesign --verify --strict "$host"

    app_identifier=$(
        /usr/libexec/PlistBuddy \
            -c "Print :CFBundleIdentifier" \
            "$info"
    )
    [ "$app_identifier" = "$expected_app_identifier" ] || {
        echo "Unexpected Polar Station bundle identifier: $app_identifier" >&2
        return 1
    }
    build_configuration=$(
        /usr/libexec/PlistBuddy \
            -c "Print :PolarStationBuildConfiguration" \
            "$info" 2>/dev/null || true
    )
    if [ "$build_configuration" != Release ] ||
        [ -e "$candidate/Contents/MacOS/Polar Station.debug.dylib" ]; then
        echo "Refusing to install a non-Release Polar Station artifact: ${build_configuration:-unstamped}" >&2
        return 1
    fi

    app_details=$(/usr/bin/codesign -dvvv "$candidate" 2>&1)
    host_details=$(/usr/bin/codesign -dvvv "$host" 2>&1)
    case "$app_details
$host_details" in
        *"Signature=adhoc"*|*"flags="*"adhoc"*)
            echo "Refusing to install an ad-hoc Polar Station artifact." >&2
            return 1
            ;;
    esac
    app_team=$(printf '%s\n' "$app_details" | /usr/bin/sed -n 's/^TeamIdentifier=//p')
    host_team=$(printf '%s\n' "$host_details" | /usr/bin/sed -n 's/^TeamIdentifier=//p')
    host_identifier=$(printf '%s\n' "$host_details" | /usr/bin/sed -n 's/^Identifier=//p')
    if [ -z "$app_team" ] || [ "$app_team" = "not set" ] ||
        [ "$host_team" != "$app_team" ] ||
        [ "$host_identifier" != "$expected_host_identifier" ]; then
        echo "Polar Station app and helper signing identities do not match." >&2
        return 1
    fi
}

# Validate the source before creating or moving anything in the destination.
validate_app "$source_app"

stage_root=$(
    /usr/bin/mktemp -d \
        "$destination_parent/.polar-station-install.XXXXXX"
)
case "$stage_root" in
    "$destination_parent"/.polar-station-install.*) ;;
    *)
        echo "Refusing unexpected installation staging path: $stage_root" >&2
        exit 70
        ;;
esac
[ -d "$stage_root" ] && [ ! -L "$stage_root" ] || {
    echo "Installation staging path is not a real directory: $stage_root" >&2
    exit 71
}
stage_app="$stage_root/Polar Station.app"

cleanup_stage() {
    status=$?
    if [ -d "$stage_root" ] && [ ! -L "$stage_root" ]; then
        /bin/rm -R -- "$stage_root"
    fi
    exit "$status"
}
trap cleanup_stage EXIT HUP INT TERM

/usr/bin/ditto --rsrc --extattr "$source_app" "$stage_app"
validate_app "$stage_app"
# Prepare and verify the independently supervised Host from this exact artifact
# before stopping the desktop. Installation never invokes a compiler.
repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd -P)
(cd "$repo_root" && python3 -m automation.polar_station.macos_host prepare-desktop \
    --app "$stage_app" --output "$stage_root/host")
"$lsregister" -u "$source_app" >/dev/null 2>&1 || :

running_app_pids() {
    app_executable="$destination/Contents/MacOS/Polar Station"
    /bin/ps -axo pid=,command= | /usr/bin/awk -v expected="$app_executable" '
        {
            pid = $1
            sub(/^[[:space:]]*[0-9]+[[:space:]]+/, "", $0)
            if ($0 == expected) print pid
        }
    '
}

confirm_native_quit() {
    /usr/bin/osascript - "$active_pids" <<'APPLESCRIPT'
on run arguments
    set expectedPids to paragraphs of item 1 of arguments
    tell application "System Events"
        repeat 30 times
            repeat with expectedPid in expectedPids
                set targetPid to expectedPid as integer
                set matchingProcesses to every application process whose unix id is targetPid
                repeat with candidateProcess in matchingProcesses
                    tell candidateProcess
                        repeat with candidateWindow in windows
                            repeat with buttonTitle in {"Quit", "Close Ghostty", "退出", "确认退出"}
                                try
                                    if exists button (buttonTitle as text) of candidateWindow then
                                        click button (buttonTitle as text) of candidateWindow
                                        return "confirmed"
                                    end if
                                end try
                                try
                                    if exists sheet 1 of candidateWindow then
                                        if exists button (buttonTitle as text) of sheet 1 of candidateWindow then
                                            click button (buttonTitle as text) of sheet 1 of candidateWindow
                                            return "confirmed"
                                        end if
                                    end if
                                end try
                            end repeat
                        end repeat
                    end tell
                end repeat
            end repeat
            delay 0.1
        end repeat
    end tell
    error "Polar Station Quit confirmation was not found"
end run
APPLESCRIPT
}

quit_active_application() {
    active_pids=$(running_app_pids)
    [ -n "$active_pids" ] || return 0

    # The installed release may still carry a retired bundle identifier.
    # Address the exact path already checked by running_app_pids instead.
    # Quit can enter NSAlert.runModal before replying to AppleEvents. Admit
    # confirmation concurrently rather than waiting for that reply first.
    /usr/bin/osascript - "$destination" <<'APPLESCRIPT' &
on run arguments
    set applicationPath to item 1 of arguments
    with timeout of 10 seconds
        tell application applicationPath to quit
    end timeout
end run
APPLESCRIPT
    quit_request_pid=$!

    # A zero-window process may quit without confirmation. Only inspect and
    # press a named affirmative button while the exact application still runs;
    # never send Return into a terminal surface.
    for wait_step in 1 2 3 4 5; do
        if [ -z "$(running_app_pids)" ]; then
            wait "$quit_request_pid" || :
            return 0
        fi
        sleep 0.2
    done
    confirm_native_quit

    for wait_step in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 \
        16 17 18 19 20 21 22 23 24 25 26 27 28 29 30; do
        if [ -z "$(running_app_pids)" ]; then
            wait "$quit_request_pid" || :
            return 0
        fi
        sleep 1
    done
    echo "Polar Station remained running after confirmed Quit; refusing bundle replacement." >&2
    return 1
}

quit_active_application

remove_prior_bundle() {
    prior_bundle=$1
    [ -e "$prior_bundle" ] || return 0
    case "$prior_bundle" in
        "$destination"|"$destination".backup-*) ;;
        *)
            echo "Refusing unexpected prior application path: $prior_bundle" >&2
            return 1
            ;;
    esac
    if [ -L "$prior_bundle" ] || [ ! -d "$prior_bundle" ]; then
        echo "Refusing to recursively remove a non-directory application path: $prior_bundle" >&2
        return 1
    fi
    echo "Removing prior Polar Station bundle: $prior_bundle"
    "$lsregister" -u "$prior_bundle" >/dev/null 2>&1 || :
    /bin/rm -R -- "$prior_bundle"
}

# Old installers left runnable application copies beside the active bundle.
# Enumerate only this exact destination's historical suffix and remove each
# validated directory before installing the sole active application.
for prior_bundle in "$destination".backup-*; do
    [ -e "$prior_bundle" ] || continue
    remove_prior_bundle "$prior_bundle"
done
remove_prior_bundle "$destination"

/bin/mv -- "$stage_app" "$destination"
validate_app "$destination"
"$lsregister" -f "$destination"
(cd "$repo_root" && python3 -m automation.polar_station.macos_host install \
    --app "$stage_root/host/Polar Station Host.app" --desktop "$destination")

printf 'POLAR_STATION_INSTALLED_APP=%s\n' "$destination"

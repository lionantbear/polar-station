#!/bin/bash
set -euo pipefail
trap 'rc=$?; printf "Station build failed at line %s (exit %s)\n" "$LINENO" "$rc" >&2; exit "$rc"' ERR

usage() {
    echo "usage: build_macos.sh [--configuration Debug|Release] [--ghostty-checkout PATH]" >&2
    exit 64
}

configuration=Release
ghostty_checkout=${POLAR_STATION_GHOSTTY_CHECKOUT:-"$HOME/.cache/polar-station/ghostty"}
while [ "$#" -gt 0 ]; do
    case "$1" in
        --configuration) [ "$#" -ge 2 ] || usage; configuration=$2; shift 2 ;;
        --ghostty-checkout) [ "$#" -ge 2 ] || usage; ghostty_checkout=$2; shift 2 ;;
        *) usage ;;
    esac
done
[ "$configuration" = Debug ] || [ "$configuration" = Release ] || usage
[ "$(uname -s)" = Darwin ] || {
    echo "Polar Station macOS builds must run on macOS." >&2
    exit 65
}

[ "$(uname -m)" = arm64 ] || {
    echo "Polar Station macOS builds require Apple Silicon arm64." >&2
    exit 65
}

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH='' cd -- "$script_dir/../.." && pwd)
overlay="$repo_root/apps/polar-station-macos/ghostty-overlay/apply-overlay.sh"
toolchain_wrappers="$repo_root/automation/polar_station/toolchain_wrappers"
expected_commit=332b2aefc6e72d363aa93ab6ecfc86eeeeb5ed28
toolchain_root=${POLAR_STATION_TOOLCHAIN_ROOT:-"$HOME/.cache/polar-tmux/toolchains"}
pinned_zig="$toolchain_root/zig-aarch64-macos-0.15.2/zig"
compatible_macos_sdk=/Library/Developer/CommandLineTools/SDKs/MacOSX15.sdk
build_source=
output_root="$repo_root/target/polar-station-macos/$configuration"
output_app="$output_root/Polar Station.app"

[ -d "$ghostty_checkout/.git" ] || [ -f "$ghostty_checkout/.git" ] || {
    echo "Controlled Ghostty checkout is missing: $ghostty_checkout" >&2
    exit 66
}
[ "$(git -C "$ghostty_checkout" rev-parse HEAD)" = "$expected_commit" ] || {
    echo "Controlled Ghostty checkout is not pinned to v1.3.1." >&2
    exit 67
}

# A build owns an independent disposable clone, never a Git worktree or an
# existing checkout. Install cleanup before cloning so partial failures are owned.
build_source=$(mktemp -d "$(dirname -- "$ghostty_checkout")/polar-station-macos-build.XXXXXX")
cleanup() {
    rm -rf -- "$build_source"
}
trap cleanup EXIT HUP INT TERM
git clone --local --no-checkout "$ghostty_checkout" "$build_source"
git -C "$build_source" checkout --detach "$expected_commit"

"$overlay" "$build_source"

zig=${POLAR_STATION_ZIG:-}
if [ -z "$zig" ] && [ -x "$pinned_zig" ]; then
    zig=$pinned_zig
fi
if [ -z "$zig" ]; then
    zig=$(command -v zig || true)
fi
[ -n "$zig" ] && [ "$($zig version)" = 0.15.2 ] || {
    echo "Polar Station requires Zig 0.15.2 at $pinned_zig or POLAR_STATION_ZIG." >&2
    exit 68
}
msgfmt=${POLAR_STATION_MSGFMT:-}
if [ -z "$msgfmt" ] && [ -x /opt/homebrew/bin/msgfmt ]; then
    msgfmt=/opt/homebrew/bin/msgfmt
fi
if [ -z "$msgfmt" ]; then
    msgfmt=$(command -v msgfmt || true)
fi
[ -n "$msgfmt" ] && [ -x "$msgfmt" ] || {
    echo "Polar Station requires gettext msgfmt." >&2
    exit 73
}
PATH="$(dirname -- "$msgfmt"):$PATH"
export PATH
macos_sdk=${POLAR_STATION_MACOS_SDK:-}
if [ -z "$macos_sdk" ] && [ -d "$compatible_macos_sdk" ]; then
    macos_sdk=$compatible_macos_sdk
fi
[ -n "$macos_sdk" ] && [ -d "$macos_sdk" ] || {
    echo "Polar Station requires a Zig-compatible macOS 15 SDK at $compatible_macos_sdk or POLAR_STATION_MACOS_SDK." >&2
    exit 71
}
SDKROOT="$macos_sdk"
POLAR_STATION_MACOS_SDK="$macos_sdk"
MACOSX_DEPLOYMENT_TARGET=13.0
PATH="$toolchain_wrappers:$PATH"
export PATH SDKROOT POLAR_STATION_MACOS_SDK MACOSX_DEPLOYMENT_TARGET
optimize=Debug
[ "$configuration" = Release ] && optimize=ReleaseFast
host_profile=debug
host_flags=
if [ "$configuration" = Release ]; then
    host_profile=release
    host_flags=--release
fi
# The Host and shared desktop-client archive target Apple Silicon together.
# Ghostty's existing native XCFramework mode avoids compiling Intel slices.
rust_target=aarch64-apple-darwin
rustup target list --installed | grep -Fxq "$rust_target" || {
    echo "Polar Station requires installed Rust target $rust_target." >&2
    exit 78
}
mkdir -p "$repo_root/target/build-evidence"
cargo build --message-format=json-render-diagnostics --locked $host_flags --manifest-path "$repo_root/Cargo.toml" --target-dir "$repo_root/target" \
    --target "$rust_target" -p polar-tmux --bin polar-tmux --lib | tee "$repo_root/target/build-evidence/cargo-host.jsonl"
client_dir="$repo_root/target/polar-station-client/current"
mkdir -p "$client_dir"
cp "$repo_root/target/$rust_target/$host_profile/libpolar_tmux.a" \
    "$client_dir/libpolar_tmux.a"
POLAR_STATION_CLIENT_LIB_DIR="$client_dir"
export POLAR_STATION_CLIENT_LIB_DIR
(
    cd "$build_source"
    "$zig" build -Dpolar-tmux=true -Demit-macos-app=false -Dxcframework-target=native -Doptimize="$optimize"
    # The pinned build.nu has no architecture override. Use the same project,
    # scheme and output contract directly, without modifying upstream sources.
    /usr/bin/env -i HOME="$HOME" PATH=/usr/bin:/bin:/usr/sbin:/sbin \
        DEVELOPER_DIR="${DEVELOPER_DIR:-$(/usr/bin/xcode-select -p)}" \
        /usr/bin/xcodebuild -project macos/Ghostty.xcodeproj -scheme Ghostty \
        -configuration "$configuration" "SYMROOT=$build_source/macos/build" \
        ARCHS=arm64 ONLY_ACTIVE_ARCH=YES \
        ENABLE_CODE_COVERAGE=NO CLANG_ENABLE_CODE_COVERAGE=NO \
        CODE_SIGNING_ALLOWED=NO CODE_SIGNING_REQUIRED=NO CODE_SIGN_IDENTITY= build
)

if [ -n "${POLAR_STATION_NATIVE_SOURCE_EVIDENCE:-}" ]; then
    mkdir -p "$POLAR_STATION_NATIVE_SOURCE_EVIDENCE"
    python3 "$repo_root/automation/polar_station/capture_native_source.py" \
        --tree "$build_source" --component ghostty \
        --source-commit "$(git -C "$repo_root" rev-parse HEAD)" \
        --build-script "$repo_root/automation/polar_station/build_macos.sh" \
        --output "$POLAR_STATION_NATIVE_SOURCE_EVIDENCE/ghostty-built-source.tar.gz"
fi

app="$build_source/macos/build/$configuration/Polar Station.app"
[ -d "$app" ] || {
    echo "Managed Ghostty did not produce Polar Station.app." >&2
    exit 69
}
info="$app/Contents/Info.plist"
if /usr/libexec/PlistBuddy -c 'Print :PolarStationBuildConfiguration' \
    "$info" >/dev/null 2>&1; then
    /usr/libexec/PlistBuddy \
        -c "Set :PolarStationBuildConfiguration $configuration" "$info"
else
    /usr/libexec/PlistBuddy \
        -c "Add :PolarStationBuildConfiguration string $configuration" "$info"
fi
# Version the exact desktop/Host pair inside the signed artifact; rollback
# consumes this bundle's helper rather than rebuilding from a checkout.
source_commit=$(python3 "$repo_root/automation/polar_station/public_source_identity.py" "$repo_root" --revision)
source_fingerprint=$(python3 "$repo_root/automation/polar_station/public_source_identity.py" "$repo_root" --fingerprint)
/usr/libexec/PlistBuddy -c 'Delete :PolarStationSourceCommit' "$info" >/dev/null 2>&1 || true
/usr/libexec/PlistBuddy -c "Add :PolarStationSourceCommit string $source_commit" "$info"
/usr/libexec/PlistBuddy -c 'Delete :PolarStationSourceStateSHA256' "$info" >/dev/null 2>&1 || true
/usr/libexec/PlistBuddy -c "Add :PolarStationSourceStateSHA256 string $source_fingerprint" "$info"
/usr/libexec/PlistBuddy -c 'Delete :PolarStationDesktopContract' "$info" >/dev/null 2>&1 || true
/usr/libexec/PlistBuddy -c 'Add :PolarStationDesktopContract integer 1' "$info"
icon_name=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIconFile' "$app/Contents/Info.plist")
[ "$icon_name" = PolarStation ] || {
    echo "Managed Polar Station Info.plist does not select PolarStation.icns." >&2
    exit 74
}
[ -f "$app/Contents/Resources/PolarStation.icns" ] || {
    echo "Managed Polar Station icon resource is missing." >&2
    exit 75
}
[ -f "$app/Contents/Resources/terminfo/78/xterm-ghostty" ] || {
    echo "Managed Polar Station xterm-ghostty terminfo resource is missing." >&2
    exit 76
}
messages_usage='Send the Codex completion notifications you enable through Messages.'
/usr/libexec/PlistBuddy -c 'Delete :NSAppleEventsUsageDescription' "$info" >/dev/null 2>&1 || true
/usr/libexec/PlistBuddy -c "Add :NSAppleEventsUsageDescription string $messages_usage" "$info"
[ "$(/usr/libexec/PlistBuddy -c 'Print :NSAppleEventsUsageDescription' "$info")" = "$messages_usage" ] || exit 77
helpers="$app/Contents/Helpers"
mkdir -p "$helpers"
cp "$repo_root/target/$rust_target/$host_profile/polar-tmux" "$helpers/polar-tmux"
chmod 0755 "$helpers/polar-tmux"

graphics=${POLAR_STATION_GRAPHICS_BUNDLE:-"$repo_root/target/polar-station-macos/graphics"}
[ -f "$graphics/libgodot.dylib" ] || {
    echo "Run just polar-station-macos-graphics-build before building Station." >&2
    exit 80
}
python3 "$repo_root/automation/polar_station/graphics_resources.py" "$graphics" \
    --expected "$(python3 "$repo_root/automation/polar_station/graphics_resources.py" "$graphics")" \
    --destination "$app/Contents/Resources/graphics"

# Ad-hoc signing is explicit local-development opt-in, not trusted distribution.
requested_identity=${POLAR_STATION_CODESIGN_IDENTITY:-}
if [ "$requested_identity" = - ]; then
    identity=-
else
    identity_inventory=$(/usr/bin/security find-identity -v -p codesigning)
    if [ -n "$requested_identity" ]; then
        identity_line=$(printf '%s\n' "$identity_inventory" | /usr/bin/awk -v requested="$requested_identity" 'index($0, requested) { print; exit }')
    else
        identity_line=$(printf '%s\n' "$identity_inventory" | /usr/bin/awk '/"Apple Development:/ { print; exit }')
    fi
    [ -n "$identity_line" ] || {
        echo "No requested signing identity is available. For local development, explicitly set POLAR_STATION_CODESIGN_IDENTITY=-." >&2
        exit 70
    }
    identity=$(printf '%s\n' "$identity_line" | /usr/bin/awk '{print $2}')
fi
python3 "$repo_root/automation/polar_station/record_macos_signing.py" before \
    --app "$app" --source "$repo_root" --before "$repo_root/target/build-evidence/macos-pre-sign.json"
/usr/bin/codesign --force --identifier com.polarealm.polar-station.host \
    --options runtime --entitlements "$repo_root/automation/polar_station/host-macos.entitlements" \
    --sign "$identity" "$helpers/polar-tmux"
# Resources contain Mach-O libraries and Graphviz plugins; sign inside-out.
find "$app/Contents/Resources/graphics" -type f -print0 | while IFS= read -r -d '' file; do
    if /usr/bin/file -b "$file" | grep -q 'Mach-O'; then
        /usr/bin/codesign --force --options runtime --sign "$identity" "$file"
    fi
done
# Xcode signing is disabled above, so there are no entitlements to preserve.
main_entitlements="$build_source/macos/Ghostty.entitlements"
[ "$configuration" != Debug ] || main_entitlements="$build_source/macos/GhosttyDebug.entitlements"
/usr/bin/codesign --force --deep --options runtime \
    --preserve-metadata=identifier --entitlements "$main_entitlements" --sign "$identity" "$app"
if [ "$identity" = - ]; then
    python3 "$repo_root/automation/polar_station/sign_trial_main.py" "$app"
fi
# Inspect the final main signature, not just an entitlement source plist.
python3 "$repo_root/automation/polar_station/sign_trial_main.py" --verify-only "$app"
# Deep bundle signing must preserve the independently signed Host entitlement.
host_entitlements=$(/usr/bin/codesign -d --entitlements :- "$helpers/polar-tmux" 2>/dev/null)
printf '%s' "$host_entitlements" | python3 -c 'import plistlib,sys; assert plistlib.loads(sys.stdin.buffer.read()).get("com.apple.security.automation.apple-events") is True'
/usr/bin/codesign --display --verbose=2 "$helpers/polar-tmux" 2>&1 | /usr/bin/grep -Fx 'Identifier=com.polarealm.polar-station.host' >/dev/null
python3 "$repo_root/automation/polar_station/record_macos_signing.py" after \
    --app "$app" --source "$repo_root" --before "$repo_root/target/build-evidence/macos-pre-sign.json" \
    --output "$repo_root/target/build-evidence/macos-signing.json"

mkdir -p "$output_root"
if [ -e "$output_app" ]; then
    rm -R -- "$output_app"
fi
/usr/bin/ditto --rsrc --extattr "$app" "$output_app"
/usr/bin/codesign --verify --deep --strict "$output_app"

# Xcode registers build products automatically. Managed build artifacts are
# not public application candidates; only the installer registers the exact
# active /Applications bundle.
lsregister=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
[ -x "$lsregister" ] || {
    echo "LaunchServices registration tool is unavailable: $lsregister" >&2
    exit 79
}
"$lsregister" -u "$app" >/dev/null 2>&1 || :
"$lsregister" -u "$output_app" >/dev/null 2>&1 || :

printf 'POLAR_STATION_APP=%s\n' "$output_app"
printf 'POLAR_STATION_HOST=%s\n' "$output_app/Contents/Helpers/polar-tmux"

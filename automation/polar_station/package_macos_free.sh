#!/bin/sh
# Package a pre-built, already signed Polar Station app without an Apple account.
# This script does not sign, notarize, build, upload, or disable Gatekeeper.
set -eu

usage() {
    echo 'usage: package_macos_free.sh --app "/path/Polar Station.app" --output /path/Polar-Station-unnotarized.dmg' >&2
    exit 64
}

app=
output=
while [ "$#" -gt 0 ]; do
    case "$1" in
        --app) [ "$#" -ge 2 ] || usage; app=$2; shift 2 ;;
        --output) [ "$#" -ge 2 ] || usage; output=$2; shift 2 ;;
        *) usage ;;
    esac
done
[ -n "$app" ] && [ -n "$output" ] || usage
[ "$(uname -s)" = Darwin ] || {
    echo 'DMG packaging requires macOS and hdiutil.' >&2
    exit 65
}
[ -d "$app/Contents" ] && [ -f "$app/Contents/Info.plist" ] || {
    echo 'Input is not a macOS .app bundle.' >&2
    exit 66
}
[ ! -e "$output" ] && [ ! -L "$output" ] || {
    echo "Output exists; refusing to overwrite: $output" >&2
    exit 67
}
case "$output" in
    /*) ;;
    *) echo 'Output must be an absolute path.' >&2; exit 68 ;;
esac
case "$output" in
    *.dmg) ;;
    *) echo 'Output filename must end with .dmg.' >&2; exit 69 ;;
esac

/usr/bin/codesign --verify --deep --strict "$app" || {
    echo 'App code signature invalid. First build and sign the app (ad hoc: codesign -s -) on a Mac.' >&2
    exit 70
}

# The build pipeline must disable the Companion listener before this step.
# A correctly signed build cannot be silently changed during packaging.
stage=$(mktemp -d "${TMPDIR:-/tmp}/polar-station-dmg.XXXXXXXX")
cleanup() { rm -rf -- "$stage"; }
trap cleanup EXIT HUP INT TERM
mkdir "$stage/volume"
/usr/bin/ditto --rsrc --extattr "$app" "$stage/volume/Polar Station.app"
ln -s /Applications "$stage/volume/Applications"
/usr/bin/hdiutil create -quiet -format UDZO -volname 'Polar Station' \
    -srcfolder "$stage/volume" "$stage/Polar-Station.dmg"
/usr/bin/hdiutil verify -quiet "$stage/Polar-Station.dmg"
/bin/mv -n "$stage/Polar-Station.dmg" "$output"
[ -f "$output" ] || {
    echo "DMG did not appear at $output" >&2
    exit 71
}
/usr/bin/shasum -a 256 "$output"
echo 'UNNOTARIZED: Gatekeeper may require System Settings > Privacy & Security > Open Anyway.'

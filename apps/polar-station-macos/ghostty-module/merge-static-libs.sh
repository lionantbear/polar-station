#!/bin/sh
set -eu

[ "$#" -ge 3 ] && [ "$1" = -o ] || {
    echo "usage: merge-static-libs.sh -o OUTPUT INPUT..." >&2
    exit 64
}

output=$2
shift 2
work=$(mktemp -d "${TMPDIR:-/tmp}/polar-station-libmerge.XXXXXX")
trap 'rm -rf -- "$work"' EXIT HUP INT TERM

index=0
for archive in "$@"; do
    case "$archive" in
        /*) ;;
        *) archive=$PWD/$archive ;;
    esac
    index=$((index + 1))
    part=$work/$index
    mkdir "$part"
    (cd "$part" && /usr/bin/ar -x "$archive")
    for object in "$part"/*.o; do
        [ -f "$object" ] || continue
        chmod 0644 "$object"
        mv "$object" "$work/${index}_$(basename "$object")"
    done
done

/usr/bin/libtool -static -o "$output" "$work"/*.o

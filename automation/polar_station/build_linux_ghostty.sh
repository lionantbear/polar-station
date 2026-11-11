#!/bin/sh
set -eu

usage() {
    cat >&2 <<'EOF'
usage:
  build_linux_ghostty.sh --prepare-search-prefix SYSROOT OUTPUT
  build_linux_ghostty.sh --build GHOSTTY_CHECKOUT ZIG_EXECUTABLE SYSROOT [OPTIMIZE] --receipt RECEIPT
EOF
    exit 64
}

require_absolute_directory() {
    label=$1
    value=$2
    case "$value" in
        /*) ;;
        *)
            echo "$label must be an absolute path: $value" >&2
            exit 65
            ;;
    esac
    if [ ! -d "$value" ]; then
        echo "$label is not a directory: $value" >&2
        exit 66
    fi
}

prepare_search_prefix() {
    sysroot=$1
    output=$2
    require_absolute_directory "sysroot" "$sysroot"
    case "$output" in
        /*) ;;
        *)
            echo "output must be an absolute path: $output" >&2
            exit 65
            ;;
    esac
    if [ -e "$output" ] || [ -L "$output" ]; then
        echo "output already exists: $output" >&2
        exit 67
    fi

    install -d -m 0755 "$output/usr/include"
    if [ ! -d "$sysroot/usr/lib" ]; then
        echo "sysroot is missing usr/lib" >&2
        exit 66
    fi
    for directory in bin share; do
        if [ ! -d "$sysroot/usr/$directory" ]; then
            echo "sysroot is missing usr/$directory" >&2
            exit 66
        fi
        ln -s "$sysroot/usr/$directory" "$output/usr/$directory"
    done

    for source in "$sysroot"/usr/include/*; do
        [ -e "$source" ] || [ -L "$source" ] || continue
        name=$(basename -- "$source")
        case "$name" in
            freetype|ft2build.h)
                continue
                ;;
        esac
        ln -s "$source" "$output/usr/include/$name"
    done

    if [ -e "$output/usr/include/freetype" ] \
        || [ -L "$output/usr/include/freetype" ] \
        || [ -e "$output/usr/include/ft2build.h" ] \
        || [ -L "$output/usr/include/ft2build.h" ]; then
        echo "filtered search prefix still shadows vendored FreeType" >&2
        exit 68
    fi
}

verify_source_unchanged() {
    guard_checkout=$1
    guard_head=$2
    guard_status=$3
    guard_archive=$4

    current_head=$(git -C "$guard_checkout" rev-parse HEAD)
    current_status=$(git -C "$guard_checkout" status --porcelain=v1 --untracked-files=all)
    current_archive=$(git -C "$guard_checkout" archive "$current_head" \
        | sha256sum | awk '{print $1}')
    if [ "$current_head" != "$guard_head" ] \
        || [ "$current_status" != "$guard_status" ] \
        || [ "$current_archive" != "$guard_archive" ]; then
        echo "source checkout changed during staged build: $guard_checkout" >&2
        return 1
    fi
}

polar_source_fingerprint() {
    fingerprint_root=$1
    python3 "$fingerprint_root/automation/polar_station/public_source_identity.py" \
        "$fingerprint_root" --fingerprint
}

materialize_staging_checkout() {
    checkout=$1
    repo_root=$2
    source_head_before=$3
    overlay_dir="$repo_root/apps/polar-station-linux/ghostty-overlay"
    overlay_patch="$overlay_dir/ghostty-v1.3.1.patch"
    staging_root="$repo_root/target/polar-station/ghostty-builds"
    install -d -m 0755 "$staging_root"
    staging_checkout=$(mktemp -d \
        "$staging_root/ghostty-$source_head_before.XXXXXX")
    git -C "$checkout" archive "$source_head_before" \
        | tar -x -C "$staging_checkout"
    (
        cd "$staging_checkout"
        # This staging directory lives below the Polar worktree. Prevent Git
        # from discovering that parent repository or `git apply` can report
        # success without changing the archived Ghostty tree.
        GIT_CEILING_DIRECTORIES="$repo_root" git apply --check "$overlay_patch"
        GIT_CEILING_DIRECTORIES="$repo_root" git apply "$overlay_patch"
    )
    python3 "$repo_root/automation/polar_station/apply_display_overlay.py" \
        --source "$staging_checkout" --platform linux >&2
    if ! grep -Fq '"polar-tmux",' "$staging_checkout/src/build/Config.zig"; then
        echo "managed Ghostty overlay did not register -Dpolar-tmux" >&2
        exit 72
    fi
    install -d -m 0755 "$staging_checkout/src/apprt/gtk/polar_tmux"
    cp -R "$repo_root/apps/polar-station-linux/ghostty-module/." \
        "$staging_checkout/src/apprt/gtk/polar_tmux/"
    cp "$repo_root/crates/polar-tmux/include/polar_station_desktop_client.h" \
        "$staging_checkout/src/apprt/gtk/polar_tmux/polar_station_desktop_client.h"
    cp "$repo_root/apps/polar-tmux-shared/display/station_display.h" \
        "$staging_checkout/src/apprt/gtk/polar_tmux/station_display.h"
    printf '%s\n' "$staging_checkout"
}

build_managed_ghostty() {
    checkout=$1
    zig=$2
    sysroot=$3
    optimize=${4:-ReleaseFast}
    receipt=$5

    case "$optimize" in
        Debug|ReleaseSafe|ReleaseFast|ReleaseSmall) ;;
        *)
            echo "unsupported optimization mode: $optimize" >&2
            exit 65
            ;;
    esac

    require_absolute_directory "Ghostty checkout" "$checkout"
    require_absolute_directory "sysroot" "$sysroot"
    case "$zig" in
        /*) ;;
        *)
            echo "Zig executable must be an absolute path: $zig" >&2
            exit 65
            ;;
    esac
    if [ ! -x "$zig" ]; then
        echo "Zig executable is unavailable: $zig" >&2
        exit 66
    fi

    script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
    repo_root=$(CDPATH='' cd -- "$script_dir/../.." && pwd)
    case "$receipt" in
        /*) ;;
        *)
            echo "runtime-set receipt path must be absolute: $receipt" >&2
            exit 65
            ;;
    esac
    if [ -L "$receipt" ]; then
        echo "refusing to replace a runtime-set receipt symlink: $receipt" >&2
        exit 67
    fi
    receipt_parent=$(dirname -- "$receipt")
    install -d -m 0755 "$receipt_parent"
    receipt_parent=$(CDPATH='' cd -- "$receipt_parent" && pwd -P)
    receipt="$receipt_parent/$(basename -- "$receipt")"
    if [ -e "$receipt" ] && [ ! -f "$receipt" ]; then
        echo "runtime-set receipt destination is not a regular file: $receipt" >&2
        exit 67
    fi
    expected_commit=332b2aefc6e72d363aa93ab6ecfc86eeeeb5ed28
    source_head_before=$(git -C "$checkout" rev-parse HEAD)
    if [ "$source_head_before" != "$expected_commit" ]; then
        echo "Ghostty commit mismatch: expected $expected_commit, got $source_head_before" >&2
        exit 69
    fi
    source_status_before=$(git -C "$checkout" status --porcelain=v1 --untracked-files=all)
    source_archive_before=$(git -C "$checkout" archive "$source_head_before" \
        | sha256sum | awk '{print $1}')
    polar_source_commit=$(python3 "$repo_root/automation/polar_station/public_source_identity.py" "$repo_root" --revision)
    polar_source_state_before=$(polar_source_fingerprint "$repo_root")
    graphics_bundle=${POLAR_STATION_GRAPHICS_BUNDLE:-}
    graphics_digest=
    graphics_path=
    if [ -n "$graphics_bundle" ]; then
        require_absolute_directory "graphics bundle" "$graphics_bundle"
        graphics_digest=$(python3 "$repo_root/automation/polar_station/graphics_resources.py" "$graphics_bundle")
    fi
    receipt_tmp=
    source_guard_active=1
    on_build_exit() {
        build_status=$?
        trap - EXIT
        if [ -n "$receipt_tmp" ] && [ -e "$receipt_tmp" ]; then
            rm -f -- "$receipt_tmp"
        fi
        if [ "$source_guard_active" -eq 1 ] && ! verify_source_unchanged \
            "$checkout" \
            "$source_head_before" \
            "$source_status_before" \
            "$source_archive_before"; then
            exit 75
        fi
        if [ "$source_guard_active" -eq 1 ] \
            && [ "$(polar_source_fingerprint "$repo_root")" != "$polar_source_state_before" ]; then
            echo "Polar source changed during the managed runtime build: $repo_root" >&2
            exit 77
        fi
        exit "$build_status"
    }
    trap on_build_exit EXIT
    staging_checkout=$(materialize_staging_checkout \
        "$checkout" \
        "$repo_root" \
        "$source_head_before")

    rust_profile=release
    if [ "$optimize" = Debug ]; then
        rust_profile=debug
    fi
    rust_client_lib_dir=$(cd "$repo_root" && python3 -m automation.polar_station.build_rust_runtime \
        --source-root "$repo_root" --profile "$rust_profile")
    if [ ! -f "$rust_client_lib_dir/libpolar_tmux.a" ]; then
        echo "shared Rust desktop client library is missing" >&2
        exit 76
    fi
    host_artifact="$rust_client_lib_dir/polar-tmux"
    if [ ! -f "$host_artifact" ] || [ ! -x "$host_artifact" ]; then
        echo "matching Rust Host executable is missing: $host_artifact" >&2
        exit 76
    fi

    task_cache_root=${XDG_CACHE_HOME:-"$HOME/.cache"}
    prefix_key=$(printf '%s\n' "$sysroot" | sha256sum | cut -c1-16)
    filtered_prefix="$task_cache_root/polar-station/ghostty-search-prefix-v1-$prefix_key"
    if [ ! -d "$filtered_prefix" ]; then
        install -d -m 0755 "$(dirname -- "$filtered_prefix")"
        prepare_search_prefix "$sysroot" "$filtered_prefix"
    fi
    if [ -e "$filtered_prefix/usr/include/freetype" ] \
        || [ -L "$filtered_prefix/usr/include/freetype" ] \
        || [ -e "$filtered_prefix/usr/include/ft2build.h" ] \
        || [ -L "$filtered_prefix/usr/include/ft2build.h" ] \
        || [ -e "$filtered_prefix/usr/lib" ] \
        || [ -L "$filtered_prefix/usr/lib" ]; then
        echo "cached search prefix violates FreeType or RUNPATH isolation" >&2
        exit 68
    fi

    inherited_path=${PATH:-/usr/bin:/bin}
    inherited_ld_library_path=${LD_LIBRARY_PATH:-}
    ld_library_path="$sysroot/usr/lib/x86_64-linux-gnu"
    if [ -n "$inherited_ld_library_path" ]; then
        ld_library_path="$ld_library_path:$inherited_ld_library_path"
    fi

    (
        cd "$staging_checkout"
        env \
            PATH="$sysroot/usr/bin:$inherited_path" \
            PYTHONPATH="$sysroot/usr/lib/python3/dist-packages" \
            LD_LIBRARY_PATH="$ld_library_path" \
            PKG_CONFIG_SYSROOT_DIR="$sysroot" \
            PKG_CONFIG_LIBDIR="$sysroot/usr/lib/x86_64-linux-gnu/pkgconfig:$sysroot/usr/share/pkgconfig" \
            "$zig" build \
                --search-prefix "$filtered_prefix/usr" \
                -Dpolar-tmux=true \
                -Dpolar-station-client-lib-dir="$rust_client_lib_dir" \
                -Demit-exe=true \
                -Dapp-runtime=gtk \
                -fsys=fontconfig -fsys=harfbuzz \
                -Dgtk-wayland=true \
                -Dgtk-x11=false \
                -Doptimize="$optimize" \
                --prefix "$staging_checkout/zig-out"
    )

    artifact="$staging_checkout/zig-out/bin/polarstation"
    if [ ! -x "$artifact" ]; then
        echo "managed Ghostty artifact is missing: $artifact" >&2
        exit 74
    fi
    digest=$(sha256sum "$artifact" | awk '{print $1}')
    host_digest=$(sha256sum "$host_artifact" | awk '{print $1}')
    verify_source_unchanged \
        "$checkout" \
        "$source_head_before" \
        "$source_status_before" \
        "$source_archive_before"
    if [ "$(polar_source_fingerprint "$repo_root")" != "$polar_source_state_before" ]; then
        echo "Polar source changed during the managed runtime build: $repo_root" >&2
        exit 77
    fi
    installable=false
    if [ "$optimize" = ReleaseFast ] && [ "$rust_profile" = release ]; then
        installable=true
    fi
    themes_path="$staging_checkout/zig-out/share/ghostty/themes"
    themes_digest=$(python3 "$repo_root/automation/polar_station/theme_resources.py" "$themes_path")
    shortcuts_path="$staging_checkout/zig-out/share/polar-station/gnome-shortcuts"
    shortcuts_digest=$(python3 "$repo_root/automation/polar_station/gnome_shortcuts.py" "$repo_root/apps/polar-station-linux/gnome-extension")
    python3 "$repo_root/automation/polar_station/theme_resources.py" "$repo_root/apps/polar-station-linux/gnome-extension" --expected "$shortcuts_digest" --destination "$shortcuts_path" >/dev/null
    runtime_set_id=$({ printf '%s\n' \
        "$polar_source_commit" \
        "$polar_source_state_before" \
        "$source_head_before" \
        "$optimize" \
        "$rust_profile" \
        "$digest" \
        "$host_digest" \
        "$themes_digest";
        printf '%s\n' "$shortcuts_digest"
        if [ -n "$graphics_digest" ]; then printf '%s\n' "$graphics_digest"; fi
        } | sha256sum | awk '{print $1}')
    if [ -n "$graphics_bundle" ]; then
        graphics_path="$staging_checkout/zig-out/bin/graphics"
        python3 "$repo_root/automation/polar_station/graphics_resources.py" "$graphics_bundle" --expected "$graphics_digest" --destination "$graphics_path" >/dev/null
    fi
    receipt_tmp=$(mktemp "$receipt_parent/.linux-runtime-set.v2.XXXXXX")
    jq -n \
        --arg shortcuts_path "$shortcuts_path" \
        --arg shortcuts_sha256 "$shortcuts_digest" \
        --arg graphics_path "$graphics_path" \
        --arg graphics_sha256 "$graphics_digest" \
        --arg themes_path "$themes_path" \
        --arg themes_sha256 "$themes_digest" \
        --arg artifact "$artifact" \
        --arg sha256 "$digest" \
        --arg host_artifact "$host_artifact" \
        --arg host_sha256 "$host_digest" \
        --arg runtime_set_id "$runtime_set_id" \
        --arg polar_source_commit "$polar_source_commit" \
        --arg polar_source_state_sha256 "$polar_source_state_before" \
        --arg upstream_commit "$source_head_before" \
        --arg source_checkout "$checkout" \
        --arg staging_checkout "$staging_checkout" \
        --arg optimize "$optimize" \
        --arg rust_profile "$rust_profile" \
        --argjson installable "$installable" \
        '{
            schema_version: 2,
            status: "development-built",
            product: "Polar Station",
            installable: $installable,
            runtime_set_id: $runtime_set_id,
            polar_source_commit: $polar_source_commit,
            polar_source_state_sha256: $polar_source_state_sha256,
            upstream_commit: $upstream_commit,
            source_checkout: $source_checkout,
            source_checkout_mode: "read-only-git-archive",
            source_checkout_unchanged: true,
            staging_checkout: $staging_checkout,
            optimize: $optimize,
            rust_profile: $rust_profile,
            artifacts: ({
                gnome_shortcuts: {path: $shortcuts_path, sha256: $shortcuts_sha256},
                themes: { path: $themes_path, sha256: $themes_sha256 },
                managed_ghostty: {
                    path: $artifact,
                    sha256: $sha256
                },
                rust_host: {
                    path: $host_artifact,
                    sha256: $host_sha256
                }
            } + (if $graphics_path == "" then {} else {graphics: {path: $graphics_path, sha256: $graphics_sha256}} end)),
            freetype_header_isolation: "filtered-search-prefix"
        }' > "$receipt_tmp"
    chmod 0644 "$receipt_tmp"
    mv -f -- "$receipt_tmp" "$receipt"
    receipt_tmp=
    cat "$receipt"
    source_guard_active=0
    trap - EXIT
}

case "${1:-}" in
    --prepare-search-prefix)
        [ "$#" -eq 3 ] || usage
        prepare_search_prefix "$2" "$3"
        ;;
    --build)
        if [ "$#" -eq 6 ] && [ "$5" = "--receipt" ]; then
            build_managed_ghostty "$2" "$3" "$4" "" "$6"
        elif [ "$#" -eq 7 ] && [ "$6" = "--receipt" ]; then
            build_managed_ghostty "$2" "$3" "$4" "$5" "$7"
        else
            usage
        fi
        ;;
    *)
        usage
        ;;
esac

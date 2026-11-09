"""Materialize a relocatable, prebuilt Linux graphics runtime for Station."""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import shlex
import shutil
import subprocess
import tempfile


def digest(directory: Path) -> str:
    if directory.is_symlink() or not directory.is_dir():
        raise ValueError("graphics runtime must be a real directory")
    value = hashlib.sha256()
    files = sorted(directory.rglob("*"))
    if not files:
        raise ValueError("graphics runtime is empty")
    for path in files:
        if path.is_symlink():
            raise ValueError("graphics runtime must be self-contained, without symlinks")
        if path.is_dir():
            continue
        if not path.is_file():
            raise ValueError("graphics runtime contains a special file")
        value.update(path.relative_to(directory).as_posix().encode() + b"\0")
        value.update(str(path.stat().st_mode & 0o777).encode() + b"\0")
        with path.open("rb") as stream:
            value.update(hashlib.file_digest(stream, "sha256").digest())
    return value.hexdigest()


def install(source: Path, expected: str, destination: Path) -> None:
    if digest(source) != expected:
        raise ValueError("graphics runtime digest mismatch")
    if destination.is_symlink() or (destination.exists() and not destination.is_dir()):
        raise ValueError("unsafe graphics runtime destination")
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".graphics-", dir=destination.parent) as temporary:
        stage = Path(temporary)
        shutil.copytree(source, stage / "new")
        if digest(stage / "new") != expected:
            raise ValueError("staged graphics runtime digest mismatch")
        if destination.exists():
            destination.rename(stage / "old")
        try:
            (stage / "new").rename(destination)
        except BaseException:
            if (stage / "old").exists():
                (stage / "old").rename(destination)
            raise


def build(root: Path, output: Path, godot: Path, editor: Path, extension: Path, graphviz: Path) -> None:
    # Inputs are already built artifacts. Launch never runs a compiler/editor.
    if output.exists():
        raise ValueError("choose a new bundle output directory")
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".graphics-build-", dir=output.parent) as temporary:
        stage = Path(temporary) / "graphics"
        stage.mkdir()
        shutil.copytree(root / "apps/polar-graphics-native/licenses", stage / "licenses")
        for package in ("d3-force-3d", "d3-octree", "d3-binarytree", "d3-quadtree"):
            license_path = root / "crates/polar-force-layout-core/upstream" / package / "LICENSE"
            if license_path.is_file():
                shutil.copy2(license_path, stage / "licenses" / (package + "-LICENSE"))
        libname = "libgodot.linuxbsd.template_debug.x86_64.so"
        shutil.copy2(godot, stage / libname)
        flags = shlex.split(subprocess.check_output(["pkg-config", "--cflags", "--libs", "gtk4", "epoxy"], text=True))
        subprocess.run(["cc", "-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-fPIC", "-shared",
                        str(root / "apps/polar-graphics-native/gtk/polar_graphics_gtk.c"),
                        "-o", str(stage / "libpolar_graphics_gtk.so"), *flags,
                        "-L" + str(stage), "-l:" + libname, "-Wl,-rpath,$ORIGIN"], check=True)
        shutil.copytree(root / "apps/polar-ideacave-godot", stage / "project",
                        ignore=shutil.ignore_patterns(".godot", "*.uid", "*.so", "AGENTS.md", "test-fixtures", "*_selftest.gd", "*_test.gd"))
        shutil.copy2(extension, stage / "project/native/libpolar_graphics_godot.so")
        shutil.copytree(graphviz / "usr/lib/x86_64-linux-gnu", stage / "graphviz/lib")
        (stage / "graphviz/bin").mkdir()
        shutil.copy2(graphviz / "usr/bin/dot", stage / "graphviz/bin/dot.bin")
        wrapper = stage / "graphviz/bin/dot"
        wrapper.write_text('''#!/bin/sh
set -eu
base=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
export GVBINDIR="$base/lib/graphviz"
export LD_LIBRARY_PATH="$base/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
exec "$base/bin/dot.bin" "$@"
''')
        wrapper.chmod(0o755)
        if (graphviz / "usr/share/doc").exists():
            shutil.copytree(graphviz / "usr/share/doc", stage / "graphviz/licenses")
        # Load the known, non-reloadable extension at engine startup. Discovering
        # it halfway through the first editor import crashes this pinned editor
        # during teardown; startup loading also matches the runtime path.
        (stage / "project/.godot").mkdir(exist_ok=True)
        (stage / "project/.godot/extension_list.cfg").write_text("res://native/polar_force_layout.gdextension\n")
        subprocess.run([str(editor), "--headless", "--path", str(stage / "project"), "--editor", "--import"], check=True)
        # Godot editor import may exit successfully despite script parse errors.
        subprocess.run([str(editor), "--headless", "--path", str(stage / "project"),
                        "--check-only", "--script", "res://itree_workspace.gd"], check=True)
        # Editor-only state must not retain paths into the build staging area.
        shutil.rmtree(stage / "project/.godot/editor", ignore_errors=True)
        shutil.rmtree(stage / "project/.godot/shader_cache", ignore_errors=True)
        provenance = {"schema": "polar.graphics.linux-bundle.v1", "godot_commit": "ed1daf0bf001b61586d9930840f2f1394092c079",
                      "inputs": {name: hashlib.sha256(path.read_bytes()).hexdigest()
                                 for name, path in (("libgodot", godot), ("gdextension", extension), ("godot_editor", editor))}}
        (stage / "provenance.json").write_text(json.dumps(provenance, indent=2) + "\n")
        digest(stage)
        stage.rename(output)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("--expected")
    parser.add_argument("--destination", type=Path)
    parser.add_argument("--build", action="store_true")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--godot-library", type=Path)
    parser.add_argument("--godot-editor", type=Path)
    parser.add_argument("--extension", type=Path)
    parser.add_argument("--graphviz-root", type=Path)
    args = parser.parse_args()
    if args.build:
        if not all((args.output, args.godot_library, args.godot_editor, args.extension, args.graphviz_root)):
            parser.error("build requires explicit output and prebuilt input artifacts")
        build(args.source, args.output, args.godot_library, args.godot_editor, args.extension, args.graphviz_root)
        print(digest(args.output))
    else:
        if args.destination:
            if not args.expected:
                parser.error("--expected is required for installation")
            install(args.source, args.expected, args.destination)
        print(digest(args.destination or args.source))


if __name__ == "__main__":
    main()

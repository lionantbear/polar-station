"""Materialize a relocatable, prebuilt Linux graphics runtime for Station."""
from __future__ import annotations

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




def materialize_browsing_config(root: Path, project: Path) -> None:
    shutil.copy2(root / "apps/polar-ideacave-godot/polar-browsing.itree.json",
                 project / "polar-browsing.itree.json")


def validate_engine_function_bindings(godot: Path) -> None:
    """Reject engines whose font calls can bind to the executable's FreeType."""
    relocations = subprocess.check_output(["readelf", "--relocs", "--wide", str(godot)], text=True)
    for symbol in ("FT_Open_Face", "FT_Init_FreeType", "FT_Error_String"):
        if any(symbol in line.split() for line in relocations.splitlines()):
            raise ValueError(f"shared engine font function is interposable: {symbol}")


def build(root: Path, output: Path, godot: Path, editor: Path, extension: Path, graphviz: Path) -> None:
    # Inputs are already built artifacts. Launch never runs a compiler/editor.
    if output.exists():
        raise ValueError("choose a new bundle output directory")
    validate_engine_function_bindings(godot)
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".graphics-build-", dir=output.parent) as temporary:
        stage = Path(temporary) / "graphics"
        stage.mkdir()
        shutil.copytree(root / "apps/polar-graphics-native/licenses", stage / "licenses")
        for package in ("d3-force-3d", "d3-octree", "d3-binarytree", "d3-quadtree"):
            license_path = root / "crates/polar-force-layout-core/upstream" / package / "LICENSE"
            if license_path.is_file():
                shutil.copy2(license_path, stage / "licenses" / (package + "-LICENSE"))
        libname = godot.name
        shutil.copy2(godot, stage / libname)
        flags = shlex.split(subprocess.check_output(["pkg-config", "--cflags", "--libs", "gtk4", "epoxy"], text=True))
        subprocess.run(["cc", "-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-fPIC", "-shared",
                        str(root / "apps/polar-graphics-native/gtk/polar_graphics_gtk.c"),
                        "-o", str(stage / "libpolar_graphics_gtk.so"), *flags,
                        "-L" + str(stage), "-l:" + libname, "-Wl,-rpath,$ORIGIN"], check=True)
        shutil.copytree(root / "apps/polar-ideacave-godot", stage / "project",
                        ignore=shutil.ignore_patterns(".godot", "*.uid", "*.so", "AGENTS.md", "test-fixtures", "*_selftest.gd", "*_test.gd"))
        materialize_browsing_config(root, stage / "project")
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





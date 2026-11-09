"""Package the prebuilt macOS Idea Tree GPU runtime for the signed Station app."""
from __future__ import annotations
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
from graphics_resources import digest


def run(*args: str, **kwargs):
    return subprocess.run(args, check=True, **kwargs)


def package(source: Path, godot: Path, editor: Path, extension: Path, graphviz: Path, output: Path):
    version = subprocess.check_output([str(editor), '--version'], text=True).strip()
    if not version.startswith('4.7.2.stable'):
        raise ValueError('Expected pinned Godot 4.7.2 editor')
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.graphics-', dir=output.parent) as temporary:
        stage = Path(temporary) / 'graphics'
        stage.mkdir()
        shutil.copytree(source / 'apps/polar-graphics-native/licenses', stage / 'licenses')
        for license in (source / 'crates/polar-force-layout-core/upstream').glob('*/LICENSE'):
            shutil.copy2(license, stage / 'licenses' / (license.parent.name + '-LICENSE'))
        shutil.copy2(godot, stage / 'libgodot.dylib')
        run('install_name_tool', '-id', '@rpath/libgodot.dylib', str(stage / 'libgodot.dylib'))
        shutil.copytree(source / 'apps/polar-ideacave-godot', stage / 'project',
                        ignore=shutil.ignore_patterns('.godot', '*.uid', '*.so', '*.dylib', 'AGENTS.md', 'test-fixtures', '*_selftest.gd', '*_test.gd'))
        shutil.copy2(extension, stage / 'project/native/libpolar_graphics_godot.dylib')
        run('install_name_tool', '-id', '@rpath/libpolar_graphics_godot.dylib', str(stage / 'project/native/libpolar_graphics_godot.dylib'))
        lib = stage / 'graphviz/lib'
        lib.mkdir(parents=True)
        (stage / 'graphviz/bin').mkdir()
        copied: dict[Path, Path] = {}
        def copy_macho(original: Path, destination: Path):
            original = original.resolve()
            if original in copied:
                return copied[original]
            if destination.exists():
                raise ValueError(f'Conflicting library basename: {destination.name}')
            copied[original] = destination
            shutil.copy2(original, destination)
            destination.chmod(destination.stat().st_mode | 0o200)
            deps = subprocess.check_output(['otool', '-L', str(original)], text=True).splitlines()[1:]
            for line in deps:
                dependency = line.strip().split(' (', 1)[0]
                if dependency.startswith(('/usr/lib/', '/System/Library/')):
                    continue
                if Path(dependency).resolve() == original:
                    run('install_name_tool', '-id', '@rpath/' + destination.name, str(destination))
                    continue
                if not dependency.startswith('/'):
                    raise ValueError(f'Unresolved dependency {dependency} in {original}')
                target = copy_macho(Path(dependency), lib / Path(dependency).name)
                relative = os.path.relpath(target, destination.parent)
                run('install_name_tool', '-change', dependency, '@loader_path/' + relative, str(destination))
            run('codesign', '--force', '--sign', '-', str(destination))
            return destination
        copy_macho(graphviz / 'bin/dot', stage / 'graphviz/bin/dot.bin')
        for pattern in ('libgvplugin_core.*.dylib', 'libgvplugin_dot_layout.*.dylib', 'libgvplugin_neato_layout.*.dylib'):
            candidates = sorted((graphviz / 'lib/graphviz').glob(pattern))
            if not candidates:
                raise ValueError(f'Missing Graphviz plugin {pattern}')
            copy_macho(candidates[0], lib / candidates[0].name)
        wrapper = stage / 'graphviz/bin/dot'
        wrapper.write_text('''#!/bin/sh
set -eu
base=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
export GVBINDIR="$base/lib"
exec "$base/bin/dot.bin" -Kdot "$@"
''')
        wrapper.chmod(0o755)
        for license in graphviz.glob('*LICENSE*'):
            shutil.copy2(license, stage / 'licenses' / ('graphviz-' + license.name))
        run(str(wrapper), '-c')
        for engine in ('dot', 'neato'):
            run(str(wrapper), '-K' + engine, '-Tplain', input=b'digraph { a -> b }', stdout=subprocess.DEVNULL)
        (stage / 'project/.godot').mkdir()
        (stage / 'project/.godot/extension_list.cfg').write_text('res://native/polar_force_layout.gdextension\n')
        run('codesign', '--force', '--sign', '-', str(stage / 'project/native/libpolar_graphics_godot.dylib'))
        run(str(editor), '--headless', '--path', str(stage / 'project'), '--editor', '--import')
        run(str(editor), '--headless', '--path', str(stage / 'project'), '--check-only', '--script', 'res://itree_workspace.gd')
        shutil.rmtree(stage / 'project/.godot/editor', ignore_errors=True)
        shutil.rmtree(stage / 'project/.godot/shader_cache', ignore_errors=True)
        receipt = {'schema': 'polar.graphics.macos-bundle.v1', 'godot_commit': 'ed1daf0bf001b61586d9930840f2f1394092c079',
                   'source_commit': subprocess.check_output(['git', '-C', str(source), 'rev-parse', 'HEAD'], text=True).strip(),
                   'macos_lifecycle_patch': 'OS_MacOS_Headless (host owns NSApplication)',
                   'inputs': {name: hashlib.sha256(path.read_bytes()).hexdigest() for name, path in [('libgodot', godot), ('gdextension', extension), ('editor', editor)]}}
        (stage / 'provenance.json').write_text(json.dumps(receipt, indent=2) + '\n')
        digest(stage)
        if output.exists():
            output.rename(Path(temporary) / 'old')
        stage.rename(output)
    print(digest(output))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ['source', 'godot', 'editor', 'extension', 'graphviz', 'output']:
        parser.add_argument('--' + name, type=Path, required=True)
    package(**vars(parser.parse_args()))

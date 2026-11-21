#!/usr/bin/env python3
"""Apply the paired Station overlays to a clean, staged Ghostty source tree."""

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess


HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
PLATFORMS = ('linux', 'macos')
SHARED_MODULES = ('StationDisplay.zig', 'StationRenderWakeup.zig', 'StationTerminal.zig')


def overlay_directory(platform):
    return ROOT / 'apps' / f'polar-station-{platform}' / 'ghostty-overlay'


def pin():
    linux, macos = (
        json.loads((overlay_directory(platform) / 'upstream.lock.json').read_text())['upstream']
        for platform in PLATFORMS
    )
    if linux != macos:
        raise ValueError('Linux and macOS must use the same Ghostty and Zig versions')
    return linux


def install(source, platform, *, tests=False):
    source = source.resolve(strict=True)
    if source.is_relative_to(Path.home() / 'dev/reference'):
        raise ValueError('The Ghostty overlay requires a disposable source directory')
    upstream = pin()
    if (source / '.git').exists():
        head = subprocess.check_output(['git', '-C', source, 'rev-parse', 'HEAD'], text=True).strip()
        if head != upstream['commit']:
            raise ValueError(f'Unexpected upstream Ghostty revision {head}')

    filename = f'ghostty-{upstream["tag"]}.patch'
    platform_dir = overlay_directory(platform)
    environment = os.environ | {'GIT_CEILING_DIRECTORIES': str(source.parent)}
    patches = [HERE / filename, platform_dir / filename]
    if tests:
        test_filename = filename.removesuffix('.patch') + '-tests.patch'
        test_dir = 'Tests' if platform == 'macos' else 'tests'
        test_patches = [HERE / 'tests' / test_filename, platform_dir.parent / test_dir / test_filename]

    destinations = [source / 'src/termio' / name for name in SHARED_MODULES]
    for path in destinations:
        if not path.resolve().is_relative_to(source) or path.exists() or path.is_symlink():
            raise ValueError(f'Unexpected existing overlay destination {path}')

    def apply(paths):
        command = ['git', '-C', source, 'apply']
        subprocess.run([*command, '--check', *paths], env=environment, check=True)
        subprocess.run([*command, *paths], env=environment, check=True)

    apply(patches)
    apply([HERE / 'station-terminal.patch'])
    apply([platform_dir / 'station-terminal.patch'])
    apply([HERE / 'station-terminal-view.patch'])
    apply([HERE / 'station-terminal-mailbox.patch'])
    if tests:
        apply(test_patches)
    for path in destinations:
        shutil.copyfile(HERE / path.name, path)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', required=True, type=Path)
    parser.add_argument('--platform', required=True, choices=PLATFORMS)
    parser.add_argument('--tests', action='store_true')
    args = parser.parse_args()
    install(args.source, args.platform, tests=args.tests)


if __name__ == '__main__':
    main()

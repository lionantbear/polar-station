"""Build the paired standalone Polar Station Rust Host and desktop client library.

No private monorepo host configuration or installed build executor is required.
Cargo diagnostics go to stderr; stdout contains only the artifact directory.
"""
from __future__ import annotations
import argparse
import os
from pathlib import Path
import subprocess
import sys


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source-root', required=True, type=Path)
    parser.add_argument('--profile', required=True, choices=('debug', 'release'))
    args = parser.parse_args()
    root = args.source_root.resolve()
    if not (root / 'Cargo.toml').is_file():
        parser.error('source root must contain Cargo.toml')
    target = Path(os.environ.get('CARGO_TARGET_DIR', str(root / 'target')))
    if not target.is_absolute():
        target = root / target
    target = target.resolve()
    env = dict(os.environ, CARGO_TARGET_DIR=str(target))
    command = ['cargo', 'build', '--locked', '--manifest-path', str(root / 'Cargo.toml'),
               '-p', 'polar-tmux', '--lib', '--bin', 'polar-tmux']
    if args.profile == 'release':
        command.append('--release')
    result = subprocess.run(command, cwd=root, env=env, stdout=sys.stderr)
    if result.returncode:
        return result.returncode
    artifacts = target / args.profile
    for name in ('polar-tmux', 'libpolar_tmux.a'):
        if not (artifacts / name).is_file():
            print(f'missing paired Rust artifact: {artifacts / name}', file=sys.stderr)
            return 1
    print(artifacts)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())

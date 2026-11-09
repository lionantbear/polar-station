"""Package and install the pinned Ghostty theme directory in Station's namespace."""
from __future__ import annotations
import argparse
import hashlib
from pathlib import Path
import os
import shutil
import tempfile


def digest(directory: Path) -> str:
    if directory.is_symlink() or not directory.is_dir():
        raise ValueError('theme directory must be a real directory')
    files = sorted(directory.iterdir(), key=lambda p: p.name)
    if not files:
        raise ValueError('theme directory is empty')
    value = hashlib.sha256()
    for path in files:
        if path.is_symlink() or not path.is_file():
            raise ValueError('theme resources must contain only regular files')
        value.update(path.name.encode() + b'\0' + hashlib.sha256(path.read_bytes()).digest())
    return value.hexdigest()


def install(source: Path, expected: str, destination: Path) -> None:
    if digest(source) != expected:
        raise ValueError('theme resource digest mismatch')
    if destination.is_symlink() or (destination.exists() and not destination.is_dir()):
        raise ValueError('unsafe theme destination')
    destination.parent.mkdir(parents=True, exist_ok=True)
    stage = Path(tempfile.mkdtemp(prefix='.themes-', dir=destination.parent))
    try:
        shutil.copytree(source, stage / 'new')
        if digest(stage / 'new') != expected:
            raise ValueError('staged theme resource digest mismatch')
        if destination.exists():
            destination.rename(stage / 'old')
        try:
            (stage / 'new').rename(destination)
        except BaseException:
            if (stage / 'old').exists():
                (stage / 'old').rename(destination)
            raise
    finally:
        shutil.rmtree(stage)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source', type=Path)
    parser.add_argument('--expected')
    parser.add_argument('--destination', type=Path)
    args = parser.parse_args()
    if args.destination:
        if not args.expected:
            parser.error('--expected is required for installation')
        install(args.source, args.expected, args.destination)
    print(digest(args.destination or args.source))

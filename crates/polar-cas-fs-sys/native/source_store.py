#!/usr/bin/env python3
"""Admit an exact liberofs Git tree from a checkout or an offline source tarball.

This module does not access the network or execute code from the admitted tree.
Git SHA-1 here identifies the *upstream Git tree*, not Polar content identities.
A six-file version probe is not sufficient to authenticate a whole C build.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import stat
import tarfile
from typing import Any

HERE = Path(__file__).resolve().parent
MAX_FILES = 4096
MAX_BYTES = 64 * 1024 * 1024
MAX_FILE_BYTES = 8 * 1024 * 1024
MAX_ARCHIVE_BYTES = 128 * 1024 * 1024
MAX_DEPTH = 64


class SourceError(RuntimeError):
    """A source is missing, not pinned, unsafe, or outside its resource budget."""


def load_lock() -> dict[str, Any]:
    return json.loads((HERE / 'upstream.lock.json').read_text())


def _object_id(kind: bytes, data: bytes) -> bytes:
    return hashlib.sha1(kind + b' ' + str(len(data)).encode() + b'\0' + data).digest()


def _safe_link(path: PurePosixPath, target: str) -> None:
    if not target or '\0' in target or PurePosixPath(target).is_absolute():
        raise SourceError(f'unsafe symlink: {path}')
    parts = list(path.parent.parts)
    for part in PurePosixPath(target).parts:
        if part == '..':
            if not parts:
                raise SourceError(f'symlink escapes source: {path}')
            parts.pop()
        elif part != '.':
            parts.append(part)


def inspect_tree(source: Path) -> dict[str, Any]:
    """Compute Git's tree from pinned directory FDs; never follow source links.

    Replacing a directory by a symlink during traversal must fail rather than
    turn a version check into a read outside the source root. Root .git is
    administrative data, ignored without executing Git or its hooks.
    """
    source = Path(source)
    count = total = visited = 0
    files: list[dict[str, Any]] = []
    dirflags = os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW | os.O_DIRECTORY

    def stamp(value: os.stat_result) -> tuple[int, ...]:
        return (value.st_dev, value.st_ino, value.st_size,
                value.st_mtime_ns, value.st_ctime_ns)

    def walk(directory: int, relative: PurePosixPath, depth: int) -> bytes:
        nonlocal count, total, visited
        if depth > MAX_DEPTH:
            raise SourceError('source depth limit exceeded')
        initial = os.fstat(directory)
        entries: list[tuple[bytes, bytes]] = []
        with os.scandir(directory) as children:
            for child in children:
                name = os.fsencode(child.name)
                rel = relative / child.name
                if len(name) > 255 or b'\0' in name:
                    raise SourceError(f'invalid source name: {rel}')
                before = os.stat(child.name, dir_fd=directory, follow_symlinks=False)
                if depth == 0 and child.name == '.git':
                    if stat.S_ISLNK(before.st_mode):
                        raise SourceError('.git cannot be a symlink')
                    continue
                visited += 1
                if visited > MAX_FILES * 2:
                    raise SourceError('source entry count limit exceeded')
                if stat.S_ISDIR(before.st_mode):
                    fd = os.open(child.name, dirflags, dir_fd=directory)
                    try:
                        if stamp(before) != stamp(os.fstat(fd)):
                            raise SourceError(f'source directory changed during open: {rel}')
                        digest = walk(fd, rel, depth + 1)
                    finally:
                        os.close(fd)
                    # Git cannot track empty directories.
                    if digest != _object_id(b'tree', b''):
                        entries.append((name + b'/', b'40000 ' + name + b'\0' + digest))
                    continue
                count += 1
                if count > MAX_FILES or before.st_size > MAX_FILE_BYTES:
                    raise SourceError('source object count/file size limit exceeded')
                if stat.S_ISLNK(before.st_mode):
                    target = os.readlink(child.name, dir_fd=directory)
                    _safe_link(rel, target)
                    data = os.fsencode(target)
                    after = os.stat(child.name, dir_fd=directory, follow_symlinks=False)
                    if stamp(before) != stamp(after):
                        raise SourceError(f'source symlink changed while hashing: {rel}')
                    mode = b'120000'
                elif stat.S_ISREG(before.st_mode):
                    fd = os.open(child.name, os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW,
                                 dir_fd=directory)
                    try:
                        opened = os.fstat(fd)
                        if not stat.S_ISREG(opened.st_mode) or stamp(before) != stamp(opened):
                            raise SourceError(f'source changed during open: {rel}')
                        chunks = []
                        length = 0
                        while True:
                            block = os.read(fd, min(65536, MAX_FILE_BYTES + 1 - length))
                            if not block:
                                break
                            chunks.append(block)
                            length += len(block)
                            if length > MAX_FILE_BYTES:
                                raise SourceError(f'oversized source: {rel}')
                        if stamp(opened) != stamp(os.fstat(fd)):
                            raise SourceError(f'source changed while hashing: {rel}')
                        data = b''.join(chunks)
                    finally:
                        os.close(fd)
                    mode = b'100755' if before.st_mode & 0o111 else b'100644'
                else:
                    raise SourceError(f'special file in source: {rel}')
                total += len(data)
                if total > MAX_BYTES:
                    raise SourceError('total source bytes limit exceeded')
                blob = _object_id(b'blob', data)
                files.append({'path': str(rel), 'mode': mode.decode(),
                              'git_blob': blob.hex(), 'sha256': hashlib.sha256(data).hexdigest(),
                              'size': len(data)})
                entries.append((name, mode + b' ' + name + b'\0' + blob))
        if stamp(initial) != stamp(os.fstat(directory)):
            raise SourceError(f'source directory changed during traversal: {relative}')
        return _object_id(b'tree', b''.join(record for _, record in sorted(entries)))

    try:
        root_fd = os.open(source, dirflags)
        try:
            root = walk(root_fd, PurePosixPath(), 0)
        finally:
            os.close(root_fd)
    except OSError as exc:
        raise SourceError(f'cannot safely read complete source: {exc}') from exc
    return {'tree': root.hex(), 'files': sorted(files, key=lambda item: item['path']),
            'file_count': count, 'size_bytes': total}


def validate_source(source: Path, lock: dict[str, Any] | None = None) -> dict[str, Any]:
    lock = lock if lock is not None else load_lock()
    expected = lock.get('tree')
    if not isinstance(expected, str) or len(expected) != 40 or any(c not in '0123456789abcdef' for c in expected):
        raise SourceError('source lock lacks a complete upstream tree ID')
    inspected = inspect_tree(source)
    if inspected['tree'] != expected:
        raise SourceError(f'upstream tree mismatch: expected {expected}, got {inspected["tree"]}')
    blobs = {item['path']: item['git_blob'] for item in inspected['files']}
    for name, value in lock.get('git_blob_sha1', {}).items():
        if blobs.get(name) != value:
            raise SourceError(f'pinned version probe mismatch: {name}')
    return inspected


def _member_path(name: str) -> PurePosixPath:
    path = PurePosixPath(name)
    if not name or '\0' in name or path.is_absolute() or '..' in path.parts:
        raise SourceError(f'unsafe archive path: {name!r}')
    if len(path.parts) > MAX_DEPTH or any(len(os.fsencode(p)) > 255 for p in path.parts):
        raise SourceError('archive path size/depth limit exceeded')
    return path


def file_sha256(path: Path) -> str:
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def import_archive(archive: Path, destination: Path,
                   lock: dict[str, Any] | None = None) -> Path:
    """Create destination/source exclusively; never overwrite a saved checkout.

    A partial import is quarantined inside a fresh mode-0700 directory, then
    deleted on error. Provenance stays OUTSIDE source so its Git tree is exact.
    """
    archive, destination = Path(archive), Path(destination)
    lock = lock if lock is not None else load_lock()
    st = archive.lstat()
    if not stat.S_ISREG(st.st_mode) or st.st_size > MAX_ARCHIVE_BYTES:
        raise SourceError('archive must be a regular file within the size limit')
    destination.mkdir(parents=True, exist_ok=False, mode=0o700)
    unpack = destination / 'unpacked'
    unpack.mkdir(mode=0o700)
    seen: set[PurePosixPath] = set()
    pending_links: list[tuple[PurePosixPath, str]] = []
    total = count = 0
    try:
        with tarfile.open(archive, 'r|*') as tf:
            for item in tf:
                count += 1
                if count > MAX_FILES * 2:
                    raise SourceError('archive member count limit exceeded')
                path = _member_path(item.name)
                if path == PurePosixPath('.'):
                    if item.isdir():
                        continue
                    raise SourceError('non-directory archive root')
                if path in seen:
                    raise SourceError(f'duplicate archive member: {path}')
                seen.add(path)
                if '.git' in path.parts:
                    raise SourceError('source archives must not contain .git metadata')
                if item.isdir():
                    (unpack / str(path)).mkdir(parents=True, exist_ok=True, mode=0o700)
                    continue
                if item.issym():
                    _safe_link(path, item.linkname)
                    pending_links.append((path, item.linkname))
                    continue
                if not item.isfile():
                    raise SourceError(f'unsupported archive member: {path}')
                if item.size < 0 or item.size > MAX_FILE_BYTES:
                    raise SourceError(f'archive file size limit: {path}')
                total += item.size
                if total > MAX_BYTES:
                    raise SourceError('archive total bytes limit exceeded')
                target = unpack / str(path)
                target.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
                src = tf.extractfile(item)
                if src is None:
                    raise SourceError(f'missing file payload: {path}')
                fd = os.open(target, os.O_CREAT | os.O_EXCL | os.O_WRONLY | os.O_NOFOLLOW, 0o600)
                try:
                    with os.fdopen(fd, 'wb') as out:
                        copied = 0
                        while copied < item.size:
                            block = src.read(min(65536, item.size - copied))
                            if not block:
                                raise SourceError(f'truncated archive file: {path}')
                            out.write(block)
                            copied += len(block)
                        out.flush()
                        os.fchmod(out.fileno(), 0o755 if item.mode & 0o111 else 0o644)
                finally:
                    src.close()
        link_names = {name for name, _ in pending_links}
        for name in seen:
            if any(parent in link_names for parent in name.parents):
                raise SourceError(f'archive member descends through symlink: {name}')
        for name, target in pending_links:
            dest = unpack / str(name)
            dest.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
            os.symlink(target, dest)
        children = list(unpack.iterdir())
        root = children[0] if len(children) == 1 and children[0].is_dir() and not children[0].is_symlink() else unpack
        inspected = validate_source(root, lock)
        final = destination / 'source'
        root.rename(final)
        if unpack.exists():
            unpack.rmdir()
        record = {'commit': lock.get('commit'), 'tree': inspected['tree'],
                  'archive_sha256': file_sha256(archive),
                  'file_count': inspected['file_count'], 'size_bytes': inspected['size_bytes']}
        (destination / 'SOURCE_PROVENANCE.json').write_text(json.dumps(record, indent=2) + '\n')
        return final
    except BaseException:
        shutil.rmtree(destination)
        raise


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument('--source', type=Path)
    group.add_argument('--archive', type=Path)
    parser.add_argument('--destination', type=Path)
    args = parser.parse_args()
    if args.archive and args.destination is None:
        parser.error('--archive requires --destination')
    try:
        if args.archive:
            print(import_archive(args.archive, args.destination))
        else:
            inspected = validate_source(args.source)
            print(json.dumps({k: v for k, v in inspected.items() if k != 'files'}, indent=2))
    except (SourceError, OSError, tarfile.TarError) as error:
        parser.exit(1, f'source admission failed: {error}\n')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())

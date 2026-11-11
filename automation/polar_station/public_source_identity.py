"""Identify exported Station inputs without relying on a surrounding Git checkout."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import subprocess


def source_identity(root: Path) -> dict:
    root = root.resolve()
    manifest = root / "PUBLIC_SOURCE_MANIFEST.json"
    if manifest.is_symlink():
        raise ValueError("public source manifest must be a regular file")
    manifest_bytes = manifest.read_bytes()
    data = json.loads(manifest_bytes)
    provenance = data.get("source_provenance", {})
    head = provenance.get("git_head")
    dirty = provenance.get("git_dirty")
    inputs = provenance.get("inputs_sha256", "")
    count = provenance.get("input_count")
    if (provenance.get("kind") != "working_tree"
            or (head is not None and not re.fullmatch(r"[a-f0-9]{40}", head))
            or (dirty is not None and type(dirty) is not bool)
            or not re.fullmatch(r"[a-f0-9]{64}", inputs)
            or type(count) is not int or count <= 0):
        raise ValueError("missing or invalid Mono source provenance")
    files = data.get("files")
    links = data.get("symlinks", {})
    if not isinstance(files, dict) or not files or not isinstance(links, dict):
        raise ValueError("missing exported source input inventory")
    modes = data.get("file_modes")
    if (not isinstance(modes, dict) or set(modes) != set(files)
            or any(mode not in ("100644", "100755") for mode in modes.values())
            or set(files) & set(links)):
        raise ValueError("missing or invalid exported file modes")
    value = hashlib.sha256(manifest_bytes)
    known = set(files) | set(links)
    # Keep the original Mono guard's coverage of newly added source inputs.
    # Build/cache outputs do not become source simply because they are present.
    ignored = {".git", "target", "build", "dist", "__pycache__", "node_modules", ".godot"}
    present = set()
    for directory, children, names in os.walk(root, followlinks=False):
        children[:] = [name for name in children if name not in ignored]
        for name in names + [name for name in children if (Path(directory) / name).is_symlink()]:
            path = Path(directory) / name
            relative = path.relative_to(root).as_posix()
            if relative != "PUBLIC_SOURCE_MANIFEST.json" and not name.endswith((".pyc", ".pyo")) and name != ".DS_Store":
                present.add(relative)
    modified = bool(present - known)
    for relative in sorted(known | present):
        key = PurePosixPath(relative)
        if key.is_absolute() or not key.parts or ".." in key.parts or key.as_posix() != relative:
            raise ValueError("unsafe exported source path")
        path = root / relative
        if not path.resolve().is_relative_to(root):
            raise ValueError("exported source path escapes source root")
        value.update(relative.encode() + b"\0")
        if relative in links or (relative not in files and path.is_symlink()):
            if not path.is_symlink():
                raise ValueError("exported source link is missing")
            target = path.readlink().as_posix()
            modified |= target != links.get(relative)
            value.update(b"link\0" + target.encode() + b"\0")
        else:
            if path.is_symlink() or not path.is_file():
                raise ValueError("exported source file is missing or is not regular")
            actual = hashlib.sha256(path.read_bytes()).hexdigest()
            mode = "100755" if path.stat().st_mode & 0o111 else "100644"
            modified |= actual != files.get(relative) or mode != modes.get(relative)
            value.update(mode.encode() + b"\0" + actual.encode() + b"\0")
    fingerprint = value.hexdigest()
    revision = head or "unversioned"
    if dirty is not False or head is None:
        label = "dirty" if dirty is True else "unverified"
        revision += "-" + label + "-" + inputs[:12]
    if modified:
        revision += "-export-modified-" + fingerprint[:12]
    build_revision = revision
    checkout = None
    if (root / '.git').exists():
        if (root / '.git').is_symlink():
            raise ValueError('build checkout metadata must not be a symlink')
        def git(*args):
            return subprocess.check_output(['git', '-C', str(root), *args],
                                           stderr=subprocess.DEVNULL, text=True).strip()
        try:
            if Path(git('rev-parse', '--show-toplevel')).resolve() != root:
                raise ValueError('build checkout root mismatch')
            checkout_head = git('rev-parse', 'HEAD')
            if not re.fullmatch('[0-9a-f]{40}', checkout_head):
                raise ValueError('build checkout commit is invalid')
            checkout_dirty = bool(git('status', '--porcelain', '--untracked-files=normal'))
        except subprocess.CalledProcessError as error:
            raise ValueError('cannot identify build checkout') from error
        checkout = {'git_head': checkout_head, 'git_dirty': checkout_dirty}
        build_revision = checkout_head
        if checkout_dirty or modified:
            build_revision += '-dirty-' + fingerprint[:12]
    return {"revision": revision, "source_provenance": provenance,
            "fingerprint": fingerprint, "export_modified": modified,
            "build_revision": build_revision, "checkout_provenance": checkout}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    output = parser.add_mutually_exclusive_group()
    output.add_argument("--revision", action="store_true")
    output.add_argument("--fingerprint", action="store_true")
    args = parser.parse_args()
    identity = source_identity(args.root)
    print(identity["build_revision"] if args.revision else identity["fingerprint"]
          if args.fingerprint else json.dumps(identity, sort_keys=True))


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Capture an actual disposable Ghostty/Godot build tree locally; never upload.
Retains source/resources, not build outputs or credentials. Not a complete
corresponding-source package: dependency caches and external inputs are separate.
"""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import subprocess
import tarfile

PINS = {'ghostty': '332b2aefc6e72d363aa93ab6ecfc86eeeeb5ed28',
        'godot': 'ed1daf0bf001b61586d9930840f2f1394092c079'}
EXCLUDED = {'.git', '.zig-cache', 'zig-out', '.cache', 'target', 'node_modules',
            'DerivedData', 'SourcePackages', '__pycache__',
            '.ssh', '.aws', '.gnupg', '.codex', '.agents'}
SECRETS = {'.env', '.netrc', '.npmrc', '.pypirc', '.git-credentials', 'credentials',
           'credentials.json', 'id_rsa', 'id_ed25519', 'auth.json'}
OUTPUT_SUFFIX = {'.o', '.a', '.dylib', '.so', '.dll', '.exe', '.pyc', '.pdb',
                 '.sconsign', '.dSYM', '.log', '.dmg', '.zip', '.tar', '.gz'}
GENERATED_SOURCE_SUFFIX = {'.h', '.hpp', '.c', '.cc', '.cpp', '.mm', '.m', '.zig',
                           '.py', '.sh', '.swift', '.json', '.zon', '.plist', '.xcconfig',
                           '.cmake', '.inc', '.txt'}


def git(root, *args):
    return subprocess.check_output(['git', '-C', str(root), *args])


def excluded(name):
    p = PurePosixPath(name)
    return (name in {'build', 'bin', 'macos/build'} or name.startswith(('build/', 'bin/', 'macos/build/'))
            or p.is_absolute() or '..' in p.parts or any(x in EXCLUDED for x in p.parts)
            or any(x.lower() in SECRETS or x.lower().startswith('.env.') for x in p.parts)
            or p.suffix in OUTPUT_SUFFIX or p.suffix.lower() in {'.p12', '.pfx', '.key', '.mobileprovision'}
            or any(re.search(r'(^|[-_.])gts($|[-_.])', x.lower()) for x in p.parts))


def selected(root):
    index = {}
    for entry in git(root, 'ls-files', '--stage', '-z').split(b'\0'):
        if not entry: continue
        meta, path = entry.split(b'\t', 1)
        mode, blob, stage = meta.decode().split()
        if stage != '0': raise ValueError('Unmerged index')
        name = path.decode()
        if mode == '160000': raise ValueError('Submodule requires separate source capture: '+name)
        if not excluded(name): index[name] = {'git_blob': blob, 'git_mode': mode}
    for name in git(root, 'ls-files', '--others', '--exclude-standard', '-z').decode().split('\0'):
        if name and not excluded(name): index[name] = {'generated_or_added': True}
    # Include overlay additions and generated sources/configuration, even ignored.
    # Never traverse build directories, package caches, credentials or symlinks.
    for base, dirs, files in os.walk(root, followlinks=False):
        relbase = Path(base).relative_to(root)
        dirs[:] = [d for d in dirs if not excluded((relbase/d).as_posix())
                   and not (Path(base)/d).is_symlink()]
        for file in files:
            name = (relbase/file).as_posix()
            if excluded(name) or name in index: continue
            if Path(name).suffix in GENERATED_SOURCE_SUFFIX or file in {'SConstruct', 'SCsub', 'Makefile', 'CMakeLists.txt'}:
                index[name] = {'generated_or_added': True}
    return index


def capture(root, output, component, source_commit, build_script, enforce_pin=True):
    root = root.resolve()
    if component not in PINS: raise ValueError('Only Ghostty and Godot clone capture is supported')
    head = git(root, 'rev-parse', 'HEAD').decode().strip()
    if enforce_pin and head != PINS[component]: raise ValueError('Unexpected upstream revision')
    if not re.fullmatch('[0-9a-f]{40}', source_commit): raise ValueError('Expected exact application commit')
    if output.exists(): raise ValueError('Refusing to overwrite source capture')
    if output.resolve().is_relative_to(root): raise ValueError('Output must be outside disposable clone')
    output.parent.mkdir(parents=True, exist_ok=True)
    records = []
    partial = output.with_name(output.name+'.partial')
    if partial.exists(): raise ValueError('Incomplete prior capture requires inspection')
    with tarfile.open(partial, 'x:gz') as archive:
        for name, record in sorted(selected(root).items()):
            path = root/name
            record = {'path': name, **record}
            if not path.exists() and not path.is_symlink():
                records.append({**record, 'deleted': True}); continue
            if path.is_symlink():
                target = os.readlink(path)
                if not path.resolve().is_relative_to(root): raise ValueError('Source symlink escapes clone: '+name)
                item = tarfile.TarInfo(component+'/'+name)
                item.type = tarfile.SYMTYPE; item.linkname = target; item.mode = 0o777
                archive.addfile(item)
                records.append({**record, 'symlink_target': target}); continue
            if not path.is_file(): raise ValueError('Source is not a regular file: '+name)
            data = path.read_bytes()
            if b'GNU Triangulated Surface' in data or b'gts.sourceforge.net' in data:
                raise ValueError('GTS-related content requires separate authorization: '+name)
            # No generated native executables disguised as source files.
            if data[:4] in (b'\x7fELF', b'\xcf\xfa\xed\xfe', b'\xfe\xed\xfa\xcf', b'\xca\xfe\xba\xbe'):
                raise ValueError('Native binary in selected source: '+name)
            item = tarfile.TarInfo(component+'/'+name)
            item.size = len(data); item.mode = 0o755 if path.stat().st_mode & 0o111 else 0o644
            archive.addfile(item, io.BytesIO(data))
            records.append({**record, 'sha256': hashlib.sha256(data).hexdigest(), 'bytes':len(data)})
        data = build_script.read_bytes()
        item = tarfile.TarInfo('build-inputs/'+build_script.name)
        item.size = len(data); item.mode = 0o755
        archive.addfile(item, io.BytesIO(data))
        manifest = {'schema_version': 1, 'component': component, 'upstream_commit': head,
                    'application_commit': source_commit, 'files':records,
                    'build_script_sha256':hashlib.sha256(data).hexdigest(),
                    'scope':'Actual modified source/resources and generated configuration before cleanup. No package caches, Homebrew/GTS sources, credentials or build outputs. External dependencies/custom modules still need their separately retained inputs.'}
        data = (json.dumps(manifest, indent=2)+'\n').encode()
        item = tarfile.TarInfo('capture-manifest.json'); item.size = len(data); item.mode = 0o644
        archive.addfile(item, io.BytesIO(data))
    partial.rename(output)
    return manifest


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tree', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--component', choices=PINS, required=True)
    parser.add_argument('--source-commit', required=True)
    parser.add_argument('--build-script', type=Path, required=True)
    args = parser.parse_args()
    capture(args.tree, args.output, args.component, args.source_commit, args.build_script)

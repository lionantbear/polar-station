#!/usr/bin/env python3
"""Record observed Mach-O hashes around the controlled final signing window.

These observations bind earlier copy receipts to final bytes. They do not prove
semantic equivalence, license clearance, static composition or release approval.
The receipt lives outside the signed bundle to avoid a circular hash dependency.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess

from macos_binary_inventory import hash_file, is_macho


def snapshot(app: Path) -> dict[str, str]:
    root = app.resolve(strict=True)
    if not (root / 'Contents/Info.plist').is_file():
        raise ValueError('Expected a complete app bundle')
    result = {}
    def fail(error):
        raise error
    for base, dirs, files in os.walk(root, followlinks=False, onerror=fail):
        for name in dirs + files:
            path = Path(base) / name
            if path.is_symlink() and not path.resolve(strict=True).is_relative_to(root):
                raise ValueError(f'Escaping app symlink: {path}')
        for name in files:
            path = Path(base) / name
            if path.is_symlink():
                continue
            # Open first so unreadable files cannot silently disappear.
            with path.open('rb') as stream:
                stream.read(4)
            if is_macho(path):
                result[path.relative_to(root).as_posix()] = hash_file(path)
    if not result:
        raise ValueError('No Mach-O files in signing snapshot')
    return dict(sorted(result.items()))


def finish_record(before: dict, after: dict[str, str], source_commit: str) -> dict:
    if before.get('schema_version') != 1 or before.get('source_commit') != source_commit:
        raise ValueError('Signing snapshot schema/source revision mismatch')
    original = before.get('components', {})
    if not original or set(original) != set(after):
        raise ValueError('Mach-O membership changed during signing window')
    return {'schema_version': 1, 'source_commit': source_commit,
            'scope': 'Observed before/after signing hashes, not semantic equivalence or license approval',
            'release_ready': False,
            'graphics_receipt_sha256': before['graphics_receipt_sha256'],
            'components': [{'bundle_path': path, 'pre_sign_sha256': original[path],
                            'final_sha256': after[path]} for path in sorted(after)]}


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('stage', choices=('before', 'after'))
    for name in ('app', 'source', 'before', 'output'):
        p.add_argument('--' + name, type=Path, required=name != 'output')
    args = p.parse_args()
    commit = subprocess.check_output(['git', '-C', str(args.source), 'rev-parse', 'HEAD'], text=True).strip()
    receipt = args.app / 'Contents/Resources/graphics/provenance.json'
    if args.stage == 'before':
        data = {'schema_version': 1, 'source_commit': commit,
                'graphics_receipt_sha256': hash_file(receipt), 'components': snapshot(args.app)}
        args.before.parent.mkdir(parents=True, exist_ok=True)
        args.before.write_text(json.dumps(data, indent=2) + '\n')
    else:
        if args.output is None:
            p.error('--output is required for after')
        before = json.loads(args.before.read_text())
        if before.get('graphics_receipt_sha256') != hash_file(receipt):
            raise ValueError('Graphics receipt changed during signing window')
        subprocess.run(['/usr/bin/codesign', '--verify', '--deep', '--strict', str(args.app)], check=True)
        data = finish_record(before, snapshot(args.app), commit)
        data['pre_sign_record_sha256'] = hash_file(args.before)
        data['final_signature_verification'] = 'passed'
        args.output.write_text(json.dumps(data, indent=2) + '\n')


if __name__ == '__main__':
    main()

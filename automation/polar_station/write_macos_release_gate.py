#!/usr/bin/env python3
"""Validate report integrity and write a deliberately blocked release gate.

This validation workflow has no release-approval mode. Valid evidence is not
approval: static inputs, notices/source obligations and GUI tests remain gates.
"""
import argparse
import base64
import binascii
import hashlib
import json
import re
from pathlib import Path
import sys


def sha(path):
    with path.open('rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()


def build_gate(root, source_commit, dmg=None):
    errors = []
    def read(name, required):
        try:
            path = root / name
            if not path.resolve().is_relative_to(root.resolve()):
                raise ValueError('path escapes evidence root')
            data = json.loads(path.read_text())
            if not isinstance(data, dict) or not required <= data.keys():
                raise ValueError('missing required fields')
            return data
        except (OSError, ValueError, TypeError) as exc:
            errors.append(f'{name}: {exc}')
            return {}
    bom = read('macos-bom.json', {'schema_version', 'components', 'status', 'blockers'})
    provenance = read('build-provenance/provenance.json', {'schema_version', 'source_commit', 'components', 'gaps', 'evidence_files', 'cargo_resolution_exit_code'})
    launch = read('installation-launch.json', {'schema_version', 'source_commit', 'installed_components', 'dmg_sha256', 'dmg_bytes', 'alive_after_15_seconds', 'exit_code_before_cleanup', 'temporary_copy_signature'})
    cargo = read('build-provenance/cargo-compiled-artifacts.json', {'schema_version', 'source_commit', 'artifacts'})
    if bom.get('schema_version') != 1 or not isinstance(bom.get('blockers'), list):
        errors.append('Inventory schema/blockers invalid')
    if bom.get('status') not in ('blocked', 'mach_o_inventory_complete_pending_manual_review'):
        errors.append('Inventory status invalid')
    if bool(bom.get('blockers')) != (bom.get('status') == 'blocked'):
        errors.append('Inventory status/blockers disagree')
    if provenance.get('schema_version') != 1 or provenance.get('source_commit') != source_commit:
        errors.append('Provenance schema/source commit mismatch')
    if not isinstance(provenance.get('gaps'), list):
        errors.append('Provenance gaps missing or malformed')
    elif provenance['gaps']:
        errors.append('Provenance collection has unresolved gaps')
    if provenance.get('cargo_resolution_exit_code') != 0:
        errors.append('Cargo metadata collection failed')
    if cargo.get('schema_version') != 1 or cargo.get('source_commit') != source_commit:
        errors.append('Cargo compiler evidence schema/source commit mismatch')
    try:
        if sha(root/'build-provenance/cargo-compiled-artifacts.json') != provenance.get('cargo_compiled_artifacts_sha256'):
            errors.append('Cargo compiler evidence hash mismatch')
    except OSError as exc:
        errors.append('Cargo compiler evidence hash unavailable: ' + str(exc))
    artifacts = cargo.get('artifacts')
    if not isinstance(artifacts, list) or not artifacts:
        errors.append('Actual Cargo compiler artifact evidence missing')
        artifacts = []
    build_records = set()
    def strings(value, nonempty=False):
        return isinstance(value, list) and (bool(value) or not nonempty) and all(isinstance(v, str) and v for v in value)
    for item in artifacts:
        if not isinstance(item, dict):
            errors.append('Malformed Cargo compiler artifact')
            continue
        target, profile = item.get('target'), item.get('profile')
        if (not isinstance(item.get('package_id'), str) or not item['package_id'] or
                not isinstance(target, dict) or
                not all(isinstance(target.get(k), str) and target[k] for k in ('name', 'src_path')) or
                not strings(target.get('kind'), True) or not strings(target.get('crate_types'), True) or
                not isinstance(profile, dict) or not isinstance(profile.get('opt_level'), str) or
                not all(type(profile.get(k)) is bool for k in ('debug_assertions', 'overflow_checks', 'test')) or
                'debuginfo' not in profile or
                not (profile['debuginfo'] is None or type(profile['debuginfo']) in (int, str)) or
                not strings(item.get('features')) or type(item.get('fresh')) is not bool or
                item.get('build_record') not in ('cargo-host.jsonl', 'cargo-graphics.jsonl')):
            errors.append('Malformed Cargo compiler artifact')
            continue
        build_records.add(item['build_record'])
    if build_records != {'cargo-host.jsonl', 'cargo-graphics.jsonl'}:
        errors.append('Cargo compiler evidence must include both Host and graphics build records')
    def components(data, label):
        result = {}
        for item in data.get('components', []) if isinstance(data.get('components'), list) else []:
            if not isinstance(item, dict) or not {'bundle_path', 'sha256'} <= item.keys():
                errors.append(label + ': malformed component')
                continue
            path, value = item['bundle_path'], item['sha256']
            if (not isinstance(path, str) or not path or
                    Path(path).is_absolute() or '..' in Path(path).parts or
                    path != Path(path).as_posix() or path == '.'):
                errors.append(label + ': unsafe or malformed component path')
                continue
            if path in result:
                errors.append(label + ': duplicate component path')
                continue
            if not isinstance(value, str) or re.fullmatch(r'[0-9a-fA-F]{64}', value) is None:
                errors.append(label + ': invalid component hash')
                continue
            result[path] = value.lower()
        if not result:
            errors.append(label + ': no components')
        return result
    inventory_components = components(bom, 'Inventory')
    if inventory_components != components(provenance, 'Provenance'):
        errors.append('Provenance does not match exact BOM component hashes')
    records = provenance.get('evidence_files', [])
    if not isinstance(records, list) or not records:
        errors.append('No retained evidence files')
        records = []
    for record in records:
        try:
            relative = Path(record['evidence_file'])
            path = root / 'build-provenance' / relative
            if relative.is_absolute() or '..' in relative.parts or not path.resolve().is_relative_to((root/'build-provenance').resolve()):
                raise ValueError('unsafe evidence path')
            if sha(path) != record['sha256']:
                raise ValueError('evidence hash mismatch')
            if record.get('storage_encoding') == 'base64_json':
                envelope = json.loads(path.read_text(encoding='utf-8'))
                if (not isinstance(envelope, dict) or envelope.get('schema_version') != 1 or
                        envelope.get('storage_encoding') != 'base64' or
                        envelope.get('text_encoding') != 'unverified' or
                        record.get('text_encoding') != 'unverified'):
                    raise ValueError('invalid encoded-text envelope')
                content = base64.b64decode(envelope['content_base64'], validate=True)
                checksum = hashlib.sha256(content).hexdigest()
                if (envelope.get('source_sha256') != checksum or record.get('source_sha256') != checksum or
                        type(envelope.get('source_bytes')) is not int or
                        type(record.get('source_bytes')) is not int or
                        envelope['source_bytes'] != len(content) or record['source_bytes'] != len(content)):
                    raise ValueError('encoded-text source hash/size mismatch')
            elif record.get('storage_encoding') not in (None, 'utf-8'):
                raise ValueError('unsupported retained-text encoding')
        except (OSError, ValueError, KeyError, TypeError, binascii.Error) as exc:
            errors.append('Retained text: ' + str(exc))
    if launch.get('alive_after_15_seconds') is not True or launch.get('exit_code_before_cleanup') is not None or launch.get('temporary_copy_signature') != 'passed':
        errors.append('Temporary installation/15-second launch did not pass')
    if launch.get('schema_version') != 1 or launch.get('source_commit') != source_commit:
        errors.append('Launch evidence schema/source commit mismatch')
    if components({'components': launch.get('installed_components')}, 'Installed app') != inventory_components:
        errors.append('Installed app does not match exact BOM component hashes')
    artifact = None
    if dmg is not None:
        try:
            artifact = {'name': dmg.name, 'sha256': sha(dmg), 'bytes': dmg.stat().st_size}
            # This binds observations only; hdiutil in native CI validates the image.
            if (launch.get('dmg_sha256') != artifact['sha256'] or
                    type(launch.get('dmg_bytes')) is not int or launch['dmg_bytes'] != artifact['bytes']):
                errors.append('Launch evidence does not match supplied DMG hash/size')
        except OSError as exc:
            errors.append('DMG hash unavailable: ' + str(exc))
    if dmg is None:
        errors.append('DMG required to bind installation evidence')
    return {'schema_version': 1, 'source_commit': source_commit,
            'release_ready': False, 'binary_upload_authorized_by_this_workflow': False,
            'evidence_integrity': 'invalid_or_incomplete' if errors else 'validated',
            'evidence_errors': errors, 'inventory_status': bom.get('status', 'missing'),
            'inventory_blockers': bom.get('blockers', ['Inventory missing']),
            'provenance_gaps': provenance.get('gaps', ['Provenance missing']),
            'dmg': artifact,
            'remaining': ['Reviewed artifact-specific licenses and installed notices',
                          'Static-link inputs, exact source/relink obligations and resources',
                          'Interactive clean-Mac terminal/graphics/TCC/upgrade/uninstall validation']}


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--evidence', required=True, type=Path)
    p.add_argument('--source-commit', required=True)
    p.add_argument('--dmg', type=Path)
    a = p.parse_args()
    a.evidence.mkdir(parents=True, exist_ok=True)
    gate = build_gate(a.evidence, a.source_commit, a.dmg)
    (a.evidence/'release-gate.json').write_text(json.dumps(gate, indent=2) + '\n')
    print(json.dumps(gate, indent=2))
    return 1 if gate['evidence_errors'] else 0


if __name__ == '__main__':
    sys.exit(main())

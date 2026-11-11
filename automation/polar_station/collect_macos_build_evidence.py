#!/usr/bin/env python3
"""Collect text-only provenance; candidate associations never grant release clearance."""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import subprocess
from macos_binary_inventory import hash_file, is_macho

LEGAL = ('license', 'licence', 'copying', 'copyright', 'notice', 'authors')

def digest(data):
    return hashlib.sha256(data).hexdigest()

def run(argv, cwd=None):
    try:
        p = subprocess.run(argv, cwd=cwd, capture_output=True, text=True, timeout=600)
        return {'exit_code': p.returncode, 'stdout': p.stdout, 'stderr': p.stderr[-4000:]}
    except (OSError, subprocess.TimeoutExpired) as exc:
        return {'exit_code': 1, 'stdout': '', 'stderr': str(exc)}

def retain_text(path, origin, texts, records, gaps):
    """Preserve legal/build text bytes without guessing an unknown encoding."""
    if not path.is_file() or path.is_symlink():
        return None
    data = path.read_bytes()
    if len(data) > 2000000 or any(b < 32 and b not in (9, 10, 12, 13) for b in data) or is_macho(path):
        gaps.append('Skipped non-text or oversized evidence: ' + origin)
        return None
    original_sha = digest(data)
    extra = {}
    try:
        data.decode('utf-8')
    except UnicodeDecodeError:
        # Legal files such as GTS AUTHORS predate UTF-8. Keep exact bytes in
        # a text-only envelope; do not silently recode or assert an encoding.
        envelope = {'schema_version': 1, 'storage_encoding': 'base64',
                    'text_encoding': 'unverified', 'source_sha256': original_sha,
                    'source_bytes': len(data),
                    'content_base64': base64.b64encode(data).decode('ascii')}
        data = (json.dumps(envelope, indent=2) + '\n').encode('utf-8')
        extra = {'storage_encoding': 'base64_json', 'source_sha256': original_sha,
                 'source_bytes': envelope['source_bytes'], 'text_encoding': 'unverified'}
    sha = digest(data)
    name = sha + '.txt'
    (texts / name).write_bytes(data)
    entry = {'origin': origin, 'sha256': sha, 'evidence_file': 'texts/' + name, **extra}
    records.append(entry)
    return entry

def verify_signing_observations(signing, before, bom, source_commit, graphics_sha, before_sha):
    """Require complete unique before/final observations for this source and BOM."""
    if (signing.get('schema_version') != 1 or before.get('schema_version') != 1 or
            signing.get('source_commit') != source_commit or before.get('source_commit') != source_commit or
            signing.get('final_signature_verification') != 'passed' or
            signing.get('graphics_receipt_sha256') != graphics_sha or
            before.get('graphics_receipt_sha256') != graphics_sha or
            signing.get('pre_sign_record_sha256') != before_sha):
        return False
    try:
        rows = signing['components']
        final = {s['bundle_path']: s['final_sha256'] for s in rows}
        initial = {s['bundle_path']: s['pre_sign_sha256'] for s in rows}
        expected = {s['bundle_path']: s['sha256'] for s in bom['components']}
        return bool(rows) and len(final) == len(rows) and final == expected and initial == before['components']
    except (KeyError, TypeError):
        return False

def match_copy_evidence(component, transformations, originals, signing):
    """Validate exact observed byte associations, never basename-based approval."""
    path = component['bundle_path']
    copies = [t for t in transformations if 'Contents/Resources/graphics/' + t['bundle_relative_path'] == path]
    errors = []
    if len(copies) != 1:
        return [], ['Expected exactly one copy transformation: ' + path]
    t = dict(copies[0])
    matched = [o for o in originals if o['source_path'] == t['source_path']
               and o['original_sha256'] == t['source_sha256']]
    if len(matched) != 1:
        errors.append('Original source path/hash not verified uniquely: ' + path)
    t['verified_originals'] = matched
    observations = [s for s in signing.get('components', []) if s['bundle_path'] == path]
    direct = t['packaged_sha256'] == component['sha256']
    observed = (len(observations) == 1 and
                observations[0]['pre_sign_sha256'] == t['packaged_sha256'] and
                observations[0]['final_sha256'] == component['sha256'])
    t['matches_final_bundle_hash'] = direct
    t['matches_signing_observation'] = observed
    if not direct and not observed:
        errors.append('No exact copy-to-final-signing hash association: ' + path)
    t['approved_attribution'] = False
    return [t], errors

def collect(source, native, bom, output, cellar=Path("/opt/homebrew/Cellar")):
    output.mkdir(parents=True, exist_ok=False)
    texts = output / 'texts'
    texts.mkdir()
    records, gaps = [], []
    source_commit = run(['git', 'rev-parse', 'HEAD'], source)['stdout'].strip()
    graphics_receipt = source/'target/polar-station-macos/Release/Polar Station.app/Contents/Resources/graphics/provenance.json'
    graphics = json.loads(graphics_receipt.read_text()) if graphics_receipt.exists() else {}
    transformations = graphics.get('graphviz_transformations', [])
    if not isinstance(transformations, list) or any(not isinstance(t, dict) or not
            {'source_path', 'source_sha256', 'bundle_relative_path', 'packaged_sha256'} <= t.keys() or
            not all(isinstance(t[k], str) for k in ('source_path', 'source_sha256', 'bundle_relative_path', 'packaged_sha256'))
            for t in transformations):
        gaps.append('Graphics transformations malformed')
        transformations = []
    # Only copied formulae belong to this artifact's Homebrew provenance.
    needed_formulae = set()
    original_paths = set()
    for t in transformations:
        path = Path(t['source_path'])
        original_paths.add(path)
        parts = path.relative_to(cellar).parts if path.is_relative_to(cellar) else ()
        if len(parts) >= 3 and '..' not in parts:
            needed_formulae.add(parts[0])
        else:
            gaps.append('Copied source is outside recognized Homebrew Cellar: ' + str(path))
    def retain(path, origin):
        return retain_text(path, origin, texts, records, gaps)
    def legal_tree(root, label):
        found = []
        if not root.is_dir():
            gaps.append('Missing source tree: ' + label)
            return found
        for base, dirs, files in os.walk(root, followlinks=False):
            dirs[:] = [d for d in dirs if d not in ('.git', 'target', '.zig-cache', 'zig-out', 'node_modules') and not (Path(base)/d).is_symlink()]
            for name in files:
                if name.lower().startswith(LEGAL):
                    e = retain(Path(base)/name, label + '/' + str((Path(base)/name).relative_to(root)))
                    if e: found.append(e)
        return found
    brew = run(['brew', 'info', '--json=v2', '--installed'])
    formulae = json.loads(brew['stdout']).get('formulae', []) if brew['exit_code'] == 0 else []
    if not formulae: gaps.append('Homebrew installed metadata unavailable')
    packages = []
    original_libraries = []
    for formula in formulae:
        name = formula['name']
        if name not in needed_formulae:
            continue
        versions = formula.get('installed', [])
        package = {'name': name, 'declared_license': formula.get('license'), 'stable_source': formula.get('urls', {}).get('stable'), 'installed': versions, 'dependencies': formula.get('dependencies'), 'evidence': []}
        for version in versions:
            root = cellar / name / version['version']
            e = retain(root/'INSTALL_RECEIPT.json', 'Homebrew/'+name+'/'+version['version']+'/INSTALL_RECEIPT.json')
            if e: package['evidence'].append(e)
            else: gaps.append('Homebrew install receipt missing: '+name+'/'+version['version'])
            package['evidence'] += legal_tree(root, 'Homebrew/'+name+'/'+version['version'])
            for base, dirs, files in os.walk(root, followlinks=False):
                dirs[:] = [d for d in dirs if not (Path(base)/d).is_symlink()]
                for filename in files:
                    path = Path(base)/filename
                    if path in original_paths and path.is_file() and path.resolve().is_relative_to(root.resolve()) and is_macho(path):
                        original_libraries.append({'formula':name,'version':version['version'],'source_path':str(path),'relative_path':str(path.relative_to(root)),'basename':filename,'original_sha256':hash_file(path)})
        packages.append(package)
    for name in sorted(needed_formulae - {p['name'] for p in packages}):
        gaps.append('Copied Homebrew formula metadata missing: ' + name)
    # Reviewed archive hashes and actual source headers are research evidence,
    # not a formula-wide license override or automatic binary clearance.
    research_file = source/'licenses/native-source-evidence.json'
    research = json.loads(research_file.read_text()) if research_file.exists() else {}
    research_entries = []
    if research.get('schema_version') != 1:
        gaps.append('Pinned native source research unavailable')
    else:
        retain(research_file, 'Polar/licenses/native-source-evidence.json')
        for entry in research.get('components', []):
            valid = True
            for evidence in entry['source_texts']:
                path = source/evidence['path']
                if (Path(evidence['path']).is_absolute() or '..' in Path(evidence['path']).parts or
                        not path.resolve().is_relative_to(source.resolve()) or not path.is_file() or
                        hash_file(path) != evidence['sha256']):
                    gaps.append('Pinned native source evidence invalid: '+evidence['path'])
                    valid = False
                else:
                    retain(path, 'Polar/'+evidence['path'])
            if valid: research_entries.append(entry)
    upstream = {}
    for name in ('ghostty', 'godot'):
        root = native/name
        upstream[name] = {'revision': run(['git', 'rev-parse', 'HEAD'], root), 'legal_texts': legal_tree(root, name)}
        for pattern in ('build.zig.zon', 'macos/**/Package.resolved'):
            for path in root.glob(pattern): retain(path, name+'/'+str(path.relative_to(root)))
    # Xcode dependency checkouts contain Sparkle's exact revision and original notices.
    sparkle = []
    derived = Path.home()/'Library/Developer/Xcode/DerivedData'
    if derived.exists():
        for root in derived.glob('*/SourcePackages/checkouts/*'):
            if root.is_symlink() or not root.is_dir(): continue
            sparkle.append({'name': root.name, 'revision': run(['git', 'rev-parse', 'HEAD'], root), 'legal_texts': legal_tree(root, 'Xcode/'+root.name)})
    if not any('sparkle' in p['name'].lower() for p in sparkle): gaps.append('Sparkle source checkout evidence not located')
    # Locked metadata may download missing declared registry/Git sources, never executes build scripts.
    # Resolution is not proof of static linkage. Compiler artifact records scope the actual build.
    cargo = run(['cargo', 'metadata', '--locked', '--format-version=1', '--filter-platform', 'aarch64-apple-darwin'], source)
    cargo_packages = []
    if cargo['exit_code'] == 0:
        data = json.loads(cargo['stdout'])
        for p in data['packages']:
            root = Path(p['manifest_path']).parent
            evidence = []
            for file in root.iterdir():
                if file.name.lower().startswith(LEGAL) or file.name in (p.get('license_file'),):
                    e = retain(file, 'Cargo/'+p['name']+'/'+p['version']+'/'+file.name)
                    if e: evidence.append(e)
            if p.get('license_file'):
                declared = (root/p['license_file']).resolve()
                if not declared.is_relative_to(root.resolve()):
                    gaps.append('Cargo declared license outside package root requires review: '+p['id'])
                elif not any(e['origin'] == 'Cargo/'+p['name']+'/'+p['version']+'/'+str(declared.relative_to(root.resolve())) for e in evidence):
                    e = retain(declared, 'Cargo/'+p['name']+'/'+p['version']+'/'+str(declared.relative_to(root.resolve())))
                    if e: evidence.append(e)
                    else: gaps.append('Cargo declared license file unavailable: '+p['id'])
            cargo_packages.append({k:p.get(k) for k in ('id','name','version','source','license','license_file')} | {'legal_texts':evidence})
        (output/'cargo-resolution.json').write_text(json.dumps({'scope':'Default-feature target resolution, not link-time composition', 'packages':cargo_packages,'resolve':data['resolve']}, indent=2)+'\n')
    else: gaps.append('Locked Cargo resolution failed: '+cargo['stderr'])
    for file in ('Cargo.lock', 'THIRD_PARTY_NOTICES.md', 'LICENSE'):
        retain(source/file, 'Polar/'+file)
    artifacts = []
    for path in (source/'target/build-evidence').glob('cargo-*.jsonl'):
        for line in path.read_text().splitlines():
            try: message = json.loads(line)
            except json.JSONDecodeError: continue
            if message.get('reason') == 'compiler-artifact':
                artifacts.append({k:message.get(k) for k in ('package_id','target','profile','features','fresh')} | {'build_record':path.name})
    if not artifacts: gaps.append('No actual Cargo compiler artifact messages collected')
    (output/'cargo-compiled-artifacts.json').write_text(json.dumps({'schema_version':1,'source_commit':source_commit,'scope':'Compiler artifacts, not proof every compiled crate was linked', 'artifacts':artifacts}, indent=2)+'\n')
    if not graphics: gaps.append('Graphics copy/transformation receipt missing')
    else: retain(graphics_receipt, 'Built app/graphics/provenance.json')
    signing_file = source/'target/build-evidence/macos-signing.json'
    signing = json.loads(signing_file.read_text()) if signing_file.exists() else {}
    before_file = source/'target/build-evidence/macos-pre-sign.json'
    before = json.loads(before_file.read_text()) if before_file.exists() else {}
    signing_valid = (before_file.is_file() and graphics_receipt.is_file() and
                     verify_signing_observations(signing, before, bom, source_commit,
                                                 hash_file(graphics_receipt), hash_file(before_file)))
    if not signing_valid:
        gaps.append('Final signing observations missing or not bound to current source/graphics receipt')
        signing = {}
    else:
        retain(signing_file, 'Build/macos-signing.json')
        retain(before_file, 'Build/macos-pre-sign.json')
    if graphics.get('source_commit') != source_commit:
        gaps.append('Graphics receipt source revision does not match current checkout')
    components = []
    for c in bom['components']:
        path = c['bundle_path']
        candidates = []
        if 'Sparkle.framework/' in path:
            candidates = ['Xcode/'+p['name'] for p in sparkle if 'sparkle' in p['name'].lower()]
        elif '/graphviz/' in path:
            # A filename association alone is not evidence of the exact upstream build.
            candidates = ['Homebrew: inspect installed receipts and original library before relocation/signing']
        elif path.endswith('/libgodot.dylib'): candidates = ['godot']
        else: candidates = ['Polar', 'Cargo resolved graph; actual static linkage requires further review']
        copies = []
        if '/graphviz/' in path:
            copies, errors = match_copy_evidence(c, transformations, original_libraries, signing)
            gaps.extend(errors)
        for t in copies:
            if len(t['verified_originals']) == 1:
                t['formula'] = t['verified_originals'][0]['formula']
                t['formula_version'] = t['verified_originals'][0]['version']
                t['license_evidence'] = [p for p in packages if p['name'] == t['formula'] and any(v['version'] == t['formula_version'] for v in p['installed'])]
                t['source_research'] = [r for r in research_entries if r['formula'] == t['formula'] and
                    r['version'] == t['formula_version'] and Path(path).name in r['library_basenames'] and
                    any((p.get('stable_source') or {}).get('checksum') == r['source_archive_sha256'] for p in t['license_evidence'])]
        components.append({'copy_transformation_evidence':copies,'comparison_note':'Exact source/copy/signing observations where present; not proof of semantic equivalence or license clearance', 'bundle_path':path,'sha256':c['sha256'],'candidate_evidence_groups':candidates,'approved_attribution':False})
    report = {'schema_version':1,'source_commit':source_commit,'release_ready':False,'scope':'Text-only build provenance and hash-bound candidate associations. Not approved license mapping, not static link composition proof.', 'components':components,'homebrew':packages,'upstream':upstream,'xcode_packages':sparkle,'cargo_resolution_exit_code':cargo['exit_code'],'cargo_compiled_artifacts_sha256':hash_file(output/'cargo-compiled-artifacts.json'),'evidence_files':records,'gaps':gaps,'remaining':['Verify exact pre-relocation Homebrew binary provenance and source obligations','Inventory static link inputs and all resources; do not infer binary inclusion from Cargo metadata','Install reviewed license notices in app before signing and run approved attribution audit']}
    (output/'provenance.json').write_text(json.dumps(report, indent=2)+'\n')
    print(json.dumps({'components':len(components),'text_records':len(records),'cargo_packages':len(cargo_packages),'gaps':gaps,'release_ready':False}))

if __name__ == '__main__':
    p=argparse.ArgumentParser(description=__doc__)
    for name in ('source','native','bom','output'): p.add_argument('--'+name, type=Path, required=True)
    a=p.parse_args()
    try:
        collect(a.source.resolve(), a.native.resolve(), json.loads(a.bom.read_text()), a.output)
    except (OSError, ValueError, TypeError, KeyError, AttributeError) as exc:
        # Preserve the exact collection failure even when partial text evidence
        # already exists; the workflow's gate treats this as incomplete.
        a.output.mkdir(parents=True, exist_ok=True)
        (a.output/'collection-error.json').write_text(json.dumps({'release_ready':False,'error':str(exc)}, indent=2)+'\n')
        raise

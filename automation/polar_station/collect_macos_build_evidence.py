#!/usr/bin/env python3
"""Collect text-only provenance; candidate associations never grant release clearance."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess

LEGAL = ('license', 'licence', 'copying', 'copyright', 'notice', 'authors')

def digest(data):
    return hashlib.sha256(data).hexdigest()

def run(argv, cwd=None):
    p = subprocess.run(argv, cwd=cwd, capture_output=True, text=True, timeout=600)
    return {'exit_code': p.returncode, 'stdout': p.stdout, 'stderr': p.stderr[-4000:]}

def collect(source, native, bom, output):
    output.mkdir(parents=True, exist_ok=False)
    texts = output / 'texts'
    texts.mkdir()
    records, gaps = [], []
    def retain(path, origin):
        if not path.is_file() or path.is_symlink():
            return None
        data = path.read_bytes()
        if len(data) > 2000000 or b'\0' in data:
            gaps.append('Skipped non-text or oversized evidence: ' + origin)
            return None
        try: data.decode('utf-8')
        except UnicodeDecodeError:
            gaps.append('Skipped non-UTF8 evidence: ' + origin)
            return None
        sha = digest(data)
        name = sha + '.txt'
        (texts / name).write_bytes(data)
        entry = {'origin': origin, 'sha256': sha, 'evidence_file': 'texts/' + name}
        records.append(entry)
        return entry
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
    bundled_names = {Path(c['bundle_path']).name for c in bom['components']}
    for formula in formulae:
        name = formula['name']
        versions = formula.get('installed', [])
        package = {'name': name, 'declared_license': formula.get('license'), 'stable_source': formula.get('urls', {}).get('stable'), 'installed': versions, 'dependencies': formula.get('dependencies'), 'evidence': []}
        for version in versions:
            root = Path('/opt/homebrew/Cellar') / name / version['version']
            e = retain(root/'INSTALL_RECEIPT.json', 'Homebrew/'+name+'/'+version['version']+'/INSTALL_RECEIPT.json')
            if e: package['evidence'].append(e)
            package['evidence'] += legal_tree(root, 'Homebrew/'+name+'/'+version['version'])
            for base, dirs, files in os.walk(root, followlinks=False):
                dirs[:] = [d for d in dirs if not (Path(base)/d).is_symlink()]
                for filename in files:
                    path = Path(base)/filename
                    if filename in bundled_names and path.is_file() and path.resolve().is_relative_to(root.resolve()):
                        original_libraries.append({'formula':name,'version':version['version'],'relative_path':str(path.relative_to(root)),'basename':filename,'original_sha256':digest(path.read_bytes())})
        packages.append(package)
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
    original_libraries = []
    bundled_names = {Path(c['bundle_path']).name for c in bom['components']}
    if cargo['exit_code'] == 0:
        data = json.loads(cargo['stdout'])
        for p in data['packages']:
            root = Path(p['manifest_path']).parent
            evidence = []
            for file in root.iterdir():
                if file.name.lower().startswith(LEGAL) or file.name in (p.get('license_file'),):
                    e = retain(file, 'Cargo/'+p['name']+'/'+p['version']+'/'+file.name)
                    if e: evidence.append(e)
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
    (output/'cargo-compiled-artifacts.json').write_text(json.dumps({'scope':'Compiler artifacts, not proof every compiled crate was linked', 'artifacts':artifacts}, indent=2)+'\n')
    graphics_receipt = source/'target/polar-station-macos/Release/Polar Station.app/Contents/Resources/graphics/provenance.json'
    graphics = json.loads(graphics_receipt.read_text()) if graphics_receipt.exists() else {}
    if not graphics: gaps.append('Graphics copy/transformation receipt missing')
    else: retain(graphics_receipt, 'Built app/graphics/provenance.json')
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
        transformations = [t for t in graphics.get('graphviz_transformations',[]) if 'Contents/Resources/graphics/'+t['bundle_relative_path'] == path]
        for t in transformations:
            parts = Path(t['source_path']).parts
            if 'Cellar' in parts:
                pos = parts.index('Cellar')
                if len(parts) > pos+2:
                    t['formula'] = parts[pos+1]
                    t['formula_version'] = parts[pos+2]
                    t['license_evidence'] = [p for p in packages if p['name'] == t['formula']]
            t['matches_final_bundle_hash'] = t['packaged_sha256'] == c['sha256']
            if not t['matches_final_bundle_hash']: gaps.append('Post-packaging bytes changed; requires signature-stage binding: '+path)
        components.append({'copy_transformation_evidence':transformations,'pre_relocation_candidates':[p for p in original_libraries if p['basename'] == Path(path).name], 'comparison_note':'Candidate basename match only; install_name_tool and codesign can change bytes; not proven equivalence', 'bundle_path':path,'sha256':c['sha256'],'candidate_evidence_groups':candidates,'approved_attribution':False})
    report = {'schema_version':1,'release_ready':False,'scope':'Text-only build provenance and hash-bound candidate associations. Not approved license mapping, not static link composition proof.', 'components':components,'homebrew':packages,'upstream':upstream,'xcode_packages':sparkle,'cargo_resolution_exit_code':cargo['exit_code'],'evidence_files':records,'gaps':gaps,'remaining':['Verify exact pre-relocation Homebrew binary provenance and source obligations','Inventory static link inputs and all resources; do not infer binary inclusion from Cargo metadata','Install reviewed license notices in app before signing and run approved attribution audit']}
    (output/'provenance.json').write_text(json.dumps(report, indent=2)+'\n')
    print(json.dumps({'components':len(components),'text_records':len(records),'cargo_packages':len(cargo_packages),'gaps':gaps,'release_ready':False}))

if __name__ == '__main__':
    p=argparse.ArgumentParser(description=__doc__)
    for name in ('source','native','bom','output'): p.add_argument('--'+name, type=Path, required=True)
    a=p.parse_args()
    collect(a.source.resolve(), a.native.resolve(), json.loads(a.bom.read_text()), a.output)

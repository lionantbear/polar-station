#!/usr/bin/env python3
"""Collect exact resolved source license evidence; not binary/legal clearance.

Read-only to dependency caches. Output contains no machine-local source paths.
"""
import argparse, hashlib, json, pathlib, re

def sha(b): return hashlib.sha256(b).hexdigest()
# License-like filenames can also name implementation or test programs.
# Keep legal-text suffixes and license-family variants (LICENSE-2.0,
# NOTICE.httprouter, COPYING.info-zip), but never auto-collect source code.
_CODE_SUFFIXES = {'.rs', '.star', '.pm', '.py', '.pyi', '.pyc', '.sh', '.bash',
                  '.c', '.h', '.cc', '.cpp', '.hpp', '.m', '.mm', '.swift',
                  '.zig', '.js', '.mjs', '.cjs', '.ts', '.tsx', '.jsx',
                  '.go', '.rb', '.pl', '.lua', '.java', '.kt', '.cs',
                  '.bzl', '.cmake', '.toml', '.json', '.yaml', '.yml'}
def legal(p):
    if p.suffix.lower() in _CODE_SUFFIXES:
        return False
    return re.match(r'^(licen[cs]e|copying|copyright|notice)([._-].*|$)', p.name, re.I) is not None

def collect(metadata, output):
    output.mkdir(parents=True, exist_ok=True)
    texts=output/'cargo-texts'; texts.mkdir(exist_ok=True)
    previous_file=output/'cargo-evidence.json'
    previous={}
    if previous_file.exists():
        previous={(p['name'],p['version'],p['source']):p for p in json.loads(previous_file.read_text()).get('packages',[])}
    records=[]
    for p in metadata['packages']:
        source=p.get('source')
        if not source: continue
        root=pathlib.Path(p['manifest_path']).parent.resolve()
        candidates=[]
        # Include nested native/vendor legal notices, not just crate root license.
        for f in root.rglob('*'):
            if f.is_file() and not f.is_symlink() and legal(f): candidates.append((f, str(f.relative_to(root))))
        if source.startswith('git+'):
            for parent in [root, *root.parents]:
                if parent.name == 'checkouts': break
                for f in parent.iterdir():
                    if f.is_file() and not f.is_symlink() and legal(f): candidates.append((f, 'ancestor/'+f.name))
                if (parent/'.git').exists(): break
        lf=p.get('license_file')
        if lf:
            f=pathlib.Path(lf)
            if not f.is_absolute():f=root/f
            if f.is_file(): candidates.append((f, 'declared-license-file/'+f.name))
        evidence=[];seen=set()
        for f,rel in sorted(candidates, key=lambda t:str(t[0])):
            data=f.read_bytes(); digest=sha(data)
            if digest in seen:continue
            seen.add(digest)
            path=texts/(digest+'.txt')
            if not path.exists():path.write_bytes(data)
            evidence.append(dict(upstream_path=rel,sha256=digest,path='licenses/cargo-texts/'+path.name,bytes=len(data)))
        if not evidence:
            prior=previous.get((p['name'],p['version'],source), {})
            # Keep previously fetched exact VCS texts only while their hashes
            # remain valid; do not silently discard supplemental evidence.
            for item in prior.get('legal_texts', []):
                f=output.parent/item['path']
                if (legal(pathlib.Path(item.get('upstream_path', '')))
                        and not pathlib.Path(item['path']).is_absolute() and f.resolve().is_relative_to(output.resolve())
                        and f.is_file() and sha(f.read_bytes())==item.get('sha256')):
                    evidence.append(item)
        checksum=root/'.cargo-checksum.json'
        unchanged=None
        if checksum.is_file():
            cs=json.loads(checksum.read_text()); checks=cs.get('files',{})
            # Verify every retained license against Cargo's source-file checksums.
            direct=[(f,rel) for f,rel in candidates if f.is_relative_to(root)]
            unchanged=all(checks.get(str(f.relative_to(root)))==sha(f.read_bytes()) for f,_ in direct)
        url=(f"https://static.crates.io/crates/{p['name']}/{p['name']}-{p['version']}.crate" if source.startswith('registry+') else source.removeprefix('git+').split('?')[0].split('#')[0]+'/tree/'+source.rsplit('#',1)[-1])
        records.append(dict(name=p['name'],version=p['version'],source=source,license=p.get('license'),source_url=url,legal_texts=evidence,cargo_license_files_unchanged=unchanged))
    result=dict(schema_version=1,scope='Resolved Cargo source evidence only; no binary clearance',packages=records,counts=dict(packages=len(records),packages_with_texts=sum(bool(r['legal_texts']) for r in records),packages_without_texts=sum(not r['legal_texts'] for r in records)))
    (output/'cargo-evidence.json').write_text(json.dumps(result,indent=2)+'\n')
    return result
if __name__=='__main__':
    a=argparse.ArgumentParser(description=__doc__);a.add_argument('--metadata',type=pathlib.Path,required=True);a.add_argument('--output',type=pathlib.Path,required=True);args=a.parse_args()
    print(json.dumps(collect(json.loads(args.metadata.read_text()),args.output)['counts'],indent=2))

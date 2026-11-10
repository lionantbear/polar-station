#!/usr/bin/env python3
"""Local-only bounded trial source capture. No downloads, uploads or clearance.
Archive names and exact hashes are enumerated in trial-inputs-manifest.json.
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

LIMIT = 1500 * 1024 * 1024  # raw bytes per part; gzip overhead stays well below 2 GB
SKIP_DIRS = {'.git', '.hg', '.svn', 'target', 'node_modules', '.zig-cache', 'zig-out',
             '.cache', '__pycache__', '.ssh', '.aws', '.gnupg', '.codex', '.agents'}
SECRET_NAMES = {'.env', '.netrc', '.npmrc', '.pypirc', '.git-credentials',
                'credentials', 'credentials.json', 'credentials.toml', 'auth.json', 'id_rsa', 'id_ed25519'}
BAD_SUFFIXES = {'.p12', '.pfx', '.key', '.mobileprovision', '.o', '.a', '.lib', '.pyc', '.dylib', '.so', '.dll', '.exe', '.dmg'}
GTS_MARKERS = (b'GNU Triangulated Surface', b'gts.sourceforge.net')


def digest(path):
    h = hashlib.sha256()
    with path.open('rb') as src:
        for data in iter(lambda: src.read(1024*1024), b''): h.update(data)
    return h.hexdigest()


def run(argv, cwd=None):
    result = subprocess.run(argv, cwd=cwd, capture_output=True, text=True)
    if result.returncode:
        raise ValueError('Command failed: '+argv[0]+' '+argv[1]+' (exit '+str(result.returncode)+')')
    return result.stdout


def blocked(name, allow_gts_metadata=False):
    p = PurePosixPath(name)
    if p.is_absolute() or '..' in p.parts: return True
    if any(x in SKIP_DIRS or x.lower() in SECRET_NAMES or x.lower().startswith('.env.') for x in p.parts): return True
    if p.suffix.lower() in BAD_SUFFIXES: return True
    return not allow_gts_metadata and any(re.search(r'(^|[-_.])gts($|[-_.])', x.lower()) for x in p.parts)


class Parts:
    def __init__(self, output, label, gaps, limit=LIMIT):
        self.output, self.label, self.gaps, self.limit = output, label, gaps, limit
        self.part = None; self.raw = 0; self.number = 0; self.archives = []; self.files = []; self.names = set()

    def close(self):
        if self.part is None: return
        self.part.close()
        final = self.partial.with_suffix('')
        self.partial.rename(final)
        if final.stat().st_size >= 1900*1024*1024:
            raise ValueError('Compressed archive exceeds conservative upload bound: '+final.name)
        self.archives.append({'path':final.name, 'sha256':digest(final), 'bytes':final.stat().st_size})
        self.part = None

    def begin(self, size):
        if self.part is not None and self.raw+size > self.limit: self.close()
        if self.part is None:
            self.number += 1
            self.partial = self.output/(self.label+'-'+str(self.number).zfill(3)+'.tar.gz.partial')
            if self.partial.exists() or self.partial.with_suffix('').exists(): raise ValueError('Output already exists')
            self.part = tarfile.open(self.partial,'x:gz'); self.raw=0

    def add(self, root, relative, prefix, allow_gts_metadata=False):
        if blocked(relative, allow_gts_metadata):
            self.gaps.append({'scope':self.label,'path':prefix+'/'+relative,'reason':'excluded path; not captured'})
            return
        path = root/relative; name = prefix+'/'+relative
        if name in self.names: raise ValueError('Duplicate archive entry: '+name)
        self.names.add(name)
        # Do not follow symlinked parents, even when file was enumerated from Git.
        if any(p.is_symlink() for p in path.parents if p != root and p.is_relative_to(root)):
            self.gaps.append({'path':name,'reason':'symlinked parent excluded'}); return
        if path.is_symlink():
            if not path.resolve().is_relative_to(root.resolve()):
                self.gaps.append({'path':name,'reason':'escaping symlink excluded'}); return
            self.begin(0); info=tarfile.TarInfo(name);info.type=tarfile.SYMTYPE
            info.linkname=os.readlink(path);info.mode=0o777;self.part.addfile(info)
            self.files.append({'path':name,'symlink_target':info.linkname});return
        if not path.is_file():
            self.gaps.append({'path':name,'reason':'missing or nonregular file'}); return
        size=path.stat().st_size
        if size > self.limit:
            self.gaps.append({'path':name,'reason':'individual file exceeds part limit','bytes':size});return
        h=hashlib.sha256();tail=b'';bad=False;binary=False;first=True
        with path.open('rb') as src:
            for chunk in iter(lambda:src.read(1024*1024),b''):
                h.update(chunk)
                if first and chunk[:4] in (b'\x7fELF',b'\xcf\xfa\xed\xfe',b'\xfe\xed\xfa\xcf',b'\xca\xfe\xba\xbe'):
                    binary=True
                first=False
                if not allow_gts_metadata and any(m in tail+chunk for m in GTS_MARKERS):bad=True
                tail=chunk[-100:]
        if binary:
            self.gaps.append({'path':name,'reason':'native build output excluded'});return
        if bad:
            self.gaps.append({'path':name,'reason':'GTS-related source content excluded'});return
        before=path.stat()
        self.begin(size); info=tarfile.TarInfo(name);info.size=size
        info.mode=0o755 if before.st_mode & 0o111 else 0o644
        with path.open('rb') as src:self.part.addfile(info,src)
        after=path.stat()
        if before.st_size != after.st_size or before.st_mtime_ns != after.st_mtime_ns or digest(path)!=h.hexdigest():
            raise ValueError('Source changed during capture: '+name)
        self.raw += size
        self.files.append({'path':name,'bytes':size,'sha256':h.hexdigest(),'archive_part':self.number})

    def tree(self, root, prefix):
        root=root.resolve()
        for base, dirs, files in os.walk(root,followlinks=False):
            rel=Path(base).relative_to(root)
            # Record excluded directories without traversing them.
            keep=[]
            for directory in dirs:
                name=(rel/directory).as_posix()
                if blocked(name):
                    self.gaps.append({'path':prefix+'/'+name,'reason':'excluded directory'});continue
                if (Path(base)/directory).is_symlink():self.add(root,name,prefix)
                else:keep.append(directory)
            dirs[:]=keep
            for file in files:self.add(root,(rel/file).as_posix(),prefix)


def compiled_ids(paths):
    ids=set()
    for path in paths:
        for line in path.read_text().splitlines():
            try:row=json.loads(line)
            except json.JSONDecodeError:continue
            if row.get('reason')=='compiler-artifact' and row.get('package_id'):ids.add(row['package_id'])
    return ids


def brew_kegs(receipt, cellar):
    result=set();gaps=[]
    for row in receipt.get('graphviz_transformations',[]):
        path=Path(row['source_path'])
        if not path.is_relative_to(cellar) or len(path.relative_to(cellar).parts)<3:
            gaps.append({'path':str(path),'reason':'copied library outside expected Cellar'});continue
        formula,version=path.relative_to(cellar).parts[:2]
        result.add((formula,version))
    return sorted(result),gaps


def collect(args):
    output=args.output.resolve()
    output.mkdir(parents=True,exist_ok=False)
    gaps=[];groups={};manifest={'schema_version':1,'release_ready':False,
        'scope':'Captured build inputs, not proof every compiled package shipped or complete legal fulfillment',
        'source_commit':run(['git','rev-parse','HEAD'],args.source).strip(),
        'gaps':gaps,'groups':groups}
    def group(label):
        obj=Parts(output,label,gaps);groups[label]={'archives':obj.archives,'files':obj.files};return obj
    app=group('application-source')
    for name in run(['git','ls-files','-z'],args.source).split('\0'):
        if name:app.add(args.source,name,'application')
    app.close()
    zig=group('zig-package-sources');package_keys=[]
    for label,cache in [('global',args.zig_global),('local',args.zig_local)]:
        package_dir=cache/'p'
        if not package_dir.is_dir():
            gaps.append({'scope':'zig','reason':label+' cache p/ missing'});continue
        for root in sorted(package_dir.iterdir()):
            if root.is_symlink() or not root.is_dir():
                gaps.append({'scope':'zig','reason':'non-directory package entry excluded','name':root.name});continue
            package_keys.append({'cache':label,'key':root.name})
            zig.tree(root,'zig/'+label+'/'+root.name)
    zig.close();groups['zig-package-sources']['package_keys']=package_keys
    groups['zig-package-sources']['scope']='p/ entries from dedicated build caches; presence is not proof of static linkage'
    cargo=group('cargo-compiled-sources')
    logs=list((args.source/'target/build-evidence').glob('cargo-*.jsonl'))
    ids=compiled_ids(logs)
    for log in logs:cargo.add(log.parent,log.name,'build-records')
    if not ids:gaps.append({'scope':'cargo','reason':'No compiler artifact package IDs'})
    try:
        metadata=json.loads(run(['cargo','metadata','--locked','--offline','--format-version=1','--filter-platform','aarch64-apple-darwin'],args.source))
    except (ValueError,json.JSONDecodeError):
        metadata={'packages':[]};gaps.append({'scope':'cargo','reason':'Locked offline metadata unavailable'})
    meta_path=output/'cargo-metadata.json';meta_path.write_text(json.dumps(metadata,indent=2)+'\n')
    cargo.add(output,meta_path.name,'build-records')
    known=set();roots={};packages=[]
    for package in metadata['packages']:
        if package['id'] not in ids:continue
        known.add(package['id']);root=Path(package['manifest_path']).parent.resolve()
        origin=package.get('source') or 'application-local'
        if origin.startswith('git+'):
            try:root=Path(run(['git','rev-parse','--show-toplevel'],root).strip()).resolve()
            except ValueError:gaps.append({'scope':'cargo','package':package['id'],'reason':'Git workspace root unavailable'})
        if root.is_relative_to(args.source.resolve()):
            packages.append({'id':package['id'],'source':origin,'retained_in':'application-source'});continue
        if root not in roots:
            prefix='cargo/package-root-'+str(len(roots)+1).zfill(4)
            roots[root]=prefix
            if origin.startswith('git+'):
                try:
                    tracked=run(['git','ls-files','-z'],root).split('\0')
                    for name in tracked:
                        if name:cargo.add(root,name,prefix)
                except ValueError:
                    gaps.append({'scope':'cargo','package':package['id'],'reason':'Git source tracked file enumeration failed'})
            else:
                cargo.tree(root,prefix)
        packages.append({'id':package['id'],'source':origin,'retained_in':roots[root],
                         'declared_license':package.get('license'),'license_file':package.get('license_file')})
    for missing in sorted(ids-known):gaps.append({'scope':'cargo','package':missing,'reason':'compiler package absent from metadata'})
    cargo.close();groups['cargo-compiled-sources']['packages']=packages
    groups['cargo-compiled-sources']['scope']='Compiled candidates including build dependencies; not all proven shipped'
    brew=group('homebrew-installed-metadata')
    receipt=json.loads(args.graphics_provenance.read_text())
    kegs,extra=brew_kegs(receipt,args.cellar);gaps.extend(extra)
    brew.add(args.graphics_provenance.parent,args.graphics_provenance.name,'graphics')
    if not kegs:gaps.append({'scope':'homebrew','reason':'No packaged keg associations found'})
    names=sorted({n for n,v in kegs})
    if names:
        try:
            info=json.loads(run(['brew','info','--json=v2',*names]))
            info_path=output/'homebrew-resolved-info.json';info_path.write_text(json.dumps(info,indent=2)+'\n')
            brew.add(output,info_path.name,'homebrew',allow_gts_metadata=True)
        except (ValueError,json.JSONDecodeError):gaps.append({'scope':'homebrew','reason':'Resolved brew info unavailable'})
    for name,version in kegs:
        root=args.cellar/name/version;prefix='homebrew/'+name+'/'+version
        for file in ['INSTALL_RECEIPT.json','sbom.spdx.json']:
            brew.add(root,file,prefix,allow_gts_metadata=True)
        recipes=list((root/'.brew').glob('*.rb'))
        if not recipes:gaps.append({'scope':'homebrew','formula':name,'reason':'Installed .brew recipe missing'})
        for recipe in recipes:brew.add(root,recipe.relative_to(root).as_posix(),prefix,allow_gts_metadata=True)
        gaps.append({'scope':'homebrew','formula':name,'reason':'No upstream source/patch/bottle download in this collector; selected bottle identity needs receipt/API reconciliation'})
    brew.close();groups['homebrew-installed-metadata']['packaged_kegs']=[{'name':n,'version':v} for n,v in kegs]
    manifest['incomplete']=True
    manifest['remaining']=['Review license texts and fulfillment for actually shipped components',
        'Resolve excluded source paths and generated/external build inputs',
        'Associate captured Homebrew bottle metadata with exact installed bytes; acquire permitted corresponding inputs separately',
        'Sparkle actual artifact and full license capture are separate',
        'GTS source is intentionally not collected; publication restriction remains']
    (output/'trial-inputs-manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
    return manifest


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source',type=Path,required=True)
    parser.add_argument('--output',type=Path,required=True)
    parser.add_argument('--zig-global',type=Path,required=True)
    parser.add_argument('--zig-local',type=Path,required=True)
    parser.add_argument('--graphics-provenance',type=Path,required=True)
    parser.add_argument('--cellar',type=Path,default=Path('/opt/homebrew/Cellar'))
    collect(parser.parse_args())

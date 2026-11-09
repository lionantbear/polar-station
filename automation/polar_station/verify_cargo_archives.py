#!/usr/bin/env python3
"""Verify downloaded registry archives against Cargo.lock without extracting or executing them."""
import argparse
import json,pathlib,tomllib,hashlib,tarfile
parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--source',type=pathlib.Path,required=True);parser.add_argument('--metadata',type=pathlib.Path,required=True);parser.add_argument('--crate-cache',type=pathlib.Path,required=True);args=parser.parse_args()
r=args.source;lock=tomllib.loads((r/'Cargo.lock').read_text());expected={(p['name'],p['version'],p.get('source')):p.get('checksum') for p in lock['package']};m=json.loads(args.metadata.read_text());records=[];mpl=[]
cache=args.crate_cache
for p in m['packages']:
 source=p.get('source') or ''
 if not source.startswith('registry+'):continue
 f=cache/(p['name']+'-'+p['version']+'.crate');h=hashlib.sha256(f.read_bytes()).hexdigest();good=h==expected[(p['name'],p['version'],source)];record=dict(name=p['name'],version=p['version'],source=source,archive_sha256=h,lock_checksum_matches=good)
 if p['license']=='MPL-2.0':
  root=pathlib.Path(p['manifest_path']).parent;changed=[];count=0
  with tarfile.open(f,'r:gz') as tar:
   for member in tar:
    if not member.isfile():continue
    rel=pathlib.PurePosixPath(member.name)
    if rel.is_absolute() or '..' in rel.parts: raise ValueError('Unsafe crate archive path')
    rel=pathlib.Path(*rel.parts[1:]);local=root/rel;data=tar.extractfile(member).read();count+=1
    if not local.is_file() or local.read_bytes()!=data:changed.append(str(rel))
  mpl.append(dict(name=p['name'],version=p['version'],source=source,source_url=f"https://static.crates.io/crates/{p['name']}/{f.name}",source_files_checked=count,changed_files=changed,archive_sha256=h,lock_checksum_matches=good))
 records.append(record)
(r/'licenses/cargo-archive-integrity.json').write_text(json.dumps(dict(schema_version=1,scope='Registry archives SHA256 checked against exported Cargo.lock, not a binary audit',packages=records),indent=2)+'\n')
(r/'licenses/mpl-source-inventory.json').write_text(json.dumps(dict(schema_version=1,scope='MPL registry source bytes compared with lock-checksummed original crate archives; Git MPL sources separately pinned',packages=mpl,git_mpl_packages=[dict(name=p['name'],version=p['version'],source=p['source']) for p in m['packages'] if p.get('license')=='MPL-2.0' and p.get('source','').startswith('git+')]),indent=2)+'\n')
print('Registry archives',len(records),'checksum failures',sum(not p['lock_checksum_matches'] for p in records));print('MPL files',sum(p['source_files_checked'] for p in mpl),'differences',[(p['name'],p['changed_files']) for p in mpl if p['changed_files']])

if any(not p['lock_checksum_matches'] for p in records) or any(p['changed_files'] for p in mpl):
 raise SystemExit(2)

"""Remove only declared test-only Cargo packages and targets, preserving TOML layout."""
import json
from pathlib import Path
import sys
import tomlkit

root = Path(sys.argv[1])
dropped = {'polar-storage-testkit', 'polar-storage-bootstrap', 'polar-postgres-runtime'}
for p in list(root.glob('crates/*/Cargo.toml')) + [root/'Cargo.toml']:
    if not p.exists(): continue
    original=p.read_text();d=tomlkit.parse(original)
    for section in ('dev-dependencies',):
        for table in [d.get(section,{})] + [v.get(section,{}) for v in d.get('target',{}).values()]:
            for name in list(table):
                if name in dropped:del table[name]
    if p==root/'Cargo.toml':
        w=d.get('workspace',{})
        for section in ('members','default-members'):
            if section in w:w[section]=[v for v in w[section] if Path(v).name not in dropped]
        for name in dropped:
            if name in w.get('dependencies',{}):del w['dependencies'][name]
        for profile in d.get('profile',{}).values():
            for name in dropped:
                if name in profile.get('package',{}):del profile['package'][name]
    if 'bin' in d:
        kept=[b for b in d['bin'] if b.get('name')!='polar-force-oracle']
        if len(kept)!=len(d['bin']):
            del d['bin']
            if kept:d['bin']=kept
    updated=tomlkit.dumps(d)
    if updated!=original:p.write_text(updated)
p=root/'Cargo.lock'
if p.exists():
    original=p.read_text();d=tomlkit.parse(original)
    for package in d.get('package',[]):
        if package['name'] in ('polar-vault-store-sqlite','polar-voice-provider-codex'):
            deps=package.get('dependencies',[])
            if 'polar-storage-testkit' in deps:deps.remove('polar-storage-testkit')
    updated=tomlkit.dumps(d)
    if updated!=original:p.write_text(updated)

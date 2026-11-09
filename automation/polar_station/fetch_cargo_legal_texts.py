#!/usr/bin/env python3
"""Supplement missing crate legal texts from exact Cargo VCS commits, without executing upstream code."""
import argparse
import json,pathlib,hashlib,urllib.request,urllib.parse,concurrent.futures
parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--source',type=pathlib.Path,required=True);parser.add_argument('--metadata',type=pathlib.Path,required=True);args=parser.parse_args()
r=args.source;ep=r/'licenses/cargo-evidence.json';e=json.loads(ep.read_text());m=json.loads(args.metadata.read_text());idx={(p['name'],p['version'],p.get('source')):p for p in m['packages']};jobs={}
for p in e['packages']:
 if p['legal_texts']:continue
 q=idx[(p['name'],p['version'],p['source'])];f=pathlib.Path(q['manifest_path']).parent/'.cargo_vcs_info.json'
 if not f.exists():continue
 v=json.loads(f.read_text());u=urllib.parse.urlparse(q.get('repository') or '')
 if u.netloc!='github.com' or len(u.path.strip('/').split('/'))<2:continue
 repo='/'.join(u.path.strip('/').split('/')[:2]).removesuffix('.git');pin=v['git']['sha1'];jobs.setdefault((repo,pin),[]).append(p)
def fetch(k):
 repo,pin=k;results=[];errors=[]
 try:
  directory=json.load(urllib.request.urlopen(f'https://api.github.com/repos/{repo}/git/trees/{pin}',timeout=20))
  names=[x['path'] for x in directory.get('tree',[]) if x['type']=='blob' and any(w in x['path'].lower() for w in ['license','licence','copying','notice','copyright'])]
 except Exception as ex: return k,[],[dict(error=str(ex))]
 for f in names:
  url=f'https://raw.githubusercontent.com/{repo}/{pin}/{f}'
  try:
   b=urllib.request.urlopen(url,timeout=20).read();h=hashlib.sha256(b).hexdigest();out=r/'licenses/cargo-texts'/(h+'.txt');out.write_bytes(b)
   results.append(dict(upstream_path=f,sha256=h,path='licenses/cargo-texts/'+out.name,bytes=len(b),url=url,repository='https://github.com/'+repo,commit=pin))
  except Exception as ex:errors.append(dict(url=url,error=str(ex)))
 return k,results,errors
fail=[]
with concurrent.futures.ThreadPoolExecutor(max_workers=10) as pool:
 for k,texts,errors in pool.map(fetch,jobs):
  for p in jobs[k]:
   p['legal_texts']=texts;p['vcs_root_texts']=True
  if not texts:fail.append(dict(repository=k[0],commit=k[1],errors=errors))
e['counts']['packages_with_texts']=sum(bool(p['legal_texts']) for p in e['packages']);e['counts']['packages_without_texts']=sum(not p['legal_texts'] for p in e['packages']);e['vcs_fetch_failures']=fail;ep.write_text(json.dumps(e,indent=2)+'\n');print(e['counts']);print('unresolved',[(p['name'],p['version']) for p in e['packages'] if not p['legal_texts']])

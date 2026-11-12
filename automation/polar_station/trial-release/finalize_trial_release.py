#!/usr/bin/env python3
"""Exact-input TWO-FILE trial packaging. No source archives are generated.
Does not build or execute downloaded sources.
A trusted write-authorized job fetches private inputs, binds the original native
launch evidence, packages/verifies without executing the app, and publishes a new
curated draft. A separate read-only job smoke-tests the PUBLIC download afterward.
An unfilled plan or pending review fails before network access or process launch.
"""
import argparse
import base64
from contextlib import contextmanager
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import shutil
import signal
import stat
import subprocess
import sys
import tarfile
import time
import urllib.error
import urllib.parse
import urllib.request
import zipfile

REPO = 'lionantbear/polar-station'
# No concrete application baseline is selected by this mechanism.
# Every command binds the actual reviewed source and retained native run.
SOURCE = RUN = RUN_ATTEMPT = RAW_TAG = FINAL_TAG = None
RAW_DMG = 'Polar-Station-macOS-arm64-UNNOTARIZED.dmg'
FINAL_DMG = 'Polar-Station-macOS-arm64-TRIAL-UNNOTARIZED.dmg'
PUBLIC_ASSETS = frozenset((FINAL_DMG, 'SHA256SUMS'))
MAX_ASSET = 1_990_000_000
MAX_TEXT = 16_000_000
MAX_TOTAL = 25_000_000_000
SOURCE_RE = re.compile(r'(?:application-source|cargo-compiled-sources|zig-package-sources|homebrew-installed-metadata)-[0-9]{3}\.tar\.gz|(?:ghostty|godot)-built-source\.tar\.gz')
RAW_FIXED = {RAW_DMG, 'draft-build-index.json', 'SHA256SUMS', 'trial-inputs-manifest.json', 'build-evidence.zip'}
ATTESTATIONS = ('actual_capture_and_source_identity', 'cargo_and_zig_notices',
    'native_versions_recipes_patches_resources', 'complete_corresponding_source_and_build_inputs',
    'license_compatibility_and_required_change_notices', 'lgpl_runtime_copyright_condition',
    'public_source_access_and_final_documents', 'no_private_logs_or_secrets_in_public_assets', 'original_native_launch_evidence_matches_exact_dmg', 'verified_upstream_source_patch_and_rebuild_access')

class Stop(Exception):
    pass

def need(ok, code):
    if not ok:
        raise Stop(code)

def digest(path):
    h = hashlib.sha256()
    with Path(path).open('rb') as f:
        for block in iter(lambda: f.read(1024 * 1024), b''):
            h.update(block)
    return h.hexdigest()

def sha(data):
    return hashlib.sha256(data).hexdigest()

def is_sha(value):
    return isinstance(value, str) and re.fullmatch('[0-9a-f]{64}', value) is not None

def read_json(path):
    def unique(rows):
        result = {}
        for k, v in rows:
            need(k not in result, 'duplicate_json_key')
            result[k] = v
        return result
    data = Path(path).read_bytes()
    need(len(data) <= 100_000_000, 'json_too_large')
    return json.loads(data, object_pairs_hook=unique)

def write_json(path, value):
    Path(path).write_text(json.dumps(value, indent=2, sort_keys=True)+'\n')

def safe_rel(value):
    need(isinstance(value, str) and 0 < len(value) < 2048, 'invalid_relative_path')
    need(not any(ord(c) < 32 or ord(c) == 127 for c in value) and '\\' not in value and ':' not in value,
         'invalid_relative_path')
    need(all(p not in ('', '.', '..') for p in value.split('/')), 'unsafe_relative_path')
    return value

def row_check(row):
    safe_rel(row['path'])
    need(type(row.get('bytes')) is int and 0 <= row['bytes'] < MAX_ASSET and is_sha(row.get('sha256')),
         'invalid_file_identity')

def check_file(path, row):
    need(path.is_file() and not path.is_symlink(), 'missing_or_symlink_input')
    need(path.stat().st_size == row['bytes'] and digest(path) == row['sha256'], 'file_identity_mismatch')

def rows_tree_hash(rows):
    need(len({r['path'] for r in rows}) == len(rows), 'duplicate_packet_path')
    return sha(''.join(r['sha256']+'  '+r['path']+'\n' for r in sorted(rows, key=lambda r:r['path'])).encode())

def regular_files(root):
    files = []
    for p in sorted(root.rglob('*')):
        need(not p.is_symlink(), 'unexpected_symlink_in_packet')
        if p.is_file():
            files.append({'path': str(p.relative_to(root)), 'bytes': p.stat().st_size, 'sha256':digest(p)})
        else:
            need(p.is_dir(), 'unexpected_special_file')
    return files

def bind_identity(plan):
    global SOURCE,RUN,RUN_ATTEMPT,RAW_TAG,FINAL_TAG
    need(plan.get('repository')==REPO,'repository_is_fixed')
    source=plan.get('source_commit');run_id=plan.get('run_id');attempt=plan.get('run_attempt')
    need(isinstance(source,str) and re.fullmatch('[0-9a-f]{40}',source),'full_source_commit_required')
    need(type(run_id) is int and 0<run_id<2**63 and type(attempt) is int and 0<attempt<10000,'exact_run_and_attempt_required')
    raw='macos-arm64-trial-build-'+str(run_id)+'-'+str(attempt)
    final='macos-arm64-trial-'+str(run_id)+'-'+str(attempt)
    need(plan.get('raw_tag')==raw and plan.get('final_tag')==final,'run_bound_tag_names_required')
    SOURCE,RUN,RUN_ATTEMPT,RAW_TAG,FINAL_TAG=source,run_id,attempt,raw,final


def check_gate(plan_path, review_path):
    plan = read_json(plan_path); review = read_json(review_path)
    need(plan.get('status') == 'ready-for-reviewed-trial' and review.get('status') == 'approved-for-this-trial',
         'real_review_required')
    bind_identity(plan)
    need(plan.get('repository') == REPO and plan.get('source_commit') == SOURCE and plan.get('run_id') == RUN
         and plan.get('raw_tag') == RAW_TAG and plan.get('final_tag') == FINAL_TAG, 'plan_identity_mismatch')
    need(review.get('source_commit') == SOURCE and review.get('run_id') == RUN and review.get('run_attempt') == RUN_ATTEMPT
         and review.get('plan_sha256') == digest(plan_path)
         and review.get('program_sha256') == digest(Path(__file__)), 'review_binding_mismatch')
    need(is_sha(review.get('inspection_report_sha256')), 'inspection_report_digest_required')
    need(isinstance(review.get('reviewer'), str) and review['reviewer'].strip()
         and review.get('reviewer') not in ('TODO', 'TBD', 'pending'), 'real_reviewer_required')
    need(isinstance(review.get('reviewed_at'), str) and re.fullmatch(r'\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z',review['reviewed_at']), 'review_date_required')
    need(review.get('unresolved') == [] and all(review.get('attestations',{}).get(k) is True for k in ATTESTATIONS),
         'unresolved_legal_or_source_review')
    need(isinstance(review.get('gap_dispositions'),list) and review['gap_dispositions']
         and all(isinstance(r,dict) and r.get('gap') and r.get('resolution') and r.get('evidence') for r in review['gap_dispositions']),
         'actual_gap_dispositions_required')
    need(type(plan.get('raw_release_id')) is int and plan['raw_release_id'] > 0, 'exact_release_id_required')
    rows = plan.get('assets',[])
    need(rows and len({r.get('name') for r in rows}) == len(rows), 'asset_set_required')
    need(len({r.get('id') for r in rows}) == len(rows), 'duplicate_asset_id')
    for r in rows:
        row_check({**r, 'path':r['name']})
        need(type(r.get('id')) is int and r['id'] > 0 and r['bytes'] > 0
             and (r['name'] in RAW_FIXED or SOURCE_RE.fullmatch(r['name'])), 'invalid_draft_asset')
    names = {r['name'] for r in rows}
    need(RAW_FIXED <= names and {'ghostty-built-source.tar.gz','godot-built-source.tar.gz'} <= names,
         'incomplete_draft_asset_set')
    for prefix in ('application-source','cargo-compiled-sources','zig-package-sources','homebrew-installed-metadata'):
        need(any(n.startswith(prefix+'-') for n in names), 'required_capture_group_missing')
    launch=plan.get('native_launch_evidence',{})
    need(launch.get('asset')=='build-evidence.zip' and launch.get('member')=='station-evidence/installation-launch.json',
         'exact_native_launch_evidence_required')
    row_check({**launch,'path':launch['member']})
    base=Path(plan_path).parent
    need(not any(k in plan for k in ('legal_packets','source_projection')), 'obsolete_source_bundle_plan_rejected')
    local=plan.get('notice_files',[]);extras=plan.get('captured_notices',[])
    need(isinstance(local,list) and isinstance(extras,list) and local+extras, 'reviewed_full_notices_required')
    destinations=set();notice_rows=[]
    for row in local:
        row_check(row);notice_path(row['destination']);check_file(base/row['path'],row)
        need(row['bytes']<=MAX_TEXT,'notice_too_large')
        need(row['destination'] not in destinations,'duplicate_notice_destination');destinations.add(row['destination'])
        notice_rows.append({'path':row['destination'],'bytes':row['bytes'],'sha256':row['sha256']})
    for row in extras:
        row_check(row);notice_path(row['path']);notice_path(row['member'])
        if row.get('asset')=='build-evidence.zip':
            need(isinstance(row.get('source_origin'),str),'original_notice_source_origin_required');notice_path(row['source_origin'])
        need(row.get('asset') in names and row['asset']!=RAW_DMG and row['path'] not in destinations,'invalid_captured_notice')
        need(row['bytes']<=MAX_TEXT,'notice_too_large');destinations.add(row['path'])
        notice_rows.append({k:row[k] for k in ('path','bytes','sha256')})
    need(rows_tree_hash(notice_rows)==review.get('notice_tree_sha256'),'reviewed_notice_tree_mismatch')
    docs=plan.get('documents',{})
    need(set(docs)=={'LICENSE-AGPL-3.0.txt','READ-ME-FIRST.txt','SOURCE-ACCESS.txt'},'final_documents_required')
    for filename,row in docs.items():
        row_check(row);need(row['bytes']<=MAX_TEXT,'document_too_large');check_file(base/row['path'],row)
    need('GNU AFFERO GENERAL PUBLIC LICENSE' in (base/docs['LICENSE-AGPL-3.0.txt']['path']).read_text(),'agpl_text_missing')
    need(sum(r['bytes'] for r in rows)<=MAX_TOTAL,'input_total_bound')
    need(type(plan.get('packaging_authorized')) is bool and type(plan.get('publication_authorized')) is bool,'explicit_execution_state_required')
    validate_source_access(plan,review,notice_rows)
    source_text=(base/docs['SOURCE-ACCESS.txt']['path']).read_text()
    need(plan['source_access']['first_party_url'] in source_text and plan['source_access']['directions_document']['url'] in source_text,
         'exact_source_directions_missing_inside_dmg')
    return plan,review



def notice_path(value):
    safe_rel(value)
    need(not {'build-records','.git','.hg','.svn'} & set(value.split('/')),'raw_record_not_a_notice')
    name=Path(value).name.lower()
    need(name not in ('provenance.json','cargo.toml','cargo.toml.orig','.cargo_vcs_info.json')
         and not name.endswith(('.json','.jsonl','.toml','.rs','.py','.c','.h','.cpp','.swift','.zig','.star','.log','.zip','.tar','.gz','.xz','.bz2','.crate','.cfg','.java','.rb','.tt')),
         'source_or_audit_file_not_a_notice')
    return value


def https_url(value):
    need(isinstance(value,str) and len(value)<4096 and not any(ord(c)<32 for c in value),'invalid_source_url')
    url=urllib.parse.urlsplit(value)
    need(url.scheme=='https' and url.hostname and not url.username and not url.password and url.port in (None,443),'invalid_source_url')
    return value


def source_access_hash(value):return sha(json.dumps(value,sort_keys=True,separators=(',',':')).encode())


def validate_source_access(plan,review,notice_rows):
    access=plan.get('source_access',{})
    need(access.get('status')=='reviewed-for-this-trial' and access.get('source_commit')==SOURCE
         and access.get('inspection_report_sha256')==review['inspection_report_sha256'] and access.get('unresolved')==[],
         'source_access_review_required')
    need(review.get('source_access_sha256')==source_access_hash(access),'source_access_review_binding_mismatch')
    need(access.get('first_party_url')=='https://github.com/'+REPO+'/tree/'+SOURCE,'exact_first_party_git_source_required')
    doc=access.get('directions_document',{});row_check(doc)
    commit=doc.get('repository_commit')
    need(isinstance(commit,str) and re.fullmatch('[0-9a-f]{40}',commit),'exact_directions_commit_required')
    need(doc['bytes']<=MAX_TEXT and doc.get('url')=='https://github.com/'+REPO+'/blob/'+commit+'/'+doc['path'],
         'exact_repository_directions_url_required')
    need(access.get('rebuild_instructions_path')==doc['path'],'reviewed_rebuild_directions_required')
    text=access.get('release_page_directions')
    need(isinstance(text,str) and 0<len(text)<1600 and doc['url'] in text,'release_page_source_directions_required')
    terms={'THIRD-PARTY-NOTICES/'+r['path'] for r in notice_rows}|set(plan['documents'])
    routes=access.get('license_route')
    need(isinstance(routes,list) and routes,'actual_license_routes_required')
    for route in routes:
        need(all(isinstance(route.get(k),str) and route[k].strip() for k in ('component','offered_grant','elected_version')),
             'specific_license_route_required')
        paths=route.get('terms_paths',[])
        need(paths and len(paths)==len(set(paths)) and set(paths)<=terms,'full_license_terms_missing_from_dmg')
        urls=route.get('source_urls',[]);patches=route.get('patch_recipe_urls')
        need(isinstance(urls,list) and urls and isinstance(patches,list),'reviewed_upstream_source_and_patch_routes_required')
        for url in urls+patches:https_url(url)
        need(isinstance(route.get('availability_checked_at'),str)
             and re.fullmatch(r'\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z',route['availability_checked_at'])
             and is_sha(route.get('evidence_sha256')),'actual_source_availability_evidence_required')


def verify_source_access(client,plan):
    """GET the exact reviewed repository directions; never publish a source bundle."""
    doc=plan['source_access']['directions_document']
    value=client.json('/repos/'+REPO+'/contents/'+urllib.parse.quote(doc['path'],safe='/')+'?ref='+doc['repository_commit'])
    need(value.get('type')=='file' and value.get('path')==doc['path'] and value.get('encoding')=='base64',
         'repository_source_directions_unavailable')
    content=value.get('content')
    need(isinstance(content,str) and len(content)<=MAX_TEXT*2,'repository_directions_size_bound')
    data=base64.b64decode(''.join(content.split()),validate=True)
    need(len(data)==doc['bytes'] and sha(data)==doc['sha256'],'repository_source_directions_changed')


def release_body(plan):
    access=plan['source_access']
    return ('Apple Silicon trial. Download '+FINAL_DMG+'; SHA256SUMS contains its SHA-256. '
        'Ad-hoc signed, not notarized. Full license and third-party notice texts are inside the DMG.\n\n'
        'First-party source: '+access['first_party_url']+'\n'+access['release_page_directions']+'\n\n'
        'The original app passed a limited 15-second macOS 26 CI launch. The final DMG preserves that signed app unchanged. '
        'A separate public-download smoke check follows publication; no clean-machine or interactive GUI compatibility claim. '
        'Use normal macOS Open Anyway approval if needed; do not disable system security.\n')

class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self,req,fp,code,msg,headers,newurl): return None


class GitHub:
    def __init__(self,token):
        need(bool(token),'github_token_required'); self.token=token
        self.opener=urllib.request.build_opener(NoRedirect)
    def request(self,path,method='GET',data=None,binary=False,upload=False):
        need(path.startswith('/repos/'+REPO+'/'),'unexpected_api_target')
        host='https://uploads.github.com' if upload else 'https://api.github.com'
        headers={'Authorization':'Bearer '+self.token,'Accept':'application/octet-stream' if binary else 'application/vnd.github+json',
            'X-GitHub-Api-Version':'2022-11-28','User-Agent':'polar-station-trial-finalizer'}
        if data is not None:
            if upload:
                headers['Content-Type']='application/octet-stream';headers['Content-Length']=str(os.fstat(data.fileno()).st_size)
            else:
                data=json.dumps(data).encode();headers['Content-Type']='application/json'
        try:
            return self.opener.open(urllib.request.Request(host+path,data=data,headers=headers,method=method),timeout=600)
        except urllib.error.HTTPError as e:
            if method == 'GET' and binary and e.code in (301,302,303,307,308):
                url=e.headers.get('Location','');u=urllib.parse.urlsplit(url)
                need(u.scheme=='https' and u.hostname in {'release-assets.githubusercontent.com','objects.githubusercontent.com'}
                     and not u.username and not u.password and u.port in (None,443),'unexpected_asset_redirect')
                return self.opener.open(urllib.request.Request(url,headers={'User-Agent':'polar-station-trial-finalizer'}),timeout=600)
            if method=='GET' and e.code==404: raise Stop('github_not_found') from None
            raise Stop('github_'+method.lower()+'_http_'+str(e.code)) from None
        except Stop: raise
        except Exception: raise Stop('github_'+method.lower()+'_outcome_uncertain_no_automatic_retry') from None
    def json(self,path,method='GET',data=None):
        with self.request(path,method,data) as r:
            content=r.read(100_000_001)
        need(len(content)<=100_000_000,'api_response_too_large')
        return json.loads(content)
    def download(self,row,path):
        with self.request('/repos/'+REPO+'/releases/assets/'+str(row['id']),binary=True) as r:
            stream_checked(r,path,row)
    def upload(self,release_id,path):
        with path.open('rb') as f, self.request('/repos/'+REPO+'/releases/'+str(release_id)+'/assets?name='+urllib.parse.quote(path.name),
                                             'POST',f,upload=True) as r:
            return json.load(r)

class ReadOnlyGitHub(GitHub):
    def request(self,path,method='GET',data=None,binary=False,upload=False):
        need(method=='GET' and data is None and not upload,'read_only_public_client_rejects_mutation')
        return super().request(path,method,data,binary,upload)


def stream_checked(stream,path,row):
    path.parent.mkdir(parents=True,exist_ok=True)
    h=hashlib.sha256();total=0
    with path.open('xb') as out:
        for block in iter(lambda:stream.read(1024*1024),b''):
            total+=len(block);need(total<=row['bytes'],'download_size_mismatch')
            h.update(block);out.write(block)
    need(total==row['bytes'] and h.hexdigest()==row['sha256'],'download_hash_mismatch')

def asset_state(client,plan):
    release=client.json('/repos/'+REPO+'/releases/'+str(plan['raw_release_id']))
    need(release.get('id')==plan['raw_release_id'] and release.get('draft') is True and release.get('prerelease') is True and release.get('tag_name')==RAW_TAG
         and release.get('target_commitish')==SOURCE and release.get('published_at') is None,'raw_draft_identity_mismatch')
    rows=[];page=1
    while True:
        batch=client.json('/repos/'+REPO+'/releases/'+str(plan['raw_release_id'])+'/assets?per_page=100&page='+str(page))
        need(isinstance(batch,list),'invalid_assets_response');rows.extend(batch)
        if len(batch)<100:break
        page+=1;need(page<20,'asset_page_bound')
    expected={r['name']:r for r in plan['assets']}
    need(len(rows)==len(expected) and {r.get('name') for r in rows}==set(expected),'raw_draft_asset_set_mismatch')
    for r in rows:
        e=expected[r['name']]
        need(r.get('id')==e['id'] and r.get('size')==e['bytes'] and r.get('state')=='uploaded'
             and r.get('digest') in (None,'','sha256:'+e['sha256']),'raw_asset_identity_mismatch')
    return {r['name']:(r['id'],r['size'],r.get('updated_at'),r.get('digest')) for r in rows}

def check_run(client):
    r=client.json('/repos/'+REPO+'/actions/runs/'+str(RUN))
    need(r.get('id')==RUN and r.get('head_sha')==SOURCE and r.get('run_attempt')==RUN_ATTEMPT
         and r.get('status')=='completed' and r.get('conclusion')=='success'
         and r.get('path') in ('.github/workflows/macos-trial.yml', '.github/workflows/macos-release-build.yml')
         and r.get('repository',{}).get('full_name')==REPO
         and r.get('repository',{}).get('private') is False,'source_build_not_successful_or_repository_not_public')

def selected_members(archive,members):
    """One pass over each archive, reading only exact bounded regular members."""
    wanted=set(members);result={}
    for name in wanted:safe_rel(name)
    if zipfile.is_zipfile(archive):
        with zipfile.ZipFile(archive) as z:
            for r in z.infolist():
                if r.filename not in wanted:continue
                need(r.filename not in result,'missing_or_duplicate_selected_member')
                mode=r.external_attr>>16
                need(not r.is_dir() and not stat.S_ISLNK(mode) and stat.S_IFMT(mode) in (0,stat.S_IFREG)
                     and r.file_size<=MAX_TEXT,'selected_member_not_small_regular_file')
                result[r.filename]=z.read(r)
    else:
        with tarfile.open(archive,'r|*') as t:
            for r in t:
                if r.name not in wanted:continue
                need(r.name not in result,'missing_or_duplicate_selected_member')
                need(r.isfile() and r.size<=MAX_TEXT,'selected_member_not_small_regular_file')
                result[r.name]=t.extractfile(r).read(MAX_TEXT+1)
    need(set(result)==wanted,'missing_or_duplicate_selected_member')
    return result


def selected_member(archive,member):
    return selected_members(archive,[member])[member]



def fetch(plan,base,work,client):
    need(not work.exists(),'work_directory_already_exists');work.mkdir(parents=True)
    check_run(client);before=asset_state(client,plan)
    download_names={RAW_DMG}
    download_names|={r['asset'] for r in plan['captured_notices']}|{plan['native_launch_evidence']['asset']}
    inputs=work/'private-inputs';inputs.mkdir()
    for r in plan['assets']:
        if r['name'] in download_names:client.download(r,inputs/r['name'])
    need(asset_state(client,plan)==before,'raw_draft_changed_during_fetch')
    write_json(work/'fetch-receipt.json',{'source_commit':SOURCE,'run_id':RUN,'input_files':regular_files(inputs)})

def run(*args):
    result=subprocess.run(args,stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.PIPE,check=False)
    need(result.returncode==0,'local_command_failed_'+Path(args[0]).name)
    return result.stdout

@contextmanager
def mounted(dmg,mount):
    mount.mkdir()
    run('/usr/bin/hdiutil','attach',str(dmg),'-readonly','-nobrowse','-mountpoint',str(mount))
    try:yield mount
    finally:run('/usr/bin/hdiutil','detach',str(mount))

def xattrs_for(path):
    """Read xattrs without following symlinks; Python's os.*xattr is Linux-only."""
    if hasattr(os,'listxattr'):
        return {k:sha(os.getxattr(path,k,follow_symlinks=False)) for k in sorted(os.listxattr(path,follow_symlinks=False))}
    need(sys.platform=='darwin','unsupported_xattr_platform')
    import ctypes
    libc=ctypes.CDLL('/usr/lib/libSystem.B.dylib',use_errno=True)
    libc.listxattr.argtypes=[ctypes.c_char_p,ctypes.c_void_p,ctypes.c_size_t,ctypes.c_int]
    libc.listxattr.restype=ctypes.c_ssize_t
    libc.getxattr.argtypes=[ctypes.c_char_p,ctypes.c_char_p,ctypes.c_void_p,ctypes.c_size_t,ctypes.c_uint32,ctypes.c_int]
    libc.getxattr.restype=ctypes.c_ssize_t
    name=os.fsencode(path);options=1 # XATTR_NOFOLLOW
    size=libc.listxattr(name,None,0,options);need(0<=size<=1_000_000,'xattr_list_failed_or_too_large')
    names=ctypes.create_string_buffer(size or 1)
    count=libc.listxattr(name,names,size,options);need(count==size,'xattr_list_changed')
    result={}
    for attr in sorted(n for n in names.raw[:count].split(b'\0') if n):
        n=libc.getxattr(name,attr,None,0,0,options);need(0<=n<=MAX_TEXT,'xattr_read_failed_or_too_large')
        value=ctypes.create_string_buffer(n or 1)
        read=libc.getxattr(name,attr,value,n,0,options);need(read==n,'xattr_changed_during_read')
        result[os.fsdecode(attr)]=sha(value.raw[:n])
    return result


def app_inventory(app):
    need(app.is_dir() and not app.is_symlink(),'app_missing_or_symlink')
    result=[{'path':'.','mode':stat.S_IMODE(app.lstat().st_mode),
             'xattrs':xattrs_for(app)}]
    for root,dirs,files in os.walk(app,followlinks=False):
        for name in sorted(dirs+files):
            p=Path(root)/name;mode=p.lstat().st_mode;rel=str(p.relative_to(app))
            row={'path':rel,'mode':stat.S_IMODE(mode)}
            if p.is_symlink():row['target']=os.readlink(p)
            elif p.is_file():row['sha256']=digest(p);row['bytes']=p.stat().st_size
            else:need(p.is_dir(),'app_contains_special_file')
            # Include resource forks and other xattrs, not merely the data fork.
            row['xattrs']=xattrs_for(p)
            result.append(row)
    return sorted(result,key=lambda r:r['path'])

def app_plist(app):
    value=plistlib.loads((app/'Contents/Info.plist').read_bytes())
    need(value.get('NSHumanReadableCopyright') is None or value.get('NSHumanReadableCopyright')=='',
         'runtime_copyright_requires_specific_lgpl_section6_review')
    executable=value.get('CFBundleExecutable')
    need(isinstance(executable,str) and re.fullmatch(r'[A-Za-z0-9 _.-]+',executable)
         and executable not in ('.','..'),'invalid_bundle_executable')
    need(value.get('CFBundleIdentifier')=='com.polarealm.polar-station','unexpected_bundle_identifier')
    usage=value.get('NSMicrophoneUsageDescription')
    need(isinstance(usage,str) and usage.strip(),'microphone_usage_description_missing')
    entitlements=plistlib.loads(run('/usr/bin/codesign','-d','--entitlements',':-',str(app)))
    need(entitlements.get('com.apple.security.device.audio-input') is True,'final_main_microphone_entitlement_missing')
    return value

def macho_versions(app):
    rows=[]
    magics={bytes.fromhex(h) for h in ('feedface','cefaedfe','feedfacf','cffaedfe','cafebabe','bebafeca','cafebabf','bfbafeca')}
    for p in sorted(app.rglob('*')):
        if not p.is_file() or p.is_symlink():continue
        with p.open('rb') as f:magic=f.read(4)
        if magic not in magics:continue
        text=run('/usr/bin/otool','-l',str(p)).decode('utf-8')
        minima=[];kind=None
        for line in text.splitlines():
            m=re.match(r'\s*cmd (LC_[A-Z0-9_]+)',line)
            if m:kind=m[1]
            m=re.match(r'\s*(minos|version) ([0-9.]+)',line)
            if m and ((kind=='LC_BUILD_VERSION' and m[1]=='minos') or (kind=='LC_VERSION_MIN_MACOSX' and m[1]=='version')):minima.append(m[2])
        rows.append({'path':str(p.relative_to(app)),'sha256':digest(p),'declared_minimum_macos':sorted(set(minima))})
    need(rows,'no_macho_files_found');return rows


def launch_check(app,home):
    home.mkdir();plist=app_plist(app)
    env={k:v for k,v in os.environ.items() if k in ('PATH','TMPDIR','LANG','LC_ALL','DISPLAY')}
    env.update({'HOME':str(home),'XDG_CONFIG_HOME':str(home/'config'),'XDG_CACHE_HOME':str(home/'cache')})
    # Only the already-reviewed signed application is launched; no downloaded source/build script runs.
    proc=subprocess.Popen([str(app/'Contents/MacOS'/plist['CFBundleExecutable'])],env=env,cwd=home,
        stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,start_new_session=True)
    alive=False
    try:
        time.sleep(15);alive=proc.poll() is None
    finally:
        try:os.killpg(proc.pid,signal.SIGTERM)
        except ProcessLookupError:pass
        try:proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            os.killpg(proc.pid,signal.SIGKILL);proc.wait(timeout=5)
    need(alive,'app_exited_before_15_seconds')
    return {'alive_after_15_seconds':True,'cleanup_exit_code':proc.returncode,'scope':'Temporary install, strict signature check and direct executable launch only; no interactive GUI/TCC/upgrade validation.'}

def verify_native_launch(plan,inputs):
    """Bind the actual native-run launch to the exact retained DMG and source."""
    evidence=plan['native_launch_evidence'];archive=inputs/evidence['asset']
    check_file(archive,next(r for r in plan['assets'] if r['name']==evidence['asset']))
    data=selected_member(archive,evidence['member'])
    need(len(data)==evidence['bytes'] and sha(data)==evidence['sha256'],'native_launch_member_hash_mismatch')
    value=json.loads(data)
    raw=next(r for r in plan['assets'] if r['name']==RAW_DMG)
    need(value.get('source_commit')==SOURCE and value.get('run_id')==RUN and value.get('run_attempt')==RUN_ATTEMPT
         and value.get('dmg_sha256')==raw['sha256'],'native_launch_source_or_dmg_binding_mismatch')
    need(value.get('bundle_id')=='com.polarealm.polar-station' and value.get('alive_after_15_seconds') is True
         and 'exit_code_before_cleanup' in value and value['exit_code_before_cleanup'] is None
         and value.get('temporary_copy_signature')=='passed' and type(value.get('cleanup_exit_code')) is int
         and ('observed_seconds' not in value or (type(value['observed_seconds']) in (int,float) and 15<=value['observed_seconds']<=900)),
         'original_native_launch_not_successful')
    return {'source_commit':SOURCE,'run_id':RUN,'run_attempt':RUN_ATTEMPT,'dmg_sha256':raw['sha256'],
        'launch_evidence_sha256':evidence['sha256'],'observed_seconds':value.get('observed_seconds'),'alive_after_15_seconds':True,'exit_code_before_cleanup':None,
        'temporary_copy_signature':'passed','scope':'Original native build DMG on macOS 26; final packaging does not execute the app.'}


def package(plan,base,work):
    need(plan.get('packaging_authorized') is True,'packaging_paused_for_user_direction')
    need(sys.platform=='darwin','macos_required_for_packaging')
    need(not any(os.environ.get(k) for k in ('GH_TOKEN','GITHUB_TOKEN')),'token_must_not_enter_packaging_step')
    inputs=work/'private-inputs'
    for row in plan['assets']:
        p=inputs/row['name']
        if p.exists():check_file(p,row)
    need((inputs/RAW_DMG).is_file(),'raw_dmg_missing')
    native_launch=verify_native_launch(plan,inputs)
    public=work/'public';need(not public.exists(),'public_staging_already_exists');public.mkdir()
    volume=work/'volume';volume.mkdir();notices=volume/'THIRD-PARTY-NOTICES';notices.mkdir()
    for row in plan['notice_files']:
        check_file(base/row['path'],row)
        dest=notices/row['destination'];dest.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(base/row['path'],dest)
    grouped={}
    for row in plan['captured_notices']:grouped.setdefault(row['asset'],[]).append(row)
    for asset,rows in grouped.items():
        members=selected_members(inputs/asset,[row['member'] for row in rows])
        for row in rows:
            data=members[row['member']]
            need(len(data)==row['bytes'] and sha(data)==row['sha256'],'captured_notice_identity_mismatch')
            dest=notices/row['path'];dest.parent.mkdir(parents=True,exist_ok=True);dest.write_bytes(data)
    for name,row in plan['documents'].items():shutil.copyfile(base/row['path'],volume/name)
    need('GNU AFFERO GENERAL PUBLIC LICENSE' in (volume/'LICENSE-AGPL-3.0.txt').read_text(), 'agpl_text_missing')
    with mounted(inputs/RAW_DMG,work/'raw-mount') as mount:
        original=mount/'Polar Station.app';app_plist(original)
        run('/usr/bin/codesign','--verify','--deep','--strict',str(original));before=app_inventory(original)
        run('/usr/bin/ditto','--rsrc','--extattr',str(original),str(volume/'Polar Station.app'))
    need(app_inventory(volume/'Polar Station.app')==before,'app_changed_during_copy')
    run('/usr/bin/codesign','--verify','--deep','--strict',str(volume/'Polar Station.app'))
    minima=macho_versions(volume/'Polar Station.app')
    (volume/'Applications').symlink_to('/Applications')
    run('/usr/bin/hdiutil','create','-fs','HFS+','-volname','Polar Station Trial','-srcfolder',str(volume),'-format','UDZO',str(public/FINAL_DMG))
    run('/usr/bin/hdiutil','verify',str(public/FINAL_DMG))
    visible=regular_files(notices)
    with mounted(public/FINAL_DMG,work/'final-mount') as mount:
        for name in plan['documents']:
            need(digest(mount/name)==digest(volume/name),'visible_document_changed')
        need(regular_files(mount/'THIRD-PARTY-NOTICES')==visible,'visible_notices_changed')
        install=work/'installation';install.mkdir();app=install/'Polar Station.app'
        run('/usr/bin/ditto','--rsrc','--extattr',str(mount/'Polar Station.app'),str(app))
        need(app_inventory(app)==before,'final_dmg_app_changed');app_plist(app)
        run('/usr/bin/codesign','--verify','--deep','--strict',str(app))
    summary={'source_commit':SOURCE,'build_run_id':RUN,'original_dmg_sha256':digest(inputs/RAW_DMG),
        'final_dmg_sha256':digest(public/FINAL_DMG),'application_content_and_xattrs_unchanged':True,
        'application_manifest_sha256':sha(json.dumps(before,sort_keys=True,separators=(',',':')).encode()),
        'strict_code_signature_verification':'passed','dmg_integrity':'passed','visible_notices_verified':True,
        'human_readable_copyright':'empty_or_absent','packaging_did_not_execute_application':True,
        'original_native_launch':native_launch,'mach_o_deployment_targets':minima,
        'signing':'Ad-hoc signed; not Developer ID signed; not notarized.',
        'tested_platform':'macOS 26 runner only. No lower-version or clean-machine compatibility claim.'}
    write_json(work/'packaging-details.json',summary)
    files=regular_files(public)
    need(all(r['bytes']<MAX_ASSET for r in files),'public_asset_size_bound')
    need({r['path'] for r in files}=={FINAL_DMG},'unexpected_public_packaging_output')
    (public/'SHA256SUMS').write_text(digest(public/FINAL_DMG)+'  '+FINAL_DMG+'\n')
    exact_public_assets(public)
    write_json(work/'packaging-receipt.json',{'source_commit':SOURCE,'run_id':RUN,'assets':regular_files(public)})

def public_name_allowed(name):return name in PUBLIC_ASSETS


def exact_public_assets(public):
    rows=regular_files(public)
    need({p.name for p in public.iterdir()}==PUBLIC_ASSETS and len(rows)==2
         and {r['path'] for r in rows}==PUBLIC_ASSETS and all(r['bytes']<MAX_ASSET for r in rows),
         'exact_two_public_assets_required')
    need((public/'SHA256SUMS').read_bytes()==(digest(public/FINAL_DMG)+'  '+FINAL_DMG+'\n').encode('ascii'),'public_checksum_file_mismatch')
    return rows


def fetch_for_publication(plan,work,client):
    """Fresh runner: fetch one exact old DMG for static comparison, never launch it."""
    need((work/'public').is_dir(),'curated_artifact_missing')
    check_run(client);before=asset_state(client,plan)
    inputs=work/'private-inputs';need(not inputs.exists(),'publication_input_directory_exists');inputs.mkdir()
    for name in (RAW_DMG,plan['native_launch_evidence']['asset']):
        row=next(r for r in plan['assets'] if r['name']==name);client.download(row,inputs/name)
    need(asset_state(client,plan)==before,'raw_draft_changed_during_fetch')



def verify_publication(plan,base,work):
    need(sys.platform=='darwin','macos_required_for_independent_verification')
    need(not any(os.environ.get(k) for k in ('GH_TOKEN','GITHUB_TOKEN')),'token_must_not_enter_verification_step')
    public=work/'public';rows=regular_files(public)
    receipt=read_json(work/'packaging-receipt.json')
    need(receipt.get('source_commit')==SOURCE and receipt.get('run_id')==RUN and rows==receipt.get('assets')
         and all(public_name_allowed(r['path']) and r['bytes']<MAX_ASSET for r in rows),'curated_artifact_identity_mismatch')
    exact_public_assets(public);notice_rows={}
    for row in plan['notice_files']:
        notice_rows[row['destination']]={'path':row['destination'],'bytes':row['bytes'],'sha256':row['sha256']}
    for row in plan['captured_notices']:
        notice_rows[row['path']]={k:row[k] for k in ('path','bytes','sha256')}
    raw=work/'private-inputs'/RAW_DMG
    check_file(raw,next(r for r in plan['assets'] if r['name']==RAW_DMG))
    with mounted(raw,work/'verify-raw-mount') as mount:
        original=mount/'Polar Station.app';app_plist(original)
        run('/usr/bin/codesign','--verify','--deep','--strict',str(original));before=app_inventory(original)
    run('/usr/bin/hdiutil','verify',str(public/FINAL_DMG))
    with mounted(public/FINAL_DMG,work/'verify-final-mount') as mount:
        app=mount/'Polar Station.app';app_plist(app)
        need(app_inventory(app)==before,'independent_app_comparison_failed')
        run('/usr/bin/codesign','--verify','--deep','--strict',str(app));minima=macho_versions(app)
        for name in plan['documents']:
            need(digest(mount/name)==plan['documents'][name]['sha256'] and (mount/name).stat().st_size==plan['documents'][name]['bytes'],'independent_visible_document_mismatch')
        need(regular_files(mount/'THIRD-PARTY-NOTICES')==sorted(notice_rows.values(),key=lambda r:r['path']),
             'independent_visible_notices_mismatch')
        need((mount/'Applications').is_symlink() and os.readlink(mount/'Applications')=='/Applications','applications_link_mismatch')
        allowed=set(plan['documents'])|{'THIRD-PARTY-NOTICES','Polar Station.app','Applications'}
        need({p.name for p in mount.iterdir()}==allowed,'unexpected_or_missing_dmg_root_entry')
        for p in mount.rglob('*'):
            if p == app or app in p.parents:continue
            need(not xattrs_for(p),'unreviewed_non_app_extended_attribute')
    summary=read_json(work/'packaging-details.json')
    native_launch=verify_native_launch(plan,work/'private-inputs')
    expected_summary={'source_commit':SOURCE,'build_run_id':RUN,'original_dmg_sha256':digest(raw),
        'final_dmg_sha256':digest(public/FINAL_DMG),'application_content_and_xattrs_unchanged':True,
        'application_manifest_sha256':sha(json.dumps(before,sort_keys=True,separators=(',',':')).encode()),
        'strict_code_signature_verification':'passed','dmg_integrity':'passed','visible_notices_verified':True,
        'human_readable_copyright':'empty_or_absent','packaging_did_not_execute_application':True,
        'original_native_launch':native_launch,'mach_o_deployment_targets':minima,
        'signing':'Ad-hoc signed; not Developer ID signed; not notarized.',
        'tested_platform':'macOS 26 runner only. No lower-version or clean-machine compatibility claim.'}
    need(summary==expected_summary,'private_packaging_details_mismatch')
    need(regular_files(public)==rows,'public_artifacts_changed_during_independent_check')
    write_json(work/'independent-verification.json',{'source_commit':SOURCE,'run_id':RUN,'assets':rows,
        'verification':'static-inspection-without-app-execution'})


def stage_candidate(plan,work,client):
    need(plan.get('publication_authorized') is True,'publication_paused_for_user_direction')
    receipt=read_json(work/'packaging-receipt.json');public=work/'public'
    need(receipt.get('source_commit')==SOURCE and receipt.get('run_id')==RUN,'packaging_receipt_identity_mismatch')
    rows=regular_files(public)
    need(read_json(work/'independent-verification.json') == {'source_commit':SOURCE,'run_id':RUN,'assets':rows,
         'verification':'static-inspection-without-app-execution'},'static_verification_required')
    need(rows==receipt.get('assets') and all(public_name_allowed(r['path']) for r in rows),'public_output_changed_or_not_allowlisted')
    names={r['path'] for r in rows}
    exact_public_assets(public)
    check_run(client);asset_state(client,plan);verify_source_access(client,plan)
    # Never update/reuse the raw draft, or resume an uncertain prior attempt automatically.
    try:client.json('/repos/'+REPO+'/releases/tags/'+FINAL_TAG)
    except Stop as e:
        if str(e)!='github_not_found':raise
    else:raise Stop('final_release_already_exists_inspect_before_retry')
    if plan.get('tag_prepared_via_ssh') is True:
        # GitHub workflow tokens cannot create every workflow-bearing tag.
        # Owner SSH prepares this exact ref; no release or asset is reused.
        verify_tag(client)
    else:
        try:client.json('/repos/'+REPO+'/git/ref/tags/'+FINAL_TAG)
        except Stop as e:
            if str(e)!='github_not_found':raise
        else:raise Stop('final_tag_already_exists_inspect_before_retry')
        client.json('/repos/'+REPO+'/git/refs','POST',{'ref':'refs/tags/'+FINAL_TAG,'sha':SOURCE})
    verify_tag(client)
    body=release_body(plan)
    release=client.json('/repos/'+REPO+'/releases','POST',{'tag_name':FINAL_TAG,'target_commitish':SOURCE,
        'name':'Polar Station macOS arm64 trial','body':body,'draft':True,'prerelease':True,'generate_release_notes':False})
    rid=release.get('id')
    need(type(rid) is int and rid!=plan['raw_release_id'] and release.get('draft') is True
         and release.get('tag_name')==FINAL_TAG,'created_draft_identity_mismatch')
    get_created_release(client,rid)
    write_json(work/'publication-attempt.json',{'release_id':rid,'tag':FINAL_TAG,'state':'new_private_draft_created'})
    uploaded=[]
    for row in rows:
        result=client.upload(rid,public/row['path'])
        need(result.get('name')==row['path'] and result.get('size')==row['bytes'] and result.get('state')=='uploaded'
             and result.get('digest')=='sha256:'+row['sha256'],'uploaded_asset_digest_mismatch')
        uploaded.append({'name':row['path'],'id':result['id'],'bytes':row['bytes'],'sha256':row['sha256']})
        write_json(work/'publication-attempt.json',{'release_id':rid,'tag':FINAL_TAG,'state':'uploading_private_draft','assets':uploaded})
    remote=[];page=1
    while True:
        batch=client.json('/repos/'+REPO+'/releases/'+str(rid)+'/assets?per_page=100&page='+str(page));remote.extend(batch)
        if len(batch)<100:break
        page+=1;need(page<20,'public_asset_page_bound')
    need(len(remote)==len(rows) and {(r['name'],r['id'],r['size'],r.get('digest'),r.get('state')) for r in remote}==
         {(r['name'],r['id'],r['bytes'],'sha256:'+r['sha256'],'uploaded') for r in uploaded},'remote_public_asset_set_mismatch')
    ready=get_created_release(client,rid)
    need(ready.get('draft') is True and ready.get('tag_name')==FINAL_TAG and ready.get('target_commitish')==SOURCE
         and ready.get('prerelease') is True and ready.get('body')==body,'final_draft_changed_before_publication')
    asset_state(client,plan)
    verify_tag(client)
    write_json(work/'candidate-receipt.json',{'source_commit':SOURCE,'run_id':RUN,'release_id':rid,
        'tag':FINAL_TAG,'plan_canonical_sha256':sha(json.dumps(plan,sort_keys=True,separators=(',',':')).encode()),
        'assets':uploaded,'body_sha256':sha(body.encode())})
    print('Curated candidate retained in a new PRIVATE draft. No public release has occurred.')


def candidate_state(client,plan,candidate):
    need(candidate.get('source_commit')==SOURCE and candidate.get('run_id')==RUN
         and candidate.get('tag')==FINAL_TAG and type(candidate.get('release_id')) is int
         and candidate['release_id']!=plan['raw_release_id']
         and candidate.get('plan_canonical_sha256')==sha(json.dumps(plan,sort_keys=True,separators=(',',':')).encode()),
         'candidate_receipt_identity_mismatch')
    rid=candidate['release_id'];release=get_created_release(client,rid)
    need(release.get('id')==rid and release.get('draft') is True and release.get('published_at') is None
         and release.get('prerelease') is True and release.get('tag_name')==FINAL_TAG and release.get('target_commitish')==SOURCE
         and sha(release.get('body','').encode())==candidate.get('body_sha256'),'private_candidate_identity_mismatch')
    rows=[];page=1
    while True:
        batch=client.json('/repos/'+REPO+'/releases/'+str(rid)+'/assets?per_page=100&page='+str(page));rows.extend(batch)
        if len(batch)<100:break
        page+=1;need(page<20,'candidate_asset_page_bound')
    expected=candidate.get('assets',[])
    need(len(expected)==2 and {r.get('name') for r in expected}==PUBLIC_ASSETS and all(is_sha(r.get('sha256')) for r in expected),'invalid_candidate_asset_set')
    need(len(rows)==len(expected) and {(r['name'],r['id'],r['size'],r.get('digest'),r.get('state')) for r in rows}==
         {(r['name'],r['id'],r['bytes'],'sha256:'+r['sha256'],'uploaded') for r in expected},'candidate_assets_changed')
    verify_tag(client)
    return release



def verify_remote_public_assets(client,release_id,expected):
    need(len(expected)==2 and {r.get('name') for r in expected}==PUBLIC_ASSETS,'exact_two_published_assets_required')
    rows=client.json('/repos/'+REPO+'/releases/'+str(release_id)+'/assets?per_page=100&page=1')
    need(isinstance(rows,list) and len(rows)==2 and
         {(r.get('name'),r.get('id'),r.get('size'),r.get('digest'),r.get('state')) for r in rows}==
         {(r['name'],r['id'],r['bytes'],'sha256:'+r['sha256'],'uploaded') for r in expected},
         'published_asset_set_changed')


def fetch_public(plan,work,client):
    receipt=read_json(work/'publication-receipt.json')
    need(receipt.get('source_commit')==SOURCE and receipt.get('run_id')==RUN and type(receipt.get('release_id')) is int,
         'published_receipt_identity_mismatch')
    rid=receipt['release_id'];release=get_created_release(client,rid)
    need(release.get('draft') is False and release.get('published_at') and release.get('prerelease') is True
         and release.get('tag_name')==FINAL_TAG and release.get('target_commitish')==SOURCE
         and release.get('html_url')=='https://github.com/'+REPO+'/releases/tag/'+FINAL_TAG,
         'release_not_public_or_identity_mismatch')
    verify_tag(client)
    verify_remote_public_assets(client,rid,receipt.get('assets',[]))
    matches=[r for r in receipt.get('assets',[]) if r.get('name')==FINAL_DMG]
    need(len(matches)==1,'exact_public_dmg_identity_required');row=matches[0]
    row_check({**row,'path':row['name']});need(type(row.get('id')) is int and row['id']>0,'public_asset_id_required')
    actual=client.json('/repos/'+REPO+'/releases/assets/'+str(row['id']))
    need(actual.get('name')==FINAL_DMG and actual.get('state')=='uploaded' and actual.get('size')==row['bytes']
         and actual.get('digest')=='sha256:'+row['sha256'],'published_asset_identity_mismatch')
    target=work/'public-download';need(not target.exists(),'public_download_directory_exists');target.mkdir()
    client.download(row,target/FINAL_DMG)


def launch_public(plan,work):
    need(sys.platform=='darwin','macos_required_for_launch')
    need(not any(os.environ.get(k) for k in ('GH_TOKEN','GITHUB_TOKEN')),'token_must_not_enter_launch_step')
    receipt=read_json(work/'publication-receipt.json');row=next(r for r in receipt['assets'] if r['name']==FINAL_DMG)
    report={'source_commit':SOURCE,'run_id':RUN,'published_release_id':receipt['release_id'],
        'final_dmg_sha256':row['sha256'],'scope':'Post-publication public-download mount/copy/signature and 15-second launch only; not interactive GUI or clean-machine acceptance.',
        'release_remains_published':True}
    try:
        dmg=work/'public-download'/FINAL_DMG;check_file(dmg,row)
        run('/usr/bin/hdiutil','verify',str(dmg))
        with mounted(dmg,work/'public-launch-mount') as mount:
            original=mount/'Polar Station.app';app_plist(original);before=app_inventory(original)
            install=work/'installation';install.mkdir();app=install/'Polar Station.app'
            run('/usr/bin/ditto','--rsrc','--extattr',str(original),str(app))
            need(app_inventory(app)==before,'launch_install_copy_changed')
            run('/usr/bin/codesign','--verify','--deep','--strict',str(app))
            launch=launch_check(app,work/'launch-home')
            need(app_inventory(app)==before,'app_modified_itself_during_launch')
        report.update({'status':'passed','temporary_copy_signature':'passed','launch':launch})
    except Exception as e:
        report.update({'status':'failed','failure':str(e) if isinstance(e,Stop) else 'local_launch_or_io_failure'})
        write_json(work/'public-launch-verification.json',report)
        raise
    write_json(work/'public-launch-verification.json',report)


def publish(plan,work,client):
    need(plan.get('publication_authorized') is True,'publication_paused_for_user_direction')
    exact_public_assets(work/'public')
    candidate=read_json(work/'candidate-receipt.json');rid=candidate['release_id']
    rows=regular_files(work/'public')
    need(read_json(work/'independent-verification.json')=={'source_commit':SOURCE,'run_id':RUN,'assets':rows,
        'verification':'static-inspection-without-app-execution'},'fresh_static_verification_required')
    need(rows==sorted([{'path':r['name'],'bytes':r['bytes'],'sha256':r['sha256']} for r in candidate['assets']],key=lambda r:r['path']),
         'candidate_public_files_changed')
    verify_native_launch(plan,work/'private-inputs')
    check_run(client);asset_state(client,plan);candidate_state(client,plan,candidate);verify_tag(client);verify_source_access(client,plan)
    final=client.json('/repos/'+REPO+'/releases/'+str(rid),'PATCH',{'draft':False,'prerelease':True,'make_latest':'false'})
    need(final.get('id')==rid and final.get('draft') is False and final.get('prerelease') is True
         and final.get('published_at') and final.get('tag_name')==FINAL_TAG,'publication_outcome_requires_inspection')
    verify_tag(client)
    verify_remote_public_assets(client,rid,candidate['assets'])
    expected_url='https://github.com/'+REPO+'/releases/tag/'+FINAL_TAG
    need(final.get('html_url')==expected_url,'unexpected_release_url')
    write_json(work/'publication-receipt.json',{'release_id':rid,'url':expected_url,'assets':candidate['assets'],'source_commit':SOURCE,'run_id':RUN})
    print(expected_url)


def get_created_release(client,release_id):
    """Bounded GET-only visibility retry, always using the POST response ID.
    A release creation/upload/PATCH is never retried after an uncertain outcome.
    """
    delays=(1,2,4,8,12,15)
    for index,delay in enumerate(delays):
        try:
            result=client.json('/repos/'+REPO+'/releases/'+str(release_id))
            need(result.get('id')==release_id,'direct_release_id_mismatch')
            return result
        except Stop as e:
            retryable=str(e) in {'github_not_found','github_get_http_429','github_get_http_500',
                'github_get_http_502','github_get_http_503','github_get_http_504',
                'github_get_outcome_uncertain_no_automatic_retry'}
            if not retryable or index==len(delays)-1:raise
            time.sleep(delay)
    raise Stop('direct_release_visibility_timeout')


def verify_tag(client):
    tag=client.json('/repos/'+REPO+'/git/ref/tags/'+FINAL_TAG)
    need(tag.get('object',{}).get('type')=='commit' and tag.get('object',{}).get('sha')==SOURCE,'final_tag_target_mismatch')

def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('command',choices=('validate','fetch','package','stage-candidate','verify-publication','publish','fetch-public','launch-public'))
    p.add_argument('--plan',type=Path,required=True);p.add_argument('--review',type=Path,required=True);p.add_argument('--work',type=Path,required=True)
    args=p.parse_args();token=os.environ.pop('GH_TOKEN','')
    try:
        plan,_=check_gate(args.plan,args.review)
        if args.command=='validate':print('Exact-input plan and explicit review bindings validated. No network, packaging or publication performed.')
        elif args.command=='fetch':fetch(plan,args.plan.parent,args.work,GitHub(token))
        elif args.command=='package':need(not token,'token_must_not_enter_packaging_step');package(plan,args.plan.parent,args.work)
        elif args.command=='stage-candidate':stage_candidate(plan,args.work,GitHub(token))
        elif args.command=='fetch-public':fetch_public(plan,args.work,ReadOnlyGitHub(token))
        elif args.command=='launch-public':need(not token,'token_must_not_enter_launch_step');launch_public(plan,args.work)
        elif args.command=='verify-publication':need(not token,'token_must_not_enter_verification_step');verify_publication(plan,args.plan.parent,args.work)
        else:publish(plan,args.work,GitHub(token))
    except Exception as e:
        # Do not print tokens, signed CDN URLs, source contents or raw service error bodies.
        print('Trial finalization stopped: '+(str(e) if isinstance(e,Stop) else 'parse_io_or_transport_error'),file=sys.stderr)
        return 1
    return 0

if __name__=='__main__':sys.exit(main())

#!/usr/bin/env python3
"""Read-only review of one exact private draft. Python 3.11+, standard library only.
Never extracts/executes archive members or writes GitHub state. Public output is
one explicit allowlisted JSON report, not source archives, binaries, or logs.
"""
import argparse
import base64
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import stat
import sys
import tarfile
import tempfile
import tomllib
import urllib.error
import urllib.parse
import urllib.request
import zipfile

REPO = 'lionantbear/polar-station'
PINS = {'ghostty': '332b2aefc6e72d363aa93ab6ecfc86eeeeb5ed28',
        'godot': 'ed1daf0bf001b61586d9930840f2f1394092c079'}
GROUPS = ('application-source', 'cargo-compiled-sources', 'zig-package-sources', 'homebrew-installed-metadata')
FIXED = {'draft-build-index.json', 'SHA256SUMS', 'trial-inputs-manifest.json',
         'ghostty-built-source.tar.gz', 'godot-built-source.tar.gz',
         'build-evidence.zip', 'Polar-Station-macOS-arm64-UNNOTARIZED.dmg'}
MAX_ASSET = 2_000_000_000
MAX_TOTAL = 18_000_000_000
MAX_RAW = 4_000_000_000
MAX_ALL_RAW = 25_000_000_000
MAX_MEMBERS = 600_000
MAX_TEXT = 2_000_000
MAX_JSON = 100_000_000
MAX_REPORT = 50_000_000
MAX_LEGAL_TOTAL = 20_000_000
LEGAL = ('license', 'licence', 'copying', 'copyright', 'notice', 'authors', 'patents')
SECRET = re.compile(r'(?i)(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|AKIA[0-9A-Z]{16}|-----BEGIN (?:[A-Z ]*PRIVATE KEY)-----|(?:password|authorization|access_token|secret_access_key)\s*[=:]\s*[\"\']?[^\s\"\']{12,})')
TOKEN = ''


class ReviewError(Exception):
    pass


def require(ok, code):
    if not ok:
        raise ReviewError(code)


def sha(data):
    return hashlib.sha256(data).hexdigest()


def digest_file(path):
    h = hashlib.sha256()
    with path.open('rb') as f:
        for data in iter(lambda: f.read(1024 * 1024), b''):
            h.update(data)
    return h.hexdigest()


def unique_json(data):
    def pairs(rows):
        obj = {}
        for key, value in rows:
            require(key not in obj, 'duplicate_json_key')
            obj[key] = value
        return obj
    require(len(data) <= MAX_JSON, 'json_too_large')
    return json.loads(data, object_pairs_hook=pairs)


def safe_path(name):
    require(isinstance(name, str) and 0 < len(name) <= 2048, 'invalid_archive_path')
    require(not any(ord(c) < 32 or ord(c) == 127 for c in name), 'invalid_archive_path')
    require('\\' not in name and ':' not in name and not name.startswith('/'), 'unsafe_archive_path')
    parts = name.rstrip('/').split('/')
    require(all(p and p not in ('.', '..') for p in parts), 'unsafe_archive_path')
    return '/'.join(parts)


def safe_link(name, target):
    require(isinstance(target, str) and target and not target.startswith('/') and '\\' not in target
            and ':' not in target and not any(ord(c) < 32 for c in target), 'unsafe_symlink')
    parts = name.split('/')[:-1]
    for p in target.split('/'):
        if p == '..':
            require(parts, 'escaping_symlink')
            parts.pop()
        elif p not in ('', '.'):
            parts.append(p)
    require(parts, 'escaping_symlink')
    # Capture roots must not cross component/package boundaries by escaping root.
    return '/'.join(parts)


def allowed_asset(name):
    return name in FIXED or any(re.fullmatch(re.escape(g) + r'-[0-9]{3}\.tar\.gz', name) for g in GROUPS)


def check_plan(plan):
    require(isinstance(plan,dict) and plan.get('repository') == REPO, 'plan_repository_mismatch')
    source = plan.get('source_commit'); run = plan.get('run_id')
    require(isinstance(source,str) and re.fullmatch('[0-9a-f]{40}',source) is not None,
            'plan_full_source_commit_required')
    require(type(run) is int and 0 < run < 2**63, 'plan_positive_run_id_required')
    require(plan.get('tag') == f'macos-arm64-trial-build-{run}-1', 'plan_run_tag_mismatch')
    require(type(plan.get('release_id')) is int and plan['release_id'] > 0, 'exact_release_id_required')
    assets = plan.get('asset_ids', {})
    require(isinstance(assets, dict) and FIXED <= assets.keys(), 'exact_asset_ids_required')
    require(all(allowed_asset(k) and type(v) is int and v > 0 for k, v in assets.items()), 'invalid_plan_asset')
    require(len(set(assets.values())) == len(assets), 'duplicate_asset_id')
    origins = plan.get('legal_text_origins', [])
    require(isinstance(origins, list) and len(origins) <= 200 and all(isinstance(x,str) and
            safe_path(x) == x and PurePosixPath(x).name.lower().startswith(LEGAL[:5]) for x in origins),
            'invalid_legal_text_allowlist')


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


class GitHub:
    def __init__(self, token):
        require(bool(token), 'github_token_required')
        self.token = token
        self.opener = urllib.request.build_opener(NoRedirect)

    def request(self, path, binary=False):
        require(path.startswith('/repos/' + REPO + '/'), 'unexpected_api_target')
        req = urllib.request.Request('https://api.github.com' + path, method='GET', headers={
            'Authorization': 'Bearer ' + self.token, 'Accept': 'application/octet-stream' if binary else 'application/vnd.github+json',
            'X-GitHub-Api-Version': '2022-11-28', 'User-Agent': 'polar-station-private-draft-review'})
        try:
            return self.opener.open(req, timeout=120)
        except urllib.error.HTTPError as e:
            if binary and e.code in (301, 302, 303, 307, 308):
                # Never forward authorization to storage/CDN, and never log signed URLs.
                url = e.headers.get('Location', '')
                parsed = urllib.parse.urlsplit(url)
                require(parsed.scheme == 'https' and parsed.hostname in
                        {'release-assets.githubusercontent.com', 'objects.githubusercontent.com'}
                        and not parsed.username and not parsed.password and parsed.port in (None, 443), 'unexpected_asset_redirect')
                try:
                    return self.opener.open(urllib.request.Request(url, method='GET', headers={
                        'User-Agent': 'polar-station-private-draft-review'}), timeout=120)
                except Exception:
                    raise ReviewError('asset_storage_get_failed') from None
            raise ReviewError('github_get_http_' + str(e.code)) from None
        except Exception:
            raise ReviewError('github_get_failed') from None

    def get(self, path):
        with self.request(path) as response:
            data = response.read(MAX_JSON + 1)
        return unique_json(data)

    def download(self, asset_id, output, size):
        require(0 <= size < MAX_ASSET, 'asset_size_bound')
        h = hashlib.sha256(); count = 0
        with self.request('/repos/' + REPO + '/releases/assets/' + str(asset_id), True) as response, output.open('xb') as out:
            while True:
                data = response.read(1024 * 1024)
                if not data:
                    break
                count += len(data)
                require(count <= size and count < MAX_ASSET, 'asset_download_size_mismatch')
                h.update(data); out.write(data)
        require(count == size, 'asset_download_size_mismatch')
        return h.hexdigest()


def release_assets(release, plan):
    require(release.get('id') == plan['release_id'] and release.get('draft') is True
            and release.get('prerelease') is True and release.get('tag_name') == plan['tag']
            and release.get('target_commitish') == plan['source_commit'] and release.get('published_at') is None,
            'release_identity_or_draft_mismatch')
    rows = release.get('assets', [])
    result = {}
    for row in rows:
        name = row.get('name')
        require(isinstance(name, str) and allowed_asset(name) and name not in result, 'unexpected_or_duplicate_asset')
        require(row.get('state') == 'uploaded' and row.get('id') == plan['asset_ids'].get(name), 'asset_id_or_state_mismatch')
        require(type(row.get('size')) is int and 0 < row['size'] < MAX_ASSET, 'asset_size_bound')
        result[name] = row
    require(set(result) == set(plan['asset_ids']), 'asset_set_mismatch')
    require(sum(r['size'] for r in rows) <= MAX_TOTAL, 'asset_total_bound')
    return result


def index_entries(index, sums, assets, source_commit):
    require(index.get('source_commit') == source_commit and index.get('draft_retention_only') is True
            and index.get('public_release_ready') is False, 'index_source_identity_mismatch')
    entries = {}
    for row in index.get('assets', []):
        name = row.get('name')
        require(name in assets and name not in entries and name not in {'SHA256SUMS', 'draft-build-index.json'}, 'unexpected_index_asset')
        require(row.get('bytes') == assets[name]['size'] and re.fullmatch('[0-9a-f]{64}', row.get('sha256', '')), 'index_size_or_hash_invalid')
        entries[name] = row
    require(set(entries) == set(assets) - {'SHA256SUMS', 'draft-build-index.json'}, 'index_asset_set_mismatch')
    parsed = {}
    for line in sums.decode('ascii').splitlines():
        match = re.fullmatch(r'([0-9a-f]{64})  ([A-Za-z0-9_.-]+)', line)
        require(match is not None and match[2] not in parsed, 'invalid_checksum_manifest')
        parsed[match[2]] = match[1]
    require(parsed == {k: v['sha256'] for k, v in entries.items()}, 'checksum_manifest_mismatch')
    return entries


def third_party_asset(asset):
    return asset in {'ghostty-built-source.tar.gz','godot-built-source.tar.gz'} or asset.startswith(
        ('cargo-compiled-sources-','zig-package-sources-'))


def legal_basename(name):
    return PurePosixPath(name).name.lower().startswith(LEGAL)


def legal_path_candidate(name):
    p = PurePosixPath(name)
    return legal_basename(name) or any(x.lower() in {'license','licenses','licence','licences','legal','copyrights'}
        for x in p.parts[:-1]) or p.name.lower() in {'unlicense','ofl','ofl.txt'}


NONLEGAL_SUFFIXES = {'.py','.pyc','.rs','.c','.cc','.cpp','.h','.hpp','.m','.mm','.swift','.zig','.go',
                    '.sh','.bash','.js','.ts','.tsx','.jsx','.log','.dmg','.exe','.dll','.so','.dylib',
                    '.o','.a','.pdf','.png','.jpg','.jpeg','.gif','.ico','.ttf','.woff','.woff2'}


class LegalTexts:
    def __init__(self, limit=MAX_LEGAL_TOTAL):
        self.limit = limit; self.total = 0; self.records = {}; self.gaps = []

    def add(self, origin, row, data):
        reason = None
        if row.get('kind') != 'file': reason = 'not_a_regular_file; symlink_not_followed'
        elif not legal_basename(origin.get('source_origin',origin['member'])): reason = 'legal_directory_or_name_outside_LEGAL_prefix_allowlist'
        elif PurePosixPath(origin.get('source_origin',origin['member'])).suffix.lower() in NONLEGAL_SUFFIXES:
            reason = 'legal_prefix_but_source_log_or_binary_suffix'
        elif row['bytes'] > MAX_TEXT: reason = 'legal_file_exceeds_2MB_limit'
        elif data is None: reason = 'legal_file_bytes_not_retained'
        if reason:
            self.gaps.append({**origin, **project(row,('bytes','sha256')), 'reason':reason}); return
        require(len(data) == row['bytes'] and sha(data) == row['sha256'],'legal_text_hash_mismatch')
        unicode_bom_text = None
        if data.startswith((b'\xff\xfe\0\0',b'\0\0\xfe\xff')):
            try: unicode_bom_text = data.decode('utf-32')
            except UnicodeDecodeError: pass
        elif data.startswith((b'\xff\xfe',b'\xfe\xff')):
            try: unicode_bom_text = data.decode('utf-16')
            except UnicodeDecodeError: pass
        require(not SECRET.search(data.decode('latin-1')) and not SECRET.search(unicode_bom_text or '')
                and (not TOKEN or (TOKEN.encode() not in data and TOKEN not in (unicode_bom_text or ''))),
                'possible_secret_in_allowlisted_legal_text')
        if (b'\0' in data and unicode_bom_text is None) or data[:4] in (b'\x7fELF',b'\xcf\xfa\xed\xfe',b'\xfe\xed\xfa\xcf',b'\xca\xfe\xba\xbe'):
            self.gaps.append({**origin, **project(row,('bytes','sha256')), 'reason':'legal_prefix_but_binary_content'}); return
        digest = row['sha256']
        if digest in self.records:
            if origin not in self.records[digest]['origins']: self.records[digest]['origins'].append(origin)
            return
        if self.total + len(data) > self.limit:
            self.gaps.append({**origin, **project(row,('bytes','sha256')), 'reason':'unique_legal_texts_exceed_20MB_aggregate_limit'}); return
        record = {'sha256':digest,'bytes':len(data),'origins':[origin]}
        try:
            record.update(encoding='utf-8',text=data.decode('utf-8'))
        except UnicodeDecodeError:
            record.update(encoding='base64',base64=base64.b64encode(data).decode('ascii'),
                          character_encoding='unknown legacy encoding; exact original bytes')
        self.total += len(data); self.records[digest] = record


SOURCE_POLICY = 'captured-source-projection-v1'
SOURCE_RE = re.compile(r'(?:application-source|cargo-compiled-sources|zig-package-sources|homebrew-installed-metadata)-[0-9]{3}\.tar\.gz|(?:ghostty|godot)-built-source\.tar\.gz')

def source_group(name):
    require(SOURCE_RE.fullmatch(name), 'invalid_source_archive_name')
    return re.sub(r'-[0-9]{3}\.tar\.gz$', '', name)


def source_disposition(asset, path, kind):
    """Only exact capture-owned audit records may be omitted. Unknowns fail closed."""
    safe_path(path)
    require(not {'.git', '.hg', '.svn'} & set(path.split('/')), 'source_contains_development_history')
    group = source_group(asset)
    if group == 'cargo-compiled-sources':
        if re.fullmatch(r'build-records/(?:cargo-[A-Za-z0-9_.-]+\.jsonl|cargo-metadata\.json)', path):
            require(kind == 'file', 'audit_record_must_be_regular')
            return 'exclude-cargo-build-record'
        if path == 'build-records' and kind == 'directory':
            return 'exclude-cargo-build-record'
        require(re.fullmatch(r'cargo(?:/package-root-[0-9]{4}(?:/.+)?)?', path), 'unexpected_cargo_capture_member')
    elif group == 'homebrew-installed-metadata':
        if path == 'graphics/provenance.json':
            require(kind == 'file', 'audit_record_must_be_regular')
            return 'exclude-graphics-audit-record'
        if path == 'graphics' and kind == 'directory':
            return 'exclude-graphics-audit-record'
        require(path == 'homebrew/homebrew-resolved-info.json' or
             re.fullmatch(r'homebrew/[A-Za-z0-9@+_.-]+/[^/]+/(?:INSTALL_RECEIPT\.json|sbom\.spdx\.json|\.brew/[^/]+\.rb)', path) or
             (kind == 'directory' and re.fullmatch(r'homebrew(?:/[A-Za-z0-9@+_.-]+(?:/[^/]+(?:/\.brew)?)?)?', path)),
             'unexpected_homebrew_capture_member')
    elif group == 'application-source':
        require(path.startswith('application/') or (path == 'application' and kind == 'directory'), 'unexpected_application_capture_member')
    elif group == 'zig-package-sources':
        require(re.fullmatch(r'zig/(?:global|local)/[^/]+/.+', path) or
             (kind == 'directory' and re.fullmatch(r'zig(?:/(?:global|local)(?:/[^/]+)?)?', path)), 'unexpected_zig_capture_member')
    else:
        component = 'ghostty' if asset == 'ghostty-built-source.tar.gz' else 'godot'
        require(path.startswith(component+'/') or path.startswith('build-inputs/') or path == 'capture-manifest.json' or
             (kind == 'directory' and path in (component, 'build-inputs')), 'unexpected_native_capture_member')
        if path == 'capture-manifest.json':require(kind == 'file', 'native_capture_manifest_not_regular')
    return 'preserve'


def source_projection_report(scans, entries):
    """Small digest-only recipe, derived from ALL already-hashed source members.
    No compiler log, cargo metadata, graphics audit contents or full file listing
    is added to the report. Modes/targets/bytes are bound in canonical digests.
    """
    archives=[]
    for name,scan in sorted(scans.items()):
        if not SOURCE_RE.fullmatch(name):continue
        rows=[]
        for path,record in sorted(scan['files'].items()):
            kind=record['kind'];mode=record['mode']
            require(kind in ('file','directory','symlink') and type(mode) is int and 0<=mode<=0o777,
                    'unsupported_source_projection_member')
            row={'path':path,'type':kind,'mode':mode}
            if kind=='file':row.update(bytes=record['bytes'],sha256=record['sha256'])
            elif kind=='symlink':row['target']=record['symlink_target']
            row['disposition']=source_disposition(name,path,kind);rows.append(row)
        kept=[r for r in rows if r['disposition']=='preserve']
        def inventory_digest(selected):
            value=[{k:v for k,v in r.items() if k!='disposition'} for r in selected]
            return sha(json.dumps(value,sort_keys=True,separators=(',',':'),ensure_ascii=True).encode())
        require(scan['sha256']==entries[name]['sha256'],'projection_archive_identity_mismatch')
        archives.append({'name':name,'bytes':entries[name]['bytes'],'sha256':scan['sha256'],
            'inventory_sha256':inventory_digest(rows),'members_count':len(rows),
            'preserved_inventory_sha256':inventory_digest(kept),'preserved_count':len(kept),
            'expanded_bytes':sum(r.get('bytes',0) for r in rows),'preserved_bytes':sum(r.get('bytes',0) for r in kept),
            'excluded':[r for r in rows if r['disposition']!='preserve']})
    return {'policy':SOURCE_POLICY,'archives':archives}

def retain_member(name, size):
    basename = PurePosixPath(name).name
    return size <= MAX_JSON and (basename in {'capture-manifest.json', 'cargo-metadata.json', '.cargo-checksum.json',
        'Package.resolved', 'Cargo.lock', 'cargo-resolution.json', 'cargo-compiled-artifacts.json',
        'provenance.json', 'macos-bom.json', 'release-gate.json', 'installation-launch.json'}
        or bool(re.fullmatch(r'homebrew/[^/]+/[^/]+/(?:INSTALL_RECEIPT\.json|sbom\.spdx\.json|\.brew/[^/]+\.rb)', name))
        or name == 'homebrew/homebrew-resolved-info.json'
        or bool(re.fullmatch(r'station-evidence/build-provenance/texts/[0-9a-f]{64}\.txt', name)))


def inspect_archive(path):
    """Hashes streams only. No extraction and no code/subprocess execution."""
    files = {}; captured = {}; raw = 0; count = 0
    def member(name, kind, size, stream=None, target=None, executable=False, mode=0o644):
        nonlocal raw, count
        name = safe_path(name); count += 1; raw += size
        require(count <= MAX_MEMBERS and raw <= MAX_RAW and 0 <= size <= MAX_RAW, 'archive_resource_bound')
        require(name not in files, 'duplicate_archive_member')
        row = {'path': name, 'kind': kind, 'mode': mode}
        if kind == 'symlink':
            safe_link(name, target)
            row['symlink_target'] = target
            row['git_blob'] = hashlib.sha1(b'blob ' + str(len(target.encode())).encode() + b'\0' + target.encode()).hexdigest()
        elif kind == 'file':
            h = hashlib.sha256(); git = hashlib.sha1(b'blob ' + str(size).encode() + b'\0')
            keep = retain_member(name, size) or (third_party_asset(path.name) and legal_basename(name) and size <= MAX_TEXT)
            data = bytearray(); read = 0
            for block in iter(lambda: stream.read(1024 * 1024), b''):
                read += len(block); require(read <= size, 'archive_member_size_mismatch')
                h.update(block); git.update(block)
                if keep: data.extend(block)
            require(read == size, 'archive_member_size_mismatch')
            row.update(bytes=size, sha256=h.hexdigest(), git_blob=git.hexdigest(), executable=executable)
            if keep: captured[name] = bytes(data)
        else:
            require(kind == 'directory', 'unsupported_archive_member')
        files[name] = row
    if path.name.endswith('.tar.gz'):
        with tarfile.open(path, 'r|gz') as archive:
            for item in archive:
                require(not item.islnk() and not item.issparse(), 'unsupported_archive_member')
                kind = 'file' if item.isfile() else 'directory' if item.isdir() else 'symlink' if item.issym() else 'other'
                require(kind != 'other', 'unsupported_archive_member')
                if item.isfile():
                    with archive.extractfile(item) as stream:
                        member(item.name, kind, item.size, stream, executable=bool(item.mode & 0o111), mode=item.mode)
                else:
                    member(item.name, kind, 0, target=item.linkname if item.issym() else None, mode=item.mode)
    else:
        with zipfile.ZipFile(path) as archive:
            require(len(archive.infolist()) <= MAX_MEMBERS, 'archive_resource_bound')
            for item in archive.infolist():
                mode = item.external_attr >> 16
                require(not item.flag_bits & 1, 'encrypted_zip')
                require(not stat.S_ISLNK(mode) and (stat.S_IFMT(mode) in (0, stat.S_IFREG, stat.S_IFDIR)), 'unsupported_zip_member')
                with archive.open(item) as stream:
                    member(item.filename, 'directory' if item.is_dir() else 'file', item.file_size, stream,
                           executable=bool(mode & 0o111), mode=stat.S_IMODE(mode))
    validate_links(files)
    canonical = [{k: r[k] for k in ('path', 'kind', 'bytes', 'sha256', 'symlink_target', 'executable') if k in r}
                 for _, r in sorted(files.items()) if r['kind'] != 'directory']
    return {'files': files, 'captured': captured, 'sha256': digest_file(path),
            'content_manifest_sha256': sha(json.dumps(canonical, sort_keys=True, separators=(',', ':')).encode()),
            'file_count': len(canonical), 'uncompressed_bytes': raw}


def validate_links(files):
    # Reject files under symlink parents. Resolve complete symlink chains, detect loops.
    for name, row in files.items():
        parts = name.split('/')
        require(not any(files.get('/'.join(parts[:i]), {}).get('kind') == 'symlink' for i in range(1, len(parts))), 'symlink_parent_member')
        if row['kind'] == 'symlink':
            current = safe_link(name, row['symlink_target']); seen = {name}
            for _ in range(64):
                require(current not in seen, 'symlink_cycle'); seen.add(current)
                parts2 = current.split('/'); found = False
                for i in range(1, len(parts2) + 1):
                    prefix = '/'.join(parts2[:i]); target = files.get(prefix, {})
                    if target.get('kind') == 'symlink':
                        current = safe_link(prefix, target['symlink_target']) + ('/' + '/'.join(parts2[i:]) if i < len(parts2) else '')
                        found = True; break
                if not found: break
            else: raise ReviewError('symlink_chain_bound')


def verify_captured(manifest, scans, entries, source_commit):
    require(manifest.get('schema_version') == 1 and manifest.get('source_commit') == source_commit, 'capture_source_identity_mismatch')
    require(set(manifest.get('groups', {})) == set(GROUPS), 'capture_group_set_mismatch')
    covered = set()
    for label in GROUPS:
        group = manifest['groups'][label]; all_files = {}; part_files = {}
        require(group.get('archives'), 'empty_capture_group')
        for number, row in enumerate(group['archives'], 1):
            name = f'{label}-{number:03d}.tar.gz'
            require(row.get('path') == name and name in entries and row.get('sha256') == entries[name]['sha256']
                    and row.get('bytes') == entries[name]['bytes'], 'capture_archive_identity_mismatch')
            covered.add(name); part_files[number] = scans[name]['files']
            for path, f in scans[name]['files'].items():
                require(path not in all_files, 'duplicate_multipart_path')
                all_files[path] = f
        validate_links(all_files)
        manifest_files = set()
        for row in group.get('files', []):
            name = safe_path(row['path']); require(name not in manifest_files and name in all_files, 'capture_file_set_mismatch')
            manifest_files.add(name); f = all_files[name]
            if 'symlink_target' in row:
                require(f['kind'] == 'symlink' and row['symlink_target'] == f['symlink_target'], 'capture_symlink_mismatch')
            else:
                require(name in part_files.get(row.get('archive_part'), {}) and f['kind'] == 'file'
                        and row.get('sha256') == f['sha256'] and row.get('bytes') == f['bytes'], 'capture_file_hash_mismatch')
        require(manifest_files == {p for p, r in all_files.items() if r['kind'] != 'directory'}, 'capture_file_set_mismatch')
    require(covered == {n for n in entries if any(n.startswith(g + '-') for g in GROUPS)}, 'capture_archive_set_mismatch')


def native_report(component, scan, source_commit):
    data = scan['captured'].get('capture-manifest.json'); require(data is not None, 'native_manifest_missing')
    m = unique_json(data)
    require(m.get('schema_version') == 1 and m.get('component') == component and m.get('application_commit') == source_commit
            and m.get('upstream_commit') == PINS[component], 'native_source_identity_mismatch')
    expected = {'capture-manifest.json'}; changes = []; deleted = []; seen = set()
    for r in m.get('files', []):
        path = safe_path(r['path']); full = component + '/' + path
        require(path not in seen, 'duplicate_native_manifest_path'); seen.add(path)
        if r.get('deleted'):
            require(full not in scan['files'], 'native_deleted_file_present'); deleted.append(path); continue
        expected.add(full); f = scan['files'].get(full, {})
        if 'symlink_target' in r:
            require(f.get('kind') == 'symlink' and f.get('symlink_target') == r['symlink_target'], 'native_symlink_mismatch')
        else:
            require(f.get('kind') == 'file' and f.get('sha256') == r.get('sha256') and f.get('bytes') == r.get('bytes'), 'native_file_hash_mismatch')
        actual_mode = '120000' if f['kind'] == 'symlink' else '100755' if f.get('executable') else '100644'
        if r.get('generated_or_added') or r.get('git_blob') != f.get('git_blob') or r.get('git_mode') != actual_mode:
            changes.append({k: v for k, v in {'path':path, 'generated_or_added':bool(r.get('generated_or_added')),
                'baseline_git_blob':r.get('git_blob'), 'actual_git_blob':f.get('git_blob'), 'sha256':f.get('sha256'),
                'baseline_git_mode':r.get('git_mode'), 'actual_git_mode':actual_mode,
                'symlink_target':f.get('symlink_target')}.items() if v is not None})
    script = 'build-inputs/build_macos.sh' if component == 'ghostty' else 'build-inputs/build_macos_graphics.sh'
    expected.add(script)
    require(scan['files'].get(script, {}).get('sha256') == m.get('build_script_sha256'), 'native_build_script_mismatch')
    require(expected == {n for n, r in scan['files'].items() if r['kind'] != 'directory'}, 'native_file_set_mismatch')
    return {'component':component, 'upstream_commit':PINS[component], 'application_commit':source_commit,
            'capture_manifest_sha256':sha(data), 'build_script_sha256':m['build_script_sha256'],
            'actual_modified_or_added_files':changes, 'deleted_files':deleted,
            'tracked_file_count':len(seen), 'source_archive_sha256':scan['sha256'],
            'source_content_manifest_sha256':scan['content_manifest_sha256'],
            'scope':'Actual source bytes verified against capture records; upstream baseline blob IDs are collector assertions, not independently fetched upstream trees.'}


def project(row, fields):
    return {k: row[k] for k in fields if k in row}


def public_tree_report(tree, scans, source_commit):
    require(tree.get('truncated') is False, 'source_tree_truncated')
    baseline = {r['path']: r for r in tree.get('tree', []) if r.get('type') == 'blob'}
    actual = {}
    for name, scan in scans.items():
        if name.startswith('application-source-'):
            for path, f in scan['files'].items():
                require(path.startswith('application/'), 'unexpected_application_prefix')
                if f['kind'] == 'directory': continue
                path = path[len('application/'):]; ref = baseline.get(path, {})
                mode = '120000' if f['kind'] == 'symlink' else '100755' if f['executable'] else '100644'
                require(ref.get('sha') == f['git_blob'] and ref.get('mode') == mode, 'application_git_tree_mismatch')
                actual[path] = f
    return {'source_commit': source_commit, 'git_tree_sha': tree.get('sha'), 'captured_files_match_commit':True,
            'captured_file_count':len(actual), 'uncaptured_tracked_paths':sorted(set(baseline) - set(actual)),
            'content_manifest_sha256':sha(json.dumps([{k:r[k] for k in ('path','sha256','git_blob','symlink_target') if k in r}
                for _, r in sorted(actual.items())],sort_keys=True,separators=(',', ':')).encode())}



def sanitized_gap(value):
    """Do not publish captured stderr/free-text diagnostics. Keep structured facts."""
    known = {
        'excluded path; not captured', 'symlinked parent excluded', 'escaping symlink excluded',
        'missing or nonregular file', 'individual file exceeds part limit', 'native build output excluded',
        'GTS-related source content excluded', 'excluded directory', 'copied library outside expected Cellar',
        'global cache p/ missing', 'local cache p/ missing', 'non-directory package entry excluded',
        'No compiler artifact package IDs', 'Locked offline metadata unavailable', 'Git workspace root unavailable',
        'Git source tracked file enumeration failed', 'compiler package absent from metadata',
        'No packaged keg associations found', 'Resolved brew info unavailable', 'Installed .brew recipe missing',
        'No upstream source/patch/bottle download in this collector; selected bottle identity needs receipt/API reconciliation',
        'Homebrew installed metadata unavailable', 'Sparkle source checkout evidence not located',
        'No actual Cargo compiler artifact messages collected', 'Graphics copy/transformation receipt missing',
        'Build provenance missing', 'Inventory missing'}
    if isinstance(value,dict):
        result = project(value,('scope','path','name','formula','package','bytes'))
        reason = value.get('reason','')
        result['reason'] = reason if reason in known else 'unrecognized diagnostic omitted'
        if reason not in known: result['diagnostic_sha256'] = sha(str(reason).encode())
        return result
    if isinstance(value,str) and value in known: return {'reason':value}
    return {'reason':'free-text diagnostic omitted', 'diagnostic_sha256':sha(str(value).encode())}


def build_report(plan, assets, entries, manifest, scans, tree):
    legal_texts = LegalTexts()
    all_files = {}; captured = {}; license_rows = []; package_manifests = []
    for asset, scan in scans.items():
        for path, row in scan['files'].items():
            all_files[(asset, path)] = row
            base = PurePosixPath(path).name
            if third_party_asset(asset) and legal_path_candidate(path):
                legal_texts.add({'asset':asset,'member':path},row,scan['captured'].get(path))
            if row['kind'] == 'file' and base.lower().startswith(LEGAL):
                license_rows.append({'asset':asset, **project(row, ('path','bytes','sha256'))})
            if row['kind'] == 'file' and base in {'Cargo.toml','Cargo.lock','build.zig.zon','Package.swift','Package.resolved','.cargo-checksum.json','.cargo_vcs_info.json'}:
                package_manifests.append({'asset':asset, **project(row, ('path','bytes','sha256'))})
        captured.update({(asset,path): data for path,data in scan['captured'].items()})
    report = {'schema_version':1, 'inspection_status':'passed_integrity_checks', 'release_ready':False,
        'repository':REPO, 'source_commit':plan['source_commit'], 'build_run_id':plan['run_id'], 'build_run_attempt':1, 'draft_tag':plan['tag'], 'release_id':plan['release_id'],
        'scope':'GET-only private draft inspection. No extraction/execution, release edits, or binary/source-archive publication. License clearance remains pending.',
        'compatibility':'Only macOS 26 runner launch was tested. No macOS 13 or clean-Mac compatibility claim.',
        'assets':[{'name':n, 'id':assets[n]['id'], 'bytes':e['bytes'], 'sha256':e['sha256']} for n,e in sorted(entries.items())],
        'control_asset_digests':[],
        'source_projection':source_projection_report(scans,entries),
        'archives':[{'name':n, **project(s, ('sha256','file_count','uncompressed_bytes','content_manifest_sha256'))} for n,s in sorted(scans.items())],
        'application_source':public_tree_report(tree, scans, plan['source_commit']),
        'native_sources':[native_report(c, scans[c+'-built-source.tar.gz'], plan['source_commit']) for c in PINS],
        'capture_gaps':[sanitized_gap(g) for g in manifest.get('gaps',[])],
        'capture_incomplete':manifest.get('incomplete') is True,
        'license_files':license_rows, 'package_manifests':package_manifests,
        'copyright_notice_candidates':[{'asset':a, **project(row,('path','bytes','sha256'))} for (a,p),row in all_files.items()
            if a in {'ghostty-built-source.tar.gz','godot-built-source.tar.gz'} and row['kind'] == 'file'
            and (PurePosixPath(p).name.lower().startswith(('about','credits')) or PurePosixPath(p).name == 'Info.plist')],
        'cargo_packages':manifest['groups']['cargo-compiled-sources'].get('packages',[]),
        'zig_package_keys':manifest['groups']['zig-package-sources'].get('package_keys',[]),
        'homebrew_packaged_kegs':manifest['groups']['homebrew-installed-metadata'].get('packaged_kegs',[]),
        'homebrew_actual_inputs':[], 'homebrew_resolved_info':[], 'sparkle_pins':[], 'required_files':[], 'gaps':[
            'Current collector excludes GTS upstream source. User authorization does not supply missing source bytes.',
            'Homebrew upstream source/patch archives are not captured by this build; metadata alone is not fulfillment.',
            'Compiled package lists are evidence candidates, not proof of final static linkage.',
            'DMG is checksum verified but never mounted, extracted, executed or independently inspected by this Linux review.',
            'App notice installation and final signature binding still require an artifact-specific review.']}
    public_blobs = {row['path']:row.get('sha') for row in tree.get('tree',[]) if row.get('type') == 'blob'}
    for component in PINS:
        name = 'build_macos.sh' if component == 'ghostty' else 'build_macos_graphics.sh'
        actual = scans[component+'-built-source.tar.gz']['files']['build-inputs/'+name]['git_blob']
        require(actual == public_blobs.get('automation/polar_station/'+name),'native_build_script_public_source_mismatch')
    # Strict per-path raw allowlist: actual installed recipe + receipt + SBOM only.
    for (asset,path), data in captured.items():
        if re.fullmatch(r'homebrew/[^/]+/[^/]+/(?:INSTALL_RECEIPT\.json|sbom\.spdx\.json|\.brew/[^/]+\.rb)',path):
            require(len(data) <= MAX_TEXT, 'allowlisted_text_too_large')
            text = data.decode('utf-8'); require('\0' not in text, 'allowlisted_text_not_utf8')
            report['homebrew_actual_inputs'].append({'asset':asset,'path':path,'sha256':sha(data),'bytes':len(data),'text':text})
        if path == 'homebrew/homebrew-resolved-info.json':
            info = unique_json(data)
            report['homebrew_resolved_info'] = [project(f,('name','full_name','tap','versions','revision',
                'version_scheme','license','urls','bottle','installed','dependencies','build_dependencies',
                'optional_dependencies','recommended_dependencies','tap_git_head','ruby_source_path',
                'ruby_source_checksum')) for f in info.get('formulae',[])]
        if path.endswith('/Package.resolved'):
            resolved = unique_json(data); pins = resolved.get('pins',resolved.get('object',{}).get('pins',[]))
            for pin in pins:
                if 'sparkle' in str(pin.get('identity',pin.get('package',''))).lower():
                    report['sparkle_pins'].append({'asset':asset,'path':path,'sha256':sha(data),
                        **project(pin,('identity','package','location','repositoryURL','state'))})
    for path in ('application/LICENSE','application/THIRD_PARTY_NOTICES.md','application/Cargo.lock',
                 'application/PUBLIC_SOURCE_MANIFEST.json','ghostty/LICENSE','godot/LICENSE.txt'):
        matches = [(a,r) for (a,p),r in all_files.items() if p == path and r['kind'] == 'file']
        report['required_files'].append({'path':path, 'present':bool(matches),
            'matches':[{'asset':a, **project(row,('bytes','sha256'))} for a,row in matches]})
        if not matches: report['gaps'].append('Missing required retained source file: '+path)
    if not report['homebrew_resolved_info']:
        report['gaps'].append('Resolved Homebrew source/bottle metadata missing')
    kegs = report['homebrew_packaged_kegs']
    for keg in kegs:
        prefix = 'homebrew/'+keg['name']+'/'+keg['version']+'/'
        for ending in ('INSTALL_RECEIPT.json','sbom.spdx.json','.brew/'+keg['name']+'.rb'):
            matches = [r for r in report['homebrew_actual_inputs'] if r['path'] == prefix+ending]
            report['required_files'].append({'path':prefix+ending,'present':bool(matches)})
            if not matches: report['gaps'].append('Missing installed Homebrew metadata: '+prefix+ending)
    evidence = scans['build-evidence.zip']['captured']
    prefix = 'station-evidence/'
    for name in ('build-provenance/provenance.json','build-provenance/cargo-resolution.json',
                 'build-provenance/cargo-compiled-artifacts.json','macos-bom.json','release-gate.json','installation-launch.json'):
        path = prefix+name; data = evidence.get(path)
        report['required_files'].append({'asset':'build-evidence.zip','path':path,'present':data is not None,
                                         'bytes':len(data) if data is not None else None,
                                         'sha256':sha(data) if data is not None else None})
        if data is None: report['gaps'].append('Missing build evidence: '+name)
    def evidence_json(name):
        data = evidence.get(prefix+name)
        return unique_json(data) if data is not None else {}
    prov = evidence_json('build-provenance/provenance.json')
    report['provenance_gaps'] = [sanitized_gap(g) for g in prov.get('gaps',[])]
    report['hash_bound_text_evidence'] = []
    report['selected_legal_texts'] = []
    requested_origins = set(plan.get('legal_text_origins',[])); found_origins = set()
    refs = list(prov.get('evidence_files',[]))
    for package in evidence_json('build-provenance/cargo-resolution.json').get('packages',[]):
        refs.extend(package.get('legal_texts',[]))
    for package in prov.get('homebrew',[]): refs.extend(package.get('evidence',[]))
    for package in list(prov.get('upstream',{}).values()) + prov.get('xcode_packages',[]):
        refs.extend(package.get('legal_texts',[]))
    seen_refs = set()
    for ref in refs:
        origin = ref.get('origin',''); checksum = ref.get('sha256',''); rel = ref.get('evidence_file','')
        require(re.fullmatch('[0-9a-f]{64}',checksum) and rel == 'texts/'+checksum+'.txt', 'invalid_text_evidence_reference')
        path = prefix+'build-provenance/'+rel
        actual = scans['build-evidence.zip']['files'].get(path,{})
        require(actual.get('sha256') == checksum, 'text_evidence_hash_mismatch')
        identity = (origin,checksum)
        if identity in seen_refs: continue
        seen_refs.add(identity)
        selected = {'origin':origin,'sha256':checksum,'asset':'build-evidence.zip','path':path,'bytes':actual['bytes']}
        report['hash_bound_text_evidence'].append(selected)
        if origin.startswith(('Homebrew/','Xcode/','godot/','ghostty/','Cargo/')) and legal_path_candidate(origin):
            legal_texts.add({'asset':'build-evidence.zip','member':path,'source_origin':origin},actual,evidence.get(path))
        if origin in requested_origins:
            data = evidence.get(path)
            require(data is not None and len(data) <= MAX_TEXT, 'selected_legal_text_unavailable')
            text = data.decode('utf-8'); require('\0' not in text,'selected_legal_text_not_utf8')
            report['selected_legal_texts'].append({**selected,'text':text}); found_origins.add(origin)
    require(found_origins == requested_origins,'selected_legal_text_origin_missing')
    report['binary_components'] = []
    for component in prov.get('components',[]):
        selected = project(component,('bundle_path','sha256','candidate_evidence_groups','approved_attribution'))
        selected['copy_transformation_evidence'] = [project(t,('bundle_relative_path','source_path','source_sha256',
            'packaged_sha256','formula','formula_version','matches_final_bundle_hash','operations'))
            for t in component.get('copy_transformation_evidence',[])]
        report['binary_components'].append(selected)
    report['xcode_packages'] = []
    for p in prov.get('xcode_packages',[]):
        rev = p.get('revision',{}); revision = rev.get('stdout','').strip() if rev.get('exit_code') == 0 else None
        require(revision is None or re.fullmatch('[0-9a-f]{40}',revision), 'invalid_xcode_revision')
        report['xcode_packages'].append({'name':p.get('name'),'revision':revision,'legal_texts':p.get('legal_texts',[])})
    sparkle_revisions = {p.get('revision') for p in report['xcode_packages'] if 'sparkle' in p.get('name','').lower()}
    for pin in report['sparkle_pins']:
        pin['matches_observed_checkout_revision'] = pin.get('state',{}).get('revision') in sparkle_revisions
        if not pin['matches_observed_checkout_revision']:
            report['gaps'].append('Sparkle lock pin does not match an observed checkout revision')
    if not report['sparkle_pins']: report['gaps'].append('Sparkle Package.resolved source pin missing')
    if not any('sparkle' in p.get('name','').lower() for p in report['xcode_packages']):
        report['gaps'].append('Sparkle actual checkout identity missing')
    resolution = evidence_json('build-provenance/cargo-resolution.json')
    report['cargo_resolution'] = {'packages':[project(p,('id','name','version','source','license','license_file','legal_texts'))
        for p in resolution.get('packages',[])], 'resolve':{'root':resolution.get('resolve',{}).get('root'), 'nodes':[
            {**project(n,('id','dependencies','features')), 'deps':[
                {**project(d,('name','pkg')), 'dep_kinds':[project(k,('kind','target')) for k in d.get('dep_kinds',[])]}
                for d in n.get('deps',[])]} for n in resolution.get('resolve',{}).get('nodes',[])]}}
    artifacts = evidence_json('build-provenance/cargo-compiled-artifacts.json')
    report['cargo_compiled_artifacts'] = []
    for row in artifacts.get('artifacts',[]):
        selected = project(row,('package_id','features','fresh','build_record'))
        selected['target'] = project(row.get('target',{}),('name','kind','crate_types'))
        selected['profile'] = project(row.get('profile',{}),('opt_level','debuginfo','debug_assertions','overflow_checks','test'))
        report['cargo_compiled_artifacts'].append(selected)
    report['launch_evidence'] = project(evidence_json('installation-launch.json'),
        ('bundle_id','alive_after_15_seconds','temporary_copy_signature','exit_code_before_cleanup','cleanup_exit_code','release_ready'))
    gate = evidence_json('release-gate.json')
    report['release_gate'] = project(gate,('release_ready','inventory_status'))
    report['release_gate']['inventory_blockers'] = [sanitized_gap(g) for g in gate.get('inventory_blockers',[])]
    report['release_gate']['provenance_gaps'] = [sanitized_gap(g) for g in gate.get('provenance_gaps',[])]
    # Compare registry package checksum and retained source files with Cargo.lock.
    lock_data = next((d for (a,p),d in captured.items() if a.startswith('application-source-') and p == 'application/Cargo.lock'),None)
    locks = tomllib.loads(lock_data.decode()) if lock_data else {}
    lockchecks = {(p['name'],p['version'],p.get('source')):p.get('checksum') for p in locks.get('package',[])}
    report['cargo_source_checks'] = []
    cargo_files = {p:r for (a,p),r in all_files.items() if a.startswith('cargo-compiled-sources-')}
    cargo_data = {p:d for (a,p),d in captured.items() if a.startswith('cargo-compiled-sources-')}
    resolution_by_id = {p['id']:p for p in resolution.get('packages',[])}
    for p in report['cargo_packages']:
        origin = p.get('source',''); root = p.get('retained_in','')
        row = {'id':p.get('id'), 'source':origin,'retained_in':root}
        if origin.startswith('registry+'):
            q = resolution_by_id.get(p.get('id'),{}); expected = lockchecks.get((q.get('name'),q.get('version'),q.get('source')))
            data = cargo_data.get(root+'/.cargo-checksum.json')
            if data:
                sums = unique_json(data); row['checksum_manifest_sha256'] = sha(data)
                row['lock_checksum'] = expected; row['captured_package_checksum'] = sums.get('package')
                row['lock_package_checksum_matches'] = bool(expected and expected == sums.get('package'))
                mismatches = []
                for rel, expected_hash in sums.get('files',{}).items():
                    rel = safe_path(rel); actual = cargo_files.get(root+'/'+rel,{})
                    if actual.get('sha256') != expected_hash: mismatches.append(rel)
                row['file_checksum_mismatches_or_missing'] = mismatches
                row['all_listed_source_files_match'] = not mismatches
                if not row['lock_package_checksum_matches'] or mismatches:
                    report['gaps'].append('Registry source checksum verification incomplete: '+str(p.get('id')))
            else:
                row['verification'] = 'missing_cargo_checksum_manifest'
                report['gaps'].append('Registry package checksum manifest missing: '+str(p.get('id')))
        elif origin.startswith('git+'):
            row['declared_commit'] = origin.rsplit('#',1)[-1] if '#' in origin else None
            row['verification'] = 'archive_verified; actual_git_HEAD_and_unchanged_baseline_not_captured'
        else: row['verification'] = 'retained_application_files_checked_against_public_commit'
        report['cargo_source_checks'].append(row)
    report['fresh_legal_texts'] = sorted(legal_texts.records.values(),key=lambda row:row['sha256'])
    report['fresh_legal_text_gaps'] = legal_texts.gaps
    report['fresh_legal_text_summary'] = {'unique_files':len(legal_texts.records),'unique_raw_bytes':legal_texts.total,
        'aggregate_raw_byte_limit':legal_texts.limit,'per_file_raw_byte_limit':MAX_TEXT,
        'omitted_origin_count':len(legal_texts.gaps),'scope':'Third-party regular legal files only; SHA-deduplicated, original byte hashes; no source/log dumping'}
    if legal_texts.gaps: report['gaps'].append('Some fresh legal candidates were omitted; inspect fresh_legal_text_gaps before packaging')
    return report


def safe_output(report, output):
    data = (json.dumps(report, indent=2, sort_keys=True, ensure_ascii=True)+'\n').encode()
    require(len(data) <= MAX_REPORT, 'report_size_bound')
    text = data.decode()
    require(not SECRET.search(text) and (not TOKEN or TOKEN not in text), 'possible_secret_in_allowlisted_report')
    output.parent.mkdir(parents=True,exist_ok=True)
    require(not output.exists() and not output.is_symlink(), 'report_already_exists')
    with output.open('xb') as f: f.write(data)


def review(client, plan, output):
    check_plan(plan)
    base = '/repos/'+REPO
    release = client.get(base+'/releases/'+str(plan['release_id']))
    assets = release_assets(release,plan)
    run = client.get(base+'/actions/runs/'+str(plan['run_id']))
    require(run.get('id') == plan['run_id'] and run.get('head_sha') == plan['source_commit'] and run.get('run_attempt') == 1
            and run.get('status') == 'completed' and run.get('conclusion') == 'success'
            and run.get('path') == '.github/workflows/macos-trial.yml'
            and run.get('repository',{}).get('full_name') == REPO, 'build_run_identity_or_success_mismatch')
    tree = client.get(base+'/git/trees/'+plan['source_commit']+'?recursive=1')
    with tempfile.TemporaryDirectory(prefix='private-draft-review-') as temp:
        root = Path(temp); hashes = {}; scans = {}
        def download(name):
            hashes[name] = client.download(assets[name]['id'],root/name,assets[name]['size'])
            api_digest = assets[name].get('digest')
            require(api_digest in (None,'') or api_digest == 'sha256:'+hashes[name], 'github_asset_digest_mismatch')
            return root/name
        index = unique_json(download('draft-build-index.json').read_bytes())
        sums = download('SHA256SUMS').read_bytes()
        entries = index_entries(index,sums,assets,plan['source_commit'])
        manifest = None
        for name,e in sorted(entries.items()):
            path = download(name); require(hashes[name] == e['sha256'], 'downloaded_asset_hash_mismatch')
            if name == 'trial-inputs-manifest.json': manifest = unique_json(path.read_bytes())
            elif name.endswith(('.tar.gz','.zip')): scans[name] = inspect_archive(path)
            path.unlink()
            require(sum(s['uncompressed_bytes'] for s in scans.values()) <= MAX_ALL_RAW,'total_uncompressed_bound')
        require(manifest is not None, 'capture_manifest_missing')
        verify_captured(manifest,scans,entries,plan['source_commit'])
        report = build_report(plan,assets,entries,manifest,scans,tree)
        report['control_asset_digests'] = [{'name':n,'id':assets[n]['id'],'bytes':assets[n]['size'],'sha256':hashes[n]} for n in ('draft-build-index.json','SHA256SUMS')]
        # Re-read exact release after all reads to catch publication/replacement races.
        final_assets = release_assets(client.get(base+'/releases/'+str(plan['release_id'])),plan)
        require({n:(a['id'],a['size'],a.get('updated_at'),a.get('digest')) for n,a in assets.items()} ==
                {n:(a['id'],a['size'],a.get('updated_at'),a.get('digest')) for n,a in final_assets.items()}, 'draft_changed_during_review')
        safe_output(report,output)
    return report


def main():
    global TOKEN
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--plan',type=Path,required=True); p.add_argument('--output',type=Path,required=True)
    args = p.parse_args()
    TOKEN = os.environ.pop('GH_TOKEN','')
    try:
        plan = unique_json(args.plan.read_bytes())
        review(GitHub(TOKEN),plan,args.output)
    except Exception as e:
        # Never emit raw HTTP errors, bodies, signed URLs, arbitrary path names or tokens.
        code = str(e) if isinstance(e,ReviewError) else 'unclassified_parse_or_io_failure'
        print('Private draft review stopped: '+code, file=sys.stderr)
        return 1
    print('Private draft integrity review complete; one allowlisted report written. Release clearance remains pending.')
    return 0


if __name__ == '__main__':
    sys.exit(main())

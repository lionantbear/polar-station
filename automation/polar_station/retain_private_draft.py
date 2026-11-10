#!/usr/bin/env python3
"""Retain explicit files in an unpublished GitHub draft; never publish or delete.

Creation is write-once. A private local receipt binds the returned numeric ID to
this run. An ambiguous create requires operator reconciliation, not a blind
retry. Ambiguous uploads are reconciled only under that ID, without overwrites.
"""
import argparse
import hashlib
import http.client
import json
import os
from pathlib import Path
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request


class RetentionError(Exception):
    pass


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


class Transport:
    """No automatic redirects, cookies, .netrc, or implicit proxy credentials."""
    def __init__(self, token, *, read_only=False):
        self.token = token
        self.read_only = read_only
        self.opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())

    def request(self, method, url, *, authenticated=False, data=None, length=None, binary=False):
        if self.read_only and (method != 'GET' or data is not None):
            raise RetentionError('Read-only transport forbids external writes')
        parsed = urllib.parse.urlsplit(url)
        if (parsed.scheme != 'https' or parsed.username or parsed.password or
                parsed.port not in (None, 443) or parsed.fragment):
            raise RetentionError('Unsafe request URL')
        headers = {'User-Agent': 'PolarStation-private-draft-retention', 'Cache-Control': 'no-cache',
                   'Accept': 'application/octet-stream' if binary and method == 'GET' else 'application/vnd.github+json'}
        if authenticated:
            if parsed.netloc not in ('api.github.com', 'uploads.github.com'):
                raise RetentionError('Refusing credentials outside GitHub API/upload origin')
            headers['Authorization'] = 'Bearer ' + self.token
            headers['X-GitHub-Api-Version'] = '2022-11-28'
        if data is not None:
            headers['Content-Type'] = 'application/octet-stream' if binary else 'application/json'
            headers['Content-Length'] = str(length if length is not None else len(data))
        request = urllib.request.Request(url, method=method, data=data, headers=headers)
        try:
            return self.opener.open(request, timeout=900 if binary and method == 'POST' else 300 if binary else 60)
        except urllib.error.HTTPError as error:
            return error
        except (OSError, urllib.error.URLError, http.client.HTTPException):
            # Never include exception text: redirect URLs may contain signed credentials.
            raise RetentionError('Network outcome unknown; no write was automatically retried') from None


def positive_id(value):
    if type(value) is not int or value <= 0:
        raise RetentionError('Invalid numeric GitHub ID')
    return value


def sha256_file(path):
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            digest.update(block)
    return digest.hexdigest()


def inventory(paths):
    result = {}
    for raw in paths:
        path = Path(raw).absolute()
        if path.is_symlink() or not path.is_file() or path.resolve() != path:
            raise RetentionError('Upload input must be a regular file without symlink components')
        if not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9._-]{0,180}', path.name):
            raise RetentionError('Unsafe asset filename')
        if path.name in result or path.name.endswith('.partial'):
            raise RetentionError('Duplicate or partial asset filename')
        size = path.stat().st_size
        if not 0 < size < 2_000_000_000:
            raise RetentionError('Empty or oversized asset')
        result[path.name] = {'path': str(path), 'bytes': size, 'sha256': sha256_file(path)}
    if not result or len(result) > 250:
        raise RetentionError('Expected between 1 and 250 explicitly selected assets')
    return result


class Retainer:
    attempts = 6
    def __init__(self, transport, repo, source, run_id, attempt, kind, receipt, paths, sleep=time.sleep):
        if not re.fullmatch(r'[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+', repo):
            raise RetentionError('Invalid repository')
        if not re.fullmatch(r'[0-9a-f]{40}', source):
            raise RetentionError('A full source commit SHA is required')
        if not re.fullmatch(r'[1-9][0-9]*', run_id) or not re.fullmatch(r'[1-9][0-9]*', attempt):
            raise RetentionError('Invalid run identity')
        if kind not in ('probe', 'build'):
            raise RetentionError('Invalid retention kind')
        self.transport, self.sleep = transport, sleep
        self.repo, self.source = repo, source
        self.tag = f'macos-arm64-trial-{kind}-{run_id}-{attempt}'
        self.api = f'https://api.github.com/repos/{repo}'
        self.identity = {'repository': repo, 'source_commit': source, 'run_id': run_id,
                         'run_attempt': attempt, 'kind': kind, 'tag': self.tag}
        self.body = ('Private draft retention only. Never publish automatically. '
                     'Not release clearance: licenses, corresponding source and interactive '
                     'Mac validation remain subject to review.\n\n' +
                     json.dumps(self.identity, sort_keys=True))
        self.expected = inventory(paths)
        self.receipt = Path(receipt)
        self.state = {'identity': self.identity, 'expected': self.expected,
                      'phase': 'create-pending', 'uploads_pending': []}

    def save(self):
        self.receipt.parent.mkdir(parents=True, exist_ok=True)
        tmp = self.receipt.with_name(self.receipt.name + '.tmp')
        with open(tmp, 'w', opener=lambda path, flags: os.open(path, flags, 0o600)) as out:
            json.dump(self.state, out, indent=2, sort_keys=True)
            out.write('\n')
            out.flush()
            os.fsync(out.fileno())
        os.replace(tmp, self.receipt)

    def pause(self, index):
        self.sleep(min(2 ** (index + 1), 30))

    def request(self, method, url, **kwargs):
        # Reads alone may be retried. A write is always a single HTTP attempt.
        for index in range(self.attempts if method == 'GET' else 1):
            try:
                response = self.transport.request(method, url, **kwargs)
            except RetentionError:
                if method != 'GET' or index == self.attempts - 1:
                    raise
            else:
                retry = response.status in (408, 429, 500, 502, 503, 504)
                retry |= response.status == 404 and kwargs.get('authenticated', False)
                if method != 'GET' or not retry or index == self.attempts - 1:
                    return response
                response.close()
            self.pause(index)
        raise RetentionError('Read retry budget exhausted')

    def json_request(self, method, url, payload=None, expected_status=200):
        data = json.dumps(payload).encode() if payload is not None else None
        with self.request(method, url, authenticated=True, data=data) as response:
            if response.status != expected_status:
                raise RetentionError(f'GitHub {method} failed with HTTP {response.status}; no write retried')
            try:
                return json.load(response)
            except (ValueError, UnicodeError, OSError, http.client.HTTPException):
                raise RetentionError('Invalid JSON response; no write retried') from None

    def upload_url(self, release):
        rid = positive_id(release.get('id'))
        expected = f'https://uploads.github.com/repos/{self.repo}/releases/{rid}/assets'
        url = release.get('upload_url')
        if url not in (expected, expected + '{?name,label}'):
            raise RetentionError('Unexpected upload URL or release ID binding')
        return expected

    def check_release(self, release):
        if not isinstance(release, dict):
            raise RetentionError('Malformed release response')
        if (positive_id(release.get('id')) != self.state['release_id'] or
                release.get('draft') is not True or release.get('prerelease') is not True or
                release.get('published_at') is not None or release.get('tag_name') != self.tag or
                release.get('target_commitish') != self.source or release.get('body') != self.body):
            raise RetentionError('Draft privacy or exact run/source identity changed')
        if self.upload_url(release) != self.upload_url(self.state['create_response']):
            raise RetentionError('Upload endpoint changed')
        return release

    def read_release(self):
        return self.check_release(self.json_request('GET', self.release_url))

    def anonymous_404(self, url, binary=False):
        with self.request('GET', url, authenticated=False, binary=binary) as response:
            if response.status != 404:
                raise RetentionError(f'Anonymous denial not established (HTTP {response.status}); stopped')

    def assets(self):
        found = []
        for page in range(1, 5):
            batch = self.json_request('GET', f'{self.release_url}/assets?per_page=100&page={page}')
            if not isinstance(batch, list):
                raise RetentionError('Malformed asset listing')
            found.extend(batch)
            if len(batch) < 100:
                break
        else:
            raise RetentionError('Unexpectedly large draft asset listing')
        ids, names = set(), set()
        for asset in found:
            aid = positive_id(asset.get('id'))
            name = asset.get('name')
            if aid in ids or name in names or name not in self.expected:
                raise RetentionError('Duplicate or unexpected asset ID/name; no replacement allowed')
            ids.add(aid)
            names.add(name)
        return {a['name']: a for a in found}

    def settled_assets(self, required):
        for index in range(self.attempts):
            result = self.assets()
            if required <= result.keys():
                return result
            if index < self.attempts - 1:
                self.pause(index)
        raise RetentionError('Asset not visible under recorded draft ID; do not retry upload blindly')

    def verify_asset(self, asset, expected_name):
        aid = positive_id(asset.get('id'))
        api = f'{self.api}/releases/assets/{aid}'
        actual = self.json_request('GET', api)
        entry = self.expected[expected_name]
        for value in (asset, actual):
            if (value.get('id') != aid or value.get('name') != expected_name or
                    value.get('state') != 'uploaded' or value.get('size') != entry['bytes'] or
                    value.get('url') != api):
                raise RetentionError('Asset ID/name/state/size mismatch')
            if value.get('digest') not in (None, 'sha256:' + entry['sha256']):
                raise RetentionError('Asset digest metadata mismatch')
        browser = actual.get('browser_download_url', '')
        parsed = urllib.parse.urlsplit(browser)
        prefix = f'/{self.repo}/releases/download/'
        if (parsed.scheme != 'https' or parsed.netloc != 'github.com' or parsed.query or
                parsed.fragment or not parsed.path.startswith(prefix) or
                urllib.parse.unquote(parsed.path.rsplit('/', 1)[-1]) != expected_name):
            raise RetentionError('Unsafe browser download URL')
        self.anonymous_404(api)
        self.anonymous_404(api, binary=True)
        self.anonymous_404(browser, binary=True)
        # Download from the numeric asset endpoint; never log signed redirect URLs.
        response = self.request('GET', api, authenticated=True, binary=True)
        try:
            for _ in range(4):
                if response.status not in (301, 302, 303, 307, 308):
                    break
                redirect = response.headers.get('Location', '')
                target = urllib.parse.urlsplit(redirect)
                if (target.scheme != 'https' or target.netloc not in
                        ('release-assets.githubusercontent.com', 'objects.githubusercontent.com') or
                        target.username or target.password or target.fragment):
                    raise RetentionError('Unexpected asset redirect origin')
                response.close()
                # Deliberately rebuild headers without Authorization on every redirect.
                response = self.request('GET', redirect, authenticated=False, binary=True)
            if response.status != 200:
                raise RetentionError(f'Authenticated asset download failed (HTTP {response.status})')
            size, digest = 0, hashlib.sha256()
            for chunk in iter(lambda: response.read(1024 * 1024), b''):
                size += len(chunk)
                if size > entry['bytes']:
                    raise RetentionError('Downloaded asset exceeds expected size')
                digest.update(chunk)
            if size != entry['bytes'] or digest.hexdigest() != entry['sha256']:
                raise RetentionError('Downloaded asset size/SHA-256 mismatch')
        except (OSError, urllib.error.URLError, http.client.HTTPException):
            raise RetentionError('Asset download interrupted; verification incomplete') from None
        finally:
            response.close()
        self.state.setdefault('verified_assets', {})[expected_name] = {
            'id': aid, 'bytes': size, 'sha256': digest.hexdigest()}
        self.save()

    def probe_metadata(self):
        name = 'private-retention-probe.txt'
        verified = self.state['verified_assets'][name]
        return {'release_id': self.state['release_id'], 'asset_id': verified['id'],
                'source_commit': self.source, 'run_id': self.identity['run_id'],
                'run_attempt': self.identity['run_attempt'], 'name': name,
                'bytes': verified['bytes'], 'sha256': verified['sha256']}

    def verify_read_only_probe(self, metadata):
        # IDs/hash metadata come from this run's write-enabled probe, never a tag lookup.
        keys = {'release_id', 'asset_id', 'source_commit', 'run_id', 'run_attempt',
                'name', 'bytes', 'sha256'}
        if not isinstance(metadata, dict) or set(metadata) != keys:
            raise RetentionError('Malformed read-only probe metadata')
        name = 'private-retention-probe.txt'
        if (self.identity['kind'] != 'probe' or set(self.expected) != {name} or
                metadata['source_commit'] != self.source or
                metadata['run_id'] != self.identity['run_id'] or
                metadata['run_attempt'] != self.identity['run_attempt'] or
                metadata['name'] != name or
                metadata['bytes'] != self.expected[name]['bytes'] or
                metadata['sha256'] != self.expected[name]['sha256']):
            raise RetentionError('Read-only probe source/run/content identity mismatch')
        rid, aid = positive_id(metadata['release_id']), positive_id(metadata['asset_id'])
        self.state['release_id'] = rid
        self.state['phase'] = 'read-only-verification'
        self.release_url = f'{self.api}/releases/{rid}'
        for index in range(self.attempts):
            release = self.json_request('GET', self.release_url)
            self.state['create_response'] = release
            self.check_release(release)
            # The direct release response binds this exact asset ID to this draft.
            members = release.get('assets', [])
            if (isinstance(members, list) and len(members) == 1 and
                    isinstance(members[0], dict) and members[0].get('id') == aid and
                    members[0].get('name') == name):
                break
            if index == self.attempts - 1:
                raise RetentionError('Exact probe asset not bound to numeric draft ID')
            self.pause(index)
        self.anonymous_404(self.release_url)
        asset = self.json_request('GET', f'{self.api}/releases/assets/{aid}')
        self.verify_asset(asset, name)
        self.read_release()
        self.anonymous_404(self.release_url)
        self.state['phase'] = 'verified-private-read-only'
        self.save()
        return {'release_id': rid, 'asset_id': aid, 'source_commit': self.source,
                'assets_verified': 1, 'read_only_verification': True,
                'draft': True, 'public_release_ready': False}

    def retain(self):
        if self.receipt.exists():
            self.state = json.loads(self.receipt.read_text())
            if self.state.get('identity') != self.identity or self.state.get('expected') != self.expected:
                raise RetentionError('Receipt belongs to different run/source/files')
            if 'release_id' not in self.state:
                raise RetentionError('Create outcome uncertain: reconcile existing draft manually; do not recreate')
        else:
            # Durable intent before network I/O protects against process termination.
            self.save()
            release = self.json_request('POST', self.api + '/releases', {
                'tag_name': self.tag, 'target_commitish': self.source,
                'name': f'Polar Station macOS arm64 trial {self.identity["kind"]} '
                        f'{self.identity["run_id"]} (DRAFT)',
                'body': self.body, 'draft': True, 'prerelease': True,
                'generate_release_notes': False}, expected_status=201)
            # Save the returned ID/upload URL BEFORE any later verification.
            self.state['create_response'] = release
            if isinstance(release, dict) and type(release.get('id')) is int:
                self.state['release_id'] = release['id']
            self.state['phase'] = 'created'
            self.save()
        rid = positive_id(self.state.get('release_id'))
        self.release_url = f'{self.api}/releases/{rid}'
        self.check_release(self.state['create_response'])
        self.read_release()
        self.anonymous_404(self.release_url)
        known = set(self.state['uploads_pending']) | set(self.state.get('verified_assets', {}))
        current = self.settled_assets(known)
        for name, entry in self.expected.items():
            self.read_release()
            if name not in current:
                path = Path(entry['path'])
                if inventory([path])[name] != entry:
                    raise RetentionError('Local upload input changed')
                self.state['uploads_pending'].append(name)
                self.save()
                upload = self.upload_url(self.state['create_response']) + '?' + urllib.parse.urlencode({'name': name})
                created_id = None
                outcome = {'kind': 'local_input_error'}
                try:
                    with path.open('rb') as data:
                        outcome = {'kind': 'transport_uncertain'}
                        with self.request('POST', upload, authenticated=True, data=data,
                                          length=entry['bytes'], binary=True) as response:
                            # Status only, never the response body or URL, is safe to retain.
                            outcome = {'kind': 'http', 'status': response.status}
                            if response.status == 201:
                                try:
                                    created_asset = json.load(response)
                                except (ValueError, UnicodeError):
                                    outcome['response'] = 'invalid_json'
                                else:
                                    try:
                                        created_id = positive_id(created_asset.get('id')
                                            if isinstance(created_asset, dict) else None)
                                    except RetentionError:
                                        outcome['response'] = 'invalid_asset_id'
                                    else:
                                        outcome['response'] = 'asset_id_received'
                                        outcome['asset_id'] = created_id
                                        self.state.setdefault('observed_uploads', {})[name] = {'id': created_id}
                except (RetentionError, OSError, http.client.HTTPException):
                    if outcome['kind'] == 'http':
                        outcome['response'] = 'read_uncertain'
                # Preserve the original upload result before any visibility retry can fail.
                self.state.setdefault('upload_outcomes', {})[name] = outcome
                self.save()
                print(json.dumps({'release_id': rid, 'upload_asset': name,
                                  'upload_outcome': outcome}, sort_keys=True), flush=True)
                # No write retry, including on 422/502/timeout: inspect this exact ID only.
                current = self.settled_assets(set(self.state['uploads_pending']))
                if created_id is not None and current[name].get('id') != created_id:
                    raise RetentionError('Uploaded asset ID differs from numeric draft listing')
            self.verify_asset(current[name], name)
            print(json.dumps({'verified_retained_asset': name, **self.state['verified_assets'][name]}, sort_keys=True), flush=True)
            if name in self.state['uploads_pending']:
                self.state['uploads_pending'].remove(name)
                self.save()
        current = self.settled_assets(set(self.expected))
        if set(current) != set(self.expected):
            raise RetentionError('Final asset inventory mismatch')
        for name, asset in current.items():
            verified = self.state['verified_assets'][name]
            if (asset.get('id') != verified['id'] or asset.get('size') != verified['bytes'] or
                    asset.get('state') != 'uploaded' or
                    asset.get('digest') not in (None, 'sha256:' + verified['sha256'])):
                raise RetentionError('Final asset identity changed')
        self.read_release()
        self.anonymous_404(self.release_url)
        self.state['phase'] = 'verified-private'
        self.save()
        return {'release_id': rid, 'source_commit': self.source,
                'assets_verified': len(current), 'draft': True, 'public_release_ready': False}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--kind', choices=('probe', 'verify-probe', 'build'), required=True)
    parser.add_argument('--receipt', type=Path, required=True)
    parser.add_argument('--files', type=Path, help='Newline-delimited explicit file paths')
    parser.add_argument('--probe-metadata', help='Only numeric IDs and exact source/run/hash metadata')
    parser.add_argument('--emit-job-output', action='store_true')
    args = parser.parse_args()
    retainer = None
    try:
        repo, source, run, attempt = (os.environ[k] for k in
                                      ('GITHUB_REPOSITORY', 'GITHUB_SHA', 'GITHUB_RUN_ID', 'GITHUB_RUN_ATTEMPT'))
        token = os.environ['GH_TOKEN']
        if not token:
            raise RetentionError('Missing GitHub token')
        read_only = args.kind == 'verify-probe'
        if (read_only != (args.probe_metadata is not None) or
                (args.emit_job_output and args.kind != 'probe')):
            raise RetentionError('Invalid probe mode/options')
        if args.kind in ('probe', 'verify-probe'):
            if args.files:
                raise RetentionError('Probe accepts no external files')
            args.receipt.parent.mkdir(parents=True, exist_ok=True)
            probe = args.receipt.parent / 'private-retention-probe.txt'
            probe.write_text('Polar Station private draft retention probe.\n' +
                             json.dumps({'source_commit': source, 'run_id': run,
                                         'run_attempt': attempt}, sort_keys=True) + '\n')
            paths = [probe]
        else:
            if not args.files:
                raise RetentionError('Build requires explicit file list')
            paths = args.files.read_text().splitlines()
        retainer = Retainer(Transport(token, read_only=read_only), repo, source, run, attempt,
                            'probe' if read_only else args.kind, args.receipt, paths)
        result = (retainer.verify_read_only_probe(json.loads(args.probe_metadata))
                  if read_only else retainer.retain())
        if args.emit_job_output:
            with open(os.environ['GITHUB_OUTPUT'], 'a') as output:
                output.write('probe=' + json.dumps(retainer.probe_metadata(), sort_keys=True) + '\n')
        print(json.dumps(result, sort_keys=True))
    except (RetentionError, OSError, ValueError, KeyError) as error:
        # No raw remote response/error/URL printing. Receipt remains local, never an asset.
        if retainer is not None:
            print(json.dumps({'retention_incomplete': True, 'release_id': retainer.state.get('release_id'),
                              'verified_retained_assets': retainer.state.get('verified_assets', {}),
                              'upload_outcomes': retainer.state.get('upload_outcomes', {}),
                              'observed_unverified_uploads': {k: v for k, v in retainer.state.get('observed_uploads', {}).items()
                                  if k not in retainer.state.get('verified_assets', {})}}, sort_keys=True), flush=True)
        reason = str(error) if isinstance(error, RetentionError) else 'Local input or response decoding failure'
        print('Private draft retention failed closed: ' + reason + '. Inspect the private local receipt; '
              'do not blindly repeat an uncertain create or upload.', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    raise SystemExit(main())

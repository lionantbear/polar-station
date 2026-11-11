#!/usr/bin/env python3
"""Reviewable, explicit synchronization of verified Polar Station snapshots.

plan --repository CHECKOUT --previous LAST_PUBLISHED_SNAPSHOT --snapshot NEW
     --branch main --output PLAN.json [--protect PATH ...]
apply --plan PLAN.json --bundle NEW.bundle --message TEXT [--receipt RECEIPT.json]
publish --receipt RECEIPT.json --confirm-publication

Previous must be the actual last-published snapshot supplied by the operator,
not the current remote manifest substituted as a baseline. plan/apply read the
remote HEAD but never push, checkout source, run repository scripts, hooks,
filters, submodules, or alter the caller's repository. apply creates a commit
in an isolated temporary Git index and outputs a bundle plus JSON receipt.
Review the plan and receipt before the separate publish command, which requires
--confirm-publication and only performs a normal, non-force push from a temporary
repository. Remote HEAD is checked before import and again before push. This is
not an atomic compare-and-swap, but Git rejects concurrent non-fast-forward
updates; it never force-overwrites a parallel successor. A changed branch must
be replanned. The flag means the operator authorizes this exact public upload.
Publishing uses existing non-interactive HTTPS credentials or an SSH agent.
SSH is restricted to git@github.com with strict host-key checking. Git credential helpers and
askpass scripts are deliberately not executed. Global/system Git configuration
and injected GIT_CONFIG environment values are disabled; credentials
are never provisioned, saved, or printed by this tool. Authentication failure
exits safely without updating the branch; no token is accepted as a CLI option.

Default repository-owned paths: .github/, CI.md, RELEASE_PLAN.md. README is
source-managed: independent README edits conflict rather than silently merge.
Extra --protect paths are permanent for this plan; protect uses path prefixes,
not globs. Preserved files remain covered by the resulting public manifest,
while sync_managed_paths records which files subsequent exports may replace.
This is a source synchronization gate, not a license or secret-content scanner.
"""
from __future__ import annotations
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import tempfile

MANIFEST = 'PUBLIC_SOURCE_MANIFEST.json'
DEFAULT_PROTECT = ['.github', 'CI.md', 'RELEASE_PLAN.md']


def github_url(url):
    path = r'[A-Za-z0-9][A-Za-z0-9_.-]*/[A-Za-z0-9][A-Za-z0-9_.-]*(?:\.git)?'
    return bool(re.fullmatch(r'(?:https://github\.com/|git@github\.com:|ssh://git@github\.com/)' + path, url))


class SyncError(RuntimeError):
    pass


def digest(data):
    return hashlib.sha256(data).hexdigest()


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=False).encode()


def safe_path(name):
    p = PurePosixPath(name)
    if not name or str(p) != name or p.is_absolute() or any(x in ('..', '.', '.git') for x in p.parts) or '\\' in name or '\x00' in name or '\n' in name:
        raise SyncError('unsafe path: ' + repr(name))
    low = [x.lower() for x in p.parts]
    if any(x in ('.ssh', '.aws', '.env', 'credentials', 'id_rsa', 'id_ed25519') or x.startswith('.env.') for x in low):
        raise SyncError('secret-associated path: ' + name)
    return name


def protected(name, prefixes):
    return any(name == p or name.startswith(p + '/') for p in prefixes)


def git_environment():
    # Caller-injected -c/environment configuration must not bypass the repository
    # audit. Git helper/transport/alternate-object overrides are also stripped.
    env = {k: v for k, v in os.environ.items() if not k.startswith('GIT_')}
    for key in ('SSH_ASKPASS', 'SSH_ASKPASS_REQUIRE'):
        env.pop(key, None)
    env.update(GIT_TERMINAL_PROMPT='0', GIT_CONFIG_NOSYSTEM='1',
               GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_SYSTEM=os.devnull,
               GIT_ATTR_NOSYSTEM='1', GIT_NO_REPLACE_OBJECTS='1',
               GIT_ASKPASS='/bin/false', GIT_PAGER='cat')
    return env


def audit_git_config(repo, args, env):
    """Read only an explicit config file, without includes or repo discovery.

    Reject unfamiliar local settings rather than trying to enumerate every
    executable Git extension. Linked worktrees and alternate Git directories
    are intentionally unsupported. No status/filter/transport runs before this.
    """
    repo = Path(repo)
    dotgit = repo / '.git'
    if dotgit.is_symlink() or (dotgit.exists() and not dotgit.is_dir()):
        raise SyncError('linked/external Git directory is unsupported')
    if dotgit.is_dir():
        gitdir = dotgit
    elif (repo / 'HEAD').is_file() and (repo / 'objects').is_dir():
        gitdir = repo  # Isolated bare test/transport repository.
    elif args and args[0] == 'init' and not any(repo.iterdir()):
        return
    else:
        raise SyncError('expected a standalone Git repository')
    for name in ('config', 'HEAD', 'index', 'objects', 'refs'):
        if (gitdir / name).is_symlink():
            raise SyncError('symlinked Git metadata is unsupported: ' + name)
    for name in ('commondir', 'config.worktree', 'objects/info/alternates', 'info/grafts'):
        if (gitdir / name).exists() or (gitdir / name).is_symlink():
            raise SyncError('external/worktree Git metadata is unsupported: ' + name)
    config = gitdir / 'config'
    if not config.is_file():
        raise SyncError('Git config is missing')
    # --file and --no-includes constrain this parser invocation to this file;
    # cwd=/ prevents discovery of the untrusted repository configuration.
    result = subprocess.run(['git', 'config', '--null', '--name-only',
                             '--no-includes', '--file', str(config.resolve()), '--list'],
                            cwd='/', env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if result.returncode:
        raise SyncError('cannot parse standalone Git config')
    harmless = {'core.repositoryformatversion', 'core.filemode', 'core.bare',
                'core.logallrefupdates', 'core.ignorecase', 'core.precomposeunicode',
                'core.symlinks', 'core.autocrlf', 'core.eol', 'core.safecrlf',
                'user.name', 'user.email'}
    for raw in result.stdout.split(b'\0'):
        if not raw:
            continue
        key = raw.decode('utf-8').lower()
        if key in harmless:
            continue
        if re.fullmatch(r'remote\.[a-z0-9_.-]+\.(url|fetch)', key):
            continue
        if re.fullmatch(r'branch\.[a-z0-9_./-]+\.(remote|merge|rebase)', key):
            continue
        raise SyncError('unsafe or unsupported local Git setting: ' + key)


def git(repo, *args, data=None):
    env = git_environment()
    audit_git_config(repo, args, env)
    cmd = ['git', '-c', 'core.hooksPath=' + os.devnull, '-c', 'core.fsmonitor=false',
           '-c', 'core.attributesFile=' + os.devnull, '-c', 'commit.gpgsign=false',
           '-c', 'protocol.ext.allow=never', '-c', 'core.quotePath=false',
           '-c', 'init.templateDir=',
           '-c', 'core.sshCommand=ssh -F /dev/null -o BatchMode=yes -o StrictHostKeyChecking=yes',
           '-C', str(repo), *args]
    result = subprocess.run(cmd, input=data, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env)
    if result.returncode:
        raise SyncError('git ' + args[0] + ' failed: ' + result.stderr.decode(errors='replace')[:500])
    return result.stdout


def remote_head(repo, url, branch):
    text = git(repo, 'ls-remote', '--exit-code', url, 'refs/heads/' + branch).decode().strip().split()
    if len(text) != 2 or not re.fullmatch('[0-9a-f]{40}', text[0]):
        raise SyncError('remote branch cannot be verified')
    return text[0]


def snapshot(root, hygiene=False):
    root = Path(root).resolve()
    if hygiene:
        from public_release import verify
        verify(root)
    manifest_path = root / MANIFEST
    if manifest_path.is_symlink():
        raise SyncError('manifest must be a regular file')
    doc = json.loads(manifest_path.read_bytes())
    entries = {}
    for name, expected in doc['files'].items():
        safe_path(name)
        p = root / name
        if p.is_symlink() or not p.is_file() or not p.resolve().is_relative_to(root):
            raise SyncError('snapshot path is not an internal regular file: ' + name)
        content = p.read_bytes()
        if digest(content) != expected:
            raise SyncError('snapshot hash mismatch: ' + name)
        entries[name] = {'mode': '100755' if p.stat().st_mode & 0o111 else '100644', 'sha256': digest(content)}
    for name, target in doc.get('symlinks', {}).items():
        safe_path(name)
        p = root / name
        if not p.is_symlink() or os.readlink(p) != target or not p.resolve().is_relative_to(root) or not p.resolve().exists():
            raise SyncError('invalid snapshot link: ' + name)
        entries[name] = {'mode': '120000', 'sha256': digest(target.encode())}
    return doc, entries, digest(canonical({'manifest': digest(manifest_path.read_bytes()), 'entries': entries}))


def tree_entries(repo, head):
    entries = {}
    for line in git(repo, 'ls-tree', '-rz', '--full-tree', head).split(b'\0'):
        if not line:
            continue
        meta, name = line.split(b'\t', 1)
        mode, kind, oid = meta.decode().split()
        name = safe_path(name.decode())
        if kind != 'blob' or mode not in ('100644', '100755', '120000'):
            raise SyncError('submodule/unsupported tree entry: ' + name)
        data = git(repo, 'cat-file', 'blob', oid)
        entries[name] = {'mode': mode, 'sha256': digest(data), 'oid': oid}
    return entries


def identity(entry):
    return {k: entry[k] for k in ('mode', 'sha256')} if entry else None


def make_plan(repository, previous, new, branch='main', protect=None, reviewed_remote_changes=None):
    repo = Path(repository).resolve()
    if git(repo, 'status', '--porcelain=v1', '--untracked-files=all'):
        raise SyncError('repository worktree is not clean')
    if not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9._/-]*', branch) or '..' in branch or branch.endswith('/'):
        raise SyncError('invalid branch')
    url = git(repo, 'remote', 'get-url', 'origin').decode().strip()
    if not github_url(url):
        raise SyncError('origin must be a credential-free HTTPS or git-user SSH github.com repository URL')
    head = git(repo, 'rev-parse', 'HEAD').decode().strip()
    if remote_head(repo, url, branch) != head:
        raise SyncError('checkout HEAD differs from remote; fetch/review before planning')
    tree = git(repo, 'rev-parse', head + '^{tree}').decode().strip()
    prefixes = sorted(set(DEFAULT_PROTECT + (protect or [])))
    for p in prefixes:
        safe_path(p)
    reviewed = dict(reviewed_remote_changes or {})
    for path, expected in reviewed.items():
        safe_path(path)
        if not isinstance(expected, dict) or expected.get('mode') not in ('100644','100755','120000') or not re.fullmatch('[0-9a-f]{64}', expected.get('sha256','')):
            raise SyncError('invalid reviewed remote identity: ' + path)
    old_doc, old, old_hash = snapshot(previous)
    new_doc, fresh, new_hash = snapshot(new, hygiene=True)
    old_managed = set(old_doc.get('sync_managed_paths', old)) - {MANIFEST}
    if not old_managed <= set(old):
        raise SyncError('baseline managed-path inventory invalid')
    old_managed = {p for p in old_managed if not protected(p, prefixes)}
    new_managed = {p for p in fresh if p != MANIFEST and not protected(p, prefixes)}
    remote = tree_entries(repo, head)
    for p in old_managed:
        observed = identity(remote.get(p))
        if observed != old[p] and reviewed.get(p) != observed:
            raise SyncError('remote changed baseline-managed path: ' + p)
    for p in fresh:
        if protected(p, prefixes) and fresh[p] != old.get(p):
            raise SyncError('new snapshot attempts to change repository-owned path: ' + p)
    for p in new_managed - old_managed:
        if p in remote and reviewed.get(p) != identity(remote[p]):
            raise SyncError('new managed path collides with unmanaged remote file: ' + p)
    if any(p not in old_managed | new_managed or reviewed[p] != identity(remote.get(p)) for p in reviewed):
        raise SyncError('reviewed remote identity no longer matches')
    final = {p: identity(e) for p, e in remote.items() if p not in old_managed and p != MANIFEST}
    final.update({p: fresh[p] for p in new_managed})
    for p in final:
        if any(str(parent) in final for parent in PurePosixPath(p).parents if str(parent) != '.'):
            raise SyncError('file/directory collision: ' + p)
    changes = []
    for p in sorted(old_managed | new_managed):
        before, after = old.get(p) if p in old_managed else None, fresh.get(p) if p in new_managed else None
        if before != after:
            changes.append({'path': p, 'action': 'add' if before is None else 'delete' if after is None else 'modify', 'before': before, 'after': after})
    doc = dict(new_doc)
    doc['files'] = {p: e['sha256'] for p, e in sorted(final.items()) if e['mode'] != '120000'}
    links = {}
    for p, e in final.items():
        if e['mode'] == '120000':
            links[p] = os.readlink(Path(new) / p) if p in new_managed else git(repo, 'cat-file', 'blob', remote[p]['oid']).decode()
    # Validate preserved remote links against the resulting tree without checkout.
    for name in links:
        pending = name.split('/')
        resolved = []
        follows = 0
        while pending:
            part = pending.pop(0)
            if part in ('', '.'):
                continue
            if part == '..':
                if not resolved:
                    raise SyncError('link escapes final tree: ' + name)
                resolved.pop()
                continue
            resolved.append(part)
            candidate = '/'.join(resolved)
            if candidate in links:
                follows += 1
                target = links[candidate]
                if follows > 40 or target.startswith('/') or '\\' in target:
                    raise SyncError('unsafe/cyclic final link: ' + name)
                resolved.pop()
                pending = target.split('/') + pending
        target = '/'.join(resolved)
        if target not in final and not any(p.startswith(target + '/') for p in final):
            raise SyncError('broken final link: ' + name)
    doc['symlinks'] = links
    doc['sync_managed_paths'] = sorted(new_managed)
    result = {'schema': 1, 'repository': str(repo), 'remote_url': url, 'branch': branch,
              'base_head': head, 'base_tree': tree, 'previous': str(Path(previous).resolve()),
              'snapshot': str(Path(new).resolve()), 'previous_digest': old_hash, 'snapshot_digest': new_hash,
              'protect': prefixes, 'reviewed_remote_changes': reviewed, 'changes': changes, 'result_manifest': doc}
    result['plan_sha256'] = digest(canonical(result))
    return result


def apply_plan(plan, bundle, message):
    if not message.strip():
        raise SyncError('commit message required')
    original = dict(plan)
    signature = original.pop('plan_sha256', None)
    if digest(canonical(original)) != signature:
        raise SyncError('plan was edited or corrupted')
    current = make_plan(plan['repository'], plan['previous'], plan['snapshot'], plan['branch'], plan['protect'], plan.get('reviewed_remote_changes'))
    if current != plan:
        raise SyncError('plan inputs or remote state drifted; replan')
    output = Path(bundle).resolve()
    if output.exists() or output.is_symlink():
        raise SyncError('bundle destination already exists')
    repo = Path(plan['repository'])
    with tempfile.TemporaryDirectory(prefix='polar-station-sync-') as temp:
        isolated = Path(temp)
        git(isolated, 'init', '--quiet')
        # Fetch local objects only, no checkout, hooks, filters or submodules.
        git(isolated, '-c', 'protocol.file.allow=always', 'fetch', '--quiet', '--no-tags', str(repo), plan['base_head'])
        git(isolated, 'read-tree', plan['base_tree'])
        for change in plan['changes']:
            p = change['path']
            if change['action'] == 'delete':
                git(isolated, 'update-index', '--force-remove', '--', p)
                continue
            f = Path(plan['snapshot']) / p
            data = os.readlink(f).encode() if change['after']['mode'] == '120000' else f.read_bytes()
            if digest(data) != change['after']['sha256']:
                raise SyncError('source changed during apply: ' + p)
            oid = git(isolated, 'hash-object', '-w', '--stdin', data=data).decode().strip()
            git(isolated, 'update-index', '--add', '--cacheinfo', change['after']['mode'], oid, p)
        data = json.dumps(plan['result_manifest'], indent=2, ensure_ascii=False).encode() + b'\n'
        oid = git(isolated, 'hash-object', '-w', '--stdin', data=data).decode().strip()
        git(isolated, 'update-index', '--add', '--cacheinfo', '100644', oid, MANIFEST)
        tree = git(isolated, 'write-tree').decode().strip()
        # Recheck source bytes/modes as well as the branch after local work.
        if snapshot(plan['snapshot'])[2] != plan['snapshot_digest']:
            raise SyncError('snapshot changed during apply')
        # No branch/ref in caller repo is touched.
        if remote_head(repo, plan['remote_url'], plan['branch']) != plan['base_head']:
            raise SyncError('remote changed during apply')
        commit = git(isolated, '-c', 'user.name=Polar Station Source Sync', '-c', 'user.email=source-sync@invalid',
                     'commit-tree', tree, '-p', plan['base_head'], data=message.encode()).decode().strip()
        git(isolated, 'update-ref', 'refs/heads/prepared-source-sync', commit)
        git(isolated, 'bundle', 'create', str(isolated / 'prepared.bundle'), 'refs/heads/prepared-source-sync', '^' + plan['base_head'])
        with output.open('xb') as destination, (isolated / 'prepared.bundle').open('rb') as source:
            shutil.copyfileobj(source, destination)
    return {'commit': commit, 'tree': tree, 'parent': plan['base_head'], 'remote_url': plan['remote_url'],
            'branch': plan['branch'], 'bundle': str(output), 'bundle_sha256': digest(output.read_bytes()),
            'published': False, 'plan_sha256': signature}


def publish_receipt(receipt, confirm=False):
    """Publish only with explicit operator approval; no force or caller mutation."""
    if not confirm:
        raise SyncError('publication requires --confirm-publication after reviewing the plan/receipt')
    if receipt.get('published') is not False:
        raise SyncError('receipt must describe an unpublished prepared commit')
    url, branch = receipt['remote_url'], receipt['branch']
    if not github_url(url):
        raise SyncError('receipt URL must be credential-free HTTPS or git-user SSH github.com')
    if not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9._/-]*', branch) or '..' in branch or branch.endswith('/'):
        raise SyncError('invalid receipt branch')
    for key in ('parent', 'tree', 'commit'):
        if not re.fullmatch('[0-9a-f]{40}', receipt[key]):
            raise SyncError('invalid receipt Git identity')
    if not re.fullmatch('[0-9a-f]{64}', receipt['plan_sha256']):
        raise SyncError('missing plan binding')
    bundle = Path(receipt['bundle']).resolve()
    if not bundle.is_file() or digest(bundle.read_bytes()) != receipt['bundle_sha256']:
        raise SyncError('bundle hash mismatch')
    with tempfile.TemporaryDirectory(prefix='polar-station-publish-') as temp:
        isolated = Path(temp)
        git(isolated, 'init', '--quiet')
        if remote_head(isolated, url, branch) != receipt['parent']:
            raise SyncError('remote HEAD changed; replan instead of publishing')
        git(isolated, 'fetch', '--quiet', '--no-tags', '--no-recurse-submodules', url, 'refs/heads/' + branch)
        if git(isolated, 'rev-parse', 'FETCH_HEAD').decode().strip() != receipt['parent']:
            raise SyncError('remote changed during prerequisite fetch')
        git(isolated, 'bundle', 'verify', str(bundle))
        git(isolated, '-c', 'protocol.file.allow=always', 'fetch', '--quiet', '--no-tags', '--no-recurse-submodules', str(bundle), 'refs/heads/prepared-source-sync')
        commit = git(isolated, 'rev-parse', 'FETCH_HEAD').decode().strip()
        parent = git(isolated, 'show', '-s', '--format=%P', commit).decode().strip()
        tree = git(isolated, 'show', '-s', '--format=%T', commit).decode().strip()
        if (commit, parent, tree) != (receipt['commit'], receipt['parent'], receipt['tree']):
            raise SyncError('bundle commit/tree/one-parent proof differs from receipt')
        if digest(bundle.read_bytes()) != receipt['bundle_sha256']:
            raise SyncError('bundle changed during import')
        if remote_head(isolated, url, branch) != receipt['parent']:
            raise SyncError('remote changed before push; replan')
        # No leading +, --force, --force-with-lease, hooks, or recursive submodules.
        git(isolated, 'push', '--porcelain', '--no-verify', '--recurse-submodules=no', url,
            commit + ':refs/heads/' + branch)
        observed = remote_head(isolated, url, branch)
        return {'published': True, 'commit': commit, 'tree': tree, 'parent': parent,
                'remote_url': url, 'branch': branch, 'observed_head': observed,
                'head_matches': observed == commit, 'plan_sha256': receipt['plan_sha256']}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest='command', required=True)
    p = sub.add_parser('plan')
    for name in ('repository', 'previous', 'snapshot', 'output'):
        p.add_argument('--' + name, required=True)
    p.add_argument('--branch', default='main')
    p.add_argument('--protect', action='append', default=[])
    p.add_argument('--reviewed-remote-changes', type=Path, help='Explicit path to exact reviewed remote identities; original baseline remains unchanged')
    p = sub.add_parser('apply')
    for name in ('plan', 'bundle', 'message'):
        p.add_argument('--' + name, required=True)
    p.add_argument('--receipt')
    p = sub.add_parser('publish')
    p.add_argument('--receipt', required=True)
    p.add_argument('--confirm-publication', action='store_true')
    args = parser.parse_args()
    try:
        if args.command == 'plan':
            out = Path(args.output)
            reviewed = json.loads(args.reviewed_remote_changes.read_text()) if args.reviewed_remote_changes else None
            result = make_plan(args.repository, args.previous, args.snapshot, args.branch, args.protect, reviewed)
            with out.open('x') as f:
                json.dump(result, f, indent=2, ensure_ascii=False)
                f.write('\n')
        elif args.command == 'apply':
            if args.receipt and Path(args.receipt).exists():
                raise SyncError('receipt destination already exists')
            result = apply_plan(json.loads(Path(args.plan).read_text()), args.bundle, args.message)
            if args.receipt:
                with Path(args.receipt).open('x') as f:
                    json.dump(result, f, indent=2)
                    f.write('\n')
        else:
            result = publish_receipt(json.loads(Path(args.receipt).read_text()), args.confirm_publication)
        print(json.dumps(result, indent=2, ensure_ascii=False))
    except (SyncError, OSError, ValueError) as error:
        parser.exit(1, 'SOURCE SYNC REFUSED: ' + str(error) + '\n')


if __name__ == '__main__':
    main()

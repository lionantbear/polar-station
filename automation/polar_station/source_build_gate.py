#!/usr/bin/env python3
"""Select the existing source compilation; metadata-only pushes are not builds."""
import json
import os
from pathlib import Path, PurePosixPath
import re
import subprocess


def metadata_only(name):
    path = PurePosixPath(name)
    if name in ('PUBLIC_SOURCE_MANIFEST.json', 'CI.md'):
        return True
    if name.startswith('docs/releases/'):
        return True
    if path.parent.as_posix() == 'automation/polar_station/release' and path.suffix == '.json':
        return True
    prefix = 'automation/polar_station/trial-release/'
    if name.startswith(prefix):
        rel = name[len(prefix):]
        return rel.startswith(('documents/', 'notices/')) or bool(
            re.fullmatch(r'release-(?:plan|review)-[^/]+\.json', rel))
    return False


def build_needed(root, before, after):
    if not re.fullmatch('[0-9a-f]{40}', before or '') or before == '0'*40:
        return True
    try:
        # Disable rename collapsing so moving source into a metadata directory
        # still exposes the deleted source path. NUL handles unusual filenames.
        data = subprocess.check_output(['git', '-C', str(root), 'diff', '--name-only',
            '-z', '--no-renames', before, after], stderr=subprocess.DEVNULL)
        names = data.decode('utf-8').rstrip('\0').split('\0') if data else []
    except (subprocess.CalledProcessError, UnicodeError):
        return True
    return not names or any(not metadata_only(name) for name in names)


def main():
    event = json.loads(Path(os.environ['GITHUB_EVENT_PATH']).read_text())
    needed = os.environ.get('GITHUB_EVENT_NAME') != 'push' or build_needed(
        Path.cwd(), event.get('before'), os.environ['GITHUB_SHA'])
    with open(os.environ['GITHUB_OUTPUT'], 'a') as out:
        out.write('build=' + str(needed).lower() + '\n')
    with open(os.environ['GITHUB_STEP_SUMMARY'], 'a') as out:
        out.write('Source compilation selected.\n' if needed else
            'Source compilation NOT RUN: only release documentation, reports or packet metadata changed. '
            'This run supplies no new compilation or product-acceptance evidence.\n')


if __name__ == '__main__':
    main()

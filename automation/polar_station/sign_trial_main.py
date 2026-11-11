#!/usr/bin/env python3
"""Explicit, main-app-only ad-hoc signing policy. Never alters host security."""
import argparse
import plistlib
import subprocess
import sys
import tempfile
from pathlib import Path

KEY = 'com.apple.security.cs.disable-library-validation'

def run(*args):
    return subprocess.run(args, check=True, capture_output=True).stdout

def sign(app):
    if sys.platform != 'darwin':
        raise RuntimeError('macOS required')
    app = Path(app).resolve(strict=True)
    signature = subprocess.run(['/usr/bin/codesign', '-d', '--verbose=4', str(app)], check=True, capture_output=True).stderr.decode()
    if 'Signature=adhoc' not in signature:
        raise RuntimeError('Refusing to weaken a non-ad-hoc application')
    raw = run('/usr/bin/codesign', '-d', '--entitlements', ':-', str(app))
    entitlements = plistlib.loads(raw) if raw.strip() else {}
    entitlements[KEY] = True
    with tempfile.TemporaryDirectory(prefix='polar-trial-sign-') as directory:
        path = Path(directory) / 'main.entitlements'
        path.write_bytes(plistlib.dumps(entitlements))
        # No --deep: the existing independent nested signatures remain intact.
        run('/usr/bin/codesign', '--force', '--options', 'runtime', '--preserve-metadata=identifier', '--entitlements', str(path), '--sign', '-', str(app))
    run('/usr/bin/codesign', '--verify', '--deep', '--strict', str(app))
    actual = plistlib.loads(run('/usr/bin/codesign', '-d', '--entitlements', ':-', str(app)))
    if actual != entitlements:
        raise RuntimeError('Signed entitlements differ from requested policy')
    print('Main-app ad-hoc entitlement verified; nested signatures preserved.')

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('app')
    sign(parser.parse_args().app)

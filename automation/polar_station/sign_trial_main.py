#!/usr/bin/env python3
"""Explicit, main-app-only ad-hoc signing policy. Never alters host security."""
import argparse
import plistlib
import subprocess
import sys
import tempfile
from pathlib import Path

KEY = 'com.apple.security.cs.disable-library-validation'
AUDIO_INPUT = 'com.apple.security.device.audio-input'

def run(*args):
    return subprocess.run(args, check=True, capture_output=True).stdout

def microphone_usage(app):
    info = plistlib.loads((app / 'Contents/Info.plist').read_bytes())
    usage = info.get('NSMicrophoneUsageDescription')
    if not isinstance(usage, str) or not usage.strip():
        raise RuntimeError('Main app is missing NSMicrophoneUsageDescription')

def verify(app):
    app = Path(app).resolve(strict=True)
    run('/usr/bin/codesign', '--verify', '--deep', '--strict', str(app))
    actual = plistlib.loads(run('/usr/bin/codesign', '-d', '--entitlements', ':-', str(app)))
    if actual.get(AUDIO_INPUT) is not True:
        raise RuntimeError('Final main signature is missing audio-input entitlement')
    microphone_usage(app)
    return actual

def sign(app):
    if sys.platform != 'darwin':
        raise RuntimeError('macOS required')
    app = Path(app).resolve(strict=True)
    signature = subprocess.run(['/usr/bin/codesign', '-d', '--verbose=4', str(app)], check=True, capture_output=True).stderr.decode()
    if 'Signature=adhoc' not in signature:
        raise RuntimeError('Refusing to weaken a non-ad-hoc application')
    raw = run('/usr/bin/codesign', '-d', '--entitlements', ':-', str(app))
    entitlements = plistlib.loads(raw) if raw.strip() else {}
    microphone_usage(app)
    entitlements[KEY] = True
    entitlements[AUDIO_INPUT] = True
    with tempfile.TemporaryDirectory(prefix='polar-trial-sign-') as directory:
        path = Path(directory) / 'main.entitlements'
        path.write_bytes(plistlib.dumps(entitlements))
        # No --deep: the existing independent nested signatures remain intact.
        run('/usr/bin/codesign', '--force', '--options', 'runtime', '--preserve-metadata=identifier', '--entitlements', str(path), '--sign', '-', str(app))
    actual = verify(app)
    if actual != entitlements:
        raise RuntimeError('Signed entitlements differ from requested policy')
    print('Main-app ad-hoc microphone entitlement verified; nested signatures preserved.')

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('app')
    parser.add_argument('--verify-only', action='store_true')
    args = parser.parse_args()
    if args.verify_only:
        verify(args.app)
        print('Final main-app microphone signature and usage description verified.')
    else:
        sign(args.app)

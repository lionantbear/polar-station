#!/usr/bin/env python3
"""Repackage the immutable reviewed trial with corrected main-app signing."""
import argparse
from contextlib import contextmanager
import hashlib
import json
import os
from pathlib import Path
import plistlib
import subprocess
import sys

OLD_SHA = 'ffd0a002afa1ac88cb2fc5374554fa773f37d80dc997871b73450c3e230c4173'
DMG = 'Polar-Station-macOS-arm64-TRIAL-UNNOTARIZED.dmg'
ITEMS = ['Polar Station.app', 'THIRD-PARTY-NOTICES', 'LICENSE-AGPL-3.0.txt', 'READ-ME-FIRST.txt', 'SOURCE-ACCESS.txt']

def run(*args):
    subprocess.run(args, check=True)

def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def inventory(root):
    return {str(p.relative_to(root)): (('symlink', os.readlink(p)) if p.is_symlink() else ('file', digest(p))) for p in root.rglob('*') if p.is_symlink() or p.is_file()}

@contextmanager
def mounted(dmg, mount):
    mount.mkdir()
    run('/usr/bin/hdiutil', 'attach', '-readonly', '-nobrowse', '-mountpoint', str(mount), str(dmg))
    try:
        yield mount
    finally:
        run('/usr/bin/hdiutil', 'detach', str(mount))

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--original', type=Path, required=True)
    parser.add_argument('--work', type=Path, required=True)
    args = parser.parse_args()
    assert sys.platform == 'darwin', 'macOS required'
    assert not any(os.environ.get(k) for k in ['GH_TOKEN','GITHUB_TOKEN']), 'No credentials in app execution job'
    assert digest(args.original) == OLD_SHA, 'Original reviewed DMG mismatch'
    work = args.work.resolve(); work.mkdir()
    tools = Path(__file__).resolve().parent
    volume = work/'volume'; volume.mkdir()
    run('/usr/bin/hdiutil','verify',str(args.original))
    with mounted(args.original, work/'original-mount') as mount:
        for name in ITEMS:
            assert (mount/name).exists(), name
            run('/usr/bin/ditto','--rsrc','--extattr',str(mount/name),str(volume/name))
        before = inventory(volume/'Polar Station.app')
        notices = {name: inventory(volume/name) if (volume/name).is_dir() else digest(volume/name) for name in ITEMS[1:]}
    app = volume/'Polar Station.app'
    info = plistlib.loads((app/'Contents/Info.plist').read_bytes())
    run('/usr/bin/codesign','--verify','--deep','--strict',str(app))
    # The regression policy must fail the historical binary before the correction.
    old_entitlements = subprocess.run(['/usr/bin/codesign','-d','--entitlements',':-',str(app)],check=True,capture_output=True).stdout
    old_entitlements = plistlib.loads(old_entitlements) if old_entitlements.strip() else {}
    assert old_entitlements.get('com.apple.security.cs.disable-library-validation') is not True, 'Unexpected source signature policy'
    run(sys.executable,str(tools/'sign_trial_main.py'),str(app))
    after = inventory(app)
    assert set(before) == set(after), 'Signing changed bundle membership'
    changed = sorted(k for k in before if before[k] != after[k])
    allowed = {'Contents/MacOS/'+info['CFBundleExecutable'], 'Contents/_CodeSignature/CodeResources'}
    assert changed and set(changed) <= allowed, ('Unexpected signing changes',changed)
    text = '''Polar Station ad-hoc launch correction\n\nThis trial is the previously reviewed application binary with its main executable\nre-signed using com.apple.security.cs.disable-library-validation=true. Nested\nframeworks, helper binaries, resources, licenses and source access instructions\nare unchanged. This is a packaging/signing correction, not a full recompilation.\n\nThe exception is limited to this ad-hoc main application. It permits loading\nnon-Team-ID libraries, including the bundled ad-hoc Sparkle framework, and\nreduces this process's library-injection protection. No SIP, Gatekeeper or\nsystem security settings are changed. This is not Developer ID signing or\nnotarization; macOS may still require normal user approval to open it.\n\nSource signing logic and validation scripts:\nhttps://github.com/lionantbear/polar-station\n\nCI tests prove signature policy and terminal command execution on the tested\nmacOS runner. They do not prove clean VM, Gatekeeper, visual GUI, TCC or updater\nacceptance. The old public release is retained for traceability.\n'''
    (volume/'SIGNING-FIX.txt').write_text(text)
    (volume/'Applications').symlink_to('/Applications')
    public=work/'public'; public.mkdir()
    run('/usr/bin/hdiutil','create','-fs','HFS+','-volname','Polar Station Trial','-srcfolder',str(volume),'-format','UDZO',str(public/DMG))
    run('/usr/bin/hdiutil','verify',str(public/DMG))
    with mounted(public/DMG,work/'final-mount') as mount:
        assert inventory(mount/'Polar Station.app') == after, 'Final DMG differs'
        for name,value in notices.items():
            assert (inventory(mount/name) if (mount/name).is_dir() else digest(mount/name)) == value, 'Notice modified'
        install=work/'installed'; install.mkdir()
        run('/usr/bin/ditto','--rsrc','--extattr',str(mount/'Polar Station.app'),str(install/'Polar Station.app'))
    installed=install/'Polar Station.app'
    assert inventory(installed)==after, 'Install copy differs'
    run(sys.executable,str(tools/'verify_trial_launch.py'),'--app',str(installed),'--output',str(work/'launch-evidence'))
    result=json.loads((work/'launch-evidence/result.json').read_text())
    assert result['passed'] is True and result['terminal_child_executed'] is True
    assert inventory(installed)==after, 'Launch modified signed application'
    sha=digest(public/DMG)
    (public/'SHA256SUMS').write_text(sha+'  '+DMG+'\n')
    report={'passed':True,'original_dmg_sha256':OLD_SHA,'final_dmg_sha256':sha,'changed_signature_files':changed,'original_policy_regression_detected':True,'nested_signatures_and_resources_unchanged':True,'notice_files_unchanged':True,'source_commit':os.environ.get('GITHUB_SHA'),'terminal_launch':result}
    (work/'validation.json').write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(report,indent=2))

if __name__ == '__main__':
    main()

#!/usr/bin/env python3
"""Repackage the immutable reviewed trial with corrected main-app signing."""
import argparse
from contextlib import contextmanager
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
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
    parser.add_argument('--original-sha256', default=OLD_SHA)
    parser.add_argument('--required-entitlement', choices=(
        'com.apple.security.cs.disable-library-validation',
        'com.apple.security.device.audio-input'),
        default='com.apple.security.cs.disable-library-validation')
    args = parser.parse_args()
    assert sys.platform == 'darwin', 'macOS required'
    assert not any(os.environ.get(k) for k in ['GH_TOKEN','GITHUB_TOKEN']), 'No credentials in app execution job'
    assert re.fullmatch('[0-9a-f]{64}', args.original_sha256), 'Exact original digest required'
    assert digest(args.original) == args.original_sha256, 'Original reviewed DMG mismatch'
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
    assert old_entitlements.get(args.required_entitlement) is not True, 'Expected missing-capability regression is absent'
    run(sys.executable,str(tools/'sign_trial_main.py'),str(app))
    after = inventory(app)
    assert set(before) == set(after), 'Signing changed bundle membership'
    changed = sorted(k for k in before if before[k] != after[k])
    allowed = {'Contents/MacOS/'+info['CFBundleExecutable'], 'Contents/_CodeSignature/CodeResources'}
    assert changed and set(changed) <= allowed, ('Unexpected signing changes',changed)
    policy_commit = os.environ['GITHUB_SHA']
    payload_commit = info.get('PolarStationSourceCommit')
    assert isinstance(payload_commit, str) and re.fullmatch('[0-9a-f]{40}', payload_commit)
    assert re.fullmatch('[0-9a-f]{40}', policy_commit)
    text = ('Polar Station final-signature correction\n\n'
        'Required main-app capability restored: '+args.required_entitlement+'=true.\n'
        'The application code is the previously reviewed compiled payload; this is a signing/package correction, not a full source recompilation. Nested signatures, helpers, resources, complete notices and original source/relink documents remain unchanged. No system security or TCC database setting is changed. Microphone access still requires normal macOS user consent.\n\n'
        'Exact compiled application source:\nhttps://github.com/lionantbear/polar-station/tree/'+payload_commit+'\n\n'
        'Exact corrected signing/build source and packaging validation:\nhttps://github.com/lionantbear/polar-station/tree/'+policy_commit+'\n\n'
        'To rebuild, use the existing SOURCE-ACCESS.txt upstream/SDK/relink directions with the corrected source checkout above. The compiled application sources are unchanged; its final builder/signing policy now restores audio input and verifies the final signature plus microphone usage description. You may also rebuild the original application source, then apply the corrected automation/polar_station/sign_trial_main.py to the resulting ad-hoc signed app and use --verify-only to inspect it. No publisher key is required.\n\n'
        'This remains an ad-hoc signed, unnotarized Apple Silicon trial. Native tests verify the actual signed capability, LaunchServices/terminal behavior and unchanged nested payload. First physical microphone consent/recording and full clean-VM acceptance remain separate.\n')
    (volume/'SIGNING-FIX.txt').write_text(text)
    (volume/'Applications').symlink_to('/Applications')
    public=work/'public'; public.mkdir()
    run('/usr/bin/hdiutil','create','-fs','HFS+','-volname','Polar Station Trial','-srcfolder',str(volume),'-format','UDZO',str(public/DMG))
    run('/usr/bin/hdiutil','verify',str(public/DMG))
    with mounted(public/DMG,work/'final-mount') as mount:
        assert inventory(mount/'Polar Station.app') == after, 'Final DMG differs'
        run(sys.executable,str(tools/'sign_trial_main.py'),'--verify-only',str(mount/'Polar Station.app'))
        for name,value in notices.items():
            assert (inventory(mount/name) if (mount/name).is_dir() else digest(mount/name)) == value, 'Notice modified'
        install=work/'installed'; install.mkdir()
        run('/usr/bin/ditto','--rsrc','--extattr',str(mount/'Polar Station.app'),str(install/'Polar Station.app'))
    installed=install/'Polar Station.app'
    assert inventory(installed)==after, 'Install copy differs'
    run(sys.executable,str(tools/'sign_trial_main.py'),'--verify-only',str(installed))
    run('/usr/bin/swift',str(tools/'verify_trial_app.swift'),'--app',str(installed),'--report',str(work/'launchservices.json'))
    gui=json.loads((work/'launchservices.json').read_text()); assert gui['passed'] is True
    run(sys.executable,str(tools/'verify_trial_launch.py'),'--app',str(installed),'--output',str(work/'launch-evidence'))
    result=json.loads((work/'launch-evidence/result.json').read_text())
    assert result['passed'] is True and result['terminal_child_executed'] is True
    assert inventory(installed)==after, 'Launch modified signed application'
    sha=digest(public/DMG)
    (public/'SHA256SUMS').write_text(sha+'  '+DMG+'\n')
    report={'passed':True,'original_dmg_sha256':args.original_sha256,'final_dmg_sha256':sha,'changed_signature_files':changed,'original_policy_regression_detected':True,'nested_signatures_and_resources_unchanged':True,'notice_files_unchanged':True,'source_commit':policy_commit,'application_source_commit':payload_commit,'signing_source_commit':policy_commit,'required_entitlement':args.required_entitlement,'audio_input_entitlement_verified':True,'microphone_usage_description_verified':True,'terminal_launch':result,'launchservices':gui}
    (work/'validation.json').write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(report,indent=2))

if __name__ == '__main__':
    main()

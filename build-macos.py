#!/usr/bin/env python3
"""Build Polar Station for Apple Silicon. See BUILD_MACOS.md for prerequisites."""
import argparse
import json
import time
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tempfile


def run(*command, **kwargs):
    started = time.monotonic()
    result = subprocess.run([str(arg) for arg in command], **kwargs)
    print('STATION_BUILD_STEP=' + json.dumps({'tool': str(command[0]),
          'elapsed_seconds': round(time.monotonic() - started, 3),
          'exit_code': result.returncode}), flush=True)
    result.check_returncode()


# These component inputs are shared with the private cache adapter.
GODOT_PIN = 'ed1daf0bf001b61586d9930840f2f1394092c079'
GODOT_OPTIONS = ('platform=macos', 'arch=arm64', 'target=template_release', 'library_type=shared_library', 'generate_bundle=no', 'vulkan=no', 'metal=yes', 'angle=no', 'opengl3=yes', 'debug_symbols=no', 'optimize=speed', 'disable_path_overrides=no', 'module_jolt_physics_enabled=no', 'module_godot_physics_2d_enabled=no', 'module_godot_physics_3d_enabled=no')
GODOT_PATCH = ('platform/macos/libgodot_macos.mm', 'new OS_MacOS_NSApp(', 'new OS_MacOS_Headless(')


def godot_toolchain(sdk):
    import hashlib
    settings = sdk / 'SDKSettings.json'
    return {'compiler': subprocess.check_output(['xcrun', 'clang', '--version'], text=True).strip(),
            'xcode': subprocess.check_output(['xcodebuild', '-version'], text=True).strip(),
            'sdk': hashlib.sha256(settings.read_bytes()).hexdigest(),
            'scons': subprocess.check_output(['scons', '--version'], text=True).strip(),
            'deployment_target': '15.0'}


def godot_inputs(root, toolchain):
    import hashlib
    modules = root / 'apps/polar-graphics-native/godot-modules'
    return {'upstream_commit': GODOT_PIN, 'options': list(GODOT_OPTIONS),
            'patch': list(GODOT_PATCH), 'toolchain': toolchain,
            'modules': {p.relative_to(modules).as_posix(): hashlib.sha256(p.read_bytes()).hexdigest()
                        for p in sorted(modules.rglob('*')) if p.is_file()}}


def validate_godot_prebuilt(directory, inputs):
    import hashlib, json
    manifest = json.loads((directory / 'manifest.json').read_text())
    library = directory / 'libgodot.dylib'
    if (manifest.get('schema_version') != 1 or manifest.get('inputs') != inputs
            or library.is_symlink() or not library.is_file()
            or hashlib.sha256(library.read_bytes()).hexdigest() != manifest.get('library_sha256')):
        raise ValueError('Godot prebuilt inputs or library digest differ')
    if sys.platform == 'darwin':
        architectures = subprocess.check_output(['lipo', '-archs', str(library)], text=True).split()
        if architectures != ['arm64']:
            raise ValueError('Godot prebuilt must contain only arm64')
        symbols = subprocess.check_output(['nm', '-gU', str(library)], text=True)
        for symbol in ('_libgodot_create_godot_instance', '_libgodot_destroy_godot_instance'):
            if symbol not in {line.split()[-1] for line in symbols.splitlines() if line.split()}:
                raise ValueError('Godot prebuilt is missing ' + symbol)
    return library


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--ghostty', type=Path, required=True, help='Ghostty v1.4.0 checkout (read only)')
    parser.add_argument('--godot-prebuilt', type=Path, help='Verified prebuilt Godot directory')
    parser.add_argument('--godot', type=Path, required=True, help='pinned Godot checkout (read only)')
    parser.add_argument('--editor', type=Path, required=True, help='Godot 4.7.2 editor executable')
    parser.add_argument('--graphviz', type=Path, required=True, help='Graphviz installation prefix')
    parser.add_argument('--sdk', type=Path, required=True, help='Zig-compatible macOS 15 SDK')
    parser.add_argument('--zig', default='zig', help='Zig 0.17.0 executable')
    parser.add_argument('--jobs', type=int, default=3)
    parser.add_argument('--debug', action='store_true')
    args = parser.parse_args()
    if sys.platform != 'darwin' or os.uname().machine != 'arm64':
        parser.error('macOS on Apple Silicon is required')
    for tool in ('cargo', 'rustup', 'git', 'scons', 'xcodebuild', 'msgfmt', args.zig):
        if not shutil.which(tool):
            parser.error(f'missing {tool}; see BUILD_MACOS.md')
    if args.jobs < 1 or not args.sdk.is_dir():
        parser.error('a positive --jobs value and an existing --sdk are required')
    if subprocess.check_output([args.zig, 'version'], text=True).strip() != '0.17.0':
        parser.error('Zig 0.17.0 is required')
    if 'v4.9.1' not in subprocess.check_output(['scons', '--version'], text=True):
        parser.error('SCons 4.9.1 is required')
    root = Path(__file__).resolve().parent
    sys.path[:0] = [str(root / 'apps/polar-tmux-shared/ghostty'),
                   str(root / 'apps/polar-graphics-native')]
    from overlay import install as apply_display
    from build_cache import workspace, sync_tree, copy_changed, cache_framework, rust_cache_environment
    from resources_macos import package as package_graphics
    configuration = 'Debug' if args.debug else 'Release'
    profile = 'debug' if args.debug else 'release'
    optimize = 'Debug' if args.debug else 'ReleaseFast'
    target = root / 'target'
    target.mkdir(exist_ok=True)
    (target / 'deps').mkdir(exist_ok=True)
    env = rust_cache_environment(os.environ.copy())
    if env.get('RUSTC_WRAPPER'):
        print('STATION_RUSTC_WRAPPER=' + env['RUSTC_WRAPPER'], flush=True)
    env.update(SDKROOT=str(args.sdk.resolve()), MACOSX_DEPLOYMENT_TARGET='15.0')
    # Keep dependency edits/build outputs in this checkout's target directory.
    with workspace(root, 'macos') as work:
        with tempfile.TemporaryDirectory(prefix='source-', dir=work) as temporary:
            prepared = Path(temporary)
            for name, source, pin in (
                ('ghostty', args.ghostty, '83fb95624b0439ee660c2679b43b38c8b63769cd'),
                ('godot', args.godot, 'ed1daf0bf001b61586d9930840f2f1394092c079'),
            ):
                destination = target / 'deps/godot' if name == 'godot' else prepared / name
                if not destination.exists():
                    run('git', 'clone', '--no-checkout', '--', source.resolve(), destination)
                    run('git', '-C', destination, 'checkout', '--detach', pin)
                elif subprocess.check_output(['git', '-C', str(destination), 'rev-parse', 'HEAD'], text=True).strip() != pin:
                    raise ValueError('cached Godot revision differs; choose a fresh target/deps directory')
            ghostty, godot = prepared / 'ghostty', target / 'deps/godot'
            apply_display(ghostty, 'macos')
            project = ghostty / 'macos/Ghostty.xcodeproj/project.pbxproj'
            text = project.read_text()
            marker = 'relativePath = ../../../polarinfrav1/apps/polar-station-macos;'
            if text.count(marker) != 1:
                raise ValueError('pinned Swift package reference changed')
            import json
            relative = os.path.relpath(root / 'apps/polar-station-macos', work / 'ghostty/macos')
            project.write_text(text.replace(marker, 'relativePath = ' + json.dumps(relative) + ';'))
            module = ghostty / 'macos/Sources/PolarStation'
            module.mkdir(parents=True, exist_ok=True)
            for path in (root / 'apps/polar-station-macos/ghostty-module').iterdir():
                if path.is_file():
                    shutil.copy2(path, module / path.name)
            for path in (root / 'apps/polar-station-macos/Resources').iterdir():
                if path.is_file():
                    shutil.copy2(path, module / path.name)
            cache_framework(ghostty)
            ghostty = sync_tree(ghostty, work / 'ghostty')
        entry = godot / GODOT_PATCH[0]
        text = entry.read_text()
        if text.count(GODOT_PATCH[1]) == 1:
            entry.write_text(text.replace(GODOT_PATCH[1], GODOT_PATCH[2]))
        elif text.count(GODOT_PATCH[2]) != 1:
            raise ValueError('pinned libgodot entry changed')
        if args.godot_prebuilt:
            library = validate_godot_prebuilt(args.godot_prebuilt, godot_inputs(root, godot_toolchain(args.sdk)))
            (godot / 'bin').mkdir(exist_ok=True)
            shutil.copy2(library, godot / 'bin/libgodot.macos.template_release.arm64.dylib')
        else:
            run('scons', '-C', godot, '-j', args.jobs, *GODOT_OPTIONS,
                'custom_modules=' + str(root / 'apps/polar-graphics-native/godot-modules'), env=env)
        cargo = ['cargo', 'build', '--locked', '--manifest-path', root / 'Cargo.toml',
                 '--target-dir', Path(env.get('CARGO_TARGET_DIR', target / 'rust')).resolve(), '--target', 'aarch64-apple-darwin']
        if not args.debug:
            cargo.append('--release')
        # Separate invocations prevent Host features from entering the desktop archive.
        run(*cargo, '-p', 'polar-tmux', '--bin', 'polar-tmux', env=env)
        run(*cargo, '-p', 'polar-station-desktop-ffi', '--lib', env=env)
        run(*cargo, '-p', 'polar-graphics-godot', '--lib', env=env)
        rust = Path(env.get('CARGO_TARGET_DIR', target / 'rust')).resolve() / 'aarch64-apple-darwin' / profile
        client = target / 'polar-station-client/current'
        client.mkdir(parents=True, exist_ok=True)
        copy_changed(rust / 'libpolar_station_desktop_ffi.a', client / 'libpolar_station_desktop_ffi.a')
        env['POLAR_STATION_CLIENT_LIB_DIR'] = str(client)
        # The pinned Zig SDK query needs macOS 15 even when Xcode uses a newer SDK.
        wrapper = work / 'tools'
        wrapper.mkdir(exist_ok=True)
        (wrapper / 'xcrun').write_text('#!/bin/sh\nif [ "$*" = "--sdk macosx --show-sdk-path" ]; then\n'
                                     '  printf "%s\\n" "$SDKROOT"\nelse\n  exec /usr/bin/xcrun "$@"\nfi\n')
        (wrapper / 'xcrun').chmod(0o755)
        env['PATH'] = str(wrapper) + os.pathsep + env['PATH']
        run(args.zig, 'build', '-Dversion-string=1.4.0', '-Dpolar-tmux=true', '-Demit-macos-app=false',
            '-Dxcframework-target=native', '-Doptimize=' + optimize, '--summary', 'all', cwd=ghostty, env=env)
        xcode_env = {key: os.environ[key] for key in ('HOME', 'DEVELOPER_DIR') if key in os.environ}
        xcode_env['PATH'] = '/usr/bin:/bin:/usr/sbin:/sbin'
        run('xcodebuild', '-project', 'macos/Ghostty.xcodeproj', '-scheme', 'Ghostty',
            '-configuration', configuration, '-derivedDataPath', work / 'DerivedData',
            'SYMROOT=' + str(work / 'xcode'), '-showBuildTimingSummary',
            'ARCHS=arm64', 'ONLY_ACTIVE_ARCH=YES', 'MACOSX_DEPLOYMENT_TARGET=15.0',
            'ENABLE_CODE_COVERAGE=NO', 'CLANG_ENABLE_CODE_COVERAGE=NO',
            'CODE_SIGNING_ALLOWED=NO', 'CODE_SIGNING_REQUIRED=NO', 'CODE_SIGN_IDENTITY=',
            'build', cwd=ghostty, env=xcode_env)
        compiled_app = work / 'xcode' / configuration / 'Polar Station.app'
        app = work / 'package' / configuration / compiled_app.name
        app.parent.mkdir(parents=True, exist_ok=True)
        if app.exists():
            shutil.rmtree(app)
        run('ditto', '--rsrc', '--extattr', compiled_app, app)
        resources = app / 'Contents/Resources'
        graphics = work / 'graphics'
        package_graphics(root, godot / 'bin/libgodot.macos.template_release.arm64.dylib',
                         args.editor.resolve(), rust / 'libpolar_graphics_godot.dylib',
                         args.graphviz.resolve(), graphics)
        run('ditto', '--rsrc', '--extattr', graphics, resources / 'graphics')
        helpers = app / 'Contents/Helpers'
        helpers.mkdir(exist_ok=True)
        shutil.copy2(rust / 'polar-tmux', helpers)
        info = app / 'Contents/Info.plist'
        data = plistlib.loads(info.read_bytes())
        data.update(PolarStationBuildConfiguration=configuration, PolarStationSourceCommit='local',
                    PolarStationDesktopContract=2,
                    NSAppleEventsUsageDescription='Send enabled Codex completion notifications through Messages.')
        if not data.get('NSMicrophoneUsageDescription', '').strip():
            raise ValueError('application is missing NSMicrophoneUsageDescription')
        info.write_bytes(plistlib.dumps(data))
        host_entitlements = root / 'apps/polar-station-macos/host.entitlements'
        run('codesign', '--force', '--identifier', 'com.polarealm.polar-station.host',
            '--options', 'runtime', '--entitlements', host_entitlements, '--sign', '-', helpers / 'polar-tmux')
        for path in (resources / 'graphics').rglob('*'):
            if path.is_file() and 'Mach-O' in subprocess.check_output(['file', '-b', str(path)], text=True):
                run('codesign', '--force', '--options', 'runtime', '--sign', '-', path)
        name = 'GhosttyDebug.entitlements' if args.debug else 'Ghostty.entitlements'
        entitlements = plistlib.loads((ghostty / 'macos' / name).read_bytes())
        entitlements.update({'com.apple.security.device.audio-input': True,
                             'com.apple.security.cs.disable-library-validation': True})
        main_entitlements = work / 'main.entitlements'
        main_entitlements.write_bytes(plistlib.dumps(entitlements))
        run('codesign', '--force', '--deep', '--options', 'runtime', '--preserve-metadata=identifier',
            '--entitlements', main_entitlements, '--sign', '-', app)
        actual = plistlib.loads(subprocess.check_output(['codesign', '-d', '--entitlements', ':-', str(app)]))
        if actual.get('com.apple.security.device.audio-input') is not True:
            raise ValueError('final application signature is missing microphone permission')
        host_actual = plistlib.loads(subprocess.check_output([
            'codesign', '-d', '--entitlements', ':-', str(helpers / 'polar-tmux')]))
        if host_actual.get('com.apple.security.automation.apple-events') is not True:
            raise ValueError('final Host signature is missing Messages permission')
        output = target / 'macos' / configuration / app.name
        output.parent.mkdir(parents=True, exist_ok=True)
        if output.exists():
            shutil.rmtree(output)
        run('ditto', '--rsrc', '--extattr', app, output)
        run('codesign', '--verify', '--deep', '--strict', output)
        print(output)


if __name__ == '__main__':
    main()

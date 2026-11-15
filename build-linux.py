#!/usr/bin/env python3
"""Build Polar Station for Linux x86_64/Wayland. See BUILD_LINUX.md."""
import argparse
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile


def run(*command, **kwargs):
    subprocess.run([str(arg) for arg in command], check=True, **kwargs)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--ghostty', type=Path, required=True, help='Ghostty v1.3.1 checkout (read only)')
    parser.add_argument('--godot', type=Path, required=True, help='pinned Godot checkout (read only)')
    parser.add_argument('--editor', type=Path, required=True, help='Godot 4.7.2 editor executable')
    parser.add_argument('--graphviz', type=Path, required=True, help='Graphviz sysroot with usr/bin and usr/lib')
    parser.add_argument('--sysroot', type=Path, required=True, help='GTK development sysroot')
    parser.add_argument('--zig', default='zig', help='Zig 0.15.2 executable')
    parser.add_argument('--jobs', type=int, default=3)
    parser.add_argument('--debug', action='store_true')
    args = parser.parse_args()
    if sys.platform != 'linux' or os.uname().machine != 'x86_64':
        parser.error('Linux x86_64 with Wayland is required')
    for tool in ('cargo', 'git', 'scons', 'cc', 'pkg-config', 'readelf', args.zig):
        if not shutil.which(tool):
            parser.error(f'missing {tool}; see BUILD_LINUX.md')
    if args.jobs < 1 or not args.sysroot.is_dir():
        parser.error('a positive --jobs value and an existing --sysroot are required')
    if subprocess.check_output([args.zig, 'version'], text=True).strip() != '0.15.2':
        parser.error('Zig 0.15.2 is required')
    if 'v4.11.1' not in subprocess.check_output(['scons', '--version'], text=True):
        parser.error('SCons 4.11.1 is required')
    if not subprocess.check_output([str(args.editor.resolve()), '--version'], text=True).startswith('4.7.2.stable'):
        parser.error('Godot 4.7.2 editor is required')
    root = Path(__file__).resolve().parent
    sys.path[:0] = [str(root / 'apps/polar-tmux-shared/ghostty'),
                   str(root / 'apps/polar-graphics-native')]
    from overlay import install as apply_display
    from resources import build as package_graphics
    target = root / 'target'
    target.mkdir(exist_ok=True)
    (target / 'deps').mkdir(exist_ok=True)
    profile = 'debug' if args.debug else 'release'
    sysroot = args.sysroot.resolve()
    env = os.environ.copy()
    env.update(PATH=str(sysroot / 'usr/bin') + os.pathsep + env['PATH'],
               PYTHONPATH=str(sysroot / 'usr/lib/python3/dist-packages'),
               LD_LIBRARY_PATH=str(sysroot / 'usr/lib/x86_64-linux-gnu') +
                   (os.pathsep + env['LD_LIBRARY_PATH'] if env.get('LD_LIBRARY_PATH') else ''),
               PKG_CONFIG_SYSROOT_DIR=str(sysroot),
               PKG_CONFIG_LIBDIR=str(sysroot / 'usr/lib/x86_64-linux-gnu/pkgconfig') +
                   os.pathsep + str(sysroot / 'usr/share/pkgconfig'))
    with tempfile.TemporaryDirectory(prefix='station-', dir=target) as temporary:
        work = Path(temporary)
        for name, source, pin in (
            ('ghostty', args.ghostty, '332b2aefc6e72d363aa93ab6ecfc86eeeeb5ed28'),
            ('godot', args.godot, 'ed1daf0bf001b61586d9930840f2f1394092c079'),
        ):
            destination = target / 'deps/godot' if name == 'godot' else work / name
            if not destination.exists():
                run('git', 'clone', '--no-checkout', '--', source.resolve(), destination)
                run('git', '-C', destination, 'checkout', '--detach', pin)
            elif subprocess.check_output(['git', '-C', str(destination), 'rev-parse', 'HEAD'], text=True).strip() != pin:
                raise ValueError('cached Godot revision differs; choose a fresh target/deps directory')
        ghostty, godot = work / 'ghostty', target / 'deps/godot'
        run('git', '-C', ghostty, 'apply',
            root / 'apps/polar-station-linux/ghostty-overlay/ghostty-v1.3.1.patch')
        apply_display(ghostty, 'linux')
        module = ghostty / 'src/apprt/gtk/polar_tmux'
        shutil.copytree(root / 'apps/polar-station-linux/ghostty-module', module)
        for header in (root / 'apps/polar-tmux-shared/display/station_display.h',
                       root / 'crates/polar-station-desktop-ffi/include/polar_station_desktop_client.h'):
            shutil.copy2(header, module)
        run('scons', '-C', godot, '-j', args.jobs, 'platform=linuxbsd', 'arch=x86_64',
            'target=template_release', 'library_type=shared_library', 'vulkan=yes', 'opengl3=yes',
            'x11=no', 'wayland=no', 'debug_symbols=no', 'optimize=speed', 'disable_path_overrides=no',
            'module_jolt_physics_enabled=no', 'module_godot_physics_2d_enabled=no',
            'module_godot_physics_3d_enabled=no',
            'custom_modules=' + str(root / 'apps/polar-graphics-native/godot-modules'))
        cargo = ['cargo', 'build', '--locked', '--manifest-path', root / 'Cargo.toml', '--target-dir', target]
        if not args.debug:
            cargo.append('--release')
        # Keep Host and frontend feature resolution separate.
        run(*cargo, '-p', 'polar-tmux', '--bin', 'polar-tmux', '--lib')
        run(*cargo, '-p', 'polar-station-desktop-ffi', '-p', 'polar-station-client', '--lib')
        run(*cargo, '-p', 'polar-graphics-godot', '--lib')
        # Avoid exposing sysroot FreeType headers or absolute library RUNPATHs to Zig.
        prefix = work / 'prefix/usr'
        (prefix / 'include').mkdir(parents=True)
        for name in ('bin', 'share'):
            (prefix / name).symlink_to(sysroot / 'usr' / name, target_is_directory=True)
        for path in (sysroot / 'usr/include').iterdir():
            if path.name not in ('freetype', 'ft2build.h'):
                (prefix / 'include' / path.name).symlink_to(path, target_is_directory=path.is_dir())
        run(args.zig, 'build', '--search-prefix', prefix, '-Dpolar-tmux=true',
            '-Dpolar-station-client-lib-dir=' + str(target / profile), '-Demit-exe=true',
            '-Dapp-runtime=gtk', '-fsys=fontconfig', '-fsys=harfbuzz',
            '-Dgtk-wayland=true', '-Dgtk-x11=false',
            '-Doptimize=' + ('Debug' if args.debug else 'ReleaseFast'), cwd=ghostty, env=env)
        graphics = work / 'graphics'
        old_env = os.environ.copy()
        try:
            os.environ.update(env)
            package_graphics(root, graphics, godot / 'bin/libgodot.linuxbsd.template_release.x86_64.so',
                             args.editor.resolve(), target / profile / 'libpolar_graphics_godot.so',
                             args.graphviz.resolve())
        finally:
            os.environ.clear()
            os.environ.update(old_env)
        output = target / 'linux' / profile
        if output.exists():
            shutil.rmtree(output)
        shutil.copytree(ghostty / 'zig-out', output)
        shutil.copy2(target / profile / 'polar-tmux', output / 'bin')
        shutil.copytree(graphics, output / 'bin/graphics')
        shutil.copytree(root / 'apps/polar-station-linux/gnome-extension',
                        output / 'share/polar-station/gnome-shortcuts')
        print(output / 'bin/polarstation')


if __name__ == '__main__':
    main()

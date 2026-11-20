"""Persistent Station build directories; compilers retain rebuild authority."""
from contextlib import contextmanager
import fcntl
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys


def rust_cache_environment(environment):
    if 'RUSTC_WRAPPER' not in environment:
        wrapper = shutil.which('sccache', path=environment.get('PATH'))
        if wrapper:
            environment['RUSTC_WRAPPER'] = wrapper
    return environment


@contextmanager
def workspace(root, platform):
    directory = Path(root).resolve() / 'target/polar-station/native' / platform
    directory.mkdir(parents=True, exist_ok=True)
    with (directory / '.lock').open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        yield directory


def sync_tree(source, destination):
    """Copy changed source bytes, preserving unchanged timestamps and outputs."""
    source, destination = Path(source).resolve(), Path(destination).absolute()
    if destination.is_symlink():
        raise ValueError('managed build directory must not be a symlink')
    destination.mkdir(parents=True, exist_ok=True)
    # Deliberately omit -t: a fresh overlay's timestamps must not dirty identical
    # destination files. Removed source files disappear; compiler outputs survive.
    subprocess.run(['rsync', '-rclp', '--delete', '--exclude=/.zig-cache/', '--exclude=/zig-pkg/',
                    '--exclude=/zig-out/', '--exclude=/macos/build/',
                    '--exclude=/macos/GhosttyKit.xcframework/',
                    str(source) + '/', str(destination) + '/'], check=True)
    return destination


def cache_framework(source):
    """Use Zig Run's declared-output cache for the pinned XCFramework step."""
    source = Path(source)
    path = source / 'src/build/XCFrameworkStep.zig'
    text = path.read_text()
    start = text.index('    // xcodebuild refuses to overwrite an existing xcframework')
    end = text.index('    const run = RunStep.create', start)
    text = text[:start] + text[end:]
    replacements = {
        '    run.has_side_effects = true;\n': '',
        '    addOutPathArg(b, run, opts.out_path);': '    const framework_output = run.addOutputDirectoryArg(b.fmt("{s}.xcframework", .{opts.name}));',
        '    run.step.dependOn(&delete.step);': '''    const run_sync = RunStep.create(b, b.fmt("xcframework install {s}", .{opts.name}));
    run_sync.has_side_effects = true;
    run_sync.addArgs(&.{ "rsync", "-rclp", "--delete" });
    run_sync.addDirectoryArg(framework_output);
    switch (opts.out_path) {
        .install_prefix => |v| run_sync.addDirectoryArg(b.graph.path(.install_prefix, std.fs.path.dirname(v) orelse ".")),
        .source_tree => |v| run_sync.addArg(std.fs.path.dirname(v) orelse "."),
    }''',
        '.step = &run.step,': '.step = &run_sync.step,',
    }
    for old, new in replacements.items():
        if text.count(old) != 1:
            raise ValueError('pinned XCFramework build step changed: ' + old)
        text = text.replace(old, new, 1)
    path.write_text(text[:text.index('/// Adds the output path as an argument.')].rstrip() + '\n')


def copy_changed(source, destination):
    source, destination = Path(source), Path(destination)
    destination.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(['rsync', '-clp', str(source), str(destination)], check=True)


def file_hash(path):
    value = hashlib.sha256()
    with Path(path).open('rb') as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            value.update(block)
    return value.hexdigest()


def graphics_inputs(root, godot, editor, extension, graphviz, adapter, tools=()):
    root = Path(root)
    paths = [Path(godot), Path(editor), Path(extension), Path(graphviz).resolve(),
             Path(adapter), Path(__file__), root / 'apps/polar-graphics-native',
             root / 'apps/polar-ideacave-godot', root / 'crates/polar-force-layout-core/upstream']
    resources = Path(adapter).with_name('graphics_resources.py')
    paths.append(resources if resources.is_file() else Path(adapter).with_name('resources.py'))
    browsing = root / 'apps/polar-ideacave-godot/polar-browsing.itree.json'
    paths.append(browsing if browsing.is_file() else root / 'itrees/polar-browsing.itree.json')
    value = hashlib.sha256()
    value.update(json.dumps({'tools': list(tools), 'environment': {
        k: os.environ.get(k) for k in ('SDKROOT', 'MACOSX_DEPLOYMENT_TARGET',
                                     'CC', 'CFLAGS', 'PKG_CONFIG_LIBDIR', 'PKG_CONFIG_SYSROOT_DIR')
    }}, sort_keys=True).encode())
    for base in paths:
        value.update(str(base.resolve()).encode() + b'\0')
        for path in sorted(base.rglob('*')) if base.is_dir() else [base]:
            if (not path.is_file() or set(path.relative_to(base).parts if base.is_dir() else ())
                    & {'.git', '.godot', '__pycache__', 'tests', 'test-fixtures'}):
                continue
            if path.name.endswith(('.uid', '.pyc', '_test.gd', '_selftest.gd')):
                continue
            value.update(str(path.relative_to(base) if base.is_dir() else path.name).encode() + b'\0')
            value.update(str(path.stat().st_mode & 0o777).encode() + b'\0')
            value.update(file_hash(path).encode())
    return value.hexdigest()


def graphics_reusable(output, inputs, digest):
    output = Path(output)
    receipt = output.with_name(output.name + '.build.json')
    try:
        saved = json.loads(receipt.read_text())
        if saved['inputs'] != inputs or saved['digest'] != digest(output):
            return False
        # macOS Graphviz also links libraries outside its installation prefix.
        provenance = json.loads((output / 'provenance.json').read_text())
        for item in provenance.get('graphviz_transformations', []):
            if file_hash(item['source_path']) != item['source_sha256']:
                return False
    except (OSError, ValueError, KeyError):
        return False
    print('STATION_GRAPHICS_REUSED=' + str(output), flush=True)
    return True


def record_graphics(output, inputs, digest):
    output = Path(output)
    receipt = output.with_name(output.name + '.build.json')
    temporary = receipt.with_suffix('.tmp')
    temporary.write_text(json.dumps({'inputs': inputs, 'digest': digest(output)}, sort_keys=True) + '\n')
    temporary.replace(receipt)





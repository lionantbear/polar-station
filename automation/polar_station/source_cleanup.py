"""Current-source syntax edits. Parser executables are explicitly prepared inputs."""
from __future__ import annotations
import ast
import json
import os
from pathlib import Path
import subprocess


def independent_test(relative: str) -> bool:
    p = Path(relative)
    if any(x in p.parts for x in ('vendor', 'upstream', 'licenses')):
        return False
    name = p.name
    if p.suffix == '.py':
        return name.startswith('test_') or name.endswith('_test.py')
    if p.suffix in ('.js', '.mjs', '.cjs', '.ts', '.tsx'):
        return '.test.' in name or '.spec.' in name or name.startswith('test-')
    if p.suffix == '.gd' or name.endswith('.gd.uid'):
        return name.endswith(('_selftest.gd', '_selftest.gd.uid'))
    return False


def executable(variable: str) -> str:
    defaults = {'POLAR_STATION_RUST_STRIP':'strip-rust', 'POLAR_STATION_ZIG_STRIP':'strip-zig', 'POLAR_STATION_AST_PYTHON':'bin/python'}
    path = os.environ.get(variable, str(Path.home()/'.cache/polar-station-source-tools'/defaults[variable]))
    if not path or not Path(path).is_file():
        raise ValueError(f'{variable} must name the prepared syntax tool; see source cleanup preparation')
    return path


def ranges(root: Path) -> dict[str, list[list[int]]]:
    result = {}
    files = [str(p) for p in root.rglob('*.rs') if not set(p.relative_to(root).parts) & {'vendor', 'upstream', 'licenses'}]
    if files:
        r = subprocess.run([executable('POLAR_STATION_RUST_STRIP')], input=json.dumps(files), text=True, capture_output=True)
        if r.returncode: raise ValueError('Rust syntax cleanup failed: ' + r.stderr)
        result.update(json.loads(r.stdout))
    for p in root.rglob('*.zig'):
        if set(p.relative_to(root).parts) & {'vendor', 'upstream', 'licenses'}:
            continue
        r = subprocess.run([executable('POLAR_STATION_ZIG_STRIP'), str(p)], text=True, capture_output=True, check=True)
        result[str(p)] = json.loads(r.stderr)
    for p in root.rglob('*.py'):
        if set(p.relative_to(root).parts) & {'vendor', 'upstream', 'licenses'}:
            continue
        code = p.read_bytes()
        lines = code.splitlines(keepends=True)
        offsets = [0]
        for line in lines: offsets.append(offsets[-1] + len(line))
        tree = ast.parse(code, filename=str(p))
        found = []
        # Only unittest classes are automatically removed in mixed modules.
        # Standalone pytest functions require a pytest decorator or import.
        pytest = any(isinstance(n, ast.Import) and any(a.name == 'pytest' for a in n.names) or isinstance(n, ast.ImportFrom) and n.module == 'pytest' for n in tree.body)
        def walk(node):
            test_class = isinstance(node, ast.ClassDef) and any(isinstance(b, ast.Attribute) and isinstance(b.value, ast.Name) and b.value.id == 'unittest' and b.attr == 'TestCase' for b in node.bases)
            test_fn = pytest and isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)) and node.name.startswith('test_')
            if test_class or test_fn:
                start = min([node.lineno] + [d.lineno for d in node.decorator_list])
                found.append([offsets[start-1], offsets[node.end_lineno]])
            else:
                for child in ast.iter_child_nodes(node): walk(child)
        walk(tree)
        surviving = code
        for a, b in sorted(found, reverse=True): surviving = surviving[:a] + surviving[b:]
        used = {n.id for n in ast.walk(ast.parse(surviving)) if isinstance(n, ast.Name)}
        for n in tree.body:
            if isinstance(n, ast.Import):
                framework = all(a.name in ('unittest', 'pytest') and (a.asname or a.name) not in used for a in n.names)
            elif isinstance(n, ast.ImportFrom):
                framework = n.module in ('unittest', 'pytest') and all((a.asname or a.name) not in used for a in n.names)
            else: framework = False
            if framework: found.append([offsets[n.lineno-1], offsets[n.end_lineno]])
        result[str(p)] = found
    structural = [str(p) for pattern in ('*.swift', '*.js', '*.mjs') for p in root.rglob(pattern)
                  if not set(p.relative_to(root).parts) & {'vendor', 'upstream', 'licenses'}]
    if structural:
        command = [executable('POLAR_STATION_AST_PYTHON'), str(Path(__file__).with_name('source_structural.py'))]
        r = subprocess.run(command, input=json.dumps(structural), text=True, capture_output=True)
        if r.returncode: raise ValueError(r.stderr)
        result.update(json.loads(r.stdout))
    return result


def apply(root: Path) -> dict:
    edits = ranges(root)
    report = {'schema_version': 1, 'rust_parser': 'syn-2.0.117/cfg-expr-0.20.9', 'zig_parser': 'std.zig.Ast-0.15.2', 'files': {}}
    for name, spans in edits.items():
        if not spans: continue
        p = Path(name)
        code = p.read_bytes()
        # Remove nested ranges only once. Reject crossing ranges.
        selected = []
        for start, end in sorted(spans, key=lambda r: (r[0], -r[1])):
            if not (0 <= start <= end <= len(code)): raise ValueError(f'invalid syntax range: {p}')
            if selected and start < selected[-1][1]:
                if end <= selected[-1][1]: continue
                raise ValueError(f'crossing syntax ranges: {p}')
            selected.append([start, end])
        for start, end in reversed(selected): code = code[:start] + code[end:]
        p.write_bytes(code)
        report['files'][p.relative_to(root).as_posix()] = selected
    remaining = ranges(root)
    if any(remaining.values()):
        raise ValueError('test syntax remains after cleanup: ' + str([p for p, r in remaining.items() if r]))
    return report


def contextual(root: Path, data: dict) -> None:
    """Apply reviewed local exceptions without binding unrelated source bytes."""
    pending = {}
    manifest = root / 'PUBLIC_SOURCE_MANIFEST.json'
    known = json.loads(manifest.read_text()).get('files', {}) if manifest.is_file() else {}
    for entry in data['edits']:
        p = root / entry['path']
        if p.is_symlink() or not p.resolve().is_relative_to(root.resolve()): raise ValueError('unsafe cleanup path: ' + entry['path'])
        if not p.exists() and entry['path'] in {e['path'] for e in data['deletions']}: continue
        import hashlib
        recorded = known.get(entry['path'])
        if isinstance(recorded, str) and hashlib.sha256(p.read_bytes()).hexdigest() == recorded:
            continue
        text = p.read_text()
        for rule in entry['rules']:
            old, new = rule['old'], rule['new']
            if old == new: continue
            if text.count(old) == 1: text = text.replace(old, new, 1)
            elif new and text.count(new) == 1 and old not in text: continue
            else: raise ValueError('public cleanup context changed: ' + entry['path'])
        pending[p] = text
    for p, text in pending.items(): p.write_text(text)
    for entry in data['deletions']:
        p = root / entry['path']
        if p.is_symlink() or not p.resolve().is_relative_to(root.resolve()): raise ValueError('unsafe cleanup path: ' + entry['path'])
        p.unlink(missing_ok=True)

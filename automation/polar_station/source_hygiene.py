"""Conservative first-party development-test gate for source exports.

This is a release validator, not a test suite. It never rewrites code. New
matches require review; production diagnostics must not be stripped by name.
Vendored/upstream sources and their licenses retain their original provenance.
"""
from __future__ import annotations
import ast
from pathlib import PurePosixPath
import re


def findings(relative: str, source: str) -> list[str]:
    path = PurePosixPath(relative)
    if not relative.startswith(('apps/', 'crates/', 'automation/')):
        return []
    if any(part in {'vendor', 'upstream', 'licenses'} for part in path.parts):
        return []
    suffix = path.suffix
    results = []
    if suffix == '.patch':
        # Context may contain original upstream tests. Only newly added lines
        # are first-party patch content, and only declarations are rejected.
        added = '\n'.join(line[1:] for line in source.splitlines()
                          if line.startswith('+') and not line.startswith('+++'))
        patch_patterns = (
            r'^\s*test(?:\s+"[^"\n]*")?\s*\{',
            r'^\s*#\[\s*(?:test|[\w:]+::test|cfg\(\s*test\s*\))\s*\]',
            r'^\s*(?:@testable\s+)?import\s+(?:XCTest|Testing)\b',
            r'^\s*(?:TEST|TEST_F|TEST_CASE|TEST_SUITE)\s*\(',
            r'^\s*(?:def|func)\s+test_\w*\s*\(',
        )
        if any(re.search(pattern, added, re.M) for pattern in patch_patterns):
            results.append('patch adds a first-party test declaration')
        return results
    patterns = {
        '.rs': (r'^\s*#\[\s*(?:test|[\w:]+::test)\s*\]',
                r'^\s*#\[\s*cfg\(\s*test\s*\)\s*\]',
                r'^\s*#\[\s*cfg\(\s*all\(\s*test\s*[,)]'),
        '.zig': (r'^\s*test(?:\s+"[^"\n]*")?\s*\{',),
        '.swift': (r'^\s*(?:@testable\s+)?import\s+(?:XCTest|Testing)\b',
                   r'^\s*@Test(?:\b|\()'),
        '.c': (r'^\s*(?:TEST|TEST_F|TEST_CASE|TEST_SUITE)\s*\(',),
        '.cpp': (r'^\s*(?:TEST|TEST_F|TEST_CASE|TEST_SUITE)\s*\(',),
        '.js': (r'''(?:from\s*|require\s*\()\s*['"]node:test['"]''',),
        '.mjs': (r'''(?:from\s*|require\s*\()\s*['"]node:test['"]''',),
        '.gd': (r'^\s*func\s+test_\w*\s*\(',),
    }
    if any(re.search(pattern, source, re.M) for pattern in patterns.get(suffix, ())):
        results.append('first-party test declaration/framework')
    if suffix == '.py':
        tree = ast.parse(source, filename=relative)
        for node in ast.walk(tree):
            if isinstance(node, (ast.Import, ast.ImportFrom)):
                names = [n.name for n in node.names] if isinstance(node, ast.Import) else [node.module or '']
                if any(name.split('.')[0] in {'unittest', 'pytest'} for name in names):
                    results.append('Python test framework import')
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)) and node.name.startswith('test_'):
                results.append('Python test function')
    return sorted(set(results))

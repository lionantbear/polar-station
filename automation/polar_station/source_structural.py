"""Pinned ast-grep adapter; only explicit framework syntax is eligible."""
import json
import re
import sys
from importlib.metadata import version
from ast_grep_py import SgRoot

if version('ast-grep-py') != '0.41.0':
    raise RuntimeError('source cleanup requires ast-grep-py==0.41.0')
result = {}
for path in json.load(sys.stdin):
    code = open(path).read()
    lang = 'swift' if path.endswith('.swift') else 'javascript'
    if lang == 'swift' and not re.search(r'^\s*(?:@(?:Testing\.)?Test\b|(?:@testable\s+)?import\s+(?:XCTest|Testing)\b)', code, re.M):
        result[path] = []
        continue
    if lang == 'javascript' and not re.search(r"from\s*['\"]node:test['\"]", code):
        result[path] = []
        continue
    root = SgRoot(code, lang).root()
    found = []
    bindings = {'test', 'describe', 'it'}
    if lang == 'javascript':
        for node in root.children():
            if node.kind() == 'import_statement' and re.search(r"from\s*['\"]node:test['\"]", node.text()):
                clause = node.text().split('from',1)[0].removeprefix('import').strip()
                if clause.startswith('{'):
                    bindings = {v.strip().split()[-1] for v in clause.strip('{} ').split(',') if v.strip()}
                elif re.fullmatch(r'[A-Za-z_$][\w$]*', clause): bindings = {clause}
                else: raise ValueError('unsupported node:test binding: ' + path)
    framework = lang == 'javascript' and bool(re.search(r'''from\s*['"]node:test['"]''', code))
    def walk(node):
        kind, text = node.kind(), node.text()
        if kind == 'ERROR': raise ValueError('syntax error: ' + path)
        remove = False
        if lang == 'swift':
            remove = (kind == 'class_declaration' and any(c.kind() == 'inheritance_specifier' and c.text() == 'XCTestCase' for c in node.children()))
            remove |= kind == 'function_declaration' and any(c.kind() == 'modifiers' and re.search(r'@(?:Testing\.)?Test\b', c.text()) for c in node.children())
            remove |= kind == 'import_declaration' and bool(re.fullmatch(r'(?:@testable\s+)?import\s+(?:XCTest|Testing)\s*',text))
        elif framework:
            remove = kind == 'import_statement' and bool(re.search(r"from\s*['\"]node:test['\"]",text))
            if kind == 'expression_statement':
                remove = any(c.kind() == 'call_expression' and any(re.match(r'^' + re.escape(b) + r'(?:\.(?:skip|only|todo))?\s*\(', c.text()) for b in bindings) for c in node.children())
        if remove:
            r=node.range();found.append([r.start.index,r.end.index])
        else:
            for child in node.children():walk(child)
    walk(root)
    if framework:
        def check(node):
            if node.kind() == 'identifier' and node.text() in bindings:
                r = node.range()
                if not any(a <= r.start.index and r.end.index <= b for a, b in found):
                    raise ValueError('unsupported node:test reference: ' + path)
            for child in node.children(): check(child)
        check(root)
    result[path] = found
json.dump(result,sys.stdout)

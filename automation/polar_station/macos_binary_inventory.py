#!/usr/bin/env python3
"""macOS-only, fail-closed Mach-O dependency and license inventory.

Run on an actual, already built .app before creating the DMG. Every Mach-O
component must have an explicit hash-bound attribution record; dependencies
may only resolve inside the app or to Apple-provided system libraries.

This covers Mach-O binaries, not assets, scripts, embedded data, source files
or license-specific source offers. Those need separate product release checks.
"""

from __future__ import annotations

import argparse
from hashlib import sha256
import json
import os
from pathlib import Path
import subprocess
import sys
from typing import Any

MACHO_MAGICS = {
    b"\xce\xfa\xed\xfe", b"\xcf\xfa\xed\xfe",
    b"\xfe\xed\xfa\xce", b"\xfe\xed\xfa\xcf",
    b"\xca\xfe\xba\xbe", b"\xbe\xba\xfe\xca",
    b"\xca\xfe\xba\xbf", b"\xbf\xba\xfe\xca",
}


def hash_file(path: Path) -> str:
    value = sha256()
    with path.open("rb") as binary:
        for block in iter(lambda: binary.read(1024 * 1024), b""):
            value.update(block)
    return value.hexdigest()


def is_macho(path: Path) -> bool:
    try:
        with path.open("rb") as stream:
            return stream.read(4) in MACHO_MAGICS
    except OSError:
        return False


def otool_links(output: str) -> list[str]:
    """Take install names from `otool -L` (first line is file name)."""
    result = []
    for line in output.splitlines()[1:]:
        candidate = line.strip().split(" (compatibility version", 1)[0].strip()
        if candidate and (candidate.startswith("@") or candidate.startswith("/")):
            result.append(candidate)
    return result


def otool_rpaths(output: str) -> list[str]:
    result: list[str] = []
    active = False
    for line in output.splitlines():
        stripped = line.strip()
        if stripped == "cmd LC_RPATH":
            active = True
        elif stripped.startswith("cmd "):
            active = False
        elif active and stripped.startswith("path "):
            result.append(stripped[5:].split(" (offset ", 1)[0])
            active = False
    return result


def expand_path(name: str, loader: Path, executable_dir: Path) -> Path | None:
    if name.startswith("@loader_path/"):
        return loader.parent / name[len("@loader_path/"):]
    if name == "@loader_path":
        return loader.parent
    if name.startswith("@executable_path/"):
        return executable_dir / name[len("@executable_path/"):]
    if name == "@executable_path":
        return executable_dir
    if name.startswith("/"):
        return Path(name)
    return None


def resolve_link(name: str, loader: Path, rpaths: list[str],
                 executable_dir: Path, app_root: Path) -> dict[str, str]:
    if name.startswith(("/usr/lib/", "/System/Library/")):
        return {"install_name": name, "classification": "apple_system"}
    attempts: list[Path] = []
    if name.startswith("@rpath/"):
        fragment = name[len("@rpath/"):]
        for entry in rpaths:
            base = expand_path(entry, loader, executable_dir)
            if base is not None:
                attempts.append(base / fragment)
    else:
        expanded = expand_path(name, loader, executable_dir)
        if expanded is not None:
            attempts.append(expanded)
    for candidate in attempts:
        resolved = candidate.resolve()
        if resolved.is_file() and resolved.is_relative_to(app_root):
            return {"install_name": name, "classification": "in_bundle",
                    "target": resolved.relative_to(app_root).as_posix()}
    return {"install_name": name, "classification": "UNRESOLVED_OR_EXTERNAL"}


def scan(app: Path, attributions: Path | None = None) -> dict[str, Any]:
    root = app.resolve()
    if not root.is_dir() or not (root / "Contents/Info.plist").is_file():
        raise ValueError("expected a real .app bundle with Contents/Info.plist")
    if not sys.platform.startswith("darwin"):
        raise RuntimeError("Mach-O dependency inventory must run on macOS")

    mapping: dict[str, dict[str, str]] = {}
    if attributions:
        data = json.loads(attributions.read_text(encoding="utf-8"))
        if data.get("schema_version") != 1:
            raise ValueError("unsupported attribution mapping schema")
        for item in data.get("components", []):
            rel = item["bundle_path"]
            if rel in mapping or Path(rel).is_absolute() or ".." in Path(rel).parts:
                raise ValueError(f"unsafe or duplicate bundle attribution: {rel}")
            mapping[rel] = item

    found: list[dict[str, Any]] = []
    blockers: list[str] = []
    symlinks: list[str] = []
    # .app frameworks commonly use in-bundle versioned symlinks; guard escapes.
    for dirpath, dirs, files in os.walk(root, followlinks=False):
        folder = Path(dirpath)
        for name in list(dirs) + files:
            path = folder / name
            if path.is_symlink():
                symlinks.append(path.relative_to(root).as_posix())
                resolved = path.resolve()
                if not resolved.exists() or not resolved.is_relative_to(root):
                    blockers.append(f"App symlink escapes bundle: {path.relative_to(root)}")
        for name in files:
            binary = folder / name
            if binary.is_symlink() or not is_macho(binary):
                continue
            relative = binary.relative_to(root).as_posix()
            checksum = hash_file(binary)
            try:
                links = subprocess.check_output(["otool", "-L", str(binary)], text=True)
                load = subprocess.check_output(["otool", "-l", str(binary)], text=True)
            except (OSError, subprocess.CalledProcessError) as exc:
                blockers.append(f"otool inspection failed: {relative}: {exc}")
                continue
            rpaths = otool_rpaths(load)
            dependencies = [resolve_link(link, binary, rpaths, root / "Contents/MacOS", root)
                            for link in otool_links(links)]
            for dependency in dependencies:
                if dependency["classification"] == "UNRESOLVED_OR_EXTERNAL":
                    blockers.append(f"Unresolved or external dylib: {relative} → {dependency['install_name']}")

            annotation = mapping.get(relative)
            license_path = None
            license_sha = None
            if annotation:
                license_path = annotation.get("license_text_path")
                if license_path and not Path(license_path).is_absolute() and ".." not in Path(license_path).parts:
                    text_file = root / license_path
                    if text_file.is_file() and text_file.resolve().is_relative_to(root):
                        license_sha = hash_file(text_file)
            if not annotation or annotation.get("sha256") != checksum \
                    or not annotation.get("name") or not annotation.get("license") \
                    or not annotation.get("source") or not license_sha:
                blockers.append(f"Mach-O component missing exact hash-bound license attribution: {relative}")
            found.append({"bundle_path": relative, "sha256": checksum,
                          "name": annotation.get("name") if annotation else None,
                          "license": annotation.get("license") if annotation else None,
                          "source": annotation.get("source") if annotation else None,
                          "license_text_path": license_path,
                          "license_text_sha256": license_sha,
                          "rpaths": rpaths, "dependencies": dependencies})

    if not found:
        blockers.append("No Mach-O executables/libraries found in the app bundle")
    unused = set(mapping) - {entry["bundle_path"] for entry in found}
    if unused:
        blockers.append(f"Attribution map entries do not match a bundled Mach-O: {sorted(unused)}")
    if attributions is None:
        blockers.append("No approved, hash-bound license attribution map provided")
    return {"schema_version": 1, "scope": "Mach-O-only; resources/scripts additionally require audit",
            "status": "blocked" if blockers else "mach_o_inventory_complete_pending_manual_review",
            "components": found, "symlinks": symlinks, "blockers": sorted(set(blockers)),
            "independently_reviewed": False}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app", required=True, type=Path)
    parser.add_argument("--attributions", type=Path,
                        help="JSON {schema_version:1,components:[{bundle_path,sha256,name,license,source,license_text_path}]}; never guess licenses")
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    try:
        if args.output.exists() or args.output.is_symlink():
            raise ValueError(f"refusing to overwrite existing report: {args.output}")
        report = scan(args.app, args.attributions)
        args.output.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        print(json.dumps({"status": report["status"], "mach_o_files": len(report["components"]),
                          "blockers": report["blockers"]}, indent=2))
        return 2 if report["blockers"] else 0
    except (OSError, RuntimeError, ValueError, subprocess.CalledProcessError, KeyError) as exc:
        print(f"BOM INSPECTION FAILED: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())

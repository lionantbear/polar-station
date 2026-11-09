#!/usr/bin/env python3
"""Fail-closed Polar Station source snapshot exporter. Never builds or publishes.

An exported snapshot is NOT a release candidate until Cargo, native macOS/Linux
builds, licensing, vulnerability checks, and binary provenance are accepted.
"""

from __future__ import annotations

import argparse
from collections import deque
from dataclasses import dataclass
from hashlib import sha256
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tomllib
from typing import Any
import public_profile

SCRIPT = Path(__file__).resolve()
DEFAULT_POLICY = SCRIPT.with_name("public_release_policy.json")
PUBLIC_MANIFEST = "PUBLIC_SOURCE_MANIFEST.json"
PUBLIC_README = """# Polar Station

Public source snapshot of Polar Station, the native macOS/Linux agent workstation.

This tree is exported from the PolarInfra monorepo. Its initial public Git
history intentionally does not include earlier private development history.
Future public releases should retain their normal public Git history.

The iPhone Companion is currently **Unavailable**. The public source profile disables
the pairing screen. Supplied desktop launchers bind Host to loopback without
mDNS advertisement; native build and installation verification remains pending.

Ghostty upstream is fetched at the pinned revision in
`apps/polar-station-macos/ghostty-overlay/upstream.lock.json`; the Polar patch
is applied separately. Consult `THIRD_PARTY_NOTICES.md` and
`SOURCE_DISTRIBUTION.md` for source-only versus binary-distribution obligations.
The root license covers first-party code; upstream licenses remain unchanged.

This source snapshot is not a tested binary release. Consult the public build
README once the portable build and installation gates are complete.
"""


class ExportError(RuntimeError):
    pass


def read_json(path: Path) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def read_toml(path: Path) -> dict[str, Any]:
    if not path.is_file():
        raise ExportError(f"missing Cargo manifest: {path}")
    return tomllib.loads(path.read_text(encoding="utf-8"))


def within(root: Path, relative: str) -> Path:
    candidate = Path(relative)
    if not relative or candidate.is_absolute() or ".." in candidate.parts:
        raise ExportError(f"unsafe relative path: {relative!r}")
    target = root.joinpath(candidate)
    if not target.resolve(strict=False).is_relative_to(root.resolve()):
        raise ExportError(f"path escapes source root: {relative}")
    return target


def manifest_dependencies(manifest: dict[str, Any]) -> list[tuple[str, Any]]:
    result: list[tuple[str, Any]] = []
    for key in ("dependencies", "dev-dependencies", "build-dependencies"):
        result.extend(manifest.get(key, {}).items())
    for value in manifest.get("target", {}).values():
        if isinstance(value, dict):
            for key in ("dependencies", "dev-dependencies", "build-dependencies"):
                result.extend(value.get(key, {}).items())
    return result


def locked_reachable_packages(source: Path, seeds: list[str]) -> list[dict[str, Any]]:
    """Over-approximate transitive packages, including indirect Git patches.

    Cargo.lock does not store selected features. Therefore this intentionally
    includes optional and target-specific dependencies; rejecting extra items
    is safer than accidentally excluding third-party source or its licenses.
    """
    packages = read_toml(source / "Cargo.lock").get("package", [])
    if not packages:
        return []  # small synthetic fixture only; production lock has entries
    by_name: dict[str, list[dict[str, Any]]] = {}
    for package in packages:
        by_name.setdefault(package["name"], []).append(package)
    queue = deque(package for package in packages if package["name"] in seeds)
    if any(seed not in by_name for seed in seeds):
        raise ExportError("Cargo.lock lacks a required Station seed package")
    seen: set[tuple[str, str, str]] = set()
    result: list[dict[str, Any]] = []
    while queue:
        package = queue.popleft()
        identity = (package["name"], package["version"], package.get("source", ""))
        if identity in seen:
            continue
        seen.add(identity)
        result.append(package)
        for reference in package.get("dependencies", []):
            parts = reference.split(" ")
            candidates = [dep for dep in by_name.get(parts[0], [])
                          if len(parts) == 1 or dep["version"] == parts[1]]
            if len(candidates) != 1:
                raise ExportError(f"ambiguous/missing Cargo.lock dependency {reference!r}")
            queue.append(candidates[0])
    return result


def toml_key(name: str) -> str:
    return name if re.fullmatch(r"[A-Za-z0-9_-]+", name) else json.dumps(name)


def toml_value(value: Any) -> str:
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, str):
        return json.dumps(value, ensure_ascii=False)
    if isinstance(value, int):
        return str(value)
    if isinstance(value, list):
        return "[" + ", ".join(toml_value(part) for part in value) + "]"
    if isinstance(value, dict):
        return "{ " + ", ".join(
            f"{toml_key(key)} = {toml_value(entry)}"
            for key, entry in value.items()
        ) + " }"
    raise ExportError(f"unsupported TOML value: {type(value).__name__}")


def table(lines: list[str], header: str, entries: dict[str, Any]) -> None:
    lines.append("")
    lines.append(f"[{header}]")
    for name in sorted(entries):
        lines.append(f"{toml_key(name)} = {toml_value(entries[name])}")


@dataclass
class CargoClosure:
    package_paths: dict[str, str]
    workspace_keys: set[str]
    patch_tables: dict[str, dict[str, Any]]
    workspace_data: dict[str, Any]


def compute_cargo_closure(source: Path, seeds: list[str]) -> CargoClosure:
    root = read_toml(source / "Cargo.toml")
    dependencies = root.get("workspace", {}).get("dependencies", {})
    package_paths: dict[str, str] = {}
    by_name: dict[str, str] = {}
    for candidate in root.get("workspace", {}).get("members", []):
        manifest_file = within(source, f"{candidate}/Cargo.toml")
        if not manifest_file.is_file():
            continue
        manifest = read_toml(manifest_file)
        name = manifest.get("package", {}).get("name")
        if name:
            if name in by_name and by_name[name] != candidate:
                raise ExportError(f"duplicate package {name}")
            by_name[name] = candidate

    for seed in seeds:
        if seed not in by_name:
            raise ExportError(f"Station crate not found among workspace members: {seed}")

    needed_workspace_keys: set[str] = set()
    seen_deps: set[str] = set()
    queue = deque(seeds)
    root_patches = root.get("patch", {})
    selected_patches: dict[str, dict[str, Any]] = {}
    locked_names = {p["name"] for p in locked_reachable_packages(source, seeds)}

    def find_local(owner: str, dep_name: str, spec: Any) -> str | None:
        if not isinstance(spec, dict):
            return None
        if spec.get("workspace") is True:
            needed_workspace_keys.add(dep_name)
            if dep_name not in dependencies:
                raise ExportError(f"{owner}: missing workspace dependency {dep_name}")
            base = dependencies[dep_name]
            if not isinstance(base, dict):
                return None
            if "path" not in base:
                return None
            relative = base["path"]
        elif "path" in spec:
            relative = str((Path(owner) / spec["path"]).as_posix())
        else:
            return None
        # Cargo manifests routinely use ../ between sibling crates. Resolve
        # that Cargo-owned path, but never allow it to leave the source root.
        target = (source / relative).resolve()
        if not target.is_relative_to(source):
            raise ExportError(f"local dependency escapes source root: {owner} → {relative}")
        if not (target / "Cargo.toml").exists():
            raise ExportError(f"{owner}: unresolved local dependency {dep_name} → {relative}")
        target_info = read_toml(target / "Cargo.toml")
        name = target_info.get("package", {}).get("name")
        if not name:
            raise ExportError(f"unnamed local crate: {relative}")
        selected = str(target.relative_to(source))
        known = by_name.get(name)
        if known and known != selected:
            raise ExportError(f"conflicting paths for {name}: {known} vs {selected}")
        by_name[name] = selected
        return name

    while True:
        while queue:
            name = queue.popleft()
            if name in package_paths:
                continue
            if name not in by_name:
                raise ExportError(f"missing Cargo package: {name}")
            relative = by_name[name]
            manifest = read_toml(within(source, f"{relative}/Cargo.toml"))
            package_paths[name] = relative
            for dep_name, spec in manifest_dependencies(manifest):
                seen_deps.add(dep_name)
                if isinstance(spec, dict) and "package" in spec:
                    seen_deps.add(spec["package"])
                local = find_local(relative, dep_name, spec)
                if local is not None:
                    queue.append(local)

        # Preserve locally patched upstream packages if any Station crate
        # directly names them. This never imports an entire unused vendor tree.
        changed = False
        for source_name, entries in root_patches.items():
            for dep_name, spec in entries.items():
                if (
                    isinstance(spec, dict)
                    and "path" in spec
                    and dep_name not in seen_deps
                    and spec.get("package", dep_name) not in seen_deps
                    and dep_name not in locked_names
                    and spec.get("package", dep_name) not in locked_names
                ):
                    continue
                if isinstance(spec, dict) and "path" in spec:
                    pkg = find_local(".", dep_name, spec)
                    if pkg and pkg not in package_paths:
                        queue.append(pkg)
                        changed = True
                selected_patches.setdefault(source_name, {})[dep_name] = spec
        if not changed:
            break

    if locked_names:
        all_reachable = locked_reachable_packages(source, seeds)
        missing_local = {p["name"] for p in all_reachable if "source" not in p}
        missing_local -= package_paths.keys()
        if missing_local:
            raise ExportError(f"Cargo.lock reaches local crates missing from exported source: {sorted(missing_local)}")

    # Add every patch using an upstream git ref. A patch with a missing local
    # vendor source is never allowed to sneak through.
    for source_name, entries in root_patches.items():
        for dep_name, spec in entries.items():
            if isinstance(spec, dict) and "path" not in spec:
                selected_patches.setdefault(source_name, {})[dep_name] = spec
    return CargoClosure(package_paths, needed_workspace_keys, selected_patches, root)


def reduced_manifest(closure: CargoClosure) -> str:
    root = closure.workspace_data
    workspace_dependencies = root["workspace"]["dependencies"]
    chosen = {}
    for name in sorted(closure.workspace_keys):
        dependency = workspace_dependencies[name]
        if isinstance(dependency, dict) and "path" in dependency:
            candidate_path = dependency["path"]
            if candidate_path not in closure.package_paths.values():
                raise ExportError(f"workspace dependency not exported: {name}: {candidate_path}")
        chosen[name] = dependency

    lines = [
        "# Generated from the public Station cargo dependency closure.",
        "# Do not replace pinned dependencies by unreviewed latest versions.",
        "[workspace]",
        "members = [",
    ]
    for member in sorted(closure.package_paths.values()):
        lines.append(f"    {toml_value(member)},")
    lines += ["]", 'resolver = "2"']
    table(lines, "workspace.dependencies", chosen)

    for source_name, entries in sorted(closure.patch_tables.items()):
        table(lines, f"patch.{toml_key(source_name)}", entries)
    for profile, profile_entries in sorted(root.get("profile", {}).items()):
        scalar = {key: value for key, value in profile_entries.items() if key != "package"}
        if scalar:
            table(lines, f"profile.{toml_key(profile)}", scalar)
        for name, rules in profile_entries.get("package", {}).items():
            if name in closure.package_paths or name not in root["workspace"]["dependencies"]:
                table(lines, f"profile.{toml_key(profile)}.package.{toml_key(name)}", rules)

    result = "\n".join(lines) + "\n"
    tomllib.loads(result)  # self-validate the generated TOML syntax
    return result


# The retired PyGObject frontend is not part of the public Station product.
# Do not exclude Python globally: build/export/license helpers remain required.
RETIRED_LINUX_FRONTEND = (
    "apps/polar-tmux-linux/src",
    "apps/polar-tmux-linux/pyproject.toml",
    "apps/polar-tmux-linux/run-dev",
)


def is_retired_linux_frontend(relative: str) -> bool:
    return any(relative == prefix or relative.startswith(prefix + "/")
               for prefix in RETIRED_LINUX_FRONTEND)


def selected_files(source: Path, policy: dict[str, Any], closure: CargoClosure) -> tuple[set[str], dict[str, str], list[str]]:
    ignored_dirs = {name.lower() for name in policy["ignored_directory_names"]}
    ignored_suffixes = tuple(policy["ignored_suffixes"])
    blocked_fragments = tuple(policy["blocked_path_fragments"])
    paths: set[str] = set()
    links: dict[str, str] = {}
    missing: list[str] = []

    def add_link(path: Path) -> None:
        if is_retired_linux_frontend(path.relative_to(source).as_posix()):
            return
        target = path.resolve()
        if not target.exists() or not target.is_relative_to(source):
            raise ExportError(f"broken or external source link: {path}")
        rel = path.relative_to(source).as_posix()
        links[rel] = os.readlink(path)

    def add_file(path: Path) -> None:
        if is_retired_linux_frontend(path.relative_to(source).as_posix()):
            return
        relative = "/" + path.relative_to(source).as_posix() + "/"
        if any(fragment in relative for fragment in blocked_fragments):
            raise ExportError(f"proprietary path reached public export: {relative}")
        if path.is_symlink():
            add_link(path)
            return
        if path.suffix in ignored_suffixes:
            return
        paths.add(relative.strip("/"))

    def add_root(relative: str) -> None:
        if is_retired_linux_frontend(relative):
            return
        root = within(source, relative)
        if not root.exists():
            missing.append(relative)
            return
        if root.is_symlink():
            add_link(root)
            return
        if root.is_file():
            add_file(root)
            return
        for current_dir, subdirectories, filenames in os.walk(root, followlinks=False):
            current = Path(current_dir)
            for name in list(subdirectories):
                item = current / name
                if is_retired_linux_frontend(item.relative_to(source).as_posix()):
                    subdirectories.remove(name)
                    continue
                if item.is_symlink():
                    add_link(item)
                    subdirectories.remove(name)
                    continue
                if name.lower() in ignored_dirs:
                    subdirectories.remove(name)
            for filename in filenames:
                if filename.lower().endswith((".test.mjs", ".test.js")):
                    continue
                add_file(current / filename)

    for relative in (
        list(closure.package_paths.values())
        + policy["native_source_roots"]
        + policy["production_build_inputs"]
        + policy["root_inputs"]
    ):
        add_root(relative)
    # Rust include_str!/include_bytes! paths may leave a crate and land in a
    # repository-level migration directory. Omit test-only fixture includes.
    # More elaborate build.rs-generated files remain a cargo build gate.
    literal_include = re.compile(
        r'include_(?:str|bytes)!\s*\(\s*"([^"\r\n]+)"\s*\)', re.MULTILINE
    )
    for relative in sorted(paths.copy()):
        if not relative.endswith(".rs"):
            continue
        source_file = source / relative
        for included in literal_include.findall(source_file.read_text(encoding="utf-8")):
            dependency = (source_file.parent / included).resolve()
            if not dependency.is_relative_to(source):
                raise ExportError(f"Rust include escapes source: {relative} → {included}")
            dependency_rel = dependency.relative_to(source).as_posix()
            if any(part.lower() in ignored_dirs for part in Path(dependency_rel).parts):
                continue  # No integration or inline test assets in public export.
            if not dependency.is_file():
                missing.append(dependency_rel)
            else:
                add_file(dependency)
    for relative in policy["required_for_macos"]:
        if not within(source, relative).is_file():
            missing.append(relative)
    for rel in links:
        target = (source / rel).resolve()
        target_rel = target.relative_to(source).as_posix()
        if not any(
            selected == target_rel or selected.startswith(target_rel + "/")
            for selected in paths
        ):
            raise ExportError(f"source link target is absent from export: {rel} → {target_rel}")
    return paths, links, sorted(set(missing))


def plan(source: Path, policy: dict[str, Any]) -> dict[str, Any]:
    if policy.get("schema_version") != 1:
        raise ExportError("unsupported policy schema")
    closure = compute_cargo_closure(source, policy["cargo_seed_packages"])
    paths, links, missing = selected_files(source, policy, closure)
    return {
        "schema_version": 1,
        "product": "polar-station",
        "files": sorted(paths),
        "symlinks": dict(sorted(links.items())),
        "crate_paths": dict(sorted(closure.package_paths.items())),
        "workspace_dependency_count": len(closure.workspace_keys),
        "missing_inputs": missing,
        "license_present": (source / "LICENSE").is_file(),
        "upstream_ghostty_lock": "apps/polar-station-macos/ghostty-overlay/upstream.lock.json" in paths,
        "source_path_count": len(paths),
        "_generated_cargo": reduced_manifest(closure),
    }


def file_hash(path: Path) -> str:
    digest = sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def normalize_cargo_lock(destination: Path) -> None:
    """Prune a copied monorepo lock offline; never permit dependency upgrades."""
    lock = destination / "Cargo.lock"
    original = lock.read_bytes()
    def identities(raw):
        packages = tomllib.loads(raw.decode())["package"]
        return {(p["name"], p["version"], p.get("source", "")): p.get("checksum") for p in packages}
    before = identities(original)
    result = subprocess.run(["cargo", "metadata", "--offline", "--all-features",
                             "--format-version=1", "--manifest-path", str(destination / "Cargo.toml")],
                            stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
    after = identities(lock.read_bytes())
    if result.returncode or any(key not in before or before[key] != checksum for key, checksum in after.items()):
        lock.write_bytes(original)
        raise ExportError("Offline lock normalization failed or would change pinned package identities/checksums: " + result.stderr[-1000:])


def export(source: Path, destination: Path, policy: dict[str, Any], *, license_file: Path | None,
           normalize_lock: bool = False) -> dict[str, Any]:
    if destination.exists() or destination.is_symlink():
        raise ExportError(f"destination exists, refusing to overwrite: {destination}")
    if destination.resolve().is_relative_to(source.resolve()):
        raise ExportError("destination cannot be inside the input source tree")
    description = plan(source, policy)
    destination.mkdir(parents=True, exist_ok=False)

    for relative in description["files"]:
        src = within(source, relative)
        dst = within(destination, relative)
        dst.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(src, dst, follow_symlinks=False)
    for relative, link_target in description["symlinks"].items():
        dst = destination / relative
        dst.parent.mkdir(parents=True, exist_ok=True)
        dst.symlink_to(link_target)
        resolved = dst.resolve()
        if not resolved.is_relative_to(destination) or not resolved.exists():
            raise ExportError(f"symlink is not internal to staged source: {relative}")
    (destination / "Cargo.toml").write_text(description.pop("_generated_cargo"), encoding="utf-8")
    (destination / "README.md").write_text(PUBLIC_README, encoding="utf-8")
    (destination / ".gitignore").write_text('/target/\n/build/\n/dist/\n.DS_Store\n__pycache__/\n*.pyc\n.env\n.env.local\n', encoding="utf-8")
    if license_file:
        if not license_file.is_file():
            raise ExportError(f"requested license does not exist: {license_file}")
        shutil.copy2(license_file, destination / "LICENSE")
    elif (source / "LICENSE").is_file():
        shutil.copy2(source / "LICENSE", destination / "LICENSE")

    lock_rel = "apps/polar-station-macos/ghostty-overlay/upstream.lock.json"
    if lock_rel in description["files"]:
        lock = read_json(destination / lock_rel)
        upstream = lock.get("upstream", {})
        license_policy_path = SCRIPT.with_name("license_policy.json")
        license_policy = read_json(license_policy_path) if license_policy_path.is_file() else None
        notices = [
            "# Third-party source provenance — NOT a completed license audit",
            "",
            "## Ghostty",
            "",
            f"- Repository: {upstream.get('repository', 'UNRESOLVED')}",
            f"- Tag: {upstream.get('tag', 'UNRESOLVED')}",
            f"- Revision: {upstream.get('commit', 'UNRESOLVED')}",
            "- Ghostty is MIT-licensed; its official full LICENSE must accompany",
            "  the fetched upstream source and redistributed binaries.",
            "",
        ]
        if license_policy:
            license_out = destination / "licenses"
            license_out.mkdir(exist_ok=True)
            shutil.copy2(license_policy_path, license_out / "third-party-policy.json")
            notices += ["## Other reviewed source-level license claims", ""]
            for entry in license_policy["required_native_components"]:
                notices += [
                    f"- **{entry['id']}**: `{entry['license']}`; pinned/input version: `{entry['version']}`.",
                    f"  Official evidence: {entry['source']}",
                    f"  License text in this source tree: {entry['bundled_license'] or 'NOT YET BUNDLED'}.",
                    f"  Additional conditions: {entry['notes']}",
                ]
            notices += ["", "## Cargo Git source attribution", ""]
            for entry in license_policy.get("required_git_sources", []):
                notices += [
                    f"- `{entry['repository']}@{entry['commit']}`: {entry['license_claim']}.",
                    f"  Source/evidence: {entry['evidence']}; exact commit license: {entry['pin_license_state']}.",
                ]
        notices += [
            "",
            "**Provisional inventory only. The full resolved Cargo license report,",
            "exact upstream/Godot/Graphviz notices and binary-level dependency",
            "inventory MUST be generated and reviewed before distribution.**",
            "",
        ]
        (destination / "THIRD_PARTY_NOTICES.md").write_text(
            "\n".join(notices), encoding="utf-8"
        )

    public_profile.apply(destination)
    if normalize_lock:
        normalize_cargo_lock(destination)
    description["crate_paths"] = {name: path for name, path in description["crate_paths"].items()
                                  if (destination / path / "Cargo.toml").is_file()}

    record: dict[str, Any] = {
        "schema_version": 1,
        "product": "polar-station",
        "status": "source_export_preview_not_release",
        "crate_count": len(description["crate_paths"]),
        "crate_paths": description["crate_paths"],
        "missing_inputs": description["missing_inputs"],
        "license_present": (destination / "LICENSE").is_file(),
        "cargo_validation": "not_run",
        "native_macos_build": "not_run",
        "native_linux_build": "not_run",
        "third_party_license_audit": "pending",
        "companion_release": "unavailable",
        "symlinks": description["symlinks"],
        "files": {},
    }
    for path in sorted(destination.rglob("*")):
        if path.is_file() and not path.is_symlink():
            rel = path.relative_to(destination).as_posix()
            record["files"][rel] = file_hash(path)
    (destination / PUBLIC_MANIFEST).write_text(
        json.dumps(record, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    return {
        "output": str(destination),
        "crate_count": record["crate_count"],
        "files": len(record["files"]),
        "missing_inputs": record["missing_inputs"],
        "license_present": record["license_present"],
        "status": record["status"],
    }


def verify(destination: Path) -> dict[str, Any]:
    record = read_json(destination / PUBLIC_MANIFEST)
    expected: dict[str, str] = record["files"]
    actual = {p.relative_to(destination).as_posix() for p in destination.rglob("*")
              if p.is_file() and not p.is_symlink()}
    links = {p.relative_to(destination).as_posix(): os.readlink(p)
             for p in destination.rglob("*") if p.is_symlink()}
    if any(is_retired_linux_frontend(rel) for rel in actual | set(links)):
        raise ExportError("retired Linux Python frontend leaked into source")
    if links != record.get("symlinks", {}):
        raise ExportError("exported symlink set differs from pinned source")
    if any(not (destination / rel).resolve().is_relative_to(destination)
           or not (destination / rel).resolve().exists() for rel in links):
        raise ExportError("export contains external or broken symlink")
    if any(part.lower() in {".git", ".agents", "tests", "handoff", "history"}
           for rel in actual for part in Path(rel).parts):
        raise ExportError("internal history/test content leaked into source")
    unexpected = actual - set(expected) - {PUBLIC_MANIFEST}
    missing = set(expected) - actual
    changed = [rel for rel, digest in expected.items()
               if rel in actual and file_hash(destination / rel) != digest]
    if unexpected or missing or changed:
        raise ExportError(
            f"snapshot integrity failure: extra={sorted(unexpected)} "
            f"missing={sorted(missing)} changed={sorted(changed)}"
        )
    for dep in read_toml(destination / "Cargo.toml")["workspace"]["dependencies"].values():
        if isinstance(dep, dict) and "path" in dep:
            if not within(destination, f"{dep['path']}/Cargo.toml").is_file():
                raise ExportError(f"unresolved exported path dependency: {dep['path']}")
    roots = [name for name in ("polar-tmux", "polar-graphics-godot", "polar-graphics-package",
                               "polar-force-layout-core")
             if name in record.get("crate_paths", {})]
    if roots:
        closure = compute_cargo_closure(destination, roots)
        if closure.package_paths != record["crate_paths"]:
            raise ExportError("exported Cargo closure changed or was incomplete")
    return {"status": "source_integrity_verified", "files": len(expected),
            "crate_count": record["crate_count"],
            "missing_macos_inputs": record["missing_inputs"],
            "license_present": record["license_present"],
            "release_ready": False}


def release_audit(destination: Path, *, cargo_metadata: list[Path] | None = None,
                  binary_bom: Path | None = None) -> dict[str, Any]:
    integrity = verify(destination)
    record = read_json(destination / PUBLIC_MANIFEST)
    blockers: list[str] = []
    test_only_crates = sorted(name for name in record.get("crate_paths", {})
                              if name.endswith("-testkit") or name.endswith("-test-utils"))
    if test_only_crates:
        blockers.append(f"Test-only Cargo crates are still required by dev-dependencies: {test_only_crates}.")
    inline_test_files = sum(1 for rel in record["files"] if rel.startswith("crates/") and rel.endswith(".rs")
                            and "#[cfg(test)]" in (destination / rel).read_text(encoding="utf-8"))
    if inline_test_files:
        blockers.append(f"Source still includes inline Rust test modules in {inline_test_files} files; clean-repo policy needs a decision.")
    if not record["license_present"]:
        blockers.append("Project LICENSE is missing; choose the open-source license.")
    for missing in record["missing_inputs"]:
        blockers.append(f"Missing source/build asset: {missing}")
    if record["third_party_license_audit"] != "accepted":
        blockers.append("Third-party and vendored-source license audit is pending.")
    for key in ("cargo_validation", "native_macos_build", "native_linux_build"):
        if record[key] != "passed":
            blockers.append(f"{key}: {record[key]}")
    mac_runtime = destination / "apps/polar-station-macos/ghostty-module/PolarStationRuntime.swift"
    if mac_runtime.is_file():
        runtime = mac_runtime.read_text(encoding="utf-8")
        if '"0.0.0.0:47820", "--advertise"' in runtime:
            blockers.append("Companion unavailable but macOS still opens and advertises a public LAN listener.")
    mac_build = destination / "automation/polar_station/build_macos.sh"
    if mac_build.is_file():
        build = mac_build.read_text(encoding="utf-8")
        if "No Apple Development signing identity is available." in build:
            blockers.append("Mac build still requires an Apple Development certificate; add tested free ad-hoc mode.")
        if "/Users/goldenpolarlion/" in build:
            blockers.append("Mac build still defaults to a private developer checkout path.")
        if 'git -C "$repo_root" rev-parse HEAD' in build and 'PUBLIC_SOURCE_MANIFEST.json' not in build:
            blockers.append("Mac build assumes private Git history exists; inject the public source revision instead.")
    mac_graphics = destination / "automation/polar_station/build_macos_graphics.sh"
    if mac_graphics.is_file() and 'p.write_text(s.replace(old, new))' in mac_graphics.read_text(encoding="utf-8") and 'godot=$build_source' not in mac_graphics.read_text(encoding="utf-8"):
        blockers.append("Mac graphics build patches a pinned Godot cache checkout in place; stage a disposable source clone instead.")
    blockers.append("Release signing, binary provenance and fresh-machine installation are unverified.")
    license_summary: dict[str, Any] | None = None
    checker = SCRIPT.with_name("license_audit.py")
    if checker.is_file():
        command = [sys.executable, str(checker), "--source", str(destination), "--summary-only"]
        for item in cargo_metadata or []:
            command.extend(["--cargo-metadata", str(item)])
        if binary_bom:
            command.extend(["--binary-bom", str(binary_bom)])
        completed = subprocess.run(command, capture_output=True, text=True, check=False)
        if completed.returncode in (0, 2):
            license_summary = json.loads(completed.stdout)
            blockers.extend(f"License: {reason}" for reason in license_summary["blockers"])
        else:
            blockers.append("License audit did not complete: " + completed.stderr.strip()[:350])
    else:
        blockers.append("License auditor is not installed beside the public release tool.")
    return {**integrity, "status": "release_blocked" if blockers else "candidate_review_required",
            "test_only_crates": test_only_crates, "inline_test_source_files": inline_test_files,
            "license_audit_counts": license_summary.get("counts") if license_summary else None,
            "blockers": blockers, "release_ready": False}


def compare_snapshots(previous: Path, current: Path) -> dict[str, Any]:
    """Preview the next public Git commit; never mutate either source tree."""
    verify(previous)
    verify(current)
    previous_files: dict[str, str] = read_json(previous / PUBLIC_MANIFEST)["files"]
    current_files: dict[str, str] = read_json(current / PUBLIC_MANIFEST)["files"]
    before = set(previous_files)
    after = set(current_files)
    modified = sorted(path for path in before & after
                      if previous_files[path] != current_files[path])
    return {
        "status": "reviewable_public_source_delta",
        "added": sorted(after - before),
        "removed": sorted(before - after),
        "modified": modified,
        "totals": {"added": len(after - before), "removed": len(before - after),
                   "modified": len(modified)},
        "previous_manifest_sha256": file_hash(previous / PUBLIC_MANIFEST),
        "current_manifest_sha256": file_hash(current / PUBLIC_MANIFEST),
        "publication_performed": False,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--policy", type=Path, default=DEFAULT_POLICY)
    sub = parser.add_subparsers(dest="command", required=True)
    for command in ("plan", "export"):
        options = sub.add_parser(command)
        options.add_argument("--source", type=Path, required=True)
        if command == "export":
            options.add_argument("--destination", type=Path, required=True)
            options.add_argument("--license-file", type=Path)
            options.add_argument("--normalize-lock", action="store_true", help="Prune copied lock offline with Cargo; reject upgrades")
    for command in ("verify", "audit"):
        options = sub.add_parser(command)
        options.add_argument("--destination", type=Path, required=True)
        if command == "audit":
            options.add_argument("--cargo-metadata", type=Path, action="append", default=[])
            options.add_argument("--binary-bom", type=Path)
    compare = sub.add_parser("diff")
    compare.add_argument("--previous", type=Path, required=True)
    compare.add_argument("--current", type=Path, required=True)
    args = parser.parse_args(argv)
    try:
        policy = read_json(args.policy)
        if args.command == "verify":
            result = verify(args.destination.resolve())
        elif args.command == "audit":
            result = release_audit(args.destination.resolve(), cargo_metadata=args.cargo_metadata,
                                   binary_bom=args.binary_bom)
        elif args.command == "diff":
            result = compare_snapshots(args.previous.resolve(), args.current.resolve())
        else:
            source = args.source.resolve(strict=True)
            if args.command == "plan":
                result = plan(source, policy)
                result.pop("_generated_cargo")
                result.pop("files")  # paths remain in generated source manifest
            else:
                result = export(source, args.destination.absolute(), policy,
                                license_file=args.license_file, normalize_lock=args.normalize_lock)
                verify(args.destination.absolute())
        print(json.dumps(result, indent=2, sort_keys=True))
        if args.command == "audit" and result["blockers"]:
            return 2
    except (ExportError, OSError, ValueError, KeyError, tomllib.TOMLDecodeError) as error:
        print(f"PUBLIC EXPORT FAILED: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
"""Fail-closed Station license/provenance inventory, not a legal clearance tool.

Run with `cargo metadata --locked --format-version=1 --all-features` JSON from
both macOS and Linux CI. Licenses listed by a manifest are *claims*; the
release must additionally ship correct texts/notices for the exact binaries.
"""

from __future__ import annotations

import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import re
import sys
import tomllib
from typing import Any

from public_release import locked_reachable_packages, read_toml

DEFAULT_POLICY = Path(__file__).with_name("license_policy.json")
MANIFEST = "PUBLIC_SOURCE_MANIFEST.json"


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def normalize_legacy_cargo_license(expression: str) -> str:
    # Cargo historically used slash as a deprecated OR separator. Normalize
    # only observed simple identifier alternatives, preserving the raw metadata.
    known = {"MIT/Apache-2.0", "Apache-2.0/MIT", "Unlicense/MIT",
             "Apache-2.0 / MIT", "MIT/BSD-3-Clause"}
    return " OR ".join(part.strip() for part in expression.split("/")) if expression in known else expression


def spdx_allowed(expression: str, permitted: set[str]) -> tuple[bool, bool]:
    """Evaluate a strict SPDX AND/OR/WITH subset; reject ambiguous syntax.

    Cargo-deny is the release authority for the complete SPDX grammar. This
    parser is an independent conservative preflight, not its replacement.
    """
    token_re = re.compile(r"\s*(\(|\)|AND\b|OR\b|WITH\b|[A-Za-z0-9][A-Za-z0-9.+-]*)")
    tokens: list[str] = []
    index = 0
    while index < len(expression):
        found = token_re.match(expression, index)
        if not found:
            return False, False
        tokens.append(found.group(1))
        index = found.end()
    if not tokens:
        return False, False

    pos = 0

    def atom() -> bool:
        nonlocal pos
        if pos >= len(tokens):
            raise ValueError("incomplete SPDX expression")
        if tokens[pos] == "(":
            pos += 1
            value = parse_or()
            if pos >= len(tokens) or tokens[pos] != ")":
                raise ValueError("unclosed SPDX expression")
            pos += 1
            return value
        identifier = tokens[pos]
        if identifier in ("AND", "OR", "WITH", ")"):
            raise ValueError("unexpected SPDX operator")
        pos += 1
        if pos < len(tokens) and tokens[pos] == "WITH":
            pos += 1
            if pos >= len(tokens) or tokens[pos] in ("AND", "OR", "WITH", "(", ")"):
                raise ValueError("missing SPDX exception identifier")
            identifier += " WITH " + tokens[pos]
            pos += 1
        return identifier in permitted

    def parse_and() -> bool:
        nonlocal pos
        value = atom()
        while pos < len(tokens) and tokens[pos] == "AND":
            pos += 1
            following = atom()
            value = value and following
        return value

    def parse_or() -> bool:
        nonlocal pos
        value = parse_and()
        while pos < len(tokens) and tokens[pos] == "OR":
            pos += 1
            following = parse_and()
            value = value or following
        return value

    try:
        result = parse_or()
        if pos != len(tokens):
            raise ValueError("extra SPDX tokens")
    except ValueError:
        return False, False
    return result, True


def cargo_metadata_index(files: list[Path]) -> tuple[dict[tuple[str, str, str], dict[str, Any]], list[str]]:
    index: dict[tuple[str, str, str], dict[str, Any]] = {}
    errors: list[str] = []
    for path in files:
        data = json.loads(path.read_text(encoding="utf-8"))
        for package in data.get("packages", []):
            key = (package["name"], package["version"], package.get("source") or "")
            previous = index.get(key)
            if previous and (previous.get("license") != package.get("license")
                             or previous.get("license_file") != package.get("license_file")):
                errors.append(f"Conflicting Cargo license metadata for {key[0]} {key[1]}")
            index[key] = package
    return index, errors


def local_license(snapshot: Path, record: dict[str, Any]) -> tuple[str | None, str | None]:
    path = record.get("local_manifest")
    if not path:
        return None, None
    manifest = read_toml(snapshot / path)
    pkg = manifest.get("package", {})
    return pkg.get("license"), pkg.get("license-file")


def native_spdx_inventory(snapshot: Path, permitted: set[str]) -> dict[str, Any]:
    root = snapshot / "crates/polar-cas-fs-sys/native/vendor/erofs-utils"
    if not root.is_dir():
        return {"status": "missing", "files": 0, "spdx_expressions": {}, "requires_review": []}
    statuses: Counter[str] = Counter()
    exceptions: list[str] = []
    missing_headers: list[str] = []
    total = 0
    for file in sorted(root.rglob("*")):
        if not file.is_file() or file.suffix.lower() not in {".c", ".h", ".cc", ".cpp", ".inc"}:
            continue
        total += 1
        header = file.read_bytes()[:4096].decode("utf-8", errors="replace")
        found = re.search(r"SPDX-License-Identifier:\s*([^\r\n*]+)", header)
        if found:
            expression = found.group(1).strip()
            statuses[expression] += 1
            allowed, valid = spdx_allowed(expression, permitted)
            if not allowed or not valid:
                exceptions.append(file.relative_to(snapshot).as_posix() + " :: " + expression)
        else:
            statuses["UNDECLARED_FILE_HEADER"] += 1
            missing_headers.append(file.relative_to(snapshot).as_posix())
    return {"status": "file_spdx_preview_only", "files": total,
            "spdx_expressions": dict(sorted(statuses.items())),
            "requires_review": exceptions, "missing_spdx_headers": missing_headers}


def pinned_text_evidence(snapshot: Path) -> tuple[list[dict[str, Any]], list[str]]:
    """Validate collected exact-pin text hashes, never infer binary clearance."""
    path = snapshot / "licenses/pinned-evidence.json"
    if not path.is_file():
        return [], []
    data = json.loads(path.read_text(encoding="utf-8"))
    verified, errors = [], []
    for entry in data.get("files", []):
        if entry.get("status") != "fetched":
            continue
        relative = entry.get("path", "")
        file = snapshot / relative
        if (not relative or Path(relative).is_absolute()
                or not file.resolve().is_relative_to(snapshot.resolve())
                or not file.is_file() or digest(file) != entry.get("sha256")):
            errors.append(f"Pinned license evidence missing or hash mismatch: {relative}")
            continue
        commit = entry.get("commit", "")
        if not re.fullmatch(r"[0-9a-f]{40}", commit) or commit not in entry.get("url", ""):
            errors.append(f"Pinned license evidence lacks exact revision URL: {relative}")
            continue
        verified.append(entry)
    return verified, errors


def audit_snapshot(
    snapshot: Path,
    policy: dict[str, Any],
    metadata_files: list[Path] | None = None,
    binary_bom: Path | None = None,
) -> dict[str, Any]:
    metadata_files = metadata_files or []
    source_manifest = json.loads((snapshot / MANIFEST).read_text(encoding="utf-8"))
    if policy["schema_version"] != 1 or source_manifest["product"] != policy["product"]:
        raise ValueError("license policy and source snapshot mismatch")
    permitted = set(policy["automatically_permissible_spdx"])
    reachable = locked_reachable_packages(snapshot, policy["cargo_roots"])
    metadata, blockers = cargo_metadata_index(metadata_files)
    # Portable, package-keyed metadata claims captured from the resolved Cargo
    # report. Never upload the full report's absolute workstation paths.
    metadata_origin = "supplied_cargo_metadata" if metadata_files else "unavailable"
    cached_claims = snapshot / "licenses/cargo-evidence.json"
    if not metadata_files and cached_claims.is_file():
        cached = json.loads(cached_claims.read_text(encoding="utf-8"))
        for item in cached.get("packages", []):
            metadata[(item["name"], item["version"], item["source"])] = {
                "license": item.get("license"), "license_file": None}
        metadata_origin = "captured_package_claims_matched_to_current_lock"
    text_evidence, text_errors = pinned_text_evidence(snapshot)
    blockers.extend(text_errors)
    workspace = source_manifest["crate_paths"]
    owner_record_path = snapshot / "automation/polar_station/first_party_licensing.json"
    owner_record = json.loads(owner_record_path.read_text()) if owner_record_path.is_file() else {}
    owner_license_valid = ((snapshot / "LICENSE").is_file() and
                           digest(snapshot / "LICENSE") == owner_record.get("license_sha256"))
    packages: list[dict[str, Any]] = []
    missing_metadata: list[str] = []
    needs_manual: list[str] = []
    local_owners_unlicensed: list[str] = []
    for locked in sorted(reachable, key=lambda p: (p["name"], p["version"], p.get("source", ""))):
        source = locked.get("source", "")
        kind = "registry" if source.startswith("registry+") else "git" if source.startswith("git+") else "local"
        key = (locked["name"], locked["version"], source)
        meta = metadata.get(key)
        local_manifest = (f"{workspace[locked['name']]}/Cargo.toml"
                          if kind == "local" and locked["name"] in workspace else None)
        package: dict[str, Any] = {
            "name": locked["name"], "version": locked["version"],
            "source": source or "workspace", "source_kind": kind,
            "checksum": locked.get("checksum"), "local_manifest": local_manifest,
        }
        if kind == "local":
            expression, license_file = local_license(snapshot, package)
            if not local_manifest:
                blockers.append(f"Unexported local crate in lock graph: {locked['name']}")
        elif meta:
            expression, license_file = meta.get("license"), meta.get("license_file")
        else:
            expression, license_file = None, None
            missing_metadata.append(f"{locked['name']}@{locked['version']} [{kind}]")
        if (kind == "local" and local_manifest and local_manifest.startswith("crates/polar-")
                and owner_license_valid and locked["name"] in owner_record.get("first_party_packages", [])
                and expression == owner_record.get("license")):
            status = "owner_approved_first_party_license"
        elif expression:
            allowed, valid = spdx_allowed(normalize_legacy_cargo_license(expression), permitted)
            status = "permissive_expression" if allowed and valid else "review_required" if valid else "invalid_spdx"
            if not allowed:
                needs_manual.append(f"{locked['name']}@{locked['version']}: {expression}")
        elif license_file:
            status = "license_file_requires_manual_review"
            needs_manual.append(f"{locked['name']}@{locked['version']}: file {license_file}")
        elif kind == "local" and local_manifest and local_manifest.startswith("crates/polar-"):
            status = "first_party_root_license_pending"
            local_owners_unlicensed.append(locked["name"])
        else:
            status = "unresolved"
            if kind == "local":
                needs_manual.append(f"{locked['name']}@{locked['version']}: missing local license")
        package["license_expression"] = expression
        package["normalized_license_expression"] = normalize_legacy_cargo_license(expression) if expression else None
        package["license_file"] = license_file
        package["expression_review"] = status
        packages.append(package)

    cargo_evidence_path = snapshot / "licenses/cargo-evidence.json"
    cargo_evidence_status = {"status": "not_provided"}
    if cargo_evidence_path.is_file():
        evidence = json.loads(cargo_evidence_path.read_text(encoding="utf-8"))
        evidence_index = {(e["name"], e["version"], e["source"]): e for e in evidence.get("packages", [])}
        no_texts, invalid = [], []
        checked = 0
        for package in packages:
            if package["source_kind"] == "local":
                continue
            record = evidence_index.get((package["name"], package["version"], package["source"]))
            if not record or not record.get("legal_texts"):
                no_texts.append(package["name"] + "@" + package["version"])
                continue
            for entry in record["legal_texts"]:
                file = snapshot / entry["path"]
                if (Path(entry["path"]).is_absolute() or not file.resolve().is_relative_to(snapshot.resolve())
                        or not file.is_file() or digest(file) != entry.get("sha256")):
                    invalid.append(package["name"] + ":" + entry["path"])
            checked += 1
        cargo_evidence_status = {"status": "source_texts_only_not_binary_clearance", "packages_with_texts": checked,
                                 "missing_text_packages": no_texts, "invalid_texts": invalid}
        if no_texts:
            blockers.append(f"{len(no_texts)} Cargo packages lack collected legal texts; metadata alone is not complete notice coverage.")
        if invalid:
            blockers.append(f"{len(invalid)} Cargo legal text hashes or paths failed validation.")

    if missing_metadata:
        blockers.append(f"Cargo metadata missing for {len(missing_metadata)} external packages (registry/git). Run locked metadata on CI.")
    if needs_manual:
        blockers.append(f"{len(needs_manual)} Cargo packages require license expression or file review.")
    if local_owners_unlicensed:
        blockers.append(f"{len(local_owners_unlicensed)} first-party crates depend on an undeclared project root license.")
    if not (snapshot / "LICENSE").is_file():
        blockers.append("Public project root LICENSE is absent.")

    git_sources = sorted({p["source"] for p in reachable if p.get("source", "").startswith("git+")})
    git_catalog: list[dict[str, Any]] = []
    for source in git_sources:
        matches = [item for item in policy.get("required_git_sources", [])
                   if source.startswith("git+" + item["repository"])
                   and source.endswith("#" + item["commit"])]
        if len(matches) != 1:
            blockers.append(f"Git dependency has no exact pinned license catalog entry: {source}")
        else:
            evidence = [e for e in text_evidence
                        if e.get("repository") == matches[0]["repository"]
                        and e.get("commit") == matches[0]["commit"]
                        and e.get("upstream_path", "").upper().startswith(("LICENSE", "COPYING", "NOTICE"))]
            git_catalog.append({**matches[0], "cargo_source": source,
                                "pin_license_texts_verified": any(e["upstream_path"].upper().startswith(("LICENSE", "COPYING")) for e in evidence),
                                "verified_legal_texts": evidence,
                                "pin_license_verified": False})
    missing_git_texts = sum(not e.get("pin_license_texts_verified") for e in git_catalog)
    if missing_git_texts:
        blockers.append(f"Exact commit license texts remain unavailable for {missing_git_texts} Git repositories.")
    if git_catalog:
        blockers.append("Git repository license texts do not resolve all package-level attribution and mixed-license obligations.")

    native: list[dict[str, Any]] = []
    for entry in policy["required_native_components"]:
        missing_files = [rel for rel in entry["local_provenance"] if not (snapshot / rel).is_file()]
        license_file = entry.get("bundled_license")
        has_license = bool(license_file and (snapshot / license_file).is_file())
        extra_texts = {
            rel: (digest(snapshot / rel) if (snapshot / rel).is_file() else None)
            for rel in entry.get("bundled_additional_legal_texts", [])
        }
        native.append({**entry, "provenance_present": not missing_files,
                       "license_text_sha256": digest(snapshot / license_file) if has_license else None,
                       "license_text_in_source": has_license,
                       "extra_legal_text_sha256": extra_texts,
                       "release_clearance": "requires_exact_binary_notice_review"})
        for rel in missing_files:
            blockers.append(f"Native license provenance missing: {entry['id']} → {rel}")
        if not has_license:
            blockers.append(f"Native license text not in exported source: {entry['id']}")
        for rel, value in extra_texts.items():
            if value is None:
                blockers.append(f"Native additional legal text absent: {entry['id']} → {rel}")

    erofs = native_spdx_inventory(snapshot, permitted)
    if erofs["requires_review"]:
        blockers.append(f"{len(erofs['requires_review'])} EROFS source file SPDX expressions require individual review.")
    if erofs.get("missing_spdx_headers"):
        blockers.append(f"{len(erofs['missing_spdx_headers'])} EROFS C source files lack a per-file SPDX header; verify inherited COPYING and any per-directory license.")
    if erofs["status"] == "missing":
        blockers.append("EROFS vendored native source is absent.")

    if binary_bom is None:
        blockers.append("No native binary dependency BOM for macOS/Linux: Graphviz recursive dylibs and Ghostty/Godot dependencies unresolved.")
        bom_status = "not_provided"
    else:
        binary = json.loads(binary_bom.read_text(encoding="utf-8"))
        entries = binary.get("components", [])
        if binary.get("schema_version") != 1 or binary.get("blockers") or not entries:
            blockers.append("Binary BOM is not a complete, blocker-free schema-v1 report.")
        if not entries or not all(e.get("name") and e.get("sha256") and e.get("license")
                                  and e.get("license_text_sha256") for e in entries):
            blockers.append("Binary BOM lacks component hashes, license or corresponding license-text hashes.")
        # A supplied BOM does not establish completeness. Native build must
        # verify that every Mach-O/shared library and asset was enumerated.
        blockers.append("Binary BOM coverage has not been independently matched to the actual app bundle.")
        bom_status = "provided_but_unverified"

    blockers.append("Full third-party notice generation and distributable license-text inspection have not passed.")
    blockers.append("Manual review of MPL/EPL/source-offer obligations and legal notices is outstanding.")
    kinds = Counter(p["source_kind"] for p in packages)
    report = {
        "schema_version": 1,
        "product": "polar-station",
        "status": "blocked_pending_licenses_and_binaries",
        "source_manifest_sha256": digest(snapshot / MANIFEST),
        "counts": {"total_cargo_packages": len(packages), **dict(sorted(kinds.items())),
                   "missing_cargo_metadata": len(missing_metadata),
                   "requires_individual_review": len(needs_manual),
                   "first_party_license_pending": len(local_owners_unlicensed),
                   "native_components": len(native),
                   "pinned_git_repositories": len(git_sources)},
        "cargo_packages": packages,
        "cargo_metadata_origin": metadata_origin,
        "cargo_text_evidence": cargo_evidence_status,
        "missing_cargo_metadata": missing_metadata,
        "cargo_needs_manual_review": needs_manual,
        "native_components": native,
        "pinned_git_sources": git_catalog,
        "pinned_text_evidence": text_evidence,
        "erofs_spdx": erofs,
        "binary_bom": bom_status,
        "required_post_build_evidence": policy["required_post_build_evidence"],
        "blockers": sorted(set(blockers)),
        "release_ready": False,
    }
    return report


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--policy", type=Path, default=DEFAULT_POLICY)
    parser.add_argument("--cargo-metadata", type=Path, action="append", default=[])
    parser.add_argument("--binary-bom", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--summary-only", action="store_true")
    args = parser.parse_args(argv)
    try:
        policy = json.loads(args.policy.read_text(encoding="utf-8"))
        report = audit_snapshot(args.source.resolve(), policy, args.cargo_metadata, args.binary_bom)
        if args.output:
            if args.output.exists() or args.output.is_symlink():
                raise ValueError(f"report exists; refusing overwrite: {args.output}")
            args.output.write_text(json.dumps(report, indent=2, ensure_ascii=False, sort_keys=True) + "\n",
                                   encoding="utf-8")
        result = ({k: report[k] for k in ("status", "counts", "binary_bom", "blockers", "release_ready")}
                  if args.summary_only else report)
        print(json.dumps(result, indent=2, ensure_ascii=False, sort_keys=True))
        return 2 if report["blockers"] else 0
    except (OSError, ValueError, KeyError, tomllib.TOMLDecodeError) as exc:
        print(f"LICENSE AUDIT FAILED: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())

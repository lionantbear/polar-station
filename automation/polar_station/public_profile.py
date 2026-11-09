#!/usr/bin/env python3
"""Idempotent, fail-closed public desktop profile for exported Station source."""
from pathlib import Path

UNAVAILABLE_VIEW = '''    public var body: some View {
        VStack(spacing: 16) {
            Image(systemName: "iphone.slash")
                .font(.largeTitle)
            Text("iPhone Companion — Unavailable")
                .font(.title2.bold())
            Text("Companion distribution is not enabled for this public build.")
                .multilineTextAlignment(.center)
            Button("Done", action: dismiss)
                .keyboardShortcut(.cancelAction)
        }
        .padding(24)
        .frame(minWidth: 390, minHeight: 240)
        .accessibilityIdentifier("iphone-companion-unavailable")
    }
'''

def replace_once(path: Path, old: str, new: str) -> None:
    if not path.exists():
        return  # synthetic export fixture; production required inputs are checked separately
    text = path.read_text(encoding="utf-8")
    if new in text and old not in text:
        return
    if text.count(old) != 1:
        raise ValueError(f"public profile source drift: {path.name}")
    path.write_text(text.replace(old, new), encoding="utf-8")


def apply(root: Path) -> None:
    apply_first_party_license(root)
    replace_once(root / "apps/polar-station-macos/ghostty-module/PolarStationRuntime.swift",
                 '"host", "--listen", "0.0.0.0:47820", "--advertise",',
                 '"host", "--listen", "127.0.0.1:47820",')
    replace_once(root / "apps/polar-station-linux/systemd/polar-tmux-host.service",
                 '--listen 0.0.0.0:47820 --advertise', '--listen 127.0.0.1:47820')
    view = root / "apps/polar-station-macos/Sources/PolarTmuxMacUI/IPhoneCompanionView.swift"
    if view.exists():
        text = view.read_text(encoding="utf-8")
        if 'accessibilityIdentifier("iphone-companion-unavailable")' not in text:
            start = text.index('    public var body: some View {')
            end = text.index('\n    private var header:', start)
            old = text[start:end]
            if '.onAppear(perform: refresh)' not in old:
                raise ValueError("Companion view source drift; manual review required")
            view.write_text(text[:start] + UNAVAILABLE_VIEW + text[end:], encoding="utf-8")

    apply_test_cleanup(root)
    apply_test_cleanup(root, "public_native_cleanup.json")

    # Standalone SwiftPM test targets refer to excluded private Tests directories.
    manifest = root / "apps/polar-station-macos/Package.swift"
    if manifest.exists():
        text = manifest.read_text(encoding="utf-8")
        marker = "        .testTarget("
        if marker in text:
            start = text.index(marker)
            # This reviewed manifest has production targets first and tests last.
            if ".target(" in text[start:]:
                raise ValueError("Swift manifest target order drift")
            manifest.write_text(text[:start] + "    ]\n)\n", encoding="utf-8")


def apply_first_party_license(root: Path) -> None:
    """Apply the owner's recorded license to only the listed first-party crates."""
    import json, hashlib, tomllib
    specification = root / "automation/polar_station/first_party_licensing.json"
    license_path = root / "LICENSE"
    if not specification.is_file() or not license_path.is_file():
        return
    data = json.loads(specification.read_text())
    if hashlib.sha256(license_path.read_bytes()).hexdigest() != data["license_sha256"]:
        raise ValueError("Project license differs from owner-approved licensing record")
    for manifest in sorted((root / "crates").glob("*/Cargo.toml")):
        text = manifest.read_text()
        package = tomllib.loads(text).get("package", {})
        if package.get("name") not in data["first_party_packages"]:
            continue
        if "license-file" in package:
            raise ValueError("Existing first-party license-file requires review")
        current = package.get("license")
        if current == data["license"]:
            continue
        if current == "Apache-2.0":
            # Explicitly superseded, unpublished first-party staging declaration.
            text = text.replace('license = "Apache-2.0"', 'license = "' + data["license"] + '"', 1)
        elif current is None:
            text = text.replace("[package]\n", '[package]\nlicense = "' + data["license"] + '"\n', 1)
        else:
            raise ValueError("Existing first-party license requires review: " + manifest.as_posix())
        manifest.write_text(text)


def apply_test_cleanup(root: Path, filename: str = "public_test_cleanup.json") -> None:
    """Replay reviewed AST removals, refusing drift before mutating any file."""
    import json, hashlib
    specification = root / "automation/polar_station" / filename
    if not specification.is_file():
        return
    data = json.loads(specification.read_text())
    digest = lambda b: hashlib.sha256(b).hexdigest()
    pending = {}
    deleted = {entry["path"] for entry in data["deletions"]}
    final_hashes = {}
    if filename == "public_test_cleanup.json":
        followup = root / "automation/polar_station/public_native_cleanup.json"
        if followup.is_file():
            later = json.loads(followup.read_text())
            final_hashes = {entry["path"]: entry["after"] for entry in later["edits"]}
            deleted.update(entry["path"] for entry in later["deletions"])

    def safe_path(relative):
        path = root / relative
        if not path.resolve().is_relative_to(root.resolve()) or path.is_symlink():
            raise ValueError("unsafe cleanup path: " + relative)
        return path
    for entry in data["edits"]:
        path = safe_path(entry["path"])
        if not path.exists() and entry["path"] in deleted:
            continue
        before = path.read_bytes()
        if digest(before) in (entry["after"], final_hashes.get(entry["path"])):
            continue
        if digest(before) != entry["before"]:
            raise ValueError("test cleanup source drift: " + entry["path"])
        after = before
        for start, end, replacement in reversed(entry["ranges"]):
            after = after[:start] + replacement.encode() + after[end:]
        if digest(after) != entry["after"]:
            raise ValueError("test cleanup patch integrity: " + entry["path"])
        pending[entry["path"]] = after
    for entry in data["deletions"]:
        path = safe_path(entry["path"])
        if path.exists():
            content = pending.get(entry["path"], path.read_bytes())
            if digest(content) != entry["sha256"]:
                raise ValueError("test deletion source drift: " + entry["path"])
    for relative, content in pending.items():
        safe_path(relative).write_bytes(content)
    for entry in data["deletions"]:
        path = safe_path(entry["path"])
        path.unlink(missing_ok=True)
        parent = path.parent
        while parent != root:
            try:
                parent.rmdir()
            except OSError:
                break
            parent = parent.parent

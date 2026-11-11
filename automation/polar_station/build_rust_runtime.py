"""Build standalone Station artifacts from an exported public source tree."""
from __future__ import annotations

import argparse
from pathlib import Path
import sys

import subprocess as public_subprocess


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-root", required=True, type=Path)
    parser.add_argument("--profile", required=True, choices=("debug", "release"))
    parser.add_argument("--operation", choices=("build", "check", "clippy", "nextest"), default="build")
    parser.add_argument("--scope", choices=("host", "host-minimal", "codex-e2e", "telegram", "bridge", "serve", "kernel", "contracts", "graphics", "local-access", "lgp", "reader-wasm"), default="host")
    parser.add_argument("--offline", action="store_true", help="Use only prefetched Cargo dependencies")
    args = parser.parse_args()
    if args.operation == "build" and args.scope not in ("host", "reader-wasm", "graphics"):
        parser.error("build produces the paired Host artifacts; use check/clippy/nextest for other scopes")
    args.source_root = args.source_root.resolve()
    if not (args.source_root / "PUBLIC_SOURCE_MANIFEST.json").is_file():
        parser.error("this adapter requires an exported public source tree")
    # Export-only adapter: never write an inherited private/shared target.
    target = args.source_root / "target"
    packages = {
        "lgp": ("polar-lgp-reader", "polar-edge-presentation", "polar-lgp-runtime", "polar-connector-runtime", "polar-lgp-presentation", "polar-inhouse-contract", "polar-inhouse-runtime", "polar-inhouse-backend-adam", "polar-inhouse-backend-codex", "polar-inhouse-backend-acp", "polar-inhouse-backend-pi", "polar-navigation-runtime", "polar-navigation-store-sqlite", "polar-station-lgp"),
        "local-access": ("polar", "polar-tmux", "polar-local-access-contract", "polar-local-access-runtime", "polar-sdk", "polar-vault", "polar-trust", "polar-vault-store-sqlite", "polar-node-ops"),
        "host": ("polar-tmux",),
        "host-minimal": ("polar-tmux",),
        "codex-e2e": ("polar-tmux",),
        "telegram": ("polar-telegram",),
        "bridge": ("polar-chat-bridge",),
        "serve": ("polar-serve",),
        "kernel": ("polar", "polar-lgp-store-sqlite"),
        "reader-wasm": ("polar-lgp-reader",),
        "graphics": ("polar-graphics-godot", "polar-graphics-package", "polar-force-layout-core"),
        "contracts": ("polar-ideacave-presentation", "polar-lgp-reader", "polar-tmux-workspace", "polar-tmux-types", "polar-station-input", "polar-station-lgp", "polar-tmux-protocol", "polar-station-settings", "polar-station-store-sqlite", "polar-station-session", "polar-graphics-godot", "polar-graphics-package", "polar-force-layout-core"),
    }[args.scope]
    if args.operation != "build" and args.scope in ("host", "host-minimal"):
        packages += ("polar-tmux-core",)
    command = ["cargo", "nextest", "run"] if args.operation == "nextest" else ["cargo", args.operation]
    command += ["--locked", "--manifest-path", "Cargo.toml", "--target-dir", str(target)]
    if args.offline:
        command.append("--offline")
    for package in packages:
        command += ["-p", package]
    if args.scope == "lgp":
        command += ["--features", "polar-inhouse-backend-pi/pi-backend"]
    if args.scope == "telegram":
        command += ["--no-default-features", "--features", "polar-telegram/bot-api"]
    if args.scope == "host-minimal":
        command += ["--no-default-features"]
    if args.scope == "codex-e2e" and args.operation == "nextest":
        command += ["--run-ignored", "only", "-E", "test(real_codex_original_thread_control)"]
    if args.scope == "kernel" and args.operation == "nextest":
        command += ["-E", "test(station_command) | package(polar-lgp-store-sqlite)"]
    if args.operation == "build":
        command += ["--lib", "--target", "wasm32-unknown-unknown"] if args.scope == "reader-wasm" else (["--lib"] if args.scope == "graphics" else ["--lib", "--bin", "polar-tmux"])
    elif args.operation in ("clippy", "check"):
        command += ["--all-targets"]
    else:
        command += ["--no-fail-fast", "--status-level", "fail", "--final-status-level", "fail"]
    if args.profile == "release":
        command.append("--release")
    if args.operation == "clippy":
        command += ["--", "-D", "warnings"]
    result = public_subprocess.run(command, cwd=args.source_root, stdout=sys.stderr, check=False).returncode
    if result:
        return result
    if args.operation == "build" and args.scope == "reader-wasm":
        import shutil
        destination = args.source_root / "site/frontend/polar-webui/src/generated/reader.wasm"
        destination.parent.mkdir(parents=True, exist_ok=True)
        artifact = target / "wasm32-unknown-unknown" / args.profile / "polar_lgp_reader.wasm"
        shutil.copyfile(artifact, destination)
        import hashlib
        import json
        inputs = [args.source_root / name for name in ("Cargo.toml", "rust-toolchain.toml",
            "crates/polar-lgp-reader/Cargo.toml", "automation/polar_station/build_rust_runtime.py",
            "site/frontend/polar-webui/src/reader-provenance.mjs")]
        inputs += sorted((args.source_root / "crates/polar-lgp-reader/src").rglob("*.rs"))
        digest = lambda path: hashlib.sha256(path.read_bytes()).hexdigest()
        import subprocess
        lock_packages = json.loads(subprocess.check_output([
            "node", str(args.source_root / "site/frontend/polar-webui/src/reader-provenance.mjs"),
            str(args.source_root / "Cargo.lock"),
        ], text=True))
        receipt = {"artifact_sha256": digest(destination), "lock_packages": lock_packages, "inputs": {
            str(path.relative_to(args.source_root)): digest(path) for path in inputs}}
        destination.with_suffix(".json").write_text(json.dumps(receipt, indent=2) + "\n")
        print(destination)
    elif args.operation == "build":
        print(target / args.profile)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

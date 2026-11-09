#!/usr/bin/env python3
"""Explicit dependency fetch, never called implicitly from Cargo/build_erofs.

Uses the fixed upstream Git commit, not latest/default branch. This script
requires network access; build_erofs.py itself is offline.
"""
import argparse
import json
from pathlib import Path
import subprocess
from source_store import import_archive, validate_source


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("destination", type=Path)
    parser.add_argument("--archive", type=Path, help="admit a local source tarball without using the network")
    args = parser.parse_args()
    if args.archive is not None:
        print(import_archive(args.archive, args.destination))
        return
    lock = json.loads(Path(__file__).with_name("upstream.lock.json").read_text())
    if args.destination.exists():
        parser.error("destination already exists; not overwriting it")
    args.destination.mkdir(parents=True)
    def run(*cmd):
        subprocess.run(["git", "-c", "core.hooksPath=/dev/null", *cmd],
                       cwd=args.destination, check=True)
    run("init", "--quiet")
    run("remote", "add", "origin", lock["repository"])
    run("fetch", "--depth=1", "origin", lock["commit"])
    run("checkout", "--detach", lock["commit"])
    found = subprocess.check_output(["git", "rev-parse", "HEAD"],
                                    cwd=args.destination, text=True).strip()
    if found != lock["commit"]:
        raise RuntimeError("upstream commit mismatch")
    validate_source(args.destination, lock)
    print(args.destination.resolve())


if __name__ == "__main__":
    main()

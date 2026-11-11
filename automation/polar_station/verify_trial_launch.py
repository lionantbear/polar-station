#!/usr/bin/env python3
"""macOS ad-hoc trial regression gate. Run in a dedicated logged-in GUI session.

This proves terminal child execution, not Finder/quarantine, visual rendering,
TCC, updater operation, or clean-machine compatibility. No tokens are forwarded.
Ghostty Config.zig custom CLI parsing consumes all arguments after -e verbatim
as initial-command (direct argv), so /bin/sh -c receives positional arguments.
"""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import platform
import plistlib
import re
import secrets
import signal
import subprocess
import time


def require(ok, message):
    if not ok:
        raise RuntimeError(message)


def clean_environment(home):
    return {"PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "HOME": str(home),
            "XDG_CONFIG_HOME": str(home / "config"),
            "XDG_CACHE_HOME": str(home / "cache"), "LANG": "en_US.UTF-8"}


def validate_signature(signature, entitlements):
    require(re.search(r"^Signature=adhoc$", signature, re.M), "not_ad_hoc_signed")
    require(re.search(r"flags=0x[0-9a-fA-F]+\([^\n)]*\bruntime\b", signature),
            "hardened_runtime_missing")
    require(entitlements.get("com.apple.security.cs.disable-library-validation") is True,
            "main_library_validation_exception_missing")


def command(executable, sentinel, token):
    # Positional arguments avoid interpreting paths/token as shell code.
    return [str(executable), "--shell-integration=none", "--quit-after-last-window-closed=true", "-e", "/bin/sh", "-c",
            'umask 077; printf "%s" "$2" > "$1"; exit 0',
            "polar-trial-smoke", str(sentinel), token]


def run(args):
    require(platform.system() == "Darwin", "macOS_required")
    require(5 <= args.timeout <= 90, "timeout_must_be_5_to_90_seconds")
    app = args.app.resolve(strict=True)
    require(app.suffix == ".app", "expected_installed_app_bundle")
    out = args.output.resolve()
    require(not out.exists(), "output_directory_must_be_new")
    require(app not in out.parents, "output_must_be_outside_app")
    out.mkdir(parents=True, mode=0o700)
    home = out / "home"
    home.mkdir(mode=0o700)
    env = clean_environment(home)
    result = {"passed": False, "app": str(app), "started_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
              "scope": __doc__, "platform": platform.platform()}
    proc = None
    try:
        with (app / "Contents/Info.plist").open("rb") as f:
            info = plistlib.load(f)
        name = info["CFBundleExecutable"]
        require(isinstance(name, str) and Path(name).name == name, "invalid_bundle_executable")
        executable = (app / "Contents/MacOS" / name).resolve(strict=True)
        require(app in executable.parents, "executable_outside_bundle")
        result["executable_sha256"] = hashlib.sha256(executable.read_bytes()).hexdigest()
        result["bundle_id"] = info.get("CFBundleIdentifier")
        checks = [("strict", ["--verify", "--deep", "--strict", str(app)]),
                  ("signature", ["--display", "--verbose=4", str(app)]),
                  ("entitlements", ["--display", "--entitlements", ":-", str(app)])]
        outputs = {}
        for label, flags in checks:
            check = subprocess.run(["/usr/bin/codesign", *flags], env=env, capture_output=True, timeout=30)
            (out / (label + ".stdout")).write_bytes(check.stdout)
            (out / (label + ".stderr")).write_bytes(check.stderr)
            require(check.returncode == 0, label + "_codesign_failed")
            outputs[label] = check
        validate_signature(outputs["signature"].stderr.decode(),
                           plistlib.loads(outputs["entitlements"].stdout))
        result["signature_verified"] = True
        token = secrets.token_hex(32)
        sentinel = out / "terminal-sentinel"
        with (out / "app.stdout").open("wb") as stdout, (out / "app.stderr").open("wb") as stderr:
            proc = subprocess.Popen(command(executable, sentinel, token), env=env, cwd=home,
                                    stdin=subprocess.DEVNULL, stdout=stdout, stderr=stderr,
                                    start_new_session=True)
            result["pid"] = proc.pid
            start = time.monotonic()
            while time.monotonic() - start < args.timeout:
                if sentinel.exists():
                    require(not sentinel.is_symlink(), "sentinel_is_symlink")
                    require(sentinel.stat().st_size <= 256, "sentinel_invalid_size")
                    if sentinel.read_text() == token:
                        result["terminal_child_executed"] = True
                        break
                require(proc.poll() is None, "app_exited_before_terminal_execution")
                time.sleep(0.1)
            require(result.get("terminal_child_executed"), "terminal_sentinel_timeout")
            remaining = max(0.1, args.timeout - (time.monotonic() - start))
            require(proc.wait(timeout=remaining) == 0, "app_did_not_exit_cleanly")
            result["observed_seconds"] = round(time.monotonic() - start, 3)
            result["passed"] = True
    except Exception as exc:
        result["error"] = str(exc)
        raise
    finally:
        if proc is not None:
            result["exit_code_before_cleanup"] = proc.poll()
            if proc.poll() is None:
                try:
                    os.killpg(proc.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(proc.pid, signal.SIGKILL)
                proc.wait(timeout=5)
            result["cleanup_exit_code"] = proc.returncode
        (out / "result.json").write_text(json.dumps(result, indent=2) + "\n")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--timeout", type=float, default=45)
    args = parser.parse_args()
    print(json.dumps(run(args), indent=2))


if __name__ == "__main__":
    main()

"""Install Station's versioned GNOME capture bundle and request activation."""
from __future__ import annotations
import argparse
import ast
import json
import os
from pathlib import Path
import subprocess
from theme_resources import digest, install

UUID = 'shortcuts@polarstation.polarealm.com'


def activate() -> str:
    if 'gnome' not in os.environ.get('XDG_CURRENT_DESKTOP', '').lower():
        return 'installed-not-gnome'
    enabled = subprocess.run(['gnome-extensions', 'enable', UUID], capture_output=True, text=True)
    if enabled.returncode == 0:
        return 'activation-requested'
    # Shell discovers newly installed extensions on login. Preserve every other
    # extension and the user's global disable setting; never restart the shell.
    previous = subprocess.check_output(['gsettings', 'get', 'org.gnome.shell', 'enabled-extensions'], text=True).strip()
    previous = previous.removeprefix('@as ')
    values = ast.literal_eval(previous)
    if UUID not in values:
        subprocess.run(['gsettings', 'set', 'org.gnome.shell', 'enabled-extensions', repr([*values, UUID])], check=True)
    return 'installed-pending-sign-in'


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source', type=Path)
    parser.add_argument('--destination', type=Path)
    parser.add_argument('--expected')
    args = parser.parse_args()
    if args.destination:
        if not args.expected:
            parser.error('--expected is required for installation')
        install(args.source, args.expected, args.destination)
        print(json.dumps({'uuid': UUID, 'sha256': digest(args.destination), 'status': activate()}))
    else:
        print(digest(args.source))

from __future__ import annotations

import argparse

from .application import run_application
from .model import SurfaceState


def main() -> int:
    parser = argparse.ArgumentParser(description="Polar Tmux Linux frontend")
    parser.add_argument(
        "--smoke-state",
        choices=[state.value for state in SurfaceState],
        help="map one deterministic UI state, print evidence, then exit",
    )
    parser.add_argument(
        "--smoke-quit-ms",
        type=int,
        default=250,
        help="milliseconds to keep a smoke window mapped",
    )
    args = parser.parse_args()
    if args.smoke_quit_ms < 1 or args.smoke_quit_ms > 30_000:
        parser.error("--smoke-quit-ms must be between 1 and 30000")
    state = SurfaceState(args.smoke_state) if args.smoke_state else None
    return run_application(smoke_state=state, smoke_quit_ms=args.smoke_quit_ms)


if __name__ == "__main__":
    raise SystemExit(main())

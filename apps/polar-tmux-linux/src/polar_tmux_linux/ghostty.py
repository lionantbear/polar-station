"""Typed Ghostty D-Bus integration.

The frontend accepts only a Host-produced ``TmuxAttachPlan``.  It never builds
shell text and never invokes tmux itself.
"""

from __future__ import annotations

import re
from dataclasses import dataclass
from typing import Any, Protocol

GHOSTTY_BUS_NAME = "com.mitchellh.ghostty"
GHOSTTY_OBJECT_PATH = "/com/mitchellh/ghostty"
_ALLOWED_ATTACH_PROGRAMS = {"tmux", "ssh"}
_SESSION_ID = re.compile(r"\$[0-9]+\Z", re.ASCII)
_SOCKET_NAME = re.compile(
    r"[A-Za-z0-9][A-Za-z0-9._-]{0,95}\Z",
    re.ASCII,
)
_SSH_DESTINATION = re.compile(
    r"[A-Za-z0-9_.:@%+\[\]][A-Za-z0-9_.:@%+\[\]-]{0,254}\Z",
    re.ASCII,
)
_REMOTE_ATTACH = re.compile(
    r"tmux(?: -L [A-Za-z0-9][A-Za-z0-9._-]{0,95})?"
    r" attach-session -t '(?P<session>\$[0-9]+)'\Z",
    re.ASCII,
)


class GhosttyError(RuntimeError):
    pass


def _contains_control(value: str) -> bool:
    return any(ord(char) < 0x20 or ord(char) == 0x7F for char in value)


def _is_exact_local_attach(args: list[str]) -> bool:
    if len(args) == 3:
        return args[:2] == ["attach-session", "-t"] and bool(
            _SESSION_ID.fullmatch(args[2])
        )
    if len(args) == 5:
        return (
            args[0] == "-L"
            and bool(_SOCKET_NAME.fullmatch(args[1]))
            and args[2:4] == ["attach-session", "-t"]
            and bool(_SESSION_ID.fullmatch(args[4]))
        )
    return False


def _is_exact_ssh_attach(args: list[str]) -> bool:
    return (
        len(args) == 3
        and args[0] == "-tt"
        and bool(_SSH_DESTINATION.fullmatch(args[1]))
        and bool(_REMOTE_ATTACH.fullmatch(args[2]))
    )


@dataclass(frozen=True, slots=True)
class TmuxAttachPlan:
    program: str
    args: tuple[str, ...]

    @classmethod
    def from_wire(cls, raw: Any) -> "TmuxAttachPlan":
        if not isinstance(raw, dict) or set(raw) != {"program", "args"}:
            raise GhosttyError("attach plan must contain only program and args")
        program = raw.get("program")
        args = raw.get("args")
        if not isinstance(program, str) or not program:
            raise GhosttyError("attach plan program must be a non-empty string")
        if _contains_control(program):
            raise GhosttyError("attach plan program contains a control character")
        if program not in _ALLOWED_ATTACH_PROGRAMS:
            raise GhosttyError("attach plan program is not supported")
        if not isinstance(args, list) or not all(isinstance(arg, str) for arg in args):
            raise GhosttyError("attach plan args must be an array of strings")
        if not args or any(not arg for arg in args):
            raise GhosttyError("attach plan args must contain non-empty arguments")
        if any(_contains_control(arg) for arg in args):
            raise GhosttyError("attach plan argument contains a control character")
        if program == "tmux" and not _is_exact_local_attach(args):
            raise GhosttyError("attach plan tmux arguments are not allowlisted")
        if program == "ssh" and not _is_exact_ssh_attach(args):
            raise GhosttyError("attach plan SSH arguments are not allowlisted")
        return cls(program=program, args=tuple(args))

    @property
    def argv(self) -> list[str]:
        return [self.program, *self.args]

    @property
    def session_id(self) -> str:
        if self.program == "tmux":
            return self.args[-1]
        match = _REMOTE_ATTACH.fullmatch(self.args[2])
        if match is None:  # Construction validates this invariant.
            raise GhosttyError("attach plan SSH arguments are not allowlisted")
        return match.group("session")


class GhosttyActionInvoker(Protocol):
    def new_window_command(self, argv: list[str]) -> None: ...

    def activate(self) -> None: ...


def build_gtk_action_parameters(argv: list[str]):
    import gi

    gi.require_version("GLib", "2.0")
    from gi.repository import GLib

    return GLib.Variant(
        "(sava{sv})",
        (
            "new-window-command",
            [GLib.Variant("as", argv)],
            {},
        ),
    )


class GioGhosttyActionInvoker:
    """Invoke Ghostty's documented GTK application actions over D-Bus."""

    def __init__(self, *, timeout_ms: int = 5_000) -> None:
        self._timeout_ms = timeout_ms

    @staticmethod
    def _connection():
        import gi

        gi.require_version("Gio", "2.0")
        from gi.repository import Gio

        return Gio.bus_get_sync(Gio.BusType.SESSION, None)

    def new_window_command(self, argv: list[str]) -> None:
        import gi

        gi.require_version("Gio", "2.0")
        from gi.repository import Gio

        try:
            self._connection().call_sync(
                GHOSTTY_BUS_NAME,
                GHOSTTY_OBJECT_PATH,
                "org.gtk.Actions",
                "Activate",
                build_gtk_action_parameters(argv),
                None,
                Gio.DBusCallFlags.NONE,
                self._timeout_ms,
                None,
            )
        except Exception as error:
            raise GhosttyError("Ghostty could not open the tmux attachment") from error

    def activate(self) -> None:
        import gi

        gi.require_version("Gio", "2.0")
        gi.require_version("GLib", "2.0")
        from gi.repository import Gio, GLib

        try:
            self._connection().call_sync(
                GHOSTTY_BUS_NAME,
                GHOSTTY_OBJECT_PATH,
                "org.freedesktop.Application",
                "Activate",
                GLib.Variant("(a{sv})", ({},)),
                None,
                Gio.DBusCallFlags.NONE,
                self._timeout_ms,
                None,
            )
        except Exception as error:
            raise GhosttyError("Ghostty could not be activated") from error


class GhosttyController:
    def __init__(self, invoker: GhosttyActionInvoker | None = None) -> None:
        self._invoker = invoker or GioGhosttyActionInvoker()

    def open_attach(self, plan: TmuxAttachPlan) -> None:
        # Ghostty's D-Bus action accepts Ghostty CLI arguments. ``-e`` marks
        # the remaining strings as one executable and argv vector.
        self._invoker.new_window_command(["-e", *plan.argv])

    def activate(self) -> None:
        self._invoker.activate()

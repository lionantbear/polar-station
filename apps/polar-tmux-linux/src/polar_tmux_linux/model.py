"""Privacy-minimal GTK presentation model for tmux-native Host snapshots."""

from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum
from typing import Any


_SESSION_FIELDS = {"id", "name", "window_count", "attached"}
_WINDOW_FIELDS = {
    "id",
    "session_id",
    "index",
    "name",
    "active",
    "pane_count",
    "layout",
}
_PANE_FIELDS = {"id", "window_id", "index", "title", "active", "dead"}
_SNAPSHOT_FIELDS = {"sessions", "windows", "panes"}
_TARGET_FIELDS = {"id", "display_name", "kind"}


class SurfaceState(str, Enum):
    LOADING = "loading"
    POPULATED = "populated"
    EMPTY = "empty"
    OFFLINE = "offline"
    ERROR = "error"


def _wire_string(value: Any, field_name: str) -> str:
    if not isinstance(value, str) or not value:
        raise ValueError(f"{field_name} must be a non-empty string")
    return value


def _wire_index(value: Any, field_name: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        raise ValueError(f"{field_name} must be a non-negative integer")
    return value


def _wire_bool(value: Any, field_name: str) -> bool:
    if not isinstance(value, bool):
        raise ValueError(f"{field_name} must be a boolean")
    return value


def _privacy_safe_object(
    raw: Any,
    fields: set[str],
    object_name: str,
) -> dict[str, Any]:
    if not isinstance(raw, dict) or set(raw) != fields:
        raise ValueError(f"{object_name} is not the exact privacy-safe DTO")
    return raw


@dataclass(frozen=True, slots=True)
class SessionView:
    id: str
    name: str
    window_count: int
    attached: bool

    @classmethod
    def from_wire(cls, raw: dict[str, Any]) -> "SessionView":
        raw = _privacy_safe_object(raw, _SESSION_FIELDS, "session")
        return cls(
            id=_wire_string(raw.get("id"), "session id"),
            name=_wire_string(raw.get("name"), "session name"),
            window_count=_wire_index(raw.get("window_count"), "window_count"),
            attached=_wire_bool(raw.get("attached"), "session attached"),
        )


@dataclass(frozen=True, slots=True)
class WindowView:
    id: str
    session_id: str
    index: int
    name: str
    active: bool
    pane_count: int

    @classmethod
    def from_wire(cls, raw: dict[str, Any]) -> "WindowView":
        raw = _privacy_safe_object(raw, _WINDOW_FIELDS, "window")
        if not isinstance(raw.get("layout"), str):
            raise ValueError("window layout must be a string")
        return cls(
            id=_wire_string(raw.get("id"), "window id"),
            session_id=_wire_string(raw.get("session_id"), "window session_id"),
            index=_wire_index(raw.get("index"), "window index"),
            name=_wire_string(raw.get("name"), "window name"),
            active=_wire_bool(raw.get("active"), "window active"),
            pane_count=_wire_index(raw.get("pane_count"), "pane_count"),
        )


@dataclass(frozen=True, slots=True)
class PaneView:
    id: str
    window_id: str
    index: int
    title: str
    active: bool
    dead: bool

    @classmethod
    def from_wire(cls, raw: dict[str, Any]) -> "PaneView":
        raw = _privacy_safe_object(raw, _PANE_FIELDS, "pane")
        title = raw.get("title", "")
        if not isinstance(title, str):
            raise ValueError("pane title must be a string")
        return cls(
            id=_wire_string(raw.get("id"), "pane id"),
            window_id=_wire_string(raw.get("window_id"), "pane window_id"),
            index=_wire_index(raw.get("index"), "pane index"),
            title=title,
            active=_wire_bool(raw.get("active"), "pane active"),
            dead=_wire_bool(raw.get("dead"), "pane dead"),
        )


@dataclass(slots=True)
class TargetView:
    id: str
    name: str
    kind: str
    state: SurfaceState = SurfaceState.LOADING
    revision: int = 0
    sessions: tuple[SessionView, ...] = ()
    windows: tuple[WindowView, ...] = ()
    panes: tuple[PaneView, ...] = ()
    error_message: str | None = None


@dataclass(slots=True)
class PolarTmuxModel:
    """Holds only facts that the Linux UI is permitted to render."""

    _targets: dict[str, TargetView] = field(default_factory=dict)
    selected_target_id: str | None = None

    @property
    def targets(self) -> tuple[TargetView, ...]:
        return tuple(self._targets.values())

    def target(self, target_id: str) -> TargetView:
        try:
            return self._targets[target_id]
        except KeyError as error:
            raise ValueError(f"unknown execution target {target_id}") from error

    def replace_targets(self, targets: list[dict[str, Any]]) -> None:
        replacement: dict[str, TargetView] = {}
        for raw in targets:
            raw = _privacy_safe_object(raw, _TARGET_FIELDS, "execution target")
            target_id = _wire_string(raw.get("id"), "execution target id")
            if target_id in replacement:
                raise ValueError(f"duplicate execution target {target_id}")
            name = _wire_string(
                raw.get("display_name", target_id),
                "execution target display_name",
            )
            kind = _wire_string(raw.get("kind"), "execution target kind")
            if kind not in {"local", "ssh", "wsl"}:
                raise ValueError(f"unsupported execution target kind {kind}")
            previous = self._targets.get(target_id)
            if previous is None:
                replacement[target_id] = TargetView(target_id, name, kind)
            else:
                previous.name = name
                previous.kind = kind
                replacement[target_id] = previous
        self._targets = replacement
        if self.selected_target_id not in replacement:
            self.selected_target_id = next(iter(replacement), None)

    def select_target(self, target_id: str) -> None:
        self.target(target_id)
        self.selected_target_id = target_id

    def mark_loading(self, target_id: str) -> None:
        target = self.target(target_id)
        target.state = SurfaceState.LOADING
        target.error_message = None

    def mark_offline(self) -> None:
        for target in self._targets.values():
            target.state = SurfaceState.OFFLINE
            target.error_message = None

    def mark_error(self, target_id: str, message: str) -> None:
        if not isinstance(message, str) or not message:
            raise ValueError("error message must be non-empty")
        target = self.target(target_id)
        target.state = SurfaceState.ERROR
        target.error_message = message

    def apply_snapshot(
        self,
        target_id: str,
        revision: int,
        snapshot: dict[str, Any],
    ) -> None:
        target = self.target(target_id)
        revision = _wire_index(revision, "snapshot revision")
        snapshot = _privacy_safe_object(
            snapshot,
            _SNAPSHOT_FIELDS,
            "snapshot",
        )
        sessions_raw = snapshot.get("sessions", [])
        windows_raw = snapshot.get("windows", [])
        panes_raw = snapshot.get("panes", [])
        if not all(isinstance(items, list) for items in [sessions_raw, windows_raw, panes_raw]):
            raise ValueError("snapshot collections must be arrays")

        sessions = tuple(
            sorted(
                (SessionView.from_wire(raw) for raw in sessions_raw),
                key=lambda item: (item.name.casefold(), item.id),
            )
        )
        session_ids = {item.id for item in sessions}
        if len(session_ids) != len(sessions):
            raise ValueError("snapshot contains duplicate session ids")

        windows = tuple(
            sorted(
                (WindowView.from_wire(raw) for raw in windows_raw),
                key=lambda item: (item.session_id, item.index, item.id),
            )
        )
        window_ids: set[str] = set()
        for window in windows:
            if window.id in window_ids:
                raise ValueError("snapshot contains duplicate window ids")
            window_ids.add(window.id)
            if window.session_id not in session_ids:
                raise ValueError(
                    f"window {window.id} references unknown session {window.session_id}"
                )

        panes = tuple(
            sorted(
                (PaneView.from_wire(raw) for raw in panes_raw),
                key=lambda item: (item.window_id, item.index, item.id),
            )
        )
        pane_ids: set[str] = set()
        for pane in panes:
            if pane.id in pane_ids:
                raise ValueError("snapshot contains duplicate pane ids")
            pane_ids.add(pane.id)
            if pane.window_id not in window_ids:
                raise ValueError(
                    f"pane {pane.id} references unknown window {pane.window_id}"
                )

        target.revision = revision
        target.sessions = sessions
        target.windows = windows
        target.panes = panes
        target.error_message = None
        target.state = SurfaceState.POPULATED if sessions else SurfaceState.EMPTY

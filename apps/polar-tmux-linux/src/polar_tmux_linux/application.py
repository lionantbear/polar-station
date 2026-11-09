"""GTK4/libadwaita presentation over the authenticated local Rust Host."""

from __future__ import annotations

import sys
from pathlib import Path
from typing import Any

import gi

gi.require_version("Adw", "1")
gi.require_version("Gio", "2.0")
gi.require_version("Gtk", "4.0")
from gi.repository import Adw, Gio, GLib, Gtk

from .frontend_dispatch import FrontendDispatcher
from .ghostty import GhosttyController, TmuxAttachPlan
from .host_client import LocalHostClient, default_socket_path
from .model import PolarTmuxModel, SurfaceState, TargetView
from .protocol import error_result

APPLICATION_ID = "com.polarealm.PolarStation"
SUPPORTED_ACTIONS = [
    "activate",
    "open_session",
    "focus_pane",
    "refresh",
]
FRONTEND_CAPABILITIES = ["refresh_snapshot_v1"]


def apply_execution_targets_response(
    model: PolarTmuxModel,
    response: dict[str, Any],
) -> None:
    result = response.get("result")
    if not isinstance(result, dict):
        raise ValueError("Host response has no result")
    if result.get("type") == "error":
        error = result.get("error")
        message = error.get("message") if isinstance(error, dict) else None
        raise ValueError(message or "The Host rejected target discovery")
    if result.get("type") != "execution_targets":
        raise ValueError("Host response is not an execution target list")
    targets = result.get("targets")
    if not isinstance(targets, list):
        raise ValueError("Host target list is malformed")
    model.replace_targets(targets)


def apply_snapshot_response(
    model: PolarTmuxModel,
    response: dict[str, Any],
) -> None:
    target_id = response.get("execution_target")
    revision = response.get("revision")
    result = response.get("result")
    if not isinstance(target_id, str) or not isinstance(result, dict):
        raise ValueError("Host snapshot response is malformed")
    if result.get("type") == "error":
        error = result.get("error")
        message = error.get("message") if isinstance(error, dict) else None
        model.mark_error(target_id, message or "The Host rejected the snapshot request")
        return
    if (
        result.get("type") != "sanitized_snapshot"
        or not isinstance(result.get("snapshot"), dict)
    ):
        raise ValueError("Host response is not a privacy-safe snapshot")
    model.apply_snapshot(target_id, revision, result["snapshot"])


def apply_frontend_refresh_snapshot(
    model: PolarTmuxModel,
    target_id: str,
    revision: int,
    snapshot: dict[str, Any],
) -> None:
    """Apply the Host observation carried by a refresh before its ACK."""
    model.apply_snapshot(target_id, revision, snapshot)


def apply_frontend_completed_response(
    model: PolarTmuxModel,
    response: dict[str, Any],
) -> dict[str, Any]:
    target_id = response.get("execution_target")
    revision = response.get("revision")
    result = response.get("result")
    if not isinstance(target_id, str) or not isinstance(result, dict):
        raise ValueError("Host frontend response is malformed")
    if result.get("type") == "error":
        error = result.get("error")
        message = error.get("message") if isinstance(error, dict) else None
        raise ValueError(message or "The Host rejected the frontend action")
    completion = result.get("completion")
    snapshot = result.get("snapshot")
    if (
        result.get("type") != "frontend_completed"
        or not isinstance(completion, dict)
        or completion.get("status")
        not in {"applied", "frontend_failed", "tmux_applied_frontend_failed"}
        or not isinstance(snapshot, dict)
    ):
        raise ValueError("Host response is not a frontend completion")
    model.apply_snapshot(target_id, revision, snapshot)
    return completion


def build_smoke_model(state: SurfaceState) -> PolarTmuxModel:
    model = PolarTmuxModel()
    model.replace_targets(
        [
            {
                "id": "local",
                "display_name": "This computer",
                "kind": "local",
            }
        ]
    )
    if state is SurfaceState.POPULATED:
        model.apply_snapshot(
            "local",
            7,
            {
                "sessions": [
                    {
                        "id": "$0",
                        "name": "private-smoke",
                        "window_count": 1,
                        "attached": False,
                    }
                ],
                "windows": [
                    {
                        "id": "@0",
                        "session_id": "$0",
                        "index": 0,
                        "name": "shell",
                        "active": True,
                        "pane_count": 1,
                        "layout": "80x24,0,0,0",
                    }
                ],
                "panes": [
                    {
                        "id": "%0",
                        "window_id": "@0",
                        "index": 0,
                        "title": "Private fixture",
                        "active": True,
                        "dead": False,
                    }
                ],
            },
        )
    elif state is SurfaceState.EMPTY:
        model.apply_snapshot(
            "local",
            7,
            {"sessions": [], "windows": [], "panes": []},
        )
    elif state is SurfaceState.OFFLINE:
        model.mark_offline()
    elif state is SurfaceState.ERROR:
        model.mark_error("local", "Private smoke error")
    return model


def selection_for_pane(
    target: TargetView,
    pane_id: str,
) -> tuple[str, str] | None:
    pane = next((item for item in target.panes if item.id == pane_id), None)
    if pane is None:
        return None
    window = next(
        (item for item in target.windows if item.id == pane.window_id),
        None,
    )
    if window is None:
        return None
    return window.session_id, window.id


class PolarTmuxApplication(Adw.Application):
    """The native window; all tmux facts and actions come from ``LocalHostClient``."""

    def __init__(
        self,
        *,
        smoke_state: SurfaceState | None = None,
        smoke_quit_ms: int = 250,
    ) -> None:
        flags = Gio.ApplicationFlags.DEFAULT_FLAGS
        if smoke_state is not None:
            flags |= Gio.ApplicationFlags.NON_UNIQUE
        super().__init__(application_id=APPLICATION_ID, flags=flags)
        self._smoke_state = smoke_state
        self._smoke_quit_ms = smoke_quit_ms
        self._model = (
            build_smoke_model(smoke_state)
            if smoke_state is not None
            else PolarTmuxModel()
        )
        self._host: LocalHostClient | None = None
        self._dispatcher: FrontendDispatcher | None = None
        self._ghostty = GhosttyController()
        self._window: Adw.ApplicationWindow | None = None
        self._builder: Gtk.Builder | None = None
        self._target_ids: list[str] = []
        self._target_handler_id: int | None = None
        self._selected_session_id: str | None = None
        self._selected_window_id: str | None = None

    def do_activate(self) -> None:
        if self._window is None:
            self._build_window()
        assert self._window is not None
        if self._smoke_state is not None:
            self._render()
            self._window.connect("map", self._smoke_mapped)
            self._window.present()
            return
        self._window.present()
        if self._host is None:
            self._start_host_client()

    def do_shutdown(self) -> None:
        if self._host is not None:
            self._host.stop()
        Gio.Application.do_shutdown(self)

    def _build_window(self) -> None:
        resource = Path(__file__).with_name("resources") / "window.ui"
        self._builder = Gtk.Builder.new_from_file(str(resource))
        window = self._object("main_window", Adw.ApplicationWindow)
        window.set_application(self)
        self._window = window

        self._object("refresh_button", Gtk.Button).connect(
            "clicked", lambda _button: self._refresh_selected()
        )
        session_list = self._object("session_list", Gtk.ListBox)
        session_list.connect("row-selected", self._session_selected)
        session_list.connect("row-activated", self._session_activated)
        self._object("window_list", Gtk.ListBox).connect(
            "row-selected", self._window_selected
        )
        self._object("pane_list", Gtk.ListBox).connect(
            "row-activated", self._pane_activated
        )

        if self._smoke_state is None:
            self._model.replace_targets(
                [
                    {
                        "id": "local",
                        "display_name": "Connecting…",
                        "kind": "local",
                    }
                ]
            )
        self._render()

    def _start_host_client(self) -> None:
        self._host = LocalHostClient(
            socket_path=default_socket_path(),
            frontend_id=APPLICATION_ID,
            supported_actions=SUPPORTED_ACTIONS,
            capabilities=FRONTEND_CAPABILITIES,
            on_registered=self._registered,
            on_frontend_execute=self._frontend_execute,
            on_offline=self._offline,
            scheduler=lambda callback, *args: GLib.idle_add(callback, *args),
        )
        self._dispatcher = FrontendDispatcher(self._ghostty, self)
        self._host.start()

    def _registered(self, _frame: dict[str, Any]) -> None:
        assert self._host is not None
        self._set_status("Connected to Polar Tmux Host")
        self._host.request(
            {"type": "list_execution_targets"},
            self._targets_received,
            execution_target="local",
            expected_revision=None,
        )

    def _targets_received(self, response: dict[str, Any]) -> None:
        try:
            apply_execution_targets_response(self._model, response)
        except ValueError:
            self._model.replace_targets(
                [
                    {
                        "id": "local",
                        "display_name": "This computer",
                        "kind": "local",
                    }
                ]
            )
            self._model.mark_error("local", "Execution targets could not be loaded")
            self._render()
            return
        self._render()
        self._refresh_selected()

    def _refresh_selected(self, completion=None) -> None:
        target = self._selected_target()
        if target is None or self._host is None:
            if completion is not None:
                completion(
                    error_result(
                        "host_offline",
                        "The Polar Tmux Host is offline",
                        retryable=True,
                    )
                )
            return
        self._model.mark_loading(target.id)
        self._render_state(target)

        def receive(response: dict[str, Any]) -> None:
            failure = None
            try:
                apply_snapshot_response(self._model, response)
                if self._model.target(target.id).state is SurfaceState.ERROR:
                    failure = error_result(
                        "snapshot_failed",
                        self._model.target(target.id).error_message
                        or "The snapshot could not be loaded",
                        retryable=True,
                    )
            except ValueError:
                self._model.mark_error(target.id, "The Host returned an invalid snapshot")
                failure = error_result(
                    "invalid_host_response",
                    "The Host returned an invalid snapshot",
                )
            self._render()
            if completion is not None:
                completion(failure)

        self._host.request(
            {"type": "get_snapshot"},
            receive,
            execution_target=target.id,
            expected_revision=None,
        )

    def _frontend_execute(self, frame: dict[str, Any]) -> None:
        if self._dispatcher is None or self._host is None:
            return
        dispatch_id = frame["dispatch_id"]
        self._dispatcher.dispatch(
            frame["execution_target"],
            frame["revision"],
            frame["action"],
            frame["attach_plan"],
            frame["snapshot"],
            lambda result: self._host
            and self._host.acknowledge(dispatch_id, result),
        )

    def _offline(self, reason: str) -> None:
        self._model.mark_offline()
        self._set_status(reason)
        self._render()

    def _session_activated(self, _list_box: Gtk.ListBox, row: Gtk.ListBoxRow) -> None:
        session_id = self._row_id(row)
        target = self._selected_target()
        if not isinstance(session_id, str) or target is None or self._host is None:
            return

        def receive(result: dict[str, Any]) -> None:
            if result.get("type") != "attach_plan":
                self._set_status("The Host could not prepare this session")
                return
            try:
                self._ghostty.open_attach(TmuxAttachPlan.from_wire(result.get("plan")))
            except Exception:
                self._set_status("Ghostty could not open this session")
                return
            self._set_status("Opened the session in Ghostty")

        self._host.prepare_attach(target.id, session_id, target.revision, receive)

    def _session_selected(
        self,
        _list_box: Gtk.ListBox,
        row: Gtk.ListBoxRow | None,
    ) -> None:
        session_id = self._row_id(row)
        if session_id == self._selected_session_id:
            return
        self._selected_session_id = session_id
        self._selected_window_id = None
        target = self._selected_target()
        if target is not None and target.state is SurfaceState.POPULATED:
            self._render_windows(target)

    def _window_selected(
        self,
        _list_box: Gtk.ListBox,
        row: Gtk.ListBoxRow | None,
    ) -> None:
        self._selected_window_id = self._row_id(row)
        self._render_panes()

    def _pane_activated(self, _list_box: Gtk.ListBox, row: Gtk.ListBoxRow) -> None:
        pane_id = self._row_id(row)
        target = self._selected_target()
        if not isinstance(pane_id, str) or target is None or self._host is None:
            return
        self._host.request(
            {
                "type": "frontend",
                "action": {"type": "focus_pane", "pane": pane_id},
            },
            self._frontend_completed_received,
            execution_target=target.id,
            expected_revision=target.revision,
        )

    def _frontend_completed_received(self, response: dict[str, Any]) -> None:
        target_id = response.get("execution_target")
        try:
            completion = apply_frontend_completed_response(self._model, response)
        except ValueError as error:
            if isinstance(target_id, str) and target_id in {
                target.id for target in self._model.targets
            }:
                self._model.mark_error(target_id, str(error))
            self._set_status(str(error))
            self._render()
            return
        if completion["status"] == "applied":
            self._set_status("Frontend action applied")
        else:
            frontend_error = completion.get("frontend_error")
            message = (
                frontend_error.get("message")
                if isinstance(frontend_error, dict)
                else None
            )
            self._set_status(message or "The frontend action was only partially applied")
        self._render()

    def _target_changed(self, dropdown: Gtk.DropDown, _property) -> None:
        index = dropdown.get_selected()
        if index == Gtk.INVALID_LIST_POSITION or index >= len(self._target_ids):
            return
        self._model.select_target(self._target_ids[index])
        self._selected_session_id = None
        self._selected_window_id = None
        self._render()
        self._refresh_selected()

    def _render(self) -> None:
        if self._builder is None:
            return
        self._render_targets()
        target = self._selected_target()
        if target is not None:
            self._render_state(target)

    def _render_targets(self) -> None:
        dropdown = self._object("target_dropdown", Gtk.DropDown)
        if self._target_handler_id is not None:
            dropdown.disconnect(self._target_handler_id)
        self._target_ids = [target.id for target in self._model.targets]
        names = [
            f"{target.name} · {target.kind.upper()}" for target in self._model.targets
        ]
        dropdown.set_model(Gtk.StringList.new(names))
        if self._model.selected_target_id in self._target_ids:
            dropdown.set_selected(
                self._target_ids.index(self._model.selected_target_id)
            )
        else:
            dropdown.set_selected(Gtk.INVALID_LIST_POSITION)
        self._target_handler_id = dropdown.connect(
            "notify::selected", self._target_changed
        )

    def _render_state(self, target: TargetView) -> None:
        stack = self._object("state_stack", Gtk.Stack)
        stack.set_visible_child_name(target.state.value)
        if target.state is SurfaceState.ERROR:
            self._object("error_page", Adw.StatusPage).set_description(
                target.error_message or "The Host request failed"
            )
        if target.state is SurfaceState.POPULATED:
            self._render_topology(target)
        elif target.state is SurfaceState.EMPTY:
            self._clear_list(self._object("session_list", Gtk.ListBox))
            self._clear_list(self._object("window_list", Gtk.ListBox))
            self._clear_list(self._object("pane_list", Gtk.ListBox))

    def _render_topology(self, target: TargetView) -> None:
        session_list = self._object("session_list", Gtk.ListBox)
        self._clear_list(session_list)

        session_ids = {session.id for session in target.sessions}
        if self._selected_session_id not in session_ids:
            active_window = next((window for window in target.windows if window.active), None)
            self._selected_session_id = (
                active_window.session_id
                if active_window is not None
                else target.sessions[0].id
            )

        for session in target.sessions:
            suffix = " · attached" if session.attached else ""
            row = self._row(
                session.id,
                f"{session.name}{suffix}",
                f"{session.window_count} window(s) · {session.id}",
            )
            session_list.append(row)
            if session.id == self._selected_session_id:
                session_list.select_row(row)

        self._render_windows(target)

    def _render_windows(self, target: TargetView) -> None:
        window_list = self._object("window_list", Gtk.ListBox)
        self._clear_list(window_list)
        windows = [
            window
            for window in target.windows
            if window.session_id == self._selected_session_id
        ]
        window_ids = {window.id for window in windows}
        if self._selected_window_id not in window_ids:
            active_window = next((window for window in windows if window.active), None)
            self._selected_window_id = (
                active_window.id if active_window is not None else windows[0].id
                if windows
                else None
            )
        for window in windows:
            row = self._row(
                window.id,
                window.name,
                f"Window {window.index} · {window.pane_count} pane(s) · {window.id}",
            )
            window_list.append(row)
            if window.id == self._selected_window_id:
                window_list.select_row(row)
        self._render_panes()

    def _render_panes(self) -> None:
        pane_list = self._object("pane_list", Gtk.ListBox)
        self._clear_list(pane_list)
        target = self._selected_target()
        if target is None or target.state is not SurfaceState.POPULATED:
            return
        for pane in target.panes:
            if pane.window_id != self._selected_window_id:
                continue
            title = pane.title or "Untitled pane"
            status = "dead" if pane.dead else "active" if pane.active else "running"
            pane_list.append(
                self._row(
                    pane.id,
                    title,
                    f"Pane {pane.index} · {status} · {pane.id}",
                )
            )

    @staticmethod
    def _row(item_id: str, title: str, subtitle: str) -> Gtk.ListBoxRow:
        row = Gtk.ListBoxRow()
        row._polar_tmux_id = item_id
        labels = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=3)
        labels.set_margin_top(9)
        labels.set_margin_bottom(9)
        labels.set_margin_start(12)
        labels.set_margin_end(12)
        heading = Gtk.Label(label=title, xalign=0)
        heading.add_css_class("heading")
        detail = Gtk.Label(label=subtitle, xalign=0)
        detail.add_css_class("dim-label")
        labels.append(heading)
        labels.append(detail)
        row.set_child(labels)
        return row

    @staticmethod
    def _row_id(row: Gtk.ListBoxRow | None) -> str | None:
        item_id = getattr(row, "_polar_tmux_id", None)
        return item_id if isinstance(item_id, str) else None

    @staticmethod
    def _clear_list(list_box: Gtk.ListBox) -> None:
        child = list_box.get_first_child()
        while child is not None:
            following = child.get_next_sibling()
            list_box.remove(child)
            child = following

    def _selected_target(self) -> TargetView | None:
        if self._model.selected_target_id is None:
            return None
        return self._model.target(self._model.selected_target_id)

    def _set_status(self, text: str) -> None:
        if self._builder is not None:
            self._object("status_label", Gtk.Label).set_label(text)

    def _object(self, object_id: str, expected_type):
        assert self._builder is not None
        value = self._builder.get_object(object_id)
        if not isinstance(value, expected_type):
            raise RuntimeError(f"GTK resource is missing {object_id}")
        return value

    def _smoke_mapped(self, _window) -> None:
        state = self._smoke_state.value if self._smoke_state is not None else "unknown"
        print(f"POLAR_TMUX_GTK_SMOKE_MAPPED state={state}", flush=True)
        GLib.timeout_add(self._smoke_quit_ms, self.quit)

    # FrontendDispatcher presentation contract.
    def present(self) -> None:
        if self._window is not None:
            self._window.present()

    def focus_pane(self, target: str, pane: str) -> None:
        if target in {item.id for item in self._model.targets}:
            self._model.select_target(target)
        target_view = self._selected_target()
        if target_view is not None:
            selection = selection_for_pane(target_view, pane)
            if selection is not None:
                self._selected_session_id, self._selected_window_id = selection
        self._render()
        self.present()

    def refresh(
        self,
        target: str,
        revision: int,
        snapshot: dict[str, Any],
    ) -> None:
        if target in {item.id for item in self._model.targets}:
            self._model.select_target(target)
        apply_frontend_refresh_snapshot(
            self._model,
            target,
            revision,
            snapshot,
        )
        self._render()
        self.present()


def run_application(
    *,
    smoke_state: SurfaceState | None = None,
    smoke_quit_ms: int = 250,
) -> int:
    application = PolarTmuxApplication(
        smoke_state=smoke_state,
        smoke_quit_ms=smoke_quit_ms,
    )
    return application.run([sys.argv[0]])

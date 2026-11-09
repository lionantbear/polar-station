"""Allowlisted Host-to-Linux frontend action dispatch."""

from __future__ import annotations

from collections.abc import Callable
from typing import Any, Protocol

from .ghostty import GhosttyController, TmuxAttachPlan
from .protocol import accepted_result, error_result

FrontendResult = dict[str, Any]
Completion = Callable[[FrontendResult], None]


class Presentation(Protocol):
    def present(self) -> None: ...

    def focus_pane(self, target: str, pane: str) -> None: ...

    def refresh(self, target: str, revision: int, snapshot: dict[str, Any]) -> None: ...


class FrontendDispatcher:
    """Executes exactly the typed platform actions advertised at registration."""

    def __init__(
        self,
        ghostty: GhosttyController,
        presentation: Presentation,
    ) -> None:
        self._ghostty = ghostty
        self._presentation = presentation

    def dispatch(
        self,
        execution_target: str,
        _revision: int,
        action: dict[str, Any],
        attach_plan: dict[str, Any] | None,
        snapshot: dict[str, Any] | None,
        completion: Completion,
    ) -> None:
        completed = False

        def finish(result: FrontendResult) -> None:
            nonlocal completed
            if completed:
                return
            completed = True
            completion(result)

        action_type = action.get("type") if isinstance(action, dict) else None
        try:
            if action_type == "activate":
                self._reject_unexpected_plan(attach_plan)
                self._reject_unexpected_snapshot(snapshot)
                self._presentation.present()
                finish(accepted_result())
            elif action_type == "open_session":
                self._reject_unexpected_snapshot(snapshot)
                session = self._required_id(action, "session")
                self._open_attach(session, attach_plan, finish)
            elif action_type == "focus_pane":
                self._reject_unexpected_plan(attach_plan)
                self._reject_unexpected_snapshot(snapshot)
                pane = self._required_id(action, "pane")
                self._presentation.focus_pane(execution_target, pane)
                self._ghostty.activate()
                finish(accepted_result())
            elif action_type == "refresh":
                self._reject_unexpected_plan(attach_plan)
                if not isinstance(snapshot, dict):
                    finish(
                        error_result(
                            "invalid_refresh_snapshot",
                            "The Host did not provide the observed tmux snapshot",
                        )
                    )
                    return
                self._presentation.refresh(
                    execution_target,
                    _revision,
                    snapshot,
                )
                finish(accepted_result())
            else:
                self._reject_unexpected_plan(attach_plan)
                self._reject_unexpected_snapshot(snapshot)
                finish(
                    error_result(
                        "unsupported_frontend_action",
                        "The Linux frontend does not support this action",
                    )
                )
        except Exception:
            finish(
                error_result(
                    "frontend_action_failed",
                    "The Linux frontend could not complete the requested action",
                )
            )

    def _open_attach(
        self,
        session: str,
        raw_plan: dict[str, Any] | None,
        completion: Completion,
    ) -> None:
        if raw_plan is None:
            completion(
                error_result(
                    "invalid_attach_plan",
                    "The Host did not provide a terminal attachment",
                )
            )
            return
        try:
            plan = TmuxAttachPlan.from_wire(raw_plan)
            if plan.session_id != session:
                raise ValueError("attachment session does not match the action")
        except Exception:
            completion(
                error_result(
                    "invalid_attach_plan",
                    "The Host returned an invalid terminal attachment",
                )
            )
            return
        try:
            self._ghostty.open_attach(plan)
        except Exception:
            completion(
                error_result(
                    "ghostty_open_failed",
                    "Ghostty could not open the requested tmux session",
                )
            )
            return
        completion(accepted_result())

    @staticmethod
    def _reject_unexpected_plan(raw_plan: dict[str, Any] | None) -> None:
        if raw_plan is not None:
            raise ValueError("only open_session may carry an attachment plan")

    @staticmethod
    def _reject_unexpected_snapshot(snapshot: dict[str, Any] | None) -> None:
        if snapshot is not None:
            raise ValueError("only refresh may carry a snapshot")

    @staticmethod
    def _required_id(action: dict[str, Any], field_name: str) -> str:
        value = action.get(field_name)
        if (
            not isinstance(value, str)
            or not value
            or any(ord(char) < 0x20 for char in value)
        ):
            raise ValueError(f"{field_name} is invalid")
        return value

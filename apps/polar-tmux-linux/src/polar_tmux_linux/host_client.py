"""Long-lived, non-polling local Host IPC client for the GTK frontend."""

from __future__ import annotations

import os
import socket
import stat
import struct
import threading
import uuid
from collections.abc import Callable, Mapping, Sequence
from pathlib import Path
from typing import Any, Protocol

from .protocol import (
    FrameDecoder,
    ProtocolCodecError,
    encode_frame,
    error_result,
    frontend_ack_frame,
    is_valid_identifier,
    pong_frame,
    register_frame,
    request_frame,
)

Frame = dict[str, Any]
FrameCallback = Callable[[Frame], None]
_UNSAFE_LINUX_TOPOLOGY_RESULTS = {
    "snapshot",
    "sessions",
    "windows",
    "panes",
    "clients",
}


class HostConnectionError(RuntimeError):
    pass


class FrameTransport(Protocol):
    def send(self, frame: Frame) -> None: ...


class SocketFrameTransport:
    def __init__(self, connection: socket.socket) -> None:
        self._connection = connection
        self._write_lock = threading.Lock()

    def send(self, frame: Frame) -> None:
        encoded = encode_frame(frame)
        with self._write_lock:
            self._connection.sendall(encoded)


def default_socket_path() -> Path:
    override = os.environ.get("POLAR_TMUX_IPC_SOCKET")
    if override:
        return Path(override)
    runtime = os.environ.get("XDG_RUNTIME_DIR")
    if not runtime:
        raise RuntimeError(
            "XDG_RUNTIME_DIR is required when POLAR_TMUX_IPC_SOCKET is unset"
        )
    return Path(runtime) / "polar-tmux" / "host.sock"


class LocalHostClient:
    """One registered local frontend connection with correlated requests."""

    def __init__(
        self,
        *,
        socket_path: Path,
        frontend_id: str,
        supported_actions: Sequence[str],
        capabilities: Sequence[str],
        on_registered: FrameCallback,
        on_frontend_execute: FrameCallback,
        on_offline: Callable[[str], None],
        scheduler: Callable[..., Any] | None = None,
    ) -> None:
        self.socket_path = Path(socket_path)
        self.frontend_id = frontend_id
        self.supported_actions = list(supported_actions)
        self.capabilities = list(capabilities)
        self._on_registered = on_registered
        self._on_frontend_execute = on_frontend_execute
        self._on_offline = on_offline
        self._scheduler = scheduler or (lambda callback, *args: callback(*args))
        self._transport: FrameTransport | None = None
        self._connection: socket.socket | None = None
        self._pending: dict[
            str,
            tuple[str, int | None, FrameCallback],
        ] = {}
        self._active_dispatches: set[str] = set()
        self._state_lock = threading.Lock()
        self._stop = threading.Event()
        self._thread: threading.Thread | None = None

    def start(self) -> None:
        if self._thread is not None and self._thread.is_alive():
            return
        self._stop.clear()
        self._thread = threading.Thread(
            target=self._connection_loop,
            name="polar-tmux-linux-host-ipc",
            daemon=True,
        )
        self._thread.start()

    def stop(self) -> None:
        self._stop.set()
        with self._state_lock:
            connection = self._connection
        if connection is not None:
            try:
                connection.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
        if self._thread is not None:
            self._thread.join(timeout=2)

    def attach_transport_for_test(self, transport: FrameTransport) -> None:
        with self._state_lock:
            self._transport = transport

    def send_registration_for_test(self) -> None:
        self._send(
            register_frame(
                self.frontend_id,
                self.supported_actions,
                self.capabilities,
            )
        )

    def request(
        self,
        command: Mapping[str, Any],
        callback: FrameCallback,
        *,
        request_id: str | None = None,
        execution_target: str,
        expected_revision: int | None,
    ) -> str:
        request_id = request_id or str(uuid.uuid4())
        with self._state_lock:
            if request_id in self._pending:
                raise ValueError(f"request_id {request_id} is already pending")
            self._pending[request_id] = (
                execution_target,
                expected_revision,
                callback,
            )
        try:
            self._send(
                request_frame(
                    request_id,
                    command,
                    execution_target=execution_target,
                    expected_revision=expected_revision,
                )
            )
        except Exception:
            with self._state_lock:
                self._pending.pop(request_id, None)
            raise
        return request_id

    def prepare_attach(
        self,
        target: str,
        session: str,
        revision: int,
        callback: Callable[[Frame], None],
    ) -> None:
        def receive(response: Frame) -> None:
            result = response.get("result")
            if isinstance(result, dict):
                callback(result)
            else:
                callback(
                    error_result(
                        "invalid_host_response",
                        "The Host returned an invalid attachment response",
                    )
                )

        self.request(
            {"type": "prepare_attach", "session": session},
            receive,
            execution_target=target,
            expected_revision=revision,
        )

    def acknowledge(self, dispatch_id: str, result: Mapping[str, Any]) -> bool:
        with self._state_lock:
            if dispatch_id not in self._active_dispatches:
                return False
            self._active_dispatches.remove(dispatch_id)
        self._send(frontend_ack_frame(dispatch_id, result))
        return True

    def handle_host_frame(self, frame: Frame) -> None:
        frame_type = frame.get("type")
        if frame_type == "registered":
            protocol_version = frame.get("protocol_version")
            if (
                not isinstance(protocol_version, int)
                or isinstance(protocol_version, bool)
                or protocol_version != 1
                or not is_valid_identifier(frame.get("registration_id"))
                or not is_valid_identifier(frame.get("host_id"))
            ):
                self._protocol_failure("Host registered frame is malformed")
                return
            self._schedule(self._on_registered, frame)
            return
        if frame_type == "response":
            response = frame.get("response")
            if not self._valid_host_envelope(response):
                self._protocol_failure("Host response frame is malformed")
                return
            request_id = response.get("request_id")
            with self._state_lock:
                pending = self._pending.get(request_id)
                if pending is not None and pending[0] == response.get(
                    "execution_target"
                ):
                    self._pending.pop(request_id, None)
            if pending is None:
                self._protocol_failure(
                    "Host response does not match an in-flight request"
                )
                return
            if pending[0] != response.get("execution_target"):
                self._protocol_failure(
                    "Host response execution target does not match its request"
                )
                return
            callback = pending[2]
            self._schedule(callback, response)
            return
        if frame_type == "frontend_execute":
            dispatch_id = frame.get("dispatch_id")
            action = frame.get("action")
            target = frame.get("execution_target")
            revision = frame.get("revision")
            attach_plan = frame.get("attach_plan")
            snapshot = frame.get("snapshot")
            action_type = action.get("type") if isinstance(action, dict) else None
            if (
                not isinstance(dispatch_id, str)
                or not dispatch_id
                or not isinstance(action, dict)
                or not isinstance(action_type, str)
                or not isinstance(target, str)
                or isinstance(revision, bool)
                or not isinstance(revision, int)
                or revision < 0
                or "attach_plan" not in frame
                or "snapshot" not in frame
                or (
                    action_type == "open_session"
                    and not isinstance(attach_plan, dict)
                )
                or (
                    action_type != "open_session"
                    and attach_plan is not None
                )
                or (
                    action_type == "refresh"
                    and not self._valid_refresh_snapshot(snapshot)
                )
                or (
                    action_type != "refresh"
                    and snapshot is not None
                )
            ):
                self._protocol_failure("Host frontend_execute frame is malformed")
                return
            with self._state_lock:
                if dispatch_id in self._active_dispatches:
                    return
                self._active_dispatches.add(dispatch_id)
            self._schedule(self._on_frontend_execute, frame)
            return
        if frame_type == "ping":
            try:
                self._send(pong_frame(frame.get("nonce")))
            except (TypeError, ValueError):
                self._protocol_failure("Host ping frame has an invalid nonce")
            return
        self._protocol_failure(f"unsupported Host frame type {frame_type!r}")

    def _send(self, frame: Frame) -> None:
        with self._state_lock:
            transport = self._transport
        if transport is None:
            raise HostConnectionError("Polar Tmux Host is offline")
        transport.send(frame)

    def _schedule(self, callback: Callable[..., Any], *args: Any) -> None:
        self._scheduler(callback, *args)

    def _protocol_failure(self, message: str) -> None:
        self._schedule(self._on_offline, f"Local IPC protocol error: {message}")
        with self._state_lock:
            connection = self._connection
        if connection is not None:
            try:
                connection.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass

    @staticmethod
    def _valid_host_envelope(response: Any) -> bool:
        if not isinstance(response, dict):
            return False
        revision = response.get("revision")
        result = response.get("result")
        result_type = result.get("type") if isinstance(result, dict) else None
        protocol_version = response.get("protocol_version")
        return (
            isinstance(protocol_version, int)
            and not isinstance(protocol_version, bool)
            and protocol_version == 1
            and is_valid_identifier(response.get("request_id"))
            and is_valid_identifier(response.get("execution_target"))
            and isinstance(revision, int)
            and not isinstance(revision, bool)
            and 0 <= revision <= (2**64 - 1)
            and isinstance(result, dict)
            and isinstance(result_type, str)
            and result_type not in _UNSAFE_LINUX_TOPOLOGY_RESULTS
        )

    @staticmethod
    def _valid_refresh_snapshot(snapshot: Any) -> bool:
        return (
            isinstance(snapshot, dict)
            and set(snapshot) == {"sessions", "windows", "panes"}
            and all(
                isinstance(snapshot[field], list)
                for field in ("sessions", "windows", "panes")
            )
        )

    def _connection_loop(self) -> None:
        delay_seconds = 0.25
        while not self._stop.is_set():
            connection: socket.socket | None = None
            try:
                connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                connection.connect(str(self.socket_path))
                self._validate_socket_and_peer(connection)
                transport = SocketFrameTransport(connection)
                with self._state_lock:
                    self._connection = connection
                    self._transport = transport
                transport.send(
                    register_frame(
                        self.frontend_id,
                        self.supported_actions,
                        self.capabilities,
                    )
                )
                delay_seconds = 0.25
                self._read_connection(connection)
                reason = "Polar Tmux Host disconnected"
            except (OSError, HostConnectionError, ProtocolCodecError) as error:
                reason = f"Polar Tmux Host is offline: {error}"
            finally:
                self._disconnect()
                if connection is not None:
                    try:
                        connection.close()
                    except OSError:
                        pass
            if self._stop.is_set():
                break
            self._schedule(self._on_offline, reason)
            self._stop.wait(delay_seconds)
            delay_seconds = min(delay_seconds * 2, 5.0)

    def _read_connection(self, connection: socket.socket) -> None:
        decoder = FrameDecoder()
        while not self._stop.is_set():
            chunk = connection.recv(64 * 1024)
            if not chunk:
                return
            for frame in decoder.feed(chunk):
                self.handle_host_frame(frame)

    def _validate_socket_and_peer(self, connection: socket.socket) -> None:
        parent = self.socket_path.parent
        try:
            parent_info = parent.stat()
            socket_info = self.socket_path.stat()
        except OSError as error:
            raise HostConnectionError("Host socket metadata is unavailable") from error
        if parent_info.st_uid != os.getuid() or stat.S_IMODE(parent_info.st_mode) != 0o700:
            raise HostConnectionError("Host socket directory is not private to this user")
        if (
            socket_info.st_uid != os.getuid()
            or not stat.S_ISSOCK(socket_info.st_mode)
            or stat.S_IMODE(socket_info.st_mode) != 0o600
        ):
            raise HostConnectionError("Host socket is not a private user socket")
        peer = connection.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12)
        _peer_pid, peer_uid, _peer_gid = struct.unpack("3i", peer)
        if peer_uid != os.getuid():
            raise HostConnectionError("Host socket peer belongs to another user")

    def _disconnect(self) -> None:
        with self._state_lock:
            connection = self._connection
            self._connection = None
            self._transport = None
            pending = self._pending
            self._pending = {}
            self._active_dispatches.clear()
        if connection is not None:
            try:
                connection.close()
            except OSError:
                pass
        for request_id, (execution_target, expected_revision, callback) in pending.items():
            self._schedule(
                callback,
                {
                    "protocol_version": 1,
                    "request_id": request_id,
                    "execution_target": execution_target,
                    "revision": expected_revision or 0,
                    "result": error_result(
                        "host_offline",
                        "The Polar Tmux Host disconnected",
                        retryable=True,
                    ),
                },
            )

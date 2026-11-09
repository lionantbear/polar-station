"""Bounded framing and fixed local-IPC wire helpers.

The Rust Host owns tmux behavior.  This module only implements the local
frontend transport contract: a four-byte big-endian length followed by one
UTF-8 JSON object.
"""

from __future__ import annotations

import json
import struct
import unicodedata
from collections.abc import Mapping, Sequence
from typing import Any

PROTOCOL_VERSION = 1
MAX_FRAME_BYTES = 1024 * 1024
_HEADER_BYTES = 4


class ProtocolCodecError(ValueError):
    """The peer supplied a frame that is not valid local-IPC protocol data."""


def _reject_json_constant(value: str) -> None:
    raise ProtocolCodecError(f"frame contains non-standard JSON constant {value}")


def encode_frame(message: Any) -> bytes:
    """Encode one bounded JSON value with the local IPC length prefix."""

    try:
        payload = json.dumps(
            message,
            ensure_ascii=False,
            allow_nan=False,
            separators=(",", ":"),
        ).encode("utf-8")
    except (TypeError, ValueError) as error:
        raise ProtocolCodecError(f"frame is not valid JSON: {error}") from error
    if len(payload) > MAX_FRAME_BYTES:
        raise ProtocolCodecError(
            f"frame exceeds the {MAX_FRAME_BYTES}-byte maximum payload"
        )
    return struct.pack(">I", len(payload)) + payload


class FrameDecoder:
    """Incrementally decode bounded frames from a Unix stream socket."""

    def __init__(self) -> None:
        self._buffer = bytearray()

    def feed(self, chunk: bytes | bytearray | memoryview) -> list[dict[str, Any]]:
        if not isinstance(chunk, (bytes, bytearray, memoryview)):
            raise TypeError("frame chunk must be bytes-like")
        self._buffer.extend(chunk)
        decoded: list[dict[str, Any]] = []

        while len(self._buffer) >= _HEADER_BYTES:
            payload_size = struct.unpack(">I", self._buffer[:_HEADER_BYTES])[0]
            if payload_size > MAX_FRAME_BYTES:
                self._buffer.clear()
                raise ProtocolCodecError(
                    f"frame exceeds the {MAX_FRAME_BYTES}-byte maximum payload"
                )
            frame_size = _HEADER_BYTES + payload_size
            if len(self._buffer) < frame_size:
                break
            payload = bytes(self._buffer[_HEADER_BYTES:frame_size])
            del self._buffer[:frame_size]
            try:
                text = payload.decode("utf-8")
            except UnicodeDecodeError as error:
                raise ProtocolCodecError("frame payload is not valid UTF-8") from error
            try:
                value = json.loads(text, parse_constant=_reject_json_constant)
            except ProtocolCodecError:
                raise
            except (json.JSONDecodeError, TypeError, ValueError) as error:
                raise ProtocolCodecError(f"frame payload is not valid JSON: {error}") from error
            if not isinstance(value, dict):
                raise ProtocolCodecError("frame payload must be a JSON object")
            decoded.append(value)

        return decoded


def is_valid_identifier(value: Any) -> bool:
    return (
        isinstance(value, str)
        and bool(value)
        and len(value.encode("utf-8")) <= 256
        and value == value.strip()
        and not any(unicodedata.category(char) == "Cc" for char in value)
    )


def _clean_identifier(value: str, field: str) -> str:
    if not is_valid_identifier(value):
        raise ValueError(f"{field} must be a non-empty string without control characters")
    return value


def register_frame(
    frontend_id: str,
    supported_actions: Sequence[str],
    capabilities: Sequence[str] = (),
) -> dict[str, Any]:
    frontend_id = _clean_identifier(frontend_id, "frontend_id")
    actions = [_clean_identifier(action, "supported action") for action in supported_actions]
    capability_names = [
        _clean_identifier(capability, "frontend capability")
        for capability in capabilities
    ]
    if len(actions) != len(set(actions)):
        raise ValueError("supported_actions must not contain duplicates")
    if len(capability_names) != len(set(capability_names)):
        raise ValueError("capabilities must not contain duplicates")
    return {
        "type": "register",
        "frontend_id": frontend_id,
        "platform": "linux",
        "supported_actions": actions,
        "capabilities": capability_names,
    }


def request_frame(
    request_id: str,
    command: Mapping[str, Any],
    *,
    execution_target: str,
    expected_revision: int | None,
) -> dict[str, Any]:
    request_id = _clean_identifier(request_id, "request_id")
    execution_target = _clean_identifier(execution_target, "execution_target")
    if (
        expected_revision is not None
        and (
            isinstance(expected_revision, bool)
            or not isinstance(expected_revision, int)
            or expected_revision < 0
        )
    ):
        raise ValueError("expected_revision must be a non-negative integer or null")
    if not isinstance(command, Mapping) or not isinstance(command.get("type"), str):
        raise ValueError("command must be a typed JSON object")
    return {
        "type": "request",
        "request": {
            "protocol_version": PROTOCOL_VERSION,
            "request_id": request_id,
            "execution_target": execution_target,
            "expected_revision": expected_revision,
            "command": dict(command),
        },
    }


def frontend_ack_frame(dispatch_id: str, result: Mapping[str, Any]) -> dict[str, Any]:
    dispatch_id = _clean_identifier(dispatch_id, "dispatch_id")
    if not isinstance(result, Mapping) or result.get("type") not in {"accepted", "error"}:
        raise ValueError("frontend acknowledgement result must be accepted or error")
    return {
        "type": "frontend_ack",
        "dispatch_id": dispatch_id,
        "result": dict(result),
    }


def pong_frame(nonce: int) -> dict[str, Any]:
    if isinstance(nonce, bool) or not isinstance(nonce, int) or nonce < 0:
        raise ValueError("heartbeat nonce must be a non-negative integer")
    return {"type": "pong", "nonce": nonce}


def accepted_result() -> dict[str, str]:
    return {"type": "accepted"}


def error_result(
    code: str,
    message: str,
    *,
    retryable: bool = False,
) -> dict[str, Any]:
    code = _clean_identifier(code, "error code")
    if not isinstance(message, str) or not message:
        raise ValueError("error message must be non-empty")
    return {
        "type": "error",
        "error": {
            "code": code,
            "message": message,
            "retryable": bool(retryable),
        },
    }

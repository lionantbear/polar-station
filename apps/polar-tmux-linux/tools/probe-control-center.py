#!/usr/bin/env python3
"""Read one installed Host projection and exercise the real Zig decoder.

No target, session, or Workspace mutation is submitted. Private projection
contents are passed only to the local decoder's stdin and are never logged.
"""
import argparse
import json
import os
import socket
import struct
import subprocess
import time


def probe(socket_path, decoder):
    with socket.socket(socket.AF_UNIX) as connection:
        connection.settimeout(10)
        connection.connect(socket_path)

        def send(frame):
            payload = json.dumps(frame).encode()
            connection.sendall(struct.pack("!I", len(payload)) + payload)

        def exact(length):
            parts = bytearray()
            while len(parts) < length:
                chunk = connection.recv(length - len(parts))
                if not chunk:
                    raise EOFError("Host disconnected before completing its frame")
                parts.extend(chunk)
            return parts

        def receive():
            length = struct.unpack("!I", exact(4))[0]
            if not 0 < length <= 16 * 1024 * 1024:
                raise ValueError("Host frame exceeds diagnostic bound")
            return json.loads(exact(length))

        send(dict(type="register", frontend_id="recovery-diagnostic",
                  platform="linux", supported_actions=["refresh"], capabilities=[]))
        if receive().get("type") != "registered":
            raise ValueError("Host did not acknowledge registration")
        send(dict(type="application_request", request=dict(
            protocol_version=1, api="control_center_v1", request_id="recovery-read",
            frontend_session_id="recovery-diagnostic", expected_control_revision=None,
            command=dict(type="get_projection"))))
        started = time.monotonic()
        while time.monotonic() - started < 10:
            frame = receive()
            if frame["type"] == "ping":
                send(dict(type="pong", nonce=frame["nonce"]))
                continue
            if frame["type"] != "application_response":
                continue
            result = subprocess.run([decoder], input=json.dumps(frame).encode(),
                                    capture_output=True, timeout=10, check=False)
            # Only the decoder status and error name are public evidence.
            error = next((line for line in result.stderr.decode(errors="replace").splitlines()
                          if line.startswith("error:")), None)
            return dict(host_response_ms=round((time.monotonic() - started) * 1000),
                        decoder_exit=result.returncode, decoder_error=error)
        raise TimeoutError("Host did not return a Control Center response")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--socket", default=os.path.join(
        os.environ.get("XDG_RUNTIME_DIR", "/nonexistent"), "polar-tmux/host.sock"))
    parser.add_argument("--decoder", required=True)
    args = parser.parse_args()
    evidence = probe(args.socket, args.decoder)
    print(json.dumps(evidence))
    raise SystemExit(evidence["decoder_exit"])

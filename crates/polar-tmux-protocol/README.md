# polar-tmux-protocol

Versioned JSON DTOs and cryptographic wire primitives shared by Polar Tmux
Hosts and native clients. The crate uses tmux-native session, window, and pane
concepts and contains no process, socket, discovery, or secure-store
implementation.

## Protocol v1

Every request names an `execution_target` and the snapshot revision on which a
mutation is based. A response repeats the target and carries the resulting
revision. `expected_revision` is nullable for reads; every mutation must supply
it. `prepare_attach` is a local-desktop-only request and returns a
`TmuxAttachPlan` containing a program and argv, never shell text.
Through Phase 5 the executable allowlist is exactly `tmux` and `ssh`; WSL
attachment remains deferred to the Windows phase. An SSH attach plan has
exactly `-tt`, one Host-configured destination, and one
Host-produced remote tmux command; arbitrary SSH options and client-provided
destinations are invalid.

The local frontend socket uses a four-byte big-endian JSON length followed by
at most 1 MiB of UTF-8 JSON. Its client frames are `register`, `request`,
`frontend_ack`, and `pong`; Host frames are `registered`, `response`,
`frontend_execute`, and `ping`. `frontend_ack.result` accepts only the
`accepted` and `error` Host result variants.

Remote pairing uses text JSON only for the handshake. The preserved legacy
client proof is compatibility-fixture-only:

```text
HMAC-SHA256(
  root_secret,
  "leotmux-companion-v1|<challenge>|<nonce_base64>|<device>"
)
```

Production Host/client proofs use:

```text
HMAC-SHA256(
  root_secret,
  "polar-tmux-pairing-v1|1|<host_id>|<device_id>|<challenge_id>|<client_nonce_base64>|<server_nonce_base64>|<direction>"
)
```

The three direction values are `host_challenge`, `client_proof`, and
`host_accepted`; a proof from one handshake step cannot be replayed as another.
After mutual proof, both peers derive independent ChaCha20-Poly1305 keys with
HKDF-SHA256:

```text
salt = SHA256(client_nonce_bytes || server_nonce_bytes)
context = "polar-tmux-session-v1|1|<host_id>|<device_id>|<challenge_id>|<client_nonce_base64>|<server_nonce_base64>"
info c2h = context || "|client_to_host"
info h2c = context || "|host_to_client"
```

An encrypted binary application frame is:

```text
"PTM1" || direction:u8 || sequence:u64be || ciphertext_and_tag
```

Direction is `1` for client-to-Host and `2` for Host-to-client. The 13-byte
header is authenticated data. The ChaCha20 nonce is three zero bytes,
direction, then the same big-endian sequence. Readers require the exact next
sequence, starting at zero; replay, reordering, gaps, wrong direction, bad
authentication, or cleartext application JSON is fatal.

The canonical cross-language vectors live in
`fixtures/v1/phase4_v1.json`. Mobile DTOs deliberately omit client, PID, TTY,
command, working-path, raw-error, terminal-window, and secret fields. The
mobile command policy allows sanitized reads, constrained session creation,
and the typed `activate`, `open_session`, `focus_pane`, and `refresh` frontend
actions only.

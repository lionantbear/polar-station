# polar-tmux

Standalone development CLI and per-host runtime for Polar Tmux.

```bash
polar-tmux sessions
polar-tmux --ssh build-host sessions
polar-tmux host --listen 0.0.0.0:47820 --advertise
polar-tmux pairing-payload
```

The Host exposes same-user length-prefixed local IPC and a production
Companion WebSocket. Companion pairing uses a persistent secure-store Host
identity, mutual HMAC proof, HKDF-derived directional keys, and
ChaCha20-Poly1305 application frames. There is no bearer-token route.

Non-loopback use requires an explicitly reachable listener. `--advertise`
publishes `_polar-tmux._tcp` only after the TCP listener, Host singleton, and
local IPC boundary are established. SSH targets come only from an
owner-controlled `--target-config`; network clients select stable target IDs
and cannot supply connection material.

This binary remains a development and migration artifact pending human review
against the repository-wide single-`polar` packaging convergence rule.

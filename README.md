# Polar Station

Public source snapshot of Polar Station, the native macOS/Linux agent workstation.

This tree is exported from the PolarInfra monorepo. Its initial public Git
history intentionally does not include earlier private development history.
Future public releases should retain their normal public Git history.

The iPhone Companion is currently **Unavailable**. The public source profile disables
the pairing screen. Supplied desktop launchers bind Host to loopback without
mDNS advertisement; native build and installation verification remains pending.

Ghostty upstream is fetched at the pinned revision in
`apps/polar-station-macos/ghostty-overlay/upstream.lock.json`; the Polar patch
is applied separately. Consult `THIRD_PARTY_NOTICES.md` and
`SOURCE_DISTRIBUTION.md` for source-only versus binary-distribution obligations.
The root license covers first-party code; upstream licenses remain unchanged.

This source snapshot is not a tested binary release. Consult the public build
README once the portable build and installation gates are complete.

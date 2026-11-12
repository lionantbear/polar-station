# bip0032 0.2.2

Source: crates.io `bip0032-0.2.2.crate`, SHA-256
`9552cc87e70ae1cb0d7af0c9710a3341b92950117ece2fd5b99eb3453a92ed0a`.
License: Apache-2.0 (included LICENSE).

Polar patch: SLIP-0010 master derivation uses Zeroizing for its internal seed
copy and retry buffer. Retry replaces the old Zeroizing Vec, wiping before any
capacity can be reused/reallocated. No derivation rule, domain, serialization
or cryptographic backend was changed. The split HMAC output is wiped after copying its halves. Existing upstream
metadata Drop wipes the chain code and curve backends wipe private keys. The registry archive declares Apache-2.0
but omits the full license text; LICENSE includes the standard Apache-2.0 text.

This crate's curve dependencies are isolated from Polar identity's existing
Ed25519 implementation. Only 32-byte results cross the adapter. Importing this
library does not change Polar root generation or apply BIP-0039 PBKDF2.

Modified production files: `src/xkey/core/mod.rs` and `src/xkey/slip10.rs`.
Both carry prominent per-file Polar modification notices. The added notice
comments do not alter the derivation implementation.

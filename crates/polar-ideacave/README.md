# polar-ideacave

Pure I-tree descriptions and filesystem metadata lowering. This development
installment implements parse/encode/decode, exact node references, facts extraction
and I-tree-owned directory layout. It reuses existing polar-cas content identities,
canonical JSON and ProjectionTree encoding.

There is no Vault, network, host-path access, CAS writer, executor daemon, persistent
index or E-tree runtime in this crate. A locally computed digest is not admission;
a returned metadata tree must not bypass the runtime's immutable-source gate.
Source paths and plaintext credentials are deliberately not accepted as inputs.

Unknown business data can be retained, but unsupported definitions required for
lowering fail explicitly. See the subproject's ideacave-p1-development record for
open runtime gates and the NOT_RUN Rust test status of this delivery.

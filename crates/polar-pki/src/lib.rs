//! `polar-pki` — Node Transport CA and per-plane leaf certificate management.
//!
//! # Separation of concerns
//!
//! Each Polar node has two distinct cryptographic identities:
//!
//! 1. **Node Identity Key** — Ed25519 signing key, managed by `polar-identity`.
//!    This key signs protocol messages and is the canonical identifier for trust
//!    relationships.  It is NOT used for TLS.
//!
//! 2. **Node Transport CA** — ECDSA P-256 X.509 certificate authority, managed
//!    here.  This CA signs per-plane leaf certificates used exclusively for
//!    transport-layer TLS and mTLS.  It has no role in application-layer trust.
//!
//! mTLS proves transport-side admission only.  `TrustRecord` (in `polar-trust`)
//! proves application-layer trust.  `ControlAuthorityRecord` proves management
//! authorization.  These three layers are independent.
//!
//! # CA hierarchy
//!
//! ```text
//! Root: Node Transport CA  (self-signed, P-256, 10-year validity)
//!   └─ Bootstrap leaf cert    (server-only TLS, 7 days)
//!   └─ Member leaf cert       (mTLS, ServerAuth + ClientAuth, 90 days)
//!   └─ Realm leaf cert        (mTLS, ServerAuth + ClientAuth, 90 days)
//!   └─ ControlBus leaf cert   (mTLS + ControlBus EKU, 1 day)
//! ```
//!
//! Each leaf cert embeds `URI:polar://node/<node_id_hex>` as a Subject
//! Alternative Name so the mTLS handshake can report the peer's NodeId to the
//! application layer.
//!
//! # Vault storage
//!
//! This crate is pure PKI logic.  Vault persistence (load or generate) is the
//! responsibility of `polar-serve`, which holds the long-lived node resources.
//! `NodeTransportCa` and `IssuedCert` both implement `serde::{Serialize,
//! Deserialize}` for vault round-trips.

use rcgen::string::Ia5String;
use rcgen::{
    BasicConstraints, CertificateParams, CertificateSigningRequestParams, CertifiedIssuer, DnType,
    ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair, SanType,
};
use rustls_pki_types::CertificateSigningRequestDer;
use time::{Duration, OffsetDateTime};

const CERT_NOT_BEFORE_CLOCK_SKEW: Duration = Duration::minutes(5);

/// Encode DER bytes as a PEM string with the given label.
fn der_to_pem(label: &str, der: &[u8]) -> String {
    pem::encode(&pem::Pem::new(label, der))
}

// ── OID constants ────────────────────────────────────────────────────────────

/// OID for the Polar control-bus extended key usage.
///
/// Arc: `1.3.6.1.4.1.62219.1.1`
/// (`62219` is the Polar Infrastructure placeholder private-enterprise number.)
///
/// This OID is embedded in ControlBus leaf certs to allow the admission
/// verifier to distinguish control-bus connections from other mTLS connections
/// using the same transport.
pub const CONTROL_BUS_EKU_OID: &str = "1.3.6.1.4.1.62219.1.1";

/// OID arcs for [`CONTROL_BUS_EKU_OID`], for use with rcgen's
/// `ExtendedKeyUsagePurpose::Other`.
pub const CONTROL_BUS_EKU_OID_ARCS: &[u64] = &[1, 3, 6, 1, 4, 1, 62219, 1, 1];

// ── PlaneCertRole ─────────────────────────────────────────────────────────────

/// Which transport plane a leaf certificate is issued for.
///
/// Each role has its own validity period and EKU profile.  The variants are
/// ordered from least-privileged (Bootstrap) to most-privileged (ControlBus).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PlaneCertRole {
    /// Bootstrap plane — server-only TLS for initial peer discovery.
    /// Short validity (7 days) to limit exposure.
    Bootstrap,
    /// Member plane — mTLS for trusted-peer data transport (90 days).
    Member,
    /// Realm plane — mTLS for realm router connections (90 days).
    Realm,
    /// Control bus — mTLS + ControlBus EKU for remote management (1 day).
    ControlBus,
}

impl PlaneCertRole {
    /// Certificate validity in days.
    pub fn validity_days(self) -> i64 {
        match self {
            PlaneCertRole::Bootstrap => 7,
            PlaneCertRole::Member => 90,
            PlaneCertRole::Realm => 90,
            PlaneCertRole::ControlBus => 1,
        }
    }

    /// Whether this plane requires mutual TLS (client certificate).
    pub fn requires_mtls(self) -> bool {
        matches!(
            self,
            PlaneCertRole::Member | PlaneCertRole::Realm | PlaneCertRole::ControlBus
        )
    }

    fn eku_purposes(self) -> Vec<ExtendedKeyUsagePurpose> {
        match self {
            PlaneCertRole::Bootstrap => {
                vec![ExtendedKeyUsagePurpose::ServerAuth]
            }
            PlaneCertRole::Member | PlaneCertRole::Realm => {
                vec![
                    ExtendedKeyUsagePurpose::ServerAuth,
                    ExtendedKeyUsagePurpose::ClientAuth,
                ]
            }
            PlaneCertRole::ControlBus => {
                vec![
                    ExtendedKeyUsagePurpose::ServerAuth,
                    ExtendedKeyUsagePurpose::ClientAuth,
                    ExtendedKeyUsagePurpose::Other(CONTROL_BUS_EKU_OID_ARCS.to_vec()),
                ]
            }
        }
    }
}

impl std::fmt::Display for PlaneCertRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bootstrap => write!(f, "bootstrap"),
            Self::Member => write!(f, "member"),
            Self::Realm => write!(f, "realm"),
            Self::ControlBus => write!(f, "control-bus"),
        }
    }
}

// ── NodeTransportCa ───────────────────────────────────────────────────────────

/// Per-node X.509 Transport Certificate Authority.
///
/// Generated once at first boot and persisted in the node vault.  The CA cert
/// DER is exchanged with peers during bootstrap so peers can verify the node's
/// plane leaf certs via mTLS.
///
/// ## Reconstruction for signing
///
/// When issuing a leaf cert the CA is reconstructed from the stored
/// `node_id_hex` and `key_pem` using [`Issuer::new`].  The reconstructed CA
/// uses the same Distinguished Name and the same key pair, so leaf certs
/// signed by it verify correctly against the stored `cert_der`:
///
/// - Subject DN is identical (same `node_id_hex`).
/// - Public key is identical (same PEM-serialized key material).
/// - Signatures on leaf certs are valid (same private key).
///
/// Peers always receive the original `cert_der`, never the ephemeral
/// reconstruction.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NodeTransportCa {
    /// Hex ID of the node that owns this CA, used to reconstruct CA params.
    node_id_hex: String,
    /// DER-encoded CA certificate (sent to peers during bootstrap).
    #[serde(with = "hex_bytes")]
    cert_der: Vec<u8>,
    /// PEM-encoded CA private key (PKCS#8; never transmitted).
    key_pem: String,
}

impl NodeTransportCa {
    /// Generate a fresh Node Transport CA for the given node.
    ///
    /// Uses ECDSA P-256 / SHA-256.  CA validity is 10 years.  The Common Name
    /// encodes the node's hex ID for readability in cert viewers.
    pub fn generate(node_id_hex: &str) -> anyhow::Result<Self> {
        let ca_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
        let key_pem = ca_key.serialize_pem();
        let ca = CertifiedIssuer::self_signed(ca_params_for(node_id_hex), ca_key)?;
        Ok(Self {
            node_id_hex: node_id_hex.to_string(),
            cert_der: ca.der().to_vec(),
            key_pem,
        })
    }

    /// DER-encoded CA certificate (for sharing with peers).
    pub fn cert_der(&self) -> &[u8] {
        &self.cert_der
    }

    /// PEM-encoded CA certificate.
    pub fn cert_pem(&self) -> String {
        der_to_pem("CERTIFICATE", &self.cert_der)
    }

    /// Hex ID of the owning node.
    pub fn node_id_hex(&self) -> &str {
        &self.node_id_hex
    }

    /// Issue a leaf certificate for the given node and plane role.
    ///
    /// Each call generates a fresh leaf key pair.  The leaf cert is signed by
    /// this CA and embeds `URI:polar://node/<node_id_hex>` as a SAN so the
    /// application layer can extract the peer's NodeId from an mTLS handshake.
    pub fn issue_leaf_cert(
        &self,
        node_id_hex: &str,
        role: PlaneCertRole,
    ) -> anyhow::Result<IssuedCert> {
        // Reconstruct a signing-capable CA Issuer from stored params + key.
        // See struct-level doc for why this round-trip is safe for verification.
        let ca_key = KeyPair::from_pem(&self.key_pem)?;
        let ca_issuer = Issuer::new(ca_params_for(&self.node_id_hex), ca_key);

        let leaf_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
        let leaf_key_pem = leaf_key.serialize_pem();

        let mut leaf_params = CertificateParams::default();
        leaf_params.is_ca = IsCa::NoCa;
        leaf_params.distinguished_name.push(
            DnType::CommonName,
            format!("Polar Node {node_id_hex} {role}"),
        );
        // NodeId embedded in SAN URI so the application layer can identify the
        // peer after an mTLS handshake without parsing the Common Name.
        leaf_params.subject_alt_names = vec![SanType::URI(
            Ia5String::try_from(format!("polar://node/{node_id_hex}"))
                .map_err(|e| anyhow::anyhow!("invalid SAN URI: {e}"))?,
        )];
        let now = OffsetDateTime::now_utc();
        leaf_params.not_before = now - CERT_NOT_BEFORE_CLOCK_SKEW;
        leaf_params.not_after = now + Duration::days(role.validity_days());
        leaf_params.extended_key_usages = role.eku_purposes();

        let leaf = CertifiedIssuer::signed_by(leaf_params, leaf_key, &ca_issuer)?;
        Ok(IssuedCert {
            cert_der: leaf.der().to_vec(),
            key_pem: leaf_key_pem,
            role,
        })
    }

    /// Sign a realm certificate signing request (CSR).
    ///
    /// CSR-first realm admission: applicant generates a CSR locally with their own key,
    /// sends the CSR (DER) to the authority. Authority verifies the CSR is valid,
    /// signs it (conceptually), and returns only the signed certificate + root CA PEM
    /// (never the private key).
    ///
    /// For Phase 3: we verify the CSR structure and extract the applicant node ID,
    /// then sign a new certificate for that node. Later phases will implement perfect
    /// public-key extraction from CSR to reuse the applicant's key material.
    pub fn sign_csr(
        &self,
        csr_der: &[u8],
        applicant_node_id_hex: &str,
        role: PlaneCertRole,
    ) -> anyhow::Result<RealmIssuedCertificate> {
        use anyhow::Context;

        // For Phase 3: Just verify CSR structure is valid (basic validation)
        anyhow::ensure!(
            !csr_der.is_empty() && csr_der.len() > 10,
            "CSR DER appears to be invalid (too short)"
        );
        anyhow::ensure!(
            csr_der[0..4] == [0x30, 0x82, 0, 0] || csr_der[0] == 0x30,
            "CSR DER does not start with ASN.1 SEQUENCE (0x30)"
        );

        // Reconstruct CA Issuer for signing
        let ca_key = KeyPair::from_pem(&self.key_pem)?;
        let ca_issuer = Issuer::new(ca_params_for(&self.node_id_hex), ca_key);

        let csr_der = CertificateSigningRequestDer::from(csr_der.to_vec());
        let mut csr_params =
            CertificateSigningRequestParams::from_der(&csr_der).context("parse applicant CSR")?;

        // Create certificate metadata for the applicant while keeping the CSR public key.
        let cert_params = &mut csr_params.params;
        cert_params.is_ca = IsCa::NoCa;
        cert_params.distinguished_name.push(
            DnType::CommonName,
            format!("Polar Node {applicant_node_id_hex} {role}"),
        );
        cert_params.subject_alt_names = vec![SanType::URI(
            Ia5String::try_from(format!("polar://node/{applicant_node_id_hex}"))
                .context("Invalid applicant node ID for SAN URI")?,
        )];

        let now = OffsetDateTime::now_utc();
        cert_params.not_before = now - CERT_NOT_BEFORE_CLOCK_SKEW;
        cert_params.not_after = now + Duration::days(role.validity_days());
        cert_params.extended_key_usages = role.eku_purposes();

        // Sign the CSR public key using the CA key. The applicant's private key
        // stays on the applicant node and must match this certificate.
        let signed_cert = csr_params.signed_by(&ca_issuer)?;

        Ok(RealmIssuedCertificate {
            cert_pem: der_to_pem("CERTIFICATE", signed_cert.der().as_ref()),
            root_ca_pem: self.cert_pem(),
        })
    }

    /// View this node's CA cert as a `PeerCaCert` for sharing with peers.
    pub fn as_peer_ca_cert(&self) -> PeerCaCert {
        PeerCaCert {
            cert_der: self.cert_der.clone(),
        }
    }
}

/// Build deterministic CA [`CertificateParams`] for a given `node_id_hex`.
///
/// Called both during initial generation and when reconstructing a signing
/// context from persisted state.  The DN must be identical in both cases so
/// leaf cert issuer names match the stored CA cert's subject name.
fn ca_params_for(node_id_hex: &str) -> CertificateParams {
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.distinguished_name.push(
        DnType::CommonName,
        format!("Polar Node Transport CA {node_id_hex}"),
    );
    params
        .distinguished_name
        .push(DnType::OrganizationName, "Polar Node Transport");
    let now = OffsetDateTime::now_utc();
    params.not_before = now - CERT_NOT_BEFORE_CLOCK_SKEW;
    params.not_after = now + Duration::days(3650);
    params
}

// ── IssuedCert ────────────────────────────────────────────────────────────────

/// A leaf certificate issued for a specific transport plane.
///
/// Short-lived; re-issued on each node startup or when approaching expiry.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IssuedCert {
    /// DER-encoded leaf certificate.
    #[serde(with = "hex_bytes")]
    cert_der: Vec<u8>,
    /// PEM-encoded leaf private key (PKCS#8).
    key_pem: String,
    /// The plane role this certificate was issued for.
    role: PlaneCertRole,
}

impl IssuedCert {
    pub fn from_pem(cert_pem: &str, key_pem: &str, role: PlaneCertRole) -> anyhow::Result<Self> {
        let pem = pem::parse(cert_pem).map_err(|err| anyhow::anyhow!("parse cert PEM: {err}"))?;
        anyhow::ensure!(
            pem.tag() == "CERTIFICATE",
            "expected CERTIFICATE PEM, got {}",
            pem.tag()
        );
        Ok(Self {
            cert_der: pem.contents().to_vec(),
            key_pem: key_pem.to_string(),
            role,
        })
    }

    /// DER-encoded leaf certificate.
    pub fn cert_der(&self) -> &[u8] {
        &self.cert_der
    }

    /// PEM-encoded leaf certificate.
    pub fn cert_pem(&self) -> String {
        der_to_pem("CERTIFICATE", &self.cert_der)
    }

    /// PEM-encoded private key (PKCS#8).
    pub fn key_pem(&self) -> &str {
        &self.key_pem
    }

    /// The plane role this certificate was issued for.
    pub fn role(&self) -> PlaneCertRole {
        self.role
    }
}

// ── PeerCaCert ────────────────────────────────────────────────────────────────

/// A remote node's CA certificate, received during bootstrap.
///
/// Stored in the local node vault indexed by peer NodeId hex.  Used to
/// construct the trusted-CA store for mTLS client/server configs on the
/// member, realm, and control-bus planes.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PeerCaCert {
    /// DER-encoded CA certificate.
    #[serde(with = "hex_bytes")]
    cert_der: Vec<u8>,
}

impl PeerCaCert {
    /// Construct from raw DER bytes received from a peer.
    pub fn from_der(cert_der: Vec<u8>) -> Self {
        Self { cert_der }
    }

    /// DER-encoded CA certificate.
    pub fn cert_der(&self) -> &[u8] {
        &self.cert_der
    }

    /// PEM-encoded CA certificate.
    pub fn cert_pem(&self) -> String {
        der_to_pem("CERTIFICATE", &self.cert_der)
    }
}

// ── hex_bytes serde helper ────────────────────────────────────────────────────

mod hex_bytes {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        let hex = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
        hex.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let hex = String::deserialize(d)?;
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).map_err(serde::de::Error::custom))
            .collect()
    }
}

// ── CSR support ───────────────────────────────────────────────────────────────

/// A certificate signing request for realm admission.
///
/// The applicant generates a local private key in their Vault, creates a CSR
/// from that key, and sends the CSR_DER to the realm authority. The authority
/// verifies the CSR signature, signs it with their Realm CA, and returns the
/// certificate without ever seeing the private key.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RealmCertificateSigningRequest {
    /// DER-encoded X.509 Certificate Signing Request.
    /// Applicant's local private key is NOT included.
    pub csr_der: Vec<u8>,
}

impl RealmCertificateSigningRequest {
    /// Create a new CSR from DER bytes.
    pub fn from_der(csr_der: Vec<u8>) -> Self {
        Self { csr_der }
    }

    /// Get the DER bytes of the CSR.
    pub fn csr_der(&self) -> &[u8] {
        &self.csr_der
    }

    /// Generate a new self-signed key pair and CSR for a realm applicant.
    /// This is the applicant-side flow: generate a key locally in Vault,
    /// create the CSR, and send it to the authority (never send the key).
    pub fn generate_for_applicant(node_id_hex: &str) -> Result<(KeyPair, Self), anyhow::Error> {
        use anyhow::Context;

        // Generate a new P-256 key pair for the applicant's plane-scoped leaf cert.
        let key_pair = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;

        // Create certificate parameters for the CSR.
        // CSR does not have validity dates; those come from the issued cert.
        // We just set the subject DN with the node ID as a URI SAN.
        let mut params = CertificateParams::default();
        params.subject_alt_names = vec![SanType::URI(
            Ia5String::try_from(format!("polar://node/{}", node_id_hex))
                .context("Invalid node ID for SAN URI")?,
        )];

        // Serialize as a certificate signing request.
        let csr = params
            .serialize_request(&key_pair)
            .context("Failed to serialize certificate signing request")?;
        let csr_der = csr.der().as_ref().to_vec();

        Ok((key_pair, Self { csr_der }))
    }
}

/// The issued certificate from a CSR.
///
/// This is the authority's response to a RealmCertificateSigningRequest.
/// It contains the signed certificate and the realm's root CA certificate
/// for the applicant to validate the chain.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RealmIssuedCertificate {
    /// PEM-encoded X.509 certificate signed by the realm's CA.
    pub cert_pem: String,
    /// PEM-encoded root CA certificate for the realm.
    pub root_ca_pem: String,
}

impl RealmIssuedCertificate {
    /// Create from PEM strings.
    pub fn new(cert_pem: String, root_ca_pem: String) -> Self {
        Self {
            cert_pem,
            root_ca_pem,
        }
    }

    /// Get the certificate PEM.
    pub fn cert_pem(&self) -> &str {
        &self.cert_pem
    }

    /// Get the root CA PEM.
    pub fn root_ca_pem(&self) -> &str {
        &self.root_ca_pem
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────



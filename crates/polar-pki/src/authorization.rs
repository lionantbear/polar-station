//! NodeId authorization of an independently held ECDSA transport CA.
//!
//! This module proves identity binding, validity and certificate integrity. It
//! does not grant Access Plane trust or decide revocation freshness. Callers must
//! apply those policies before installing a TLS trust anchor.
//!
//! There is deliberately no default OID until the production namespace is
//! allocated. An OID is trusted configuration, never selected by a peer.
use std::{cell::RefCell, collections::HashSet};

use anyhow::{bail, ensure, Context, Result};
use polar_crypto::ca::{CaAlgorithm, CaDigest, CaPublicKey, CaSigner};
use polar_identity::{
    signed_identity::NodeIdSignatureVerifier, AsyncSigner, NodeId, SignOptions, SignRequest,
    SignatureBytes, SignatureVerifier, SigningPurpose,
};
use rcgen::{CertificateParams, CustomExtension, PublicKeyData, SigningKey};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use x509_cert::{
    der::{
        asn1::{AnyRef, ObjectIdentifier, OctetStringRef},
        Decode, Encode, Reader, SliceReader, Tag, TagNumber, Tagged,
    },
    ext::{
        pkix::{BasicConstraints, KeyUsage, KeyUsages},
        Extension,
    },
    Certificate, TbsCertificate,
};

pub const AUTHORIZATION_PAYLOAD_BYTES: usize = 104;
/// Local isolated-test placeholder, NOT an assigned PEN or a production default.
pub const ISOLATED_TEST_AUTHORIZATION_OID: &str = "1.3.6.1.4.1.0.42";
/// Resource bound for public certificates, independent of hardware sign inputs.
pub const MAX_CA_CERTIFICATE_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizationOid(ObjectIdentifier);
impl AuthorizationOid {
    pub fn new(oid: &str) -> Result<Self> {
        Ok(Self(ObjectIdentifier::new(oid)?))
    }
    fn arcs(&self) -> Vec<u64> {
        self.0.arcs().map(u64::from).collect()
    }
}

/// Validated public material only. It contains no private key or export path.
#[derive(Debug, Clone)]
pub struct AuthorizedCa {
    der: Vec<u8>,
    owner: NodeId,
    epoch: u64,
    spki_sha256: [u8; 32],
    public_key: CaPublicKey,
}
impl AuthorizedCa {
    pub fn der(&self) -> &[u8] {
        &self.der
    }
    pub fn owner(&self) -> NodeId {
        self.owner
    }
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
    /// Key identity for revocation, not the hash of a particular certificate.
    pub fn spki_sha256(&self) -> &[u8; 32] {
        &self.spki_sha256
    }
    pub fn public_key(&self) -> &CaPublicKey {
        &self.public_key
    }

    /// Validate a remote issuer response against this pinned CA, the exact CSR
    /// public key, the authorized applicant, and the requested connection role.
    pub fn verify_csr_result(
        &self,
        csr_der: &[u8],
        applicant: NodeId,
        role: crate::PlaneCertRole,
        issued: &crate::RealmIssuedCertificate,
    ) -> Result<()> {
        use x509_parser::prelude::FromDer;
        ensure!(
            csr_der.len() <= MAX_CA_CERTIFICATE_BYTES
                && issued.cert_pem.len() <= 2 * MAX_CA_CERTIFICATE_BYTES
                && issued.root_ca_pem.len() <= 2 * MAX_CA_CERTIFICATE_BYTES,
            "certificate response exceeds bound"
        );
        let root_pem = pem::parse(&issued.root_ca_pem)?;
        ensure!(
            root_pem.tag() == "CERTIFICATE" && root_pem.contents() == self.der(),
            "CA response changed issuer"
        );
        let leaf_pem = pem::parse(&issued.cert_pem)?;
        ensure!(leaf_pem.tag() == "CERTIFICATE", "not a certificate");
        let cert = Certificate::from_der(leaf_pem.contents())?;
        ensure!(cert.to_der()? == leaf_pem.contents(), "noncanonical leaf");
        let root = Certificate::from_der(self.der())?;
        validate_ca_profile(&root.tbs_certificate, OffsetDateTime::now_utc())?;
        ensure!(
            cert.signature_algorithm == cert.tbs_certificate.signature
                && cert.signature_algorithm == root.signature_algorithm,
            "leaf algorithm mismatch"
        );
        let (rest, request) =
            x509_parser::certification_request::X509CertificationRequest::from_der(csr_der)
                .map_err(|e| anyhow::anyhow!("CSR parse: {e}"))?;
        ensure!(rest.is_empty(), "trailing CSR data");
        request.verify_signature()?;
        let (_, leaf) = x509_parser::parse_x509_certificate(leaf_pem.contents())
            .map_err(|e| anyhow::anyhow!("leaf parse: {e}"))?;
        let (_, issuer) = x509_parser::parse_x509_certificate(self.der())
            .map_err(|e| anyhow::anyhow!("CA parse: {e}"))?;
        leaf.verify_signature(Some(issuer.public_key()))?;
        ensure!(
            leaf.issuer() == issuer.subject()
                && leaf.validity().is_valid()
                && leaf.validity().not_before >= issuer.validity().not_before
                && leaf.validity().not_after <= issuer.validity().not_after
                && leaf.validity().not_after.timestamp() - leaf.validity().not_before.timestamp()
                    <= role.validity_days() * 86400 + 300
                && !leaf.is_ca()
                && leaf.public_key().raw == request.certification_request_info.subject_pki.raw,
            "leaf key, issuer or validity mismatch"
        );
        let san = leaf
            .subject_alternative_name()?
            .context("missing leaf SAN")?;
        ensure!(
            san.value.general_names
                == [x509_parser::extensions::GeneralName::URI(&format!(
                    "polar://node/{}",
                    applicant.to_hex()
                ))],
            "leaf identity mismatch"
        );
        let usage = leaf.key_usage()?.context("missing leaf KeyUsage")?;
        ensure!(
            usage.critical && usage.value.flags == 1,
            "invalid leaf KeyUsage"
        );
        let eku = leaf.extended_key_usage()?.context("missing leaf EKU")?;
        let purpose_matches = if role == crate::PlaneCertRole::ControlBus {
            eku.value.other.len() == 1
                && eku.value.other[0].to_id_string() == crate::CONTROL_BUS_EKU_OID
        } else {
            eku.value.other.is_empty()
        };
        ensure!(
            eku.value.server_auth
                && eku.value.client_auth == role.requires_mtls()
                && !eku.value.any
                && !eku.value.code_signing
                && !eku.value.email_protection
                && !eku.value.time_stamping
                && !eku.value.ocsp_signing
                && purpose_matches,
            "invalid leaf EKU"
        );
        let mut seen = HashSet::new();
        for ext in leaf.extensions() {
            ensure!(
                seen.insert(ext.oid.clone())
                    && matches!(
                        ext.oid.to_id_string().as_str(),
                        "2.5.29.35" | "2.5.29.17" | "2.5.29.15" | "2.5.29.37"
                    ),
                "unexpected leaf extension"
            );
        }
        Ok(())
    }

    /// A new local P-256 TLS key, signed by this fixed CA. The connection key
    /// remains local software material; CA offload does not offload TLS itself.
    pub async fn issue_leaf_cert(
        &self,
        node: NodeId,
        role: crate::PlaneCertRole,
        ca: &dyn CaSigner,
    ) -> Result<crate::IssuedCert> {
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
        let params = crate::plane_leaf_params(&node.to_hex(), role, OffsetDateTime::now_utc())?;
        let der = self.sign_leaf(params, &key, ca).await?;
        Ok(crate::IssuedCert {
            cert_der: der,
            key_pem: key.serialize_pem(),
            role,
        })
    }

    /// Verify the applicant's CSR self-signature and retain only its public key.
    /// Admission/Realm membership is the caller's authorization responsibility.
    pub async fn sign_csr(
        &self,
        csr_der: &[u8],
        applicant: NodeId,
        role: crate::PlaneCertRole,
        ca: &dyn CaSigner,
    ) -> Result<crate::RealmIssuedCertificate> {
        ensure!(
            csr_der.len() <= MAX_CA_CERTIFICATE_BYTES,
            "CSR exceeds bound"
        );
        let csr = x509_cert::request::CertReq::from_der(csr_der)?;
        ensure!(csr.to_der()? == csr_der, "noncanonical CSR DER");
        let csr_der = rustls_pki_types::CertificateSigningRequestDer::from(csr_der);
        let request = rcgen::CertificateSigningRequestParams::from_der(&csr_der)?;
        ensure!(
            request.public_key.algorithm() == &rcgen::PKCS_ECDSA_P256_SHA256,
            "connection key must be P-256"
        );
        let params =
            crate::plane_leaf_params(&applicant.to_hex(), role, OffsetDateTime::now_utc())?;
        let der = self.sign_leaf(params, &request.public_key, ca).await?;
        Ok(crate::RealmIssuedCertificate {
            cert_pem: crate::der_to_pem("CERTIFICATE", &der),
            root_ca_pem: crate::der_to_pem("CERTIFICATE", &self.der),
        })
    }

    async fn sign_leaf(
        &self,
        mut params: CertificateParams,
        subject: &impl PublicKeyData,
        ca: &dyn CaSigner,
    ) -> Result<Vec<u8>> {
        ensure!(
            ca.public_key() == &self.public_key,
            "CA signing handle mismatch"
        );
        let cert = Certificate::from_der(&self.der)?;
        validate_ca_profile(&cert.tbs_certificate, OffsetDateTime::now_utc())?;
        // A leaf never outlives its issuer, and a newly authorized CA can have a
        // later not_before than the standard leaf clock-skew allowance.
        params.not_before = params.not_before.max(OffsetDateTime::from_unix_timestamp(
            cert.tbs_certificate
                .validity
                .not_before
                .to_unix_duration()
                .as_secs()
                .try_into()?,
        )?);
        params.not_after = params.not_after.min(OffsetDateTime::from_unix_timestamp(
            cert.tbs_certificate
                .validity
                .not_after
                .to_unix_duration()
                .as_secs()
                .try_into()?,
        )?);
        let ca_der = rustls_pki_types::CertificateDer::from(self.der.as_slice());
        let public = Public(self.public_key.clone());
        let tbs = capture(&public, |key| {
            let issuer = rcgen::Issuer::from_ca_cert_der(&ca_der, key)?;
            params.signed_by(subject, &issuer)
        })?;
        let signature = ca
            .sign_digest(CaDigest::from_tbs(public.0.algorithm(), &tbs))
            .await?;
        ensure!(signature.len() <= 104, "oversized ECDSA signature");
        let replay = Replay {
            public,
            tbs,
            signature,
        };
        let issuer = rcgen::Issuer::from_ca_cert_der(&ca_der, &replay)?;
        let leaf = params.signed_by(subject, &issuer)?;
        let (_, root) = x509_parser::parse_x509_certificate(&self.der)
            .map_err(|e| anyhow::anyhow!("CA parse: {e}"))?;
        let (_, parsed) = x509_parser::parse_x509_certificate(leaf.der())
            .map_err(|e| anyhow::anyhow!("leaf parse: {e}"))?;
        parsed
            .verify_signature(Some(root.public_key()))
            .context("invalid leaf backend signature")?;
        Ok(leaf.der().to_vec())
    }
}

/// Freeze params once, capture TBS without signing a temporary certificate, then
/// perform exactly one NodeId and one CA operation. No executor/block_on bridge.
pub async fn issue_node_authorized_ca(
    oid: &AuthorizationOid,
    epoch: u64,
    identity: &dyn AsyncSigner,
    ca: &dyn CaSigner,
    options: SignOptions,
) -> Result<AuthorizedCa> {
    issue_authorized_ca(
        crate::ca_params_for(&identity.descriptor().node_id.to_hex()),
        oid,
        epoch,
        identity,
        ca,
        options,
    )
    .await
}

pub async fn issue_authorized_ca(
    params: CertificateParams,
    oid: &AuthorizationOid,
    epoch: u64,
    identity: &dyn AsyncSigner,
    ca: &dyn CaSigner,
    options: SignOptions,
) -> Result<AuthorizedCa> {
    ensure!(epoch != 0, "zero CA authorization epoch");
    options.check_deadline()?;
    let now = OffsetDateTime::now_utc();
    let owner = identity.descriptor().node_id;
    let public = Public(ca.public_key().clone());
    let t0 = capture(&public, |key| params.self_signed(key))?;
    let parsed = strict_tbs(&t0)?;
    validate_ca_profile(&parsed, now)?;
    ensure!(
        parsed
            .extensions
            .as_ref()
            .is_some_and(|extensions| extensions.iter().all(|ext| ext.extn_id != oid.0)),
        "authorization extension already present"
    );
    let preimage = authorization_preimage(owner, epoch, &t0)?;
    let request = SignRequest::new(SigningPurpose::CaAuthorizationV1, preimage.to_vec())?;
    let signature = request.sign_with(owner, identity, options).await?;
    let mut payload = [0u8; AUTHORIZATION_PAYLOAD_BYTES];
    payload[..32].copy_from_slice(owner.as_bytes());
    payload[32..40].copy_from_slice(&epoch.to_be_bytes());
    payload[40..].copy_from_slice(signature.as_bytes());
    let mut params = params;
    params
        .custom_extensions
        .push(CustomExtension::from_oid_content(
            &oid.arcs(),
            OctetStringRef::new(&payload)?.to_der()?,
        ));
    let tbs = capture(&public, |key| params.self_signed(key))?;
    ensure!(
        tbs.len() <= MAX_CA_CERTIFICATE_BYTES - 256,
        "CA certificate exceeds bound"
    );
    options.check_deadline()?;
    let signature = ca
        .sign_digest(CaDigest::from_tbs(public.0.algorithm(), &tbs))
        .await?;
    // A backend may be faulty or return stale/cross-key material. Verification
    // below covers the exact frozen TBS and both independent signatures.
    ensure!(signature.len() <= 104, "oversized ECDSA signature");
    let replay = Replay {
        public,
        tbs,
        signature,
    };
    let cert = params.self_signed(&replay)?;
    verify_authorized_ca(cert.der(), oid, owner, epoch, now)
}

pub fn authorization_preimage(owner: NodeId, epoch: u64, t0: &[u8]) -> Result<[u8; 40]> {
    ensure!(epoch != 0, "zero CA authorization epoch");
    ensure!(t0.len() <= MAX_CA_CERTIFICATE_BYTES, "CA TBS exceeds bound");
    let mut hash = Sha256::new();
    hash.update(owner.as_bytes());
    hash.update(epoch.to_be_bytes());
    hash.update(t0);
    let mut preimage = [0; 40];
    preimage[..8].copy_from_slice(b"PCAUTH01");
    preimage[8..].copy_from_slice(&hash.finalize());
    Ok(preimage)
}

/// Strict DER/profile, NodeId authorization, epoch floor, expiry and self-signature.
/// `minimum_epoch` is supplied by the caller's authenticated persistent state.
/// It does not substitute for key fingerprint revocation or Access Plane trust.
/// Validate a public CA carried inside an already authenticated identity message.
/// This checks certificate structure/profile/self-signature, NOT NodeId ownership;
/// the caller must retain and verify the enclosing NodeId signature and approval.
pub fn validate_identity_message_ca(der: &[u8]) -> Result<()> {
    ensure!(
        der.len() <= MAX_CA_CERTIFICATE_BYTES,
        "CA certificate exceeds bound"
    );
    let cert = Certificate::from_der(der)?;
    ensure!(cert.to_der()? == der, "noncanonical certificate DER");
    ensure!(
        cert.signature_algorithm == cert.tbs_certificate.signature,
        "signature algorithm mismatch"
    );
    validate_ca_profile(&cert.tbs_certificate, OffsetDateTime::now_utc())?;
    let (remaining, parsed) = x509_parser::parse_x509_certificate(der)
        .map_err(|e| anyhow::anyhow!("certificate parse: {e}"))?;
    ensure!(remaining.is_empty(), "trailing certificate bytes");
    parsed
        .verify_signature(None)
        .context("invalid CA self-signature")?;
    Ok(())
}

pub fn verify_authorized_ca_now(
    der: &[u8],
    oid: &AuthorizationOid,
    expected_owner: NodeId,
    minimum_epoch: u64,
) -> Result<AuthorizedCa> {
    verify_authorized_ca(
        der,
        oid,
        expected_owner,
        minimum_epoch,
        OffsetDateTime::now_utc(),
    )
}

pub fn verify_authorized_ca(
    der: &[u8],
    oid: &AuthorizationOid,
    expected_owner: NodeId,
    minimum_epoch: u64,
    now: OffsetDateTime,
) -> Result<AuthorizedCa> {
    ensure!(
        der.len() <= MAX_CA_CERTIFICATE_BYTES,
        "CA certificate exceeds bound"
    );
    let cert = Certificate::from_der(der)?;
    ensure!(cert.to_der()? == der, "noncanonical certificate DER");
    let tbs = &cert.tbs_certificate;
    ensure!(
        cert.signature_algorithm == tbs.signature,
        "signature algorithm mismatch"
    );
    let public_key = validate_ca_profile(tbs, now)?;
    let raw_tbs = first_child(der)?;
    let (t0, payload) = remove_authorization(raw_tbs, oid)?;
    ensure!(
        payload.len() == AUTHORIZATION_PAYLOAD_BYTES,
        "invalid authorization size"
    );
    let owner = NodeId::from_bytes(payload[..32].try_into()?);
    ensure!(owner == expected_owner, "unexpected CA owner");
    let epoch = u64::from_be_bytes(payload[32..40].try_into()?);
    ensure!(
        epoch != 0 && epoch >= minimum_epoch,
        "stale CA authorization"
    );
    let signature = SignatureBytes::from_bytes(payload[40..].try_into()?);
    NodeIdSignatureVerifier.verify_signature(
        &owner,
        &authorization_preimage(owner, epoch, &t0)?,
        &signature,
    )?;
    let (remaining, parsed) = x509_parser::parse_x509_certificate(der)
        .map_err(|e| anyhow::anyhow!("certificate parse: {e}"))?;
    ensure!(remaining.is_empty(), "trailing certificate bytes");
    parsed
        .verify_signature(None)
        .context("invalid CA self-signature")?;
    Ok(AuthorizedCa {
        der: der.to_vec(),
        owner,
        epoch,
        spki_sha256: Sha256::digest(tbs.subject_public_key_info.to_der()?).into(),
        public_key,
    })
}

fn strict_tbs(bytes: &[u8]) -> Result<TbsCertificate> {
    ensure!(
        bytes.len() <= MAX_CA_CERTIFICATE_BYTES,
        "CA TBS exceeds bound"
    );
    let tbs = TbsCertificate::from_der(bytes)?;
    ensure!(tbs.to_der()? == bytes, "noncanonical TBS DER");
    Ok(tbs)
}

fn validate_ca_profile(tbs: &TbsCertificate, now: OffsetDateTime) -> Result<CaPublicKey> {
    ensure!(
        tbs.version == x509_cert::certificate::Version::V3,
        "CA must be v3"
    );
    ensure!(tbs.issuer == tbs.subject, "CA must be self-issued");
    ensure!(
        tbs.issuer_unique_id.is_none() && tbs.subject_unique_id.is_none(),
        "unsupported CA unique ID"
    );
    let since = i64::try_from(tbs.validity.not_before.to_unix_duration().as_secs())?;
    let until = i64::try_from(tbs.validity.not_after.to_unix_duration().as_secs())?;
    ensure!(
        since <= now.unix_timestamp() && now.unix_timestamp() <= until && since < until,
        "CA outside validity"
    );
    let extensions = tbs.extensions.as_ref().context("missing CA extensions")?;
    let mut seen = HashSet::new();
    for ext in extensions {
        ensure!(seen.insert(ext.extn_id), "duplicate extension OID");
        ensure!(
            !ext.critical || matches!(ext.extn_id.to_string().as_str(), "2.5.29.19" | "2.5.29.15"),
            "unsupported critical CA extension"
        );
    }
    let (critical, bc) = tbs
        .get::<BasicConstraints>()?
        .context("missing BasicConstraints")?;
    ensure!(
        critical && bc.ca && bc.path_len_constraint == Some(0),
        "CA must only issue leaves"
    );
    let (critical, ku) = tbs.get::<KeyUsage>()?.context("missing KeyUsage")?;
    ensure!(
        critical && ku == KeyUsage(KeyUsages::KeyCertSign.into()),
        "invalid CA KeyUsage"
    );
    // AlgorithmIdentifier parameters are absent for ECDSA signatures. The
    // public key must explicitly select the matching named curve.
    ensure!(
        tbs.signature.parameters.is_none(),
        "unexpected signature parameters"
    );
    let spki = &tbs.subject_public_key_info;
    ensure!(
        spki.algorithm.oid.to_string() == "1.2.840.10045.2.1",
        "CA key must be EC"
    );
    let curve = spki
        .algorithm
        .parameters
        .as_ref()
        .context("missing named curve")?
        .decode_as::<ObjectIdentifier>()?;
    let algorithm = match (
        tbs.signature.oid.to_string().as_str(),
        curve.to_string().as_str(),
    ) {
        ("1.2.840.10045.4.3.2", "1.2.840.10045.3.1.7") => CaAlgorithm::P256Sha256,
        ("1.2.840.10045.4.3.3", "1.3.132.0.34") => CaAlgorithm::P384Sha384,
        _ => bail!("unsupported CA curve/signature combination"),
    };
    Ok(CaPublicKey::from_sec1(
        algorithm,
        spki.subject_public_key
            .as_bytes()
            .context("unaligned public key")?
            .to_vec(),
    )?)
}

fn first_child(der: &[u8]) -> Result<&[u8]> {
    let sequence = AnyRef::from_der(der)?;
    ensure!(sequence.tag() == Tag::Sequence, "expected sequence");
    Ok(SliceReader::new(sequence.value())?.tlv_bytes()?)
}

/// Preserve every unaffected original TLV, including unknown extensions and
/// their order. DER libraries parse/encode lengths; this is not an ASN.1 codec.
fn remove_authorization(tbs: &[u8], oid: &AuthorizationOid) -> Result<(Vec<u8>, Vec<u8>)> {
    strict_tbs(tbs)?;
    let sequence = AnyRef::from_der(tbs)?;
    let mut fields = SliceReader::new(sequence.value())?;
    let mut kept = Vec::with_capacity(tbs.len());
    let mut payload = None;
    while !fields.is_finished() {
        let raw = fields.tlv_bytes()?;
        let value = AnyRef::from_der(raw)?;
        if value.tag()
            != (Tag::ContextSpecific {
                constructed: true,
                number: TagNumber::N3,
            })
        {
            kept.extend_from_slice(raw);
            continue;
        }
        let extensions = AnyRef::from_der(value.value())?;
        ensure!(extensions.tag() == Tag::Sequence, "invalid Extensions");
        let mut reader = SliceReader::new(extensions.value())?;
        let mut remaining = Vec::new();
        let mut seen = HashSet::new();
        while !reader.is_finished() {
            let raw = reader.tlv_bytes()?;
            let extension = Extension::from_der(raw)?;
            ensure!(seen.insert(extension.extn_id), "duplicate extension OID");
            if extension.extn_id == oid.0 {
                ensure!(
                    payload.is_none() && !extension.critical,
                    "invalid authorization extension"
                );
                payload = Some(
                    OctetStringRef::from_der(extension.extn_value.as_bytes())?
                        .as_bytes()
                        .to_vec(),
                );
            } else {
                remaining.extend_from_slice(raw);
            }
        }
        if !remaining.is_empty() {
            let inner = AnyRef::new(Tag::Sequence, &remaining)?.to_der()?;
            kept.extend_from_slice(&AnyRef::new(value.tag(), &inner)?.to_der()?);
        }
    }
    Ok((
        AnyRef::new(Tag::Sequence, &kept)?.to_der()?,
        payload.context("missing CA authorization")?,
    ))
}

#[derive(Clone)]
struct Public(CaPublicKey);
impl PublicKeyData for Public {
    fn der_bytes(&self) -> &[u8] {
        self.0.sec1()
    }
    fn algorithm(&self) -> &'static rcgen::SignatureAlgorithm {
        match self.0.algorithm() {
            CaAlgorithm::P256Sha256 => &rcgen::PKCS_ECDSA_P256_SHA256,
            CaAlgorithm::P384Sha384 => &rcgen::PKCS_ECDSA_P384_SHA384,
        }
    }
}
struct Capture<'a> {
    public: &'a Public,
    tbs: RefCell<Option<Vec<u8>>>,
}
impl PublicKeyData for Capture<'_> {
    fn der_bytes(&self) -> &[u8] {
        self.public.der_bytes()
    }
    fn algorithm(&self) -> &'static rcgen::SignatureAlgorithm {
        self.public.algorithm()
    }
}
impl SigningKey for Capture<'_> {
    fn sign(&self, msg: &[u8]) -> Result<Vec<u8>, rcgen::Error> {
        *self.tbs.borrow_mut() = Some(msg.to_vec());
        Err(rcgen::Error::RemoteKeyError)
    }
}
fn capture(
    public: &Public,
    encode: impl FnOnce(&Capture<'_>) -> Result<rcgen::Certificate, rcgen::Error>,
) -> Result<Vec<u8>> {
    let key = Capture {
        public,
        tbs: RefCell::new(None),
    };
    match encode(&key) {
        Err(rcgen::Error::RemoteKeyError) => key.tbs.into_inner().context("TBS capture missing"),
        Err(error) => Err(error.into()),
        Ok(_) => bail!("encoder bypassed external signing key"),
    }
}
struct Replay {
    public: Public,
    tbs: Vec<u8>,
    signature: Vec<u8>,
}
impl PublicKeyData for Replay {
    fn der_bytes(&self) -> &[u8] {
        self.public.der_bytes()
    }
    fn algorithm(&self) -> &'static rcgen::SignatureAlgorithm {
        self.public.algorithm()
    }
}
impl SigningKey for Replay {
    fn sign(&self, msg: &[u8]) -> Result<Vec<u8>, rcgen::Error> {
        if msg != self.tbs {
            return Err(rcgen::Error::RemoteKeyError);
        }
        Ok(self.signature.clone())
    }
}

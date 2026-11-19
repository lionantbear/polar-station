//! Certificate-level port for transport and admission. No CA key export or
//! arbitrary digest signing is available to consumers of this interface.
use anyhow::Result;
use async_trait::async_trait;
use polar_identity::NodeId;

use crate::{
    IssuedCert, PeerCaCert, PlaneCertRole, RealmCertificateSigningRequest, RealmIssuedCertificate,
};

#[async_trait]
pub trait ConnectionCa: Send + Sync {
    /// Invalidates independently owned local CA handles during owner shutdown.
    fn close(&self) -> Result<()> {
        Ok(())
    }
    /// Capability of this configured CA handle, not of the root Ed25519 signer.
    /// Admission to other algorithms still belongs to their typed backend.
    fn uses_hardware_key(&self) -> bool {
        false
    }
    fn owner(&self) -> NodeId;
    fn as_peer_ca_cert(&self) -> PeerCaCert;
    async fn sign_csr(
        &self,
        csr: &[u8],
        applicant: NodeId,
        role: PlaneCertRole,
    ) -> Result<RealmIssuedCertificate>;

    /// The TLS connection key remains local. Only the public CSR crosses the
    /// signing boundary; the selected CA verifies and applies its leaf profile.
    async fn issue_leaf_cert(&self, role: PlaneCertRole) -> Result<IssuedCert> {
        let (key, csr) =
            RealmCertificateSigningRequest::generate_for_applicant(&self.owner().to_hex())?;
        let issued = self.sign_csr(csr.csr_der(), self.owner(), role).await?;
        IssuedCert::from_pem(issued.cert_pem(), &key.serialize_pem(), role)
    }
}

/// Existing explicitly local installations keep their original CA. This
/// adapter never generates a key and is not a fallback from a failed provider.
pub struct LocalConnectionCa {
    owner: NodeId,
    ca: crate::NodeTransportCa,
}
impl LocalConnectionCa {
    pub fn open(ca: crate::NodeTransportCa, owner: NodeId) -> Result<Self> {
        anyhow::ensure!(
            ca.node_id_hex() == owner.to_hex(),
            "local CA owner mismatch"
        );
        Ok(Self { owner, ca })
    }
}
#[async_trait]
impl ConnectionCa for LocalConnectionCa {
    fn owner(&self) -> NodeId {
        self.owner
    }
    fn as_peer_ca_cert(&self) -> PeerCaCert {
        self.ca.as_peer_ca_cert()
    }
    async fn sign_csr(
        &self,
        csr: &[u8],
        applicant: NodeId,
        role: PlaneCertRole,
    ) -> Result<RealmIssuedCertificate> {
        anyhow::ensure!(
            role == PlaneCertRole::Realm
                || (applicant == self.owner
                    && matches!(
                        role,
                        PlaneCertRole::Member
                            | PlaneCertRole::Bootstrap
                            | PlaneCertRole::ControlBus
                    )),
            "CA purpose/applicant denied"
        );
        self.ca.sign_csr(csr, &applicant.to_hex(), role)
    }
}

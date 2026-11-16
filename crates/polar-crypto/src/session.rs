use hkdf::Hkdf;
use rand_core::OsRng;
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionKey([u8; 32]);

impl SessionKey {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

pub struct SessionKeypair {
    secret: StaticSecret,
}

impl SessionKeypair {
    pub fn generate() -> Self {
        Self {
            secret: StaticSecret::random_from_rng(OsRng),
        }
    }

    pub fn from_secret_bytes(secret: [u8; 32]) -> Self {
        Self {
            secret: StaticSecret::from(secret),
        }
    }

    pub fn secret_bytes(&self) -> [u8; 32] {
        self.secret.to_bytes()
    }

    pub fn public_key(&self) -> [u8; 32] {
        *PublicKey::from(&self.secret).as_bytes()
    }
}

pub fn derive_session_key(
    local: &SessionKeypair,
    remote_public: [u8; 32],
    context: &[u8],
) -> SessionKey {
    let remote = PublicKey::from(remote_public);
    let shared = local.secret.diffie_hellman(&remote);
    let hkdf = Hkdf::<Sha256>::new(Some(context), shared.as_bytes());
    let mut out = [0u8; 32];
    hkdf.expand(b"polar-session-v1", &mut out)
        .expect("hkdf expand for session key");
    SessionKey(out)
}

use crate::errors::IdentityError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BindingPublicKey([u8; 32]);

impl BindingPublicKey {
    pub fn from_hex(s: &str) -> Result<Self, IdentityError> {
        let trimmed = s.trim();
        let bytes = hex::decode(trimmed).map_err(|_| IdentityError::InvalidHex)?;
        let raw: [u8; 32] = bytes.try_into().map_err(|_| IdentityError::InvalidLength)?;
        Ok(Self(raw))
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BindingSecretKey(pub(crate) [u8; 32]);

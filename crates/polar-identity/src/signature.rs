use crate::errors::IdentityError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignatureBytes([u8; 64]);

impl SignatureBytes {
    pub fn from_bytes(bytes: [u8; 64]) -> Self {
        Self(bytes)
    }

    pub fn from_hex(s: &str) -> Result<Self, IdentityError> {
        let bytes = hex::decode(s.trim()).map_err(|_| IdentityError::InvalidHex)?;
        let raw: [u8; 64] = bytes.try_into().map_err(|_| IdentityError::InvalidLength)?;
        Ok(Self(raw))
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    pub fn as_bytes(&self) -> &[u8; 64] {
        &self.0
    }
}

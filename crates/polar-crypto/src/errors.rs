use std::error::Error;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CryptoError {
    EncryptFailed,
    DecryptFailed,
}

impl fmt::Display for CryptoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::EncryptFailed => "encrypt failed",
            Self::DecryptFailed => "decrypt failed",
        };
        f.write_str(text)
    }
}

impl Error for CryptoError {}

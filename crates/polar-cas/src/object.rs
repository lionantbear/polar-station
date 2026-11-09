//! Algorithm-tagged content identity, independent of placement and business scope.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DigestAlgorithm {
    Sha256,
    Blake3,
}

impl DigestAlgorithm {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sha256 => "sha256",
            Self::Blake3 => "blake3",
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ObjectId {
    algorithm: DigestAlgorithm,
    digest: [u8; 32],
}

impl ObjectId {
    /// Compatibility default: never silently change old callers' content identity.
    pub fn for_bytes(bytes: &[u8]) -> Self {
        Self::for_bytes_with(DigestAlgorithm::Sha256, bytes)
    }

    pub fn for_bytes_with(algorithm: DigestAlgorithm, bytes: &[u8]) -> Self {
        let mut hasher = ObjectHasher::new(algorithm);
        hasher.update(bytes);
        hasher.finish()
    }

    pub fn for_reader(reader: impl std::io::Read) -> Result<Self> {
        Self::for_reader_with(DigestAlgorithm::Sha256, reader)
    }

    /// Blocking, bounded-memory hashing; runtime callers use a blocking worker.
    pub fn for_reader_with(
        algorithm: DigestAlgorithm,
        mut reader: impl std::io::Read,
    ) -> Result<Self> {
        let mut hash = ObjectHasher::new(algorithm);
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let count = reader.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            hash.update(&buffer[..count]);
        }
        Ok(hash.finish())
    }

    pub fn from_sha256(digest: [u8; 32]) -> Self {
        Self::from_digest(DigestAlgorithm::Sha256, digest)
    }

    /// Labels digest bytes; this constructor is not proof that content was verified.
    pub fn from_digest(algorithm: DigestAlgorithm, digest: [u8; 32]) -> Self {
        Self { algorithm, digest }
    }

    pub const fn algorithm(&self) -> DigestAlgorithm {
        self.algorithm
    }

    pub fn digest_bytes(&self) -> &[u8; 32] {
        &self.digest
    }

    pub fn hex(&self) -> String {
        hex::encode(self.digest)
    }

    /// Existing shared-content layout, with no namespace or storage prefix.
    pub fn content_key(&self) -> String {
        let hex = self.hex();
        format!("{}/{}/{}", self.algorithm.as_str(), &hex[..2], hex)
    }

    pub fn from_content_key(key: &str) -> Result<Self> {
        let parts: Vec<_> = key.split('/').collect();
        if parts.len() != 3 {
            return Err(Error::Invalid("invalid content key".into()));
        }
        let id = Self::try_from(format!("{}:{}", parts[0], parts[2]))?;
        if id.content_key() != key {
            return Err(Error::Invalid("noncanonical content key".into()));
        }
        Ok(id)
    }

    /// Existing ObjectRef/CasStore adapters only implement SHA-256 verification.
    pub fn require_sha256(&self) -> Result<()> {
        if self.algorithm == DigestAlgorithm::Sha256 {
            Ok(())
        } else {
            Err(Error::Invalid("legacy CAS binding requires SHA-256".into()))
        }
    }
}

impl std::fmt::Display for ObjectId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.algorithm.as_str(), self.hex())
    }
}

impl TryFrom<String> for ObjectId {
    type Error = Error;

    fn try_from(value: String) -> Result<Self> {
        let (algorithm, hex) = value
            .split_once(':')
            .ok_or_else(|| Error::Invalid("expected tagged content identifier".into()))?;
        let algorithm = match algorithm {
            "sha256" => DigestAlgorithm::Sha256,
            "blake3" => DigestAlgorithm::Blake3,
            _ => {
                return Err(Error::Invalid(
                    "unsupported content digest algorithm".into(),
                ))
            }
        };
        if hex.len() != 64
            || !hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::Invalid(
                "expected canonical lowercase 256-bit digest".into(),
            ));
        }
        let mut digest = [0; 32];
        hex::decode_to_slice(hex, &mut digest)
            .map_err(|_| Error::Invalid("invalid content digest".into()))?;
        Ok(Self::from_digest(algorithm, digest))
    }
}

impl From<ObjectId> for String {
    fn from(value: ObjectId) -> Self {
        value.to_string()
    }
}

/// Streaming adapter to upstream algorithms, not a new hashing implementation.
pub struct ObjectHasher(HashState);

enum HashState {
    Sha256(Sha256),
    Blake3(Box<blake3::Hasher>),
}

impl ObjectHasher {
    pub fn new(algorithm: DigestAlgorithm) -> Self {
        Self(match algorithm {
            DigestAlgorithm::Sha256 => HashState::Sha256(Sha256::new()),
            DigestAlgorithm::Blake3 => HashState::Blake3(Box::default()),
        })
    }

    pub fn update(&mut self, bytes: &[u8]) {
        match &mut self.0 {
            HashState::Sha256(hash) => hash.update(bytes),
            HashState::Blake3(hash) => {
                blake3::Hasher::update(hash.as_mut(), bytes);
            }
        }
    }

    pub fn finish(self) -> ObjectId {
        match self.0 {
            HashState::Sha256(hash) => ObjectId::from_sha256(hash.finalize().into()),
            HashState::Blake3(hash) => {
                // Full applications may enable blake3's RustCrypto traits.
                // Select the inherent method explicitly, not Digest::finalize
                // on Box<Hasher>, whose return type depends on feature union.
                ObjectId::from_digest(
                    DigestAlgorithm::Blake3,
                    *blake3::Hasher::finalize(hash.as_ref()).as_bytes(),
                )
            }
        }
    }
}

/// Public content reference without attachment, product or namespace identity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentRef {
    pub id: ObjectId,
    pub size_bytes: u64,
}

/// An unverified assertion. A claim must never be treated as a ready replica.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentClaim {
    pub id: Option<ObjectId>,
    pub size_bytes: Option<u64>,
}

impl ContentClaim {
    pub fn check(&self, content: &ContentRef) -> Result<()> {
        if self.id.as_ref().is_some_and(|id| id != &content.id)
            || self
                .size_bytes
                .is_some_and(|size| size != content.size_bytes)
        {
            return Err(Error::Integrity);
        }
        Ok(())
    }
}

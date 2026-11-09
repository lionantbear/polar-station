//! In-process verification witnesses; deliberately not deserializable.
use crate::{
    ContentClaim, ContentRef, DigestAlgorithm, Error, ObjectHasher, ObjectId, Reader, Result,
};
use tokio::io::AsyncReadExt;

/// Revalidate an existing replica from bytes, with bounded memory and input.
/// Metadata-only observations and a matching prefix cannot create this witness.
pub async fn verify_replica(mut input: Reader, expected: &ContentRef) -> Result<VerifiedContent> {
    let mut verifier = ContentVerifier::default();
    let mut size = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = input.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        size = size.checked_add(count as u64).ok_or(Error::Integrity)?;
        if size > expected.size_bytes {
            return Err(Error::Integrity);
        }
        verifier.update(&buffer[..count])?;
    }
    let verified = verifier.finish();
    if !verified.ids().contains(&expected.id) || verified.size_bytes() != expected.size_bytes {
        return Err(Error::Integrity);
    }
    Ok(verified)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DigestEvidence {
    FullHash,
    Trusted,
}

#[derive(Clone, Debug)]
pub struct VerifiedContent {
    ids: Vec<ObjectId>,
    size_bytes: u64,
    evidence: DigestEvidence,
}
impl VerifiedContent {
    pub fn hash_bytes(bytes: &[u8]) -> Self {
        let mut verifier = ContentVerifier::default();
        verifier.update(bytes).expect("a Rust slice fits u64");
        verifier.finish()
    }
    pub fn ids(&self) -> &[ObjectId] {
        &self.ids
    }
    pub fn size_bytes(&self) -> u64 {
        self.size_bytes
    }
    pub fn evidence(&self) -> DigestEvidence {
        self.evidence
    }
    pub fn preferred(&self) -> ContentRef {
        ContentRef {
            id: self.ids[0].clone(),
            size_bytes: self.size_bytes,
        }
    }
    pub fn reference(&self, id: &ObjectId) -> Result<ContentRef> {
        if !self.ids.contains(id) {
            return Err(Error::MissingDigest);
        }
        Ok(ContentRef {
            id: id.clone(),
            size_bytes: self.size_bytes,
        })
    }
    pub fn satisfies(&self, claim: &ContentClaim) -> bool {
        claim.id.as_ref().is_none_or(|id| self.ids.contains(id))
            && claim.size_bytes.is_none_or(|size| size == self.size_bytes)
    }
    /// Explicit in-process trusted admission, never a network DTO. The caller
    /// must already have verified these digests over the same immutable bytes.
    /// Local admission must additionally check path/stat/owner/mode. A missing
    /// digest stays missing: this method does not infer additional aliases.
    pub fn from_trusted_admission(ids: Vec<ObjectId>, size_bytes: u64) -> Result<Self> {
        validate_algorithms(ids.iter().map(ObjectId::algorithm))?;
        let mut ids = ids;
        ids.sort_by_key(|id| match id.algorithm() {
            DigestAlgorithm::Sha256 => 0,
            DigestAlgorithm::Blake3 => 1,
        });
        Ok(Self {
            ids,
            size_bytes,
            evidence: DigestEvidence::Trusted,
        })
    }
}

pub struct ContentVerifier {
    hashers: Vec<ObjectHasher>,
    size_bytes: u64,
}
impl Default for ContentVerifier {
    fn default() -> Self {
        Self::new(&[DigestAlgorithm::Sha256, DigestAlgorithm::Blake3])
            .expect("two distinct algorithms")
    }
}
impl ContentVerifier {
    pub fn new(algorithms: &[DigestAlgorithm]) -> Result<Self> {
        validate_algorithms(algorithms.iter().copied())?;
        let hashers = [DigestAlgorithm::Sha256, DigestAlgorithm::Blake3]
            .into_iter()
            .filter(|algorithm| algorithms.contains(algorithm))
            .map(ObjectHasher::new)
            .collect();
        Ok(Self {
            hashers,
            size_bytes: 0,
        })
    }
    pub fn update(&mut self, bytes: &[u8]) -> Result<()> {
        self.size_bytes = self
            .size_bytes
            .checked_add(bytes.len() as u64)
            .ok_or(Error::TooLarge)?;
        for hasher in &mut self.hashers {
            hasher.update(bytes);
        }
        Ok(())
    }
    pub fn finish(self) -> VerifiedContent {
        VerifiedContent {
            ids: self.hashers.into_iter().map(ObjectHasher::finish).collect(),
            size_bytes: self.size_bytes,
            evidence: DigestEvidence::FullHash,
        }
    }
}
fn validate_algorithms(algorithms: impl Iterator<Item = DigestAlgorithm>) -> Result<()> {
    let mut seen = Vec::new();
    for algorithm in algorithms {
        if seen.contains(&algorithm) {
            return Err(Error::Invalid("duplicate digest algorithm".into()));
        }
        seen.push(algorithm);
    }
    if seen.is_empty() {
        return Err(Error::Invalid("at least one digest is required".into()));
    }
    Ok(())
}

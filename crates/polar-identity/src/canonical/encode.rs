use sha2::{Digest, Sha256};

use crate::NodeName;

pub(crate) fn hash_node_name(name: &NodeName) -> [u8; 32] {
    let digest = Sha256::digest(name.as_str().as_bytes());
    digest.into()
}

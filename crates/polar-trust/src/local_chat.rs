//! A target-machine account grant, independent of node management authority.
use polar_identity::NodeId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalChatAuthority {
    #[serde(with = "crate::serde_value::node_id")]
    pub instance_node_id: NodeId,
    pub uid: u32,
    pub revision: u64,
    pub active: bool,
}

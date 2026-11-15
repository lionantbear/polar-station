//! Optional stream upgrade on a Polar Admin UDS. No SDK/domain types here.
//! Frames use the existing bounded, big-endian length-prefixed JSON codec.
use crate::{
    LocalExtensionCancelV1, LocalExtensionEventV1, LocalExtensionOfferV1, LocalExtensionRequestV1,
    LocalExtensionResponseV1,
};
use serde::{Deserialize, Serialize};

pub const POLAR_ACCESS_STREAM_VERSION: u16 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum PolarAccessClientFrame {
    Hello {
        version: u16,
        expected_node_id: String,
        extensions: Vec<LocalExtensionOfferV1>,
    },
    Request {
        request: LocalExtensionRequestV1,
    },
    Cancel {
        request: LocalExtensionCancelV1,
    },
    Ping {
        nonce: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum PolarAccessServerFrame {
    Ready {
        version: u16,
        node_id: String,
        extensions: Vec<LocalExtensionOfferV1>,
    },
    Response {
        response: LocalExtensionResponseV1,
    },
    Event {
        event: LocalExtensionEventV1,
    },
    Pong {
        nonce: u64,
    },
}

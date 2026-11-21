use serde::Serialize;

use crate::{PacketKind, PolarWireHeader, WireBodyBox};

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SignablePacket<'a> {
    pub packet_kind: PacketKind,
    pub header: &'a PolarWireHeader,
    pub body_box: &'a WireBodyBox,
}

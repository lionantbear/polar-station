#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerState {
    Unknown,
    Discovered,
    PendingInbound,
    PendingOutbound,
    TrustedOneWay,
    MutualTrusted,
    IdentityChangePending,
    RateLimited,
    Quarantined,
    Revoked,
}

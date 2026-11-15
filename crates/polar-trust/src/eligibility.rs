#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemberEligibility {
    pub allows_member_plane: bool,
    pub allows_inbound_member_plane: bool,
    pub is_mutual: bool,
}

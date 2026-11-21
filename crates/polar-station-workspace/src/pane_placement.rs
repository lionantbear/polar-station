//! Stateless admission for a native Tab. Native code owns its layout and commit.
use polar_station_protocol::SessionIdentityV1;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct PanePlacementInput {
    pub panes: Vec<PanePlacementMember>,
    pub sessions: Vec<SessionIdentityV1>,
    #[serde(default)]
    pub reserved: usize,
    #[serde(default)]
    pub unbound: usize,
}

#[derive(Deserialize)]
pub struct PanePlacementMember {
    pub id: String,
    pub session: Option<SessionIdentityV1>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct PanePlacementDecision {
    pub additions: Vec<SessionIdentityV1>,
    pub existing: Vec<String>,
    pub full: bool,
}

pub fn plan_pane_placement(input: PanePlacementInput) -> PanePlacementDecision {
    let mut decision = PanePlacementDecision {
        additions: Vec::new(),
        existing: Vec::new(),
        full: false,
    };
    for session in input.sessions {
        if let Some(pane) = input
            .panes
            .iter()
            .find(|pane| pane.session.as_ref() == Some(&session))
        {
            if !decision.existing.contains(&pane.id) {
                decision.existing.push(pane.id.clone());
            }
        } else if !decision.additions.contains(&session) {
            decision.additions.push(session);
        }
    }
    let requested = decision.additions.len().saturating_add(input.unbound);
    decision.full =
        requested > 9usize.saturating_sub(input.panes.len().saturating_add(input.reserved));
    decision
}



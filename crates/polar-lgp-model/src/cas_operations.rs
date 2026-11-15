//! Explicit recovery of operations in the current instance's bound CAS catalog.
use polar_cas::source::OperationId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use polar_cas::catalog::query::RecoveryRecord as CasRecoveryRecord;
pub use polar_cas::operation::OperationOutcome as CasOperationOutcome;
pub use polar_cas::Error as CasRecoveryError;

fn page_size() -> u32 {
    30
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CasRecoveryListInput {
    #[serde(default)]
    pub after: Option<String>,
    #[serde(default = "page_size")]
    #[schemars(range(min = 1, max = 100))]
    pub limit: u32,
}
impl CasRecoveryListInput {
    pub fn validate(&self) -> polar_cas::Result<Option<OperationId>> {
        if !(1..=100).contains(&self.limit) {
            return Err(CasRecoveryError::Invalid(
                "recovery limit must be 1..=100".into(),
            ));
        }
        self.after
            .as_ref()
            .map(|id| OperationId::new(id.clone()))
            .transpose()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CasRecoverOperationInput {
    pub operation_id: String,
}
impl CasRecoverOperationInput {
    pub fn validate(&self) -> polar_cas::Result<OperationId> {
        OperationId::new(self.operation_id.clone())
    }
}



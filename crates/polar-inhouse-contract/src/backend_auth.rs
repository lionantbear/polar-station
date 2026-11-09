//! Profile-scoped authentication. Credentials remain inside the native backend.
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum InhouseBackendAuthAction {
    /// Explicit offline import into the selected native profile, never host discovery.
    Import {
        auth_json: String,
        overwrite: bool,
    },
    BeginBrowser,
    BeginDevice,
    Read,
    Cancel {
        login_id: String,
    },
    Logout,
}

impl std::fmt::Debug for InhouseBackendAuthAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Import { overwrite, .. } => f
                .debug_struct("Import")
                .field("auth_json", &"[REDACTED]")
                .field("overwrite", overwrite)
                .finish(),
            Self::BeginBrowser => f.write_str("BeginBrowser"),
            Self::BeginDevice => f.write_str("BeginDevice"),
            Self::Read => f.write_str("Read"),
            Self::Cancel { login_id } => f
                .debug_struct("Cancel")
                .field("login_id", login_id)
                .finish(),
            Self::Logout => f.write_str("Logout"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InhouseBackendAuthInput {
    pub backend_profile_id: String,
    pub action: InhouseBackendAuthAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum InhouseBackendAuthView {
    /// Local import receipt, not evidence of a successful provider login.
    Imported {
        changed: bool,
    },
    LoginRequired {
        login_id: String,
        url: String,
        user_code: Option<String>,
    },
    Account {
        kind: Option<String>,
        email: Option<String>,
        requires_auth: bool,
    },
    Cancelled {
        found: bool,
    },
    LoggedOut,
}



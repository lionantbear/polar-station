//! Content sources transported with a Workspace; no layout or executable commands.
use crate::{validate_stable_identifier, HostRefV1, ProtocolValidationError, TerminalSourceV1};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PortableContentV1 {
    Terminal {
        source: TerminalSourceV1,
    },
    Lgp {
        node_id: String,
        owner: String,
        thread: String,
        actor: String,
    },
    /// Absolute paths refer to the source client; import never treats them as
    /// destination-local paths. A new view ID is allocated only after resolving.
    IdeaTree {
        source_workspace: String,
        view_id: String,
        paths: Vec<String>,
    },
    Settings {
        field: String,
    },
    ToolSetup {
        host: HostRefV1,
        setup_id: String,
    },
}

impl PortableContentV1 {
    pub fn host(&self) -> Option<&HostRefV1> {
        match self {
            Self::Terminal { source } => Some(&source.host),
            Self::ToolSetup { host, .. } => Some(host),
            _ => None,
        }
    }
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        match self {
            Self::Terminal { source } => source.validate(),
            Self::Lgp {
                node_id,
                owner,
                thread,
                actor,
            } => {
                for value in [node_id, owner, thread, actor] {
                    validate_stable_identifier(value, "conversation source")?;
                }
                Ok(())
            }
            Self::IdeaTree {
                source_workspace,
                view_id,
                paths,
            } => {
                validate_stable_identifier(source_workspace, "source Workspace")?;
                validate_stable_identifier(view_id, "source Idea view")?;
                if paths.len() > 256
                    || paths
                        .iter()
                        .any(|p| p.is_empty() || p.len() > 4096 || p.chars().any(char::is_control))
                {
                    return Err(ProtocolValidationError::InvalidIdentifier(
                        "Idea source path",
                    ));
                }
                Ok(())
            }
            Self::Settings { field } if matches!(field.as_str(), "cleanup" | "english") => Ok(()),
            Self::Settings { .. } => {
                Err(ProtocolValidationError::InvalidIdentifier("Settings field"))
            }
            Self::ToolSetup { host, setup_id } => {
                host.validate()?;
                validate_stable_identifier(setup_id, "setup source")
            }
        }
    }
}

use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentifierError {
    field: &'static str,
    value: String,
}

impl IdentifierError {
    fn new(field: &'static str, value: impl Into<String>) -> Self {
        Self {
            field,
            value: value.into(),
        }
    }
}

impl fmt::Display for IdentifierError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid {} identifier: {:?}",
            self.field, self.value
        )
    }
}

impl std::error::Error for IdentifierError {}

macro_rules! uuid_id {
    ($name:ident, $field:literal) => {
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(polar_identity::generation::new_uuid_v4())
            }

            pub fn parse(value: &str) -> Result<Self, IdentifierError> {
                Uuid::parse_str(value)
                    .map(Self)
                    .map_err(|_| IdentifierError::new($field, value))
            }

            pub fn as_uuid(self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

macro_rules! string_id {
    ($name:ident, $field:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, IdentifierError> {
                let value = value.into();
                if value.is_empty()
                    || value.trim() != value
                    || value.chars().any(char::is_control)
                    || value.len() > 512
                {
                    return Err(IdentifierError::new($field, value));
                }
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

uuid_id!(WorkspaceId, "workspace");
uuid_id!(WorkspaceTabId, "workspace tab");
uuid_id!(ManagedPaneId, "managed pane");
uuid_id!(ManagedSessionId, "managed session");
uuid_id!(OperationId, "operation");

string_id!(StableTmuxSessionId, "tmux session");

macro_rules! revision {
    ($name:ident) => {
        #[derive(
            Debug,
            Clone,
            Copy,
            Default,
            PartialEq,
            Eq,
            Hash,
            PartialOrd,
            Ord,
            Serialize,
            Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(u64);

        impl $name {
            pub const fn new(value: u64) -> Self {
                Self(value)
            }

            pub const fn get(self) -> u64 {
                self.0
            }

            pub const fn next(self) -> Self {
                Self(self.0.saturating_add(1))
            }
        }
    };
}

revision!(CatalogRevision);
revision!(WorkspaceRevision);

pub use polar_tmux_protocol::ExecutionTarget as ExecutionTargetId;

use crate::errors::IdentityError;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NodeName(String);

impl NodeName {
    pub fn parse(raw: &str) -> Result<Self, IdentityError> {
        let trimmed = raw.trim();
        if trimmed.is_empty()
            || trimmed.chars().count() > 64
            || trimmed.chars().any(|c| c.is_control())
        {
            return Err(IdentityError::InvalidNodeName);
        }
        Ok(Self(trimmed.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}



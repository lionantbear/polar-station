use std::collections::BTreeMap;

use crate::{
    validate_catalog, ResourceCatalog, ResourceError, ResourceErrorCode, ResourceTargetKind,
};

pub fn apply_catalog(
    current: &ResourceCatalog,
    expected_revision: u64,
    mut candidate: ResourceCatalog,
    targets: &BTreeMap<String, ResourceTargetKind>,
) -> Result<ResourceCatalog, ResourceError> {
    if expected_revision != current.revision || candidate.revision != expected_revision {
        return Err(ResourceError::new(
            ResourceErrorCode::CatalogStale,
            "The Resource catalog changed. Reload it before applying this draft.",
        ));
    }
    validate_catalog(&candidate, targets)?;
    candidate.revision = current.revision.checked_add(1).ok_or_else(|| {
        ResourceError::new(
            ResourceErrorCode::ConfigInvalid,
            "Resource catalog revision is exhausted.",
        )
    })?;
    Ok(candidate)
}

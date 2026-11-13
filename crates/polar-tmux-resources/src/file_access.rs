//! Resolve a source path through existing Resource mappings; no transport/cache.
use crate::{ResourceCatalog, ResourceId, ResourceRuntimeState, ResourceRuntimeStatus};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
};

/// Pure access planning for an already resolved source file. Only the Host
/// supplies locality; frontend opt-in flags do not determine machine identity.
pub fn plan_file_access(
    file: &crate::ResolvedFile,
    catalog: &ResourceCatalog,
    statuses: &BTreeMap<ResourceId, ResourceRuntimeStatus>,
    source_is_desktop_local: bool,
) -> Result<crate::FileAccessPlan, String> {
    use crate::FileAccessPlan;
    if file.source.flavor != crate::PathFlavor::Posix {
        return Err("File access on a Windows source is not supported by this Host.".into());
    }
    if source_is_desktop_local {
        return Ok(FileAccessPlan::NativeLocal {
            local_path: file.reference.path.clone(),
        });
    }
    match resolve_file(
        catalog,
        statuses,
        &file.source.origin.execution_target_id,
        &file.reference.path,
        None,
    )? {
        FileAccess::Ready {
            resource_id,
            local_path,
        } => Ok(FileAccessPlan::MappedReady {
            resource_id,
            local_path,
        }),
        FileAccess::StartResource { resource_id } => {
            if let Some(status) = statuses
                .get(&resource_id)
                .filter(|s| s.state == ResourceRuntimeState::NeedsAttention)
            {
                return Err(status.message.clone());
            }
            Ok(FileAccessPlan::StartExistingMapping { resource_id })
        }
        FileAccess::CreateMapping {
            execution_target_id,
            remote_directory,
            source_path,
        } => Ok(FileAccessPlan::NeedMapping {
            execution_target_id,
            remote_directory,
            source_path,
        }),
        _ => Err("Unexpected file access plan".into()),
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum FileAccess {
    Verified {
        reference: crate::FileReference,
        access: Box<FileAccess>,
    },
    ChooseFile {
        origin: crate::FileOrigin,
        candidates: Vec<crate::FileReference>,
    },
    LocalFile {
        local_path: String,
    },
    Ready {
        resource_id: ResourceId,
        local_path: String,
    },
    StartResource {
        resource_id: ResourceId,
    },
    CreateMapping {
        execution_target_id: String,
        remote_directory: String,
        source_path: String,
    },
}
fn absolute(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("The source working directory is unavailable".into());
    }
    let mut output = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                if !output.pop() {
                    return Err("Invalid source path".into());
                }
            }
            Component::CurDir => {}
            other => output.push(other.as_os_str()),
        }
    }
    Ok(output)
}
/// Resolve a reference in its actual source session. Harness adapters supply
/// their own verified cwd; this function has no harness or transport policy.
pub fn resolve_reference_path(
    reference: &crate::FileReference,
    session_cwd: Option<&str>,
) -> Result<PathBuf, String> {
    if reference.path.is_empty() || reference.path.chars().any(char::is_control) {
        return Err("Invalid source file reference".into());
    }
    let path = Path::new(&reference.path);
    if path.is_absolute() {
        absolute(path)
    } else {
        absolute(
            &Path::new(session_cwd.ok_or("The source session working directory is unavailable")?)
                .join(path),
        )
    }
}

pub fn resolve_file(
    catalog: &ResourceCatalog,
    statuses: &BTreeMap<ResourceId, ResourceRuntimeStatus>,
    target: &str,
    path: &str,
    cwd: Option<&str>,
) -> Result<FileAccess, String> {
    if target.is_empty()
        || path.is_empty()
        || path.len() > 16384
        || path.chars().any(char::is_control)
    {
        return Err("Invalid source file reference".into());
    }
    let reference = crate::FileReference {
        path: path.to_owned(),
        location: None,
    };
    let source = resolve_reference_path(&reference, cwd)?;
    let mapping = catalog
        .file_mappings
        .iter()
        .filter(|m| m.enabled && m.execution_target_id == target)
        .filter_map(|m| {
            absolute(Path::new(&m.remote_path))
                .ok()
                .map(|root| (m, root))
        })
        .filter(|(_, root)| source.strip_prefix(root).is_ok())
        .max_by_key(|(_, root)| root.components().count());
    let Some((mapping, root)) = mapping else {
        return Ok(FileAccess::CreateMapping {
            execution_target_id: target.into(),
            remote_directory: source
                .parent()
                .ok_or("Invalid source file")?
                .to_string_lossy()
                .into(),
            source_path: source.to_string_lossy().into(),
        });
    };
    if statuses
        .get(&mapping.id)
        .is_none_or(|s| s.state != ResourceRuntimeState::Running)
    {
        return Ok(FileAccess::StartResource {
            resource_id: mapping.id,
        });
    }
    let tail = source
        .strip_prefix(root)
        .map_err(|_| "Source mapping changed")?;
    if tail.as_os_str().is_empty() {
        return Err("Select a file, not the mapped directory".into());
    }
    Ok(FileAccess::Ready {
        resource_id: mapping.id,
        local_path: Path::new(&mapping.local_path)
            .join(tail)
            .to_string_lossy()
            .into(),
    })
}


/// Only local filesystem references are intercepted; web URLs keep native routing.
pub fn image_path(value: &str) -> Option<String> {
    let reference = crate::parse_file_reference(value)?;
    if reference.location.is_some() {
        return None;
    }
    let path = reference.path;
    let extension = std::path::Path::new(&path)
        .extension()?
        .to_str()?
        .to_ascii_lowercase();
    if !matches!(
        extension.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff" | "heic" | "avif" | "svg"
    ) || path.contains('\0')
    {
        return None;
    }
    Some(path)
}


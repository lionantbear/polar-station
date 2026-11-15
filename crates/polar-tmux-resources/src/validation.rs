use std::collections::{BTreeMap, BTreeSet};

use crate::{
    AndroidMirrorSpec, FileMappingSpec, PortForwardSpec, ResourceCatalog, ResourceError,
    ResourceErrorCode, ResourceId, ResourceTargetKind, RESOURCE_CATALOG_SCHEMA_VERSION,
};

pub fn validate_catalog(
    catalog: &ResourceCatalog,
    targets: &BTreeMap<String, ResourceTargetKind>,
) -> Result<(), ResourceError> {
    if catalog.schema_version != RESOURCE_CATALOG_SCHEMA_VERSION {
        return Err(invalid("Resource catalog schema is unsupported."));
    }

    let mut ids = BTreeSet::<ResourceId>::new();
    let mut names = BTreeSet::<String>::new();
    let mut mounts = BTreeSet::<String>::new();
    let mut binds = BTreeSet::<(String, u16)>::new();

    for mapping in &catalog.file_mappings {
        validate_common(
            mapping.id,
            &mapping.name,
            &mapping.execution_target_id,
            targets,
            &mut ids,
            &mut names,
        )?;
        validate_file_mapping(mapping)?;
        if mapping.enabled && !mounts.insert(normalize_path(&mapping.local_path)) {
            return Err(ResourceError::new(
                ResourceErrorCode::MountConflict,
                "Enabled File Mappings cannot use the same local mount path.",
            ));
        }
    }

    for forward in &catalog.port_forwards {
        validate_common(
            forward.id,
            &forward.name,
            &forward.execution_target_id,
            targets,
            &mut ids,
            &mut names,
        )?;
        validate_port_forward(forward)?;
        if forward.enabled {
            for port in forward.local_port_start..=forward.local_port_end {
                if !binds.insert((normalize_bind(&forward.bind_address), port)) {
                    return Err(ResourceError::new(
                        ResourceErrorCode::BindConflict,
                        "Enabled Resources cannot bind the same local address and port.",
                    ));
                }
            }
        }
    }

    for mirror in &catalog.android_mirrors {
        validate_common(
            mirror.id,
            &mirror.name,
            &mirror.execution_target_id,
            targets,
            &mut ids,
            &mut names,
        )?;
        validate_android(mirror)?;
        if mirror.enabled
            && !binds.insert((normalize_bind(&mirror.bind_address), mirror.local_adb_port))
        {
            return Err(ResourceError::new(
                ResourceErrorCode::BindConflict,
                "Android ADB and Port Forward Resources cannot share a local bind.",
            ));
        }
    }
    Ok(())
}

fn validate_common(
    id: ResourceId,
    name: &str,
    target_id: &str,
    targets: &BTreeMap<String, ResourceTargetKind>,
    ids: &mut BTreeSet<ResourceId>,
    names: &mut BTreeSet<String>,
) -> Result<(), ResourceError> {
    validate_label(name, "Resource name")?;
    validate_token(target_id, "execution target ID")?;
    if !ids.insert(id) {
        return Err(invalid("Resource IDs must be unique."));
    }
    if !names.insert(name.to_lowercase()) {
        return Err(invalid("Resource names must be unique."));
    }
    match targets.get(target_id) {
        None => Err(ResourceError::new(
            ResourceErrorCode::TargetNotFound,
            "The selected execution target is not configured.",
        )),
        Some(ResourceTargetKind::Local) => Err(ResourceError::new(
            ResourceErrorCode::TargetNotSsh,
            "Resources require a registered SSH execution target.",
        )),
        Some(ResourceTargetKind::Ssh) => Ok(()),
    }
}

fn validate_file_mapping(mapping: &FileMappingSpec) -> Result<(), ResourceError> {
    validate_path(&mapping.remote_path, "Remote path")?;
    if !mapping.remote_path.starts_with('/') {
        return Err(invalid("Remote File Mapping path must be absolute."));
    }
    validate_path(&mapping.local_path, "Local path")?;
    if !(mapping.local_path.starts_with('/') || mapping.local_path.starts_with("~/")) {
        return Err(invalid(
            "Local File Mapping path must be absolute or explicitly home-relative.",
        ));
    }
    Ok(())
}

fn validate_port_forward(forward: &PortForwardSpec) -> Result<(), ResourceError> {
    validate_bind(
        &forward.bind_address,
        forward.allow_non_loopback,
        "Port Forward bind address",
    )?;
    validate_host(&forward.remote_host, "remote host")?;
    if forward.local_port_start == 0
        || forward.local_port_end == 0
        || forward.remote_port_start == 0
        || forward.local_port_end < forward.local_port_start
    {
        return Err(invalid("Port Forward range is invalid."));
    }
    let translated_end = u32::from(forward.remote_port_start)
        + u32::from(forward.local_port_end - forward.local_port_start);
    if translated_end > u32::from(u16::MAX) {
        return Err(invalid(
            "Port Forward translated remote range exceeds port 65535.",
        ));
    }
    Ok(())
}

fn validate_android(mirror: &AndroidMirrorSpec) -> Result<(), ResourceError> {
    validate_bind(
        &mirror.bind_address,
        mirror.allow_non_loopback,
        "Android ADB bind address",
    )?;
    validate_host(&mirror.remote_adb_host, "remote ADB host")?;
    if mirror.local_adb_port == 0 || mirror.remote_adb_port == 0 {
        return Err(invalid("Android ADB ports must be in 1 through 65535."));
    }
    if mirror.scrcpy_arguments.len() > 64 {
        return Err(invalid("Android scrcpy argument count exceeds 64."));
    }
    for argument in &mirror.scrcpy_arguments {
        if argument.is_empty()
            || argument.len() > 512
            || argument.chars().any(char::is_control)
            || matches!(
                argument.as_str(),
                "-s" | "--serial" | "--tcpip" | "--otg" | "--record"
            )
            || argument.starts_with("--serial=")
            || argument.starts_with("--tcpip=")
            || argument.starts_with("--record=")
        {
            return Err(invalid(
                "Android scrcpy arguments contain an Engine-owned or unsafe option.",
            ));
        }
    }
    Ok(())
}

fn validate_bind(address: &str, explicit: bool, label: &str) -> Result<(), ResourceError> {
    validate_host(address, label)?;
    let loopback = matches!(normalize_bind(address).as_str(), "127.0.0.1" | "::1");
    if !loopback && !explicit {
        return Err(invalid(
            "A non-loopback Resource bind requires explicit exposure approval.",
        ));
    }
    Ok(())
}

fn validate_label(value: &str, label: &str) -> Result<(), ResourceError> {
    if value.is_empty()
        || value.len() > 128
        || value != value.trim()
        || value.chars().any(char::is_control)
    {
        return Err(invalid(format!("{label} is invalid.")));
    }
    Ok(())
}

fn validate_token(value: &str, label: &str) -> Result<(), ResourceError> {
    if value.is_empty()
        || value.len() > 255
        || value.starts_with('-')
        || value.chars().any(|character| {
            character.is_control() || character.is_whitespace() || matches!(character, '\'' | '"')
        })
    {
        return Err(invalid(format!("{label} is invalid.")));
    }
    Ok(())
}

fn validate_path(value: &str, label: &str) -> Result<(), ResourceError> {
    if value.is_empty()
        || value.len() > 4096
        || value.chars().any(char::is_control)
        || value.split('/').any(|part| part == "..")
    {
        return Err(invalid(format!("{label} is invalid.")));
    }
    Ok(())
}

fn validate_host(value: &str, label: &str) -> Result<(), ResourceError> {
    validate_token(value, label)
}

fn normalize_bind(value: &str) -> String {
    match value.to_ascii_lowercase().as_str() {
        "localhost" => "127.0.0.1".to_owned(),
        other => other.to_owned(),
    }
}

fn normalize_path(value: &str) -> String {
    value.trim_end_matches('/').to_owned()
}

fn invalid(message: impl Into<String>) -> ResourceError {
    ResourceError::new(ResourceErrorCode::ConfigInvalid, message)
}

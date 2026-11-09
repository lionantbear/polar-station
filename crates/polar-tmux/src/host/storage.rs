use anyhow::{Context, Result};
use polar_runtime_paths::{RuntimeArea, RuntimeComponent, RuntimeModule, RuntimeRoot};
use polar_station_store_sqlite::HostDatabase;
use std::path::PathBuf;

pub(crate) fn host_database(root: Option<PathBuf>, host_id: &str) -> Result<HostDatabase> {
    tracing::info!(target: "polar_station_operation", component="Storage", event="storage_prepare_started");
    let root = match root {
        Some(root) => root,
        None => platform_runtime_root()?,
    };
    tracing::info!(target: "polar_station_operation", component="Storage", event="storage_root_selected");
    let root = if root.join("layout.json").exists() {
        RuntimeRoot::open(root)?
    } else {
        // initialize rejects nonempty legacy directories; never import old files.
        RuntimeRoot::initialize(root, "polar-station")?
    };
    tracing::info!(target: "polar_station_operation", component="Storage", event="storage_layout_ready");
    let component = root.component(RuntimeComponent::PolarTmux, host_id)?;
    tracing::info!(target: "polar_station_operation", component="Storage", event="storage_component_ready");
    let data = component.directory(RuntimeModule::Station, RuntimeArea::Data)?;
    tracing::info!(target: "polar_station_operation", component="Storage", event="storage_directory_ready");
    let database = HostDatabase::new(data.path().join("host.db"));
    database.runtime()?;
    tracing::info!(target: "polar_station_operation", component="Storage", event="storage_runtime_ready");
    Ok(database)
}

fn platform_runtime_root() -> Result<PathBuf> {
    #[cfg(target_os = "linux")]
    if let Some(path) = std::env::var_os("XDG_DATA_HOME").filter(|p| !p.is_empty()) {
        let path = PathBuf::from(path);
        anyhow::ensure!(path.is_absolute(), "XDG_DATA_HOME must be absolute");
        return Ok(path.join("polar-runtime"));
    }
    #[cfg(target_os = "windows")]
    {
        return Ok(PathBuf::from(
            std::env::var_os("LOCALAPPDATA").context("LOCALAPPDATA unavailable")?,
        )
        .join("PolarRuntime"));
    }
    #[cfg(not(target_os = "windows"))]
    {
        let home = PathBuf::from(std::env::var_os("HOME").context("HOME unavailable")?);
        #[cfg(target_os = "macos")]
        return Ok(home.join("Library/Application Support/PolarRuntime"));
        #[cfg(not(target_os = "macos"))]
        Ok(home.join(".local/share/polar-runtime"))
    }
}



//! Local trusted graphics packages. Archive integrity reuses polar-package-tree.
//! No executable sandbox or plugin-tool runtime is provided by this crate.
use anyhow::{bail, Context, Result};
use polar_package_tree::{extract_zip_file, validate_directory, ArchiveLimits, FileDigest};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub const API_VERSION: u32 = 1;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub id: String,
    pub title: String,
    pub entry: String,
    #[serde(default)]
    pub modes: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: String,
    pub id: String,
    pub version: String,
    pub title: String,
    pub api_version: u32,
    pub godot_version: String,
    #[serde(default)]
    pub browsers: Vec<Entry>,
    #[serde(default)]
    pub themes: Vec<Entry>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Installed {
    pub manifest: Manifest,
    pub digest: String,
    pub archive: PathBuf,
    pub archive_digest: String,
}
fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
}
impl Manifest {
    pub fn prefix(&self) -> String {
        format!("view_packages/{}/{}/", self.id, self.version)
    }
    pub fn validate(&self) -> Result<()> {
        if self.schema != "polar.graphics.view-package.v1" || self.api_version != API_VERSION {
            bail!("unsupported graphics package interface");
        }
        if !identifier(&self.id) || !identifier(&self.version) || self.title.trim().is_empty() {
            bail!("invalid graphics package identity");
        }
        if self.godot_version != "4.7.2" {
            bail!("package requires a different Godot runtime");
        }
        if self.browsers.is_empty() && self.themes.is_empty() {
            bail!("package has no browser or theme");
        }
        for entries in [&self.browsers, &self.themes] {
            let mut ids = std::collections::HashSet::new();
            for entry in entries {
                if !identifier(&entry.id) || entry.title.trim().is_empty() || !ids.insert(&entry.id)
                {
                    bail!("invalid or duplicate package entry");
                }
                if !entry.entry.starts_with(&self.prefix())
                    || entry
                        .entry
                        .split('/')
                        .any(|part| part == ".." || part == ".")
                    || entry.entry.contains('\\')
                {
                    bail!("entry escapes package resource namespace");
                }
            }
        }
        Ok(())
    }
}
pub fn inspect(directory: &Path) -> Result<(Manifest, String)> {
    let tree = validate_directory(directory)?;
    let manifest: Manifest = serde_json::from_slice(&fs::read(directory.join("manifest.json"))?)?;
    manifest.validate()?;
    for entry in &tree.entries {
        if entry.path != "manifest.json" && !entry.path.starts_with(&manifest.prefix()) {
            bail!("resource outside package namespace: {}", entry.path);
        }
        if ["so", "dylib", "dll", "gdextension"].contains(
            &Path::new(&entry.path)
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or(""),
        ) {
            bail!("native libraries are not supported in view packages");
        }
    }
    for entry in manifest.browsers.iter().chain(&manifest.themes) {
        if !directory.join(&entry.entry).is_file() {
            bail!("package entry is missing: {}", entry.entry);
        }
    }
    Ok((manifest, tree.digest.as_str().to_owned()))
}
pub fn install(archive: &Path, root: &Path) -> Result<Installed> {
    fs::create_dir_all(root)?;
    let staging = tempfile::Builder::new()
        .prefix(".view-install-")
        .tempdir_in(root)?;
    let tree = staging.path().join("tree");
    let stored_archive = staging.path().join("package.zip");
    fs::copy(archive, &stored_archive)?;
    extract_zip_file(&stored_archive, &tree, ArchiveLimits::default())?;
    let (manifest, digest) = inspect(&tree)?;
    let destination = root.join(&manifest.id).join(&manifest.version);
    if destination.exists() {
        let installed = read_installed(&destination)?;
        if installed.digest != digest {
            bail!("version already installed with different contents; publish a new version");
        }
        return Ok(installed);
    }
    fs::create_dir_all(destination.parent().context("installation parent")?)?;
    let archive_digest = FileDigest::for_file(&stored_archive)?.as_str().to_owned();
    let installed = Installed {
        manifest,
        digest,
        archive: destination.join("package.zip"),
        archive_digest,
    };
    fs::write(
        staging.path().join("receipt.json"),
        serde_json::to_vec_pretty(&installed)?,
    )?;
    fs::rename(staging.path(), &destination)?;
    Ok(installed)
}
pub fn list(root: &Path) -> Result<Vec<Installed>> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut result = Vec::new();
    for package in fs::read_dir(root)? {
        let package = package?;
        if !package.file_type()?.is_dir() || package.file_name().to_string_lossy().starts_with('.')
        {
            continue;
        }
        for version in fs::read_dir(package.path())? {
            let receipt = version?.path().join("receipt.json");
            if !receipt.is_file() {
                continue;
            }
            let installed = read_installed(receipt.parent().context("receipt parent")?)?;
            result.push(installed);
        }
    }
    result.sort_by(|a, b| {
        (&a.manifest.id, &a.manifest.version).cmp(&(&b.manifest.id, &b.manifest.version))
    });
    Ok(result)
}
fn read_installed(directory: &Path) -> Result<Installed> {
    let mut installed: Installed =
        serde_json::from_slice(&fs::read(directory.join("receipt.json"))?)?;
    installed.manifest.validate()?;
    if directory.file_name().and_then(|p| p.to_str()) != Some(&installed.manifest.version)
        || directory
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|p| p.to_str())
            != Some(&installed.manifest.id)
    {
        bail!("receipt identity does not match installation");
    }
    installed.archive = directory.join("package.zip");
    if FileDigest::for_file(&installed.archive)?.as_str() != installed.archive_digest {
        bail!("installed archive integrity check failed");
    }
    Ok(installed)
}
fn directory(root: &Path, id: &str, version: &str) -> Result<PathBuf> {
    if !identifier(id) || !identifier(version) {
        bail!("invalid package identity");
    }
    Ok(root.join(id).join(version))
}
pub fn export(root: &Path, id: &str, version: &str, destination: &Path) -> Result<()> {
    let source = read_installed(&directory(root, id, version)?)?.archive;
    if destination.exists() {
        bail!("export destination already exists");
    }
    fs::copy(source, destination)?;
    Ok(())
}
pub fn uninstall(root: &Path, id: &str, version: &str) -> Result<()> {
    fs::remove_dir_all(directory(root, id, version)?)?;
    Ok(())
}



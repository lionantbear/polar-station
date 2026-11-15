//! Bounded registration coverage check for an explicitly authorized local scope.
//! No registration, payload hashing, root creation, or traversal of other tenants.
use cap_std::fs::Dir;
use polar_cas::{
    catalog::backup::BackupInventory, source::LocationId, CasNamespace, Error, ObjectId, Result,
};
use std::{fs::File, path::Path};

/// A shared-content layout has no namespace in physical keys. Only its owner
/// may grant an exclusive root for enumeration. This enum is not authorization.
#[derive(Clone, Debug)]
pub enum ManagedCoverageScope {
    Namespace(CasNamespace),
    ExclusiveContentRoot,
}
#[derive(Clone, Copy, Debug)]
pub struct CoverageAuditLimits {
    pub max_entries: u64,
    pub max_issue_samples: usize,
}
impl Default for CoverageAuditLimits {
    fn default() -> Self {
        Self {
            max_entries: 1_000_000,
            max_issue_samples: 32,
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagedCoverageIssue {
    pub key: String,
    pub reason: &'static str,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagedCoverageReport {
    pub catalog_snapshot: ObjectId,
    pub location: LocationId,
    pub scope: String,
    pub entries_examined: u64,
    pub files_examined: u64,
    pub registered_files: u64,
    pub unknown_entries: u64,
    pub issues: Vec<ManagedCoverageIssue>,
}
impl ManagedCoverageReport {
    /// Only extra-file registration coverage, not object integrity, retention,
    /// absence of missing catalog replicas, or completed-backup verification.
    pub fn require_registered_coverage(&self) -> Result<()> {
        if self.unknown_entries != 0 {
            return Err(Error::Invalid(
                "backup coverage unknown: unregistered or unsupported local entries".into(),
            ));
        }
        Ok(())
    }
}

pub fn audit_managed_coverage(
    root: &Dir,
    scope: &ManagedCoverageScope,
    location: &LocationId,
    inventory: &dyn BackupInventory,
    limits: CoverageAuditLimits,
) -> Result<ManagedCoverageReport> {
    if limits.max_entries == 0 || limits.max_issue_samples > 1024 {
        return Err(Error::Invalid(
            "bounded coverage audit limits required".into(),
        ));
    }
    // A typo or disabled location is not an empty, successfully covered store.
    if !inventory
        .locations(polar_cas::backup_v2::MAX_LOCATIONS)?
        .iter()
        .any(|l| &l.spec.id == location && l.enabled)
    {
        return Err(Error::Invalid(
            "coverage audit requires an enabled catalog location".into(),
        ));
    }
    let (dir, prefix, legacy) = match scope {
        ManagedCoverageScope::Namespace(ns) => {
            let prefix = format!("{}/", ns.as_str());
            match open_directory(root, Path::new(ns.as_str())) {
                Ok(dir) => (dir, prefix, true),
                Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                    // Lazy namespaces need not exist until their first payload.
                    // Do not mistake a lost registered replica/source for this case.
                    require_unplaced_namespace(inventory, location, limits.max_entries)?;
                    return Ok(ManagedCoverageReport {
                        catalog_snapshot: inventory.snapshot().database.id.clone(),
                        location: location.clone(),
                        scope: prefix,
                        entries_examined: 0,
                        files_examined: 0,
                        registered_files: 0,
                        unknown_entries: 0,
                        issues: Vec::new(),
                    });
                }
                Err(error) => return Err(error),
            }
        }
        ManagedCoverageScope::ExclusiveContentRoot => (root.try_clone()?, String::new(), false),
    };
    let mut audit = Audit {
        inventory,
        location,
        limits,
        legacy,
        root: &dir,
        report: ManagedCoverageReport {
            catalog_snapshot: inventory.snapshot().database.id.clone(),
            location: location.clone(),
            scope: if legacy {
                prefix.clone()
            } else {
                "exclusive-content-root".into()
            },
            entries_examined: 0,
            files_examined: 0,
            registered_files: 0,
            unknown_entries: 0,
            issues: Vec::new(),
        },
    };
    audit.visit(&dir, &prefix, "", 0)?;
    Ok(audit.report)
}

// This check uses the same frozen inventory as the backup planner. It neither
// creates the namespace nor weakens the subsequent copy/integrity checks.
fn require_unplaced_namespace(
    inventory: &dyn BackupInventory,
    location: &LocationId,
    max_entries: u64,
) -> Result<()> {
    use polar_cas::backup_v2::{MAX_LOCATIONS, MAX_PAGE_SIZE};
    if inventory
        .locations(MAX_LOCATIONS)?
        .iter()
        .any(|entry| &entry.spec.id == location && entry.unbound_sources != 0)
    {
        return Err(Error::Invalid(
            "absent namespace has unbound sources".into(),
        ));
    }
    let mut after = None;
    let mut examined = 0_u64;
    loop {
        let page = inventory.objects_page(after.as_ref(), MAX_PAGE_SIZE)?;
        examined = examined
            .checked_add(page.objects.len() as u64)
            .ok_or(Error::TooLarge)?;
        if examined > max_entries {
            return Err(Error::TooLarge);
        }
        for object in &page.objects {
            if inventory
                .placements(&object.content, MAX_LOCATIONS)?
                .iter()
                .any(|placement| &placement.location == location)
            {
                return Err(Error::Invalid(
                    "absent namespace has registered placements".into(),
                ));
            }
        }
        match page.next {
            None => return Ok(()),
            Some(next) => {
                if page.objects.is_empty()
                    || next.snapshot_id != inventory.snapshot().database.id
                    || after
                        .as_ref()
                        .is_some_and(|old: &polar_cas::backup_v2::BackupCursor| {
                            next.after_key <= old.after_key
                        })
                {
                    return Err(Error::Invalid("invalid coverage inventory cursor".into()));
                }
                after = Some(next);
            }
        }
    }
}

struct Audit<'a> {
    root: &'a Dir,
    inventory: &'a dyn BackupInventory,
    location: &'a LocationId,
    limits: CoverageAuditLimits,
    legacy: bool,
    report: ManagedCoverageReport,
}
impl Audit<'_> {
    fn issue(&mut self, key: &str, reason: &'static str) {
        self.report.unknown_entries += 1;
        if self.report.issues.len() < self.limits.max_issue_samples {
            self.report.issues.push(ManagedCoverageIssue {
                key: key.into(),
                reason,
            });
        }
    }
    fn visit(&mut self, dir: &Dir, prefix: &str, relative: &str, depth: u8) -> Result<()> {
        for entry in dir.entries()? {
            let entry = entry?;
            self.report.entries_examined = self
                .report
                .entries_examined
                .checked_add(1)
                .ok_or(Error::TooLarge)?;
            if self.report.entries_examined > self.limits.max_entries {
                return Err(Error::TooLarge);
            }
            let name = entry.file_name();
            let Some(name_str) = name.to_str() else {
                self.issue(&format!("{prefix}{relative}<non-utf8>"), "non_utf8_name");
                continue;
            };
            let rel = format!("{relative}{name_str}");
            let key = format!("{prefix}{rel}");
            let metadata = dir.symlink_metadata(&name)?;
            if metadata.file_type().is_symlink() {
                self.issue(&key, "symlink_refused");
                continue;
            }
            // Rebuildable shared physical indexes are metadata, not an
            // unregistered payload. Never follow a symlink at this boundary.
            if depth == 0
                && !self.legacy
                && matches!(
                    name_str,
                    ".polar-pack-index-v1"
                        | ".polar-catalog-index-v2"
                        | ".polar-index-artifacts-v1"
                )
                && metadata.is_dir()
            {
                if name_str == ".polar-catalog-index-v2"
                    && catalog_has_selected_sources(self.root)?
                    && !shared_head_covered(self.root, self.location, self.inventory)?
                {
                    self.issue(&key, "shared_catalog_selection_requires_backup");
                }
                continue;
            }
            if depth < 2 {
                let valid = if depth == 0 {
                    name_str == "sha256" || (!self.legacy && name_str == "blake3")
                } else {
                    name_str.len() == 2 && name_str.bytes().all(is_hex)
                };
                if !valid || !metadata.is_dir() {
                    self.issue(&key, "unsupported_managed_layout");
                    continue;
                }
                let child = open_directory(dir, Path::new(&name))?;
                self.visit(&child, prefix, &format!("{rel}/"), depth + 1)?;
            } else {
                if !metadata.is_file() {
                    self.issue(&key, "non_regular_payload");
                    continue;
                }
                self.report.files_examined += 1;
                let id = match ObjectId::from_content_key(&rel) {
                    Ok(id) => id,
                    Err(_) => {
                        self.issue(&key, "noncanonical_content_key");
                        continue;
                    }
                };
                if !self.legacy && crate::LocalCas::index_artifact_in(self.root, &id)? {
                    continue;
                }
                if self
                    .inventory
                    .registered_file(self.location, &key, &id, metadata.len())?
                {
                    self.report.registered_files += 1;
                } else {
                    self.issue(&key, "missing_verified_catalog_registration");
                }
            }
        }
        Ok(())
    }
}
// Only inspect bounded Catalog metadata, not payload. A local-only projection
// can be rebuilt from admission records; selected remote coverage is group state
// absent from the legacy SQL payload inventory and must not be silently omitted.
fn shared_head_covered(
    root: &Dir,
    location: &LocationId,
    inventory: &dyn BackupInventory,
) -> Result<bool> {
    use std::io::Read;
    let Some(shared) = inventory.shared_catalog() else {
        return Ok(false);
    };
    if &shared.source_location != location {
        return Ok(false);
    }
    shared.validate(1_000_000, u64::MAX)?;
    let (pointer, _) = crate::admission::files::open_read_file(
        root,
        Path::new(".polar-catalog-index-v2/current"),
    )?;
    let mut bytes = Vec::new();
    pointer.take(1025).read_to_end(&mut bytes)?;
    if bytes.len() > 1024 {
        return Err(Error::TooLarge);
    }
    let reference = crate::catalog_index::decode_current(&bytes)?;
    Ok(reference == shared.root)
}

fn catalog_has_selected_sources(root: &Dir) -> Result<bool> {
    use std::io::Read;
    let (pointer, _) = match crate::admission::files::open_read_file(
        root,
        Path::new(".polar-catalog-index-v2/current"),
    ) {
        Ok(file) => file,
        Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    let mut bytes = Vec::new();
    pointer.take(1025).read_to_end(&mut bytes)?;
    if bytes.len() > 1024 {
        return Err(Error::TooLarge);
    }
    let reference = crate::catalog_index::decode_current(&bytes)?;
    if reference.size_bytes > 4 << 20 {
        return Err(Error::TooLarge);
    }
    let (file, _) =
        crate::admission::files::open_read_file(root, Path::new(&reference.id.content_key()))?;
    bytes.clear();
    file.take(reference.size_bytes + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 != reference.size_bytes || ObjectId::for_bytes(&bytes) != reference.id {
        return Err(Error::Integrity);
    }
    let directory = crate::shared_index::partitions::Root::decode(&bytes)?;
    let metadata = &directory.metadata;
    if !matches!(
        metadata["schema"].as_str(),
        Some(
            "polar.cas.catalog-index.v2"
                | "polar.cas.catalog-index.v3"
                | "polar.cas.catalog-index.v4"
                | "polar.cas.catalog-index.v5"
        )
    ) {
        return Err(Error::Integrity);
    }
    let has_projections = match metadata.get("projections") {
        Some(projections) => !projections.as_object().ok_or(Error::Integrity)?.is_empty(),
        None if matches!(
            metadata["schema"].as_str(),
            Some("polar.cas.catalog-index.v4" | "polar.cas.catalog-index.v5")
        ) =>
        {
            return Err(Error::Integrity);
        }
        None => false,
    };
    let writer_records = match metadata.get("writer_records") {
        Some(count) => count.as_u64().ok_or(Error::Integrity)?,
        None if metadata["schema"] == "polar.cas.catalog-index.v5" => return Err(Error::Integrity),
        None => 0,
    };
    Ok(writer_records > 0
        || has_projections
        || !metadata["sources"]
            .as_array()
            .ok_or(Error::Integrity)?
            .is_empty())
}

fn is_hex(b: u8) -> bool {
    b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
}
pub(crate) fn open_directory(root: &Dir, name: &Path) -> Result<Dir> {
    // NOFOLLOW also closes the lstat -> open race; traversal uses capability FDs.
    let fd = rustix::fs::openat(
        root,
        name,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::DIRECTORY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .map_err(std::io::Error::from)?;
    Ok(Dir::from_std_file(File::from(fd)))
}

/// Path façade for an already-authorized canonical managed root; never creates
/// roots or follows aliases. The Dir capability type stays inside this adapter.
pub fn audit_managed_path(
    root: &Path,
    scope: &ManagedCoverageScope,
    location: &LocationId,
    inventory: &dyn BackupInventory,
    limits: CoverageAuditLimits,
) -> Result<ManagedCoverageReport> {
    if !root.is_absolute() || root.canonicalize()? != root || !root.is_dir() {
        return Err(Error::Invalid(
            "canonical existing managed root required".into(),
        ));
    }
    let dir = Dir::open_ambient_dir(root, cap_std::ambient_authority())?;
    audit_managed_coverage(&dir, scope, location, inventory, limits)
}

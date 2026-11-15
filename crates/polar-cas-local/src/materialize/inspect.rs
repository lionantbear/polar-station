use super::{
    filesystem::{self, Target},
    LocalTreePublisher, LocalTreeRequest, NativeProjection,
};
use cap_std::fs::MetadataExt as _;
use polar_cas::{
    canonical::canonical_json_bytes,
    projection::{ProjectionNode, TreeProjectionRecord, LOCAL_TREE_SCHEMA},
    Error, Result,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    os::unix::fs::MetadataExt as _,
    path::Path,
};

impl LocalTreePublisher {
    pub(super) fn inspect(
        &self,
        request: &LocalTreeRequest,
        target: &Target,
    ) -> Result<Option<NativeProjection>> {
        self.inspect_with(request, target, true, |path, content, executable, method| {
            request.file(content, executable)?.inspect_placement(path, content, request.profile.owner, method)
        })
    }

    pub(super) fn inspect_with(
        &self, request: &LocalTreeRequest, target: &Target, durable: bool,
        mut verify: impl FnMut(&Path, &polar_cas::ContentRef, bool, polar_cas::projection::FilePlacementMethod) -> Result<()>,
    ) -> Result<Option<NativeProjection>> {
        let metadata = match target.parent.symlink_metadata(&target.name) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if !metadata.is_dir() {
            return Err(Error::Conflict);
        }
        let bundle = filesystem::open_directory(&target.parent, Path::new(&target.name))?;
        filesystem::directory_mode(&bundle, request.profile.owner, 0o555)?;
        let mut names = BTreeSet::new();
        for entry in bundle.entries()? {
            let name = entry?
                .file_name()
                .into_string()
                .map_err(|_| Error::Integrity)?;
            if !matches!(name.as_str(), "data" | "receipt.json") || !names.insert(name) {
                return Err(Error::Integrity);
            }
        }
        if names.len() != 2 {
            return Err(Error::Integrity);
        }
        let (_, _, mut file) =
            crate::admission::files::open_relative(&bundle, Path::new("receipt.json"))?;
        let metadata = file.metadata()?;
        let max_bytes = request.tree.limits().max_bundle_bytes;
        if metadata.len() > max_bytes as u64
            || metadata.mode() & 0o7777 != 0o444
            || metadata.uid() != request.profile.owner.uid
            || metadata.gid() != request.profile.owner.gid
        {
            return Err(Error::Integrity);
        }
        let mut bytes = Vec::new();
        file.by_ref()
            .take(max_bytes.checked_add(1).ok_or(Error::TooLarge)? as u64)
            .read_to_end(&mut bytes)?;
        if bytes.len() > max_bytes {
            return Err(Error::TooLarge);
        }
        let record: TreeProjectionRecord =
            serde_json::from_slice(&bytes).map_err(|_| Error::Integrity)?;
        if canonical_json_bytes(&record).map_err(|_| Error::Integrity)? != bytes {
            return Err(Error::Integrity);
        }
        if record.schema != LOCAL_TREE_SCHEMA
            || record.input != *request.tree.input()
            || record.manifest != *request.tree.output().root()
            || record.layout != *request.tree.layout()
            || record.profile != request.profile
        {
            return Err(Error::Conflict);
        }
        let statistics = record.statistics(&request.tree)?;
        let mut placements = BTreeMap::new();
        for placed in &record.files {
            placements.insert(placed.path.as_str(), placed.method);
        }
        let expected: BTreeMap<_, _> = request
            .tree
            .entries()
            .iter()
            .map(|e| (e.path.as_str(), &e.node))
            .collect();
        let data = filesystem::open_directory(&bundle, Path::new("data"))?;
        filesystem::directory_mode(&data, request.profile.owner, 0o555)?;
        let mut pending = vec![String::new()];
        let mut seen = BTreeSet::from([String::new()]);
        while let Some(parent) = pending.pop() {
            let directory = filesystem::open_directory(&data, Path::new(&parent))?;
            for entry in directory.entries()? {
                let name = entry?.file_name();
                let name_text = name.to_str().ok_or(Error::Integrity)?;
                let relative = if parent.is_empty() {
                    name_text.into()
                } else {
                    format!("{parent}/{name_text}")
                };
                let node = expected.get(relative.as_str()).ok_or(Error::Integrity)?;
                if !seen.insert(relative.clone()) {
                    return Err(Error::Integrity);
                }
                let metadata = directory.symlink_metadata(&name)?;
                if metadata.uid() != request.profile.owner.uid
                    || metadata.gid() != request.profile.owner.gid
                {
                    return Err(Error::Integrity);
                }
                match node {
                    ProjectionNode::Directory => {
                        if !metadata.is_dir() || metadata.mode() & 0o7777 != 0o555 {
                            return Err(Error::Integrity);
                        }
                        pending.push(relative);
                    }
                    ProjectionNode::Symlink { target: wanted } => {
                        if !metadata.is_symlink()
                            || directory.read_link(&name)?.to_str() != Some(wanted.as_str())
                        {
                            return Err(Error::Integrity);
                        }
                    }
                    ProjectionNode::File {
                        content,
                        executable,
                    } => {
                        if !metadata.is_file() {
                            return Err(Error::Integrity);
                        }
                        verify(&target.path().join("data").join(&relative), content, *executable,
                            *placements.get(relative.as_str()).ok_or(Error::Integrity)?)?;
                    }
                }
            }
        }
        if seen.len() != expected.len() {
            return Err(Error::Integrity);
        }
        if durable {
            file.sync_all()?;
            for entry in request.tree.entries().iter().rev() {
                if matches!(entry.node, ProjectionNode::Directory) {
                    filesystem::sync_directory(&filesystem::open_directory(
                        &data,
                        Path::new(&entry.path),
                    )?)?;
                }
            }
            filesystem::sync_directory(&bundle)?;
            target.sync()?;
        }
        Ok(Some(NativeProjection {
            root: target.path().join("data"),
            record,
            reused: true,
            statistics,
        }))
    }
}

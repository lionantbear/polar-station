use super::{files, FileOwner, FileStamp, LocalFile};
use polar_cas::{
    projection::{FileMaterialization, FilePlacementMethod},
    ContentRef, Error, Result,
};
use std::path::Path;

impl LocalFile {
    pub(crate) fn place(
        &self,
        target: &Path,
        policy: FileMaterialization,
        owner: FileOwner,
    ) -> Result<FilePlacementMethod> {
        match self.link_to(target) {
            Ok(_) => {
                self.sync()?;
                return Ok(FilePlacementMethod::Hardlink);
            }
            Err(Error::Io(error))
                if policy != FileMaterialization::HardlinkRequired
                    && matches!(
                        error.kind(),
                        std::io::ErrorKind::CrossesDevices
                            | std::io::ErrorKind::Unsupported
                            | std::io::ErrorKind::PermissionDenied
                    ) => {}
            Err(error) => return Err(error),
        }
        let (_, _, mut source) = files::open_absolute(&self.path)?;
        if FileStamp::of(&source.metadata()?) != self.stamp {
            return Err(Error::Integrity);
        }
        let mut output = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(target)?;
        let expected = self.content.preferred();
        let method = if files::try_reflink(&source, &output)? {
            files::verify_file(&mut output, &expected, &expected.id, None)?;
            FilePlacementMethod::Reflink
        } else if policy == FileMaterialization::CopyAllowed {
            files::verify_file(&mut source, &expected, &expected.id, Some(&mut output))?;
            FilePlacementMethod::Copy
        } else {
            return Err(Error::UnsupportedCapability);
        };
        files::set_private_metadata(&output, self.mode(), owner)?;
        output.sync_all()?;
        Ok(method)
    }
    pub(crate) fn inspect_placement(
        &self,
        target: &Path,
        content: &ContentRef,
        owner: FileOwner,
        method: FilePlacementMethod,
    ) -> Result<()> {
        self.validate_for(content, self.mode(), owner)?;
        if method == FilePlacementMethod::Hardlink {
            self.verify_link(target)?;
            return self.sync();
        }
        if self.verify_link(target).is_ok() {
            return Err(Error::Integrity);
        }
        let (_, _, mut file) = files::open_absolute(target)?;
        files::check_file(&file, content.size_bytes, self.mode(), owner)?;
        files::verify_file(&mut file, content, &content.id, None)?;
        file.sync_all()?;
        Ok(())
    }
}

impl LocalFile {
    /// Target-only byte check. No source opens or durability/permission changes.
    pub(crate) fn inspect_target_bytes(
        target: &Path, content: &ContentRef, mode: super::FileMode, owner: FileOwner, durable: bool,
    ) -> Result<()> {
        let (_, _, mut file) = files::open_absolute(target)?;
        let before = FileStamp::of(&files::check_file(&file, content.size_bytes, mode, owner)?);
        files::verify_file(&mut file, content, &content.id, None)?;
        let (_, _, reopened) = files::open_absolute(target)?;
        if before != FileStamp::of(&reopened.metadata()?) { return Err(Error::Integrity); }
        if durable { file.sync_all()?; }
        Ok(())
    }
}

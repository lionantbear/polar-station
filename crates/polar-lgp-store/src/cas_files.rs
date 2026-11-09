//! Read existing file registrations; no new inventory or authorization database.
#[derive(Debug, Clone)]
pub struct RegisteredFile {
    pub key: String,
    pub name: Option<String>,
    pub mime_type: Option<String>,
    pub digest: String,
    pub size_bytes: u64,
    pub owner_id: String,
    pub thread_id: String,
    pub message_id: Option<String>,
    pub attachment_id: Option<String>,
    pub upload_ref: Option<String>,
    pub locator: String,
    pub storage_family: String,
}

#[derive(Debug, Clone)]
pub struct RegisteredFileQuery {
    pub name: Option<String>,
    pub digest: Option<String>,
    pub key: Option<String>,
    pub thread: Option<String>,
    pub after: Option<String>,
    pub limit: u32,
}

/// Metadata view identity for one registration-store adapter instance.
///
/// This is not an authorization grant or a query/cursor binding. Permissions must
/// still be checked live. A separately constructed adapter requires a fresh page.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct RegisteredFileMetadataToken {
    pub incarnation: [u8; 32],
    pub revision: u64,
}

impl RegisteredFileMetadataToken {
    // Preserve the shared typed StorageError contract instead of boxing it here.
    #[allow(clippy::result_large_err)]
    pub fn validate(&self) -> polar_storage_core::StorageResult<()> {
        if self.revision > i64::MAX as u64 {
            return Err(fence_input_error(
                polar_storage_core::StorageErrorKind::InvalidInput,
                "registered file metadata revision is out of range",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct FencedRegisteredFilePage {
    pub files: Vec<RegisteredFile>,
    pub metadata: RegisteredFileMetadataToken,
}

impl RegisteredFileQuery {
    /// Validates only the opt-in fence contract; legacy query behavior is unchanged.
    // Preserve the shared typed StorageError contract instead of boxing it here.
    #[allow(clippy::result_large_err)]
    pub fn validate_fenced(
        &self,
        expected: Option<&RegisteredFileMetadataToken>,
    ) -> polar_storage_core::StorageResult<()> {
        if !(1..=201).contains(&self.limit) {
            return Err(fence_input_error(
                polar_storage_core::StorageErrorKind::InvalidInput,
                "invalid registered file page size",
            ));
        }
        if let Some(expected) = expected {
            expected.validate()?;
        }
        if self.after.is_some() && expected.is_none() {
            return Err(fence_input_error(
                polar_storage_core::StorageErrorKind::Conflict,
                "registered file continuation requires a metadata token",
            ));
        }
        Ok(())
    }
}

fn fence_input_error(
    kind: polar_storage_core::StorageErrorKind,
    message: &str,
) -> polar_storage_core::StorageError {
    polar_storage_core::StorageError::new(
        kind,
        message,
        None,
        None,
        polar_storage_core::RetryDisposition::Never,
    )
}

pub(crate) fn unsupported_registration_fence() -> polar_storage_core::StorageError {
    fence_input_error(
        polar_storage_core::StorageErrorKind::Unavailable,
        "registered file metadata fencing is unsupported by this LGP store",
    )
    .with_diagnostic(polar_storage_core::StorageDiagnostic {
        code: Some("registered_file_metadata_fence_unsupported".into()),
        ..Default::default()
    })
}



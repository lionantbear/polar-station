//! Bounded, cancellable prefix cleanup. Provider sessions remain in the injected
//! editor; this module owns only recording text and progress.
use crate::Recording;
use anyhow::Result;
use sha2::{Digest, Sha256};
use std::{future::Future, pin::Pin};

pub(crate) type PrefixCleanup = Pin<Box<dyn Future<Output = Result<(Vec<String>, String)>> + Send>>;

impl crate::runtime::DictateRuntime {
    pub(crate) fn prefix_cleanup(&self, record: &Recording) -> Result<Option<PrefixCleanup>> {
        if !record.processing_enabled
            || !record.cleanup_enabled
            || record.cleanup_deferred
            || record.cleanup_error.is_some()
            || record.cleaned_prefix_parts >= record.confirmed_transcript.len()
        {
            return Ok(None);
        }
        let original = record.confirmed_transcript[record.cleaned_prefix_parts..].join(" ");
        // Only provider-confirmed segments reach this path. Missing punctuation
        // does not make that confirmation provisional or justify delaying work.
        let cleanup = self.cleanup_provider()?;
        self.cancel_preparation(&record.id);
        let context = record.cleanup_reference_context(self.recording_context(record)?)?;
        let source = record.confirmed_transcript.clone();
        let prefix = record.cleaned_prefix.clone();
        let request = polar_transcreation::EditRequest {
            project: record.project.clone(),
            scope: None,
            operation_id: format!(
                "{:x}",
                Sha256::digest(format!(
                    "{}:prefix:{}:{}",
                    record.id,
                    source.len(),
                    record.revision
                ))
            ),
            base_revision: record.revision,
            original,
            edit: record
                .output_mode()
                .edit(context)
                .ok_or_else(|| anyhow::anyhow!("dictate.refinement_disabled"))?,
        };
        let recording_id = record.id.clone();
        Ok(Some(Box::pin(async move {
            let text = tokio::time::timeout(
                std::time::Duration::from_secs(8),
                cleanup.clean_recording(&recording_id, request),
            )
            .await??;
            let text = if prefix.is_empty() {
                text
            } else {
                format!("{prefix} {text}")
            };
            Ok((source, text))
        })))
    }
}

impl crate::runtime::DictateRuntime {
    pub(crate) async fn apply_prefix_cleanup(
        &self,
        id: &str,
        result: Result<(Vec<String>, String)>,
    ) -> Result<()> {
        let current = self.store().get(id).await?;
        if !current.processing_enabled || !current.cleanup_enabled {
            return Ok(());
        }
        match result {
            Ok((source, text))
                if current.confirmed_transcript.get(..source.len()) == Some(source.as_slice()) =>
            {
                self.change(id, crate::Change::CleanTranscriptPrefix { source, text })
                    .await?;
            }
            Ok(_) => {}
            Err(error) => {
                self.change(
                    id,
                    crate::Change::PartialCleanupFailed {
                        error: error.to_string(),
                    },
                )
                .await?;
            }
        }
        Ok(())
    }
}

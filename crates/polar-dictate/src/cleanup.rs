//! Bounded, cancellable prefix cleanup. Provider sessions remain in the injected
//! editor; this module owns only recording text and progress.
use crate::{Recording, VoiceSettings};
use anyhow::Result;
use sha2::{Digest, Sha256};
use std::{future::Future, pin::Pin};

pub(crate) type PrefixCleanup = Pin<Box<dyn Future<Output = Result<(Vec<String>, String)>> + Send>>;

impl crate::runtime::DictateRuntime {
    pub(crate) fn prefix_cleanup(&self, record: &Recording) -> Result<Option<PrefixCleanup>> {
        if !record.processing_enabled || !record.cleanup_enabled || record.cleanup_deferred
            || record.cleanup_error.is_some() || record.cleaned_prefix_parts >= record.confirmed_transcript.len() {
            return Ok(None);
        }
        let original = record.confirmed_transcript[record.cleaned_prefix_parts..].join(" ");
        // Only provider-confirmed segments reach this path. Missing punctuation
        // does not make that confirmation provisional or justify delaying work.
        let cleanup = self.cleanup_provider()?;
        let settings = VoiceSettings { glossary: record.glossary.clone(), context: record.context_enabled, ..Default::default() };
        let context = settings.cleanup_context(self.recording_context(&record.id)?)?;
        let source = record.confirmed_transcript.clone();
        let prefix = record.cleaned_prefix.clone();
        let request = polar_text_edit::EditRequest {
            operation_id: format!("{:x}", Sha256::digest(format!("{}:prefix:{}:{}", record.id, source.len(), record.revision))),
            base_revision: record.revision,
            original,
            edit: polar_text_edit::EditKind::CleanupTranscript { context },
        };
        let recording_id = record.id.clone();
        Ok(Some(Box::pin(async move {
            let text = tokio::time::timeout(std::time::Duration::from_secs(8), cleanup.clean_recording(&recording_id, request)).await??;
            let text = if prefix.is_empty() { text } else { format!("{prefix} {text}") };
            Ok((source, text))
        })))
    }
}

impl crate::runtime::DictateRuntime {
    pub(crate) async fn clean_prefix_once(&self, record: &Recording) -> Result<Recording> {
        let mut events = self.subscribe();
        let work = match self.prefix_cleanup(record) {
            Ok(Some(work)) => work,
            Ok(None) => return Ok(record.clone()),
            Err(error) => return self.change(&record.id, crate::Change::PartialCleanupFailed { error: error.to_string() }).await,
        };
        tokio::pin!(work);
        let result = loop {
            tokio::select! {
                result = &mut work => break result,
                _ = events.recv() => {
                    let current = self.store().get(&record.id).await?;
                    if !current.processing_enabled || !current.cleanup_enabled { return Ok(current); }
                }
            }
        };
        match result {
            Ok((source, text)) => self.change(&record.id, crate::Change::CleanTranscriptPrefix { source, text }).await,
            Err(error) => self.change(&record.id, crate::Change::PartialCleanupFailed { error: error.to_string() }).await,
        }
    }
}

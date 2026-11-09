//! Shared SDK and Station contract. No paths, owners, provider results or secrets
//! are accepted from a frontend. The authenticated service binding supplies them.
use crate::{AudioBlock, Recording, SaveReceipt};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    RetrySegmented { id: String },
    TranscriptionModes,
    ShortcutModes,
    RefineText { operation_id: String, text: String, context: Option<String> },
    Start {
        id: String,
        now_ms: u64,
        #[serde(default)]
        is_test: bool,
        #[serde(default = "enabled")]
        cleanup: bool,
        context: Option<String>,
    },
    Subscribe { id: String },
    Feed { id: String, pcm: Vec<u8> },
    Append {
        block: AudioBlock,
    },
    Stop {
        id: String,
        reason: Option<String>,
    },
    Retry {
        id: String,
    },
    Pause {
        id: String,
    },
    Cancel {
        id: String,
    },
    Get {
        id: String,
    },
    Wait {
        id: String,
        #[serde(default)]
        stage: WaitStage,
    },
    Usage,
    List {
        offset: u32,
        limit: u32,
    },
    BindContext {
        id: String,
        context: String,
    },
    UseOriginal {
        id: String,
    },
    PrepareDelivery {
        id: String,
        token: String,
        expected_revision: u64,
    },
    CompleteDelivery {
        id: String,
        token: String,
        applied: Option<bool>,
        now_ms: u64,
    },
    Accept {
        id: String,
        now_ms: u64,
    },
    Pin {
        id: String,
        pinned: bool,
    },
    Delete {
        id: String,
        confirmation: String,
    },
    Audio {
        id: String,
        start_sample: u64,
        sample_count: u64,
    },
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitStage {
    Transcription,
    #[default]
    Cleanup,
}

fn enabled() -> bool {
    true
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    ShortcutModes { choices: Vec<crate::ShortcutModeChoice> },
    TranscriptionModes { choices: Vec<crate::TranscriptionModeChoice> },
    Refined { operation_id: String, text: String },
    Recording {
        recording: Box<Recording>,
    },
    Saved {
        receipt: SaveReceipt,
    },
    Usage {
        usage: crate::StorageUsage,
    },
    Recordings {
        recordings: Vec<crate::RecordingSummary>,
    },
    Audio {
        start_sample: u64,
        pcm: Vec<u8>,
    },
    Deleted,
}
#[cfg(feature = "runtime")]
impl crate::runtime::DictateRuntime {
    pub async fn dispatch(&self, request: Request) -> anyhow::Result<Response> {
        use crate::Change;
        let recording = match request {
            Request::ShortcutModes => return Ok(Response::ShortcutModes { choices: crate::ShortcutMode::choices() }),
            Request::TranscriptionModes => return Ok(Response::TranscriptionModes { choices: crate::TranscriptionMode::choices() }),
            Request::RefineText { operation_id, text, context } => {
                let result = self.refine_text(polar_text_edit::EditRequest {
                    operation_id, base_revision: 0, original: text,
                    edit: polar_text_edit::EditKind::CleanupTranscript { context },
                }).await?;
                return Ok(Response::Refined { operation_id: result.operation_id, text: result.text });
            }
            Request::Wait { id, stage } => self.wait(&id, stage).await?,
            Request::Start {
                id,
                now_ms,
                is_test,
                cleanup,
                context,
            } => {
                anyhow::ensure!(
                    context
                        .as_ref()
                        .is_none_or(|s| s.len() <= 16 * 1024 && !s.contains('\0')),
                    "dictate.context_limit"
                );
                let mut record = Recording::new(id, now_ms, is_test)?;
                record.apply(Change::Configure { cleanup, context })?;
                self.create_configured(record).await?
            }
            Request::Feed { id, pcm } => return Ok(Response::Saved { receipt: self.feed(&id, pcm).await? }),
            Request::Append { block } => {
                return Ok(Response::Saved {
                    receipt: self.append(block).await?,
                })
            }
            Request::Stop { id, reason } => self.change(&id, Change::Stop { reason }).await?,
            Request::RetrySegmented { id } => self.change(&id, Change::RetrySegmented).await?,
            Request::Retry { id } => self.change(&id, Change::ResumeProcessing).await?,
            Request::Pause { id } => self.change(&id, Change::CancelProcessing).await?,
            Request::Cancel { id } => {
                self.change(&id, Change::CancelProcessing).await?;
                self.change(
                    &id,
                    Change::Stop {
                        reason: Some("Capture cancelled; saved audio retained".into()),
                    },
                )
                .await?
            }
            Request::Get { id } | Request::Subscribe { id } => self.store().get(&id).await?,
            Request::Usage => {
                return Ok(Response::Usage {
                    usage: self.store().usage().await?,
                })
            }
            Request::List { offset, limit } => {
                return Ok(Response::Recordings {
                    recordings: self
                        .store()
                        .list(offset, limit.min(100))
                        .await?
                        .into_iter()
                        .map(Into::into)
                        .collect(),
                })
            }
            Request::BindContext { id, context } => self.bind_context(&id, context).await?,
            Request::UseOriginal { id } => self.change(&id, Change::UseOriginal).await?,
            Request::PrepareDelivery {
                id,
                token,
                expected_revision,
            } => {
                self.change(
                    &id,
                    Change::PrepareDelivery {
                        token,
                        expected_revision,
                    },
                )
                .await?
            }
            Request::CompleteDelivery {
                id,
                token,
                applied,
                now_ms,
            } => {
                self.change(
                    &id,
                    Change::CompleteDelivery {
                        token,
                        applied,
                        now_ms,
                    },
                )
                .await?
            }
            Request::Accept { id, now_ms } => self.change(&id, Change::Accept { now_ms }).await?,
            Request::Pin { id, pinned } => self.change(&id, Change::Pin { pinned }).await?,
            Request::Delete { id, confirmation } => {
                anyhow::ensure!(confirmation == id, "dictate.delete_confirmation_required");
                self.delete_recording(&id).await?;
                return Ok(Response::Deleted);
            }
            Request::Audio {
                id,
                start_sample,
                sample_count,
            } => {
                anyhow::ensure!(
                    sample_count <= crate::MAX_BLOCK_SAMPLES,
                    "dictate.audio_range_too_large"
                );
                return Ok(Response::Audio {
                    start_sample,
                    pcm: self.store().audio(&id, start_sample, sample_count).await?,
                });
            }
        };
        Ok(Response::Recording {
            recording: Box::new(recording),
        })
    }
}

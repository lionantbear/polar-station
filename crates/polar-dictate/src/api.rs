//! Shared SDK and Station contract. No paths, owners, provider results or secrets
//! are accepted from a frontend. The authenticated service binding supplies them.
use crate::{AudioBlock, Recording, SaveReceipt};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    RetrySegmented {
        id: String,
    },
    MarkCopied {
        id: String,
        now_ms: u64,
    },
    ReopenCopy {
        id: String,
    },
    TranscriptionModes,
    ShortcutModes,
    RefineText {
        operation_id: String,
        text: String,
        context: Option<String>,
        #[serde(default)]
        project: Option<String>,
        #[serde(default)]
        scope: Option<String>,
        #[serde(default)]
        mode: polar_transcreation::RefineMode,
    },
    Start {
        #[serde(default)]
        project: Option<String>,
        id: String,
        now_ms: u64,
        #[serde(default)]
        is_test: bool,
        #[serde(default = "enabled")]
        cleanup: bool,
        context: Option<String>,
        #[serde(default)]
        reference: String,
    },
    Subscribe {
        id: String,
    },
    SubscribeHistory,
    Feed {
        id: String,
        pcm: Vec<u8>,
    },
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
    FinishLegacyImport,
    ImportIssues {
        offset: u32,
        limit: u32,
    },
    TextBegin {
        id: String,
        text: String,
        mode: polar_transcreation::RefineMode,
    },
    ImportLegacy {
        entries: Vec<crate::LegacyInput>,
    },
    History {
        cursor: Option<crate::HistoryCursor>,
        limit: u32,
    },
    Item {
        id: String,
    },
    ItemChange {
        id: String,
        change: crate::ItemChange,
    },
    CleanupPreview,
    CleanupApply {
        confirmation: bool,
        plan: crate::CleanupPlan,
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
    UseAvailable {
        id: String,
    },
    PrepareTextDelivery {
        id: String,
        token: String,
        text: String,
        edited: bool,
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
    Capture,
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
    ImportIssues {
        items: Vec<crate::LegacyInput>,
    },
    Imported {
        report: crate::ImportReport,
    },
    History {
        page: crate::HistoryPage,
    },
    Item {
        item: crate::HistoryItem,
    },
    Cleanup {
        report: crate::CleanupReport,
        presentation: serde_json::Value,
        plan: crate::CleanupPlan,
    },
    ShortcutModes {
        choices: Vec<crate::ShortcutModeChoice>,
    },
    TranscriptionModes {
        choices: Vec<crate::TranscriptionModeChoice>,
    },
    Refined {
        operation_id: String,
        text: String,
    },
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
            Request::ImportIssues { offset, limit } => {
                return Ok(Response::ImportIssues {
                    items: self.store().import_issues(offset, limit).await?,
                })
            }
            Request::TextBegin { id, text, mode } => {
                return Ok(Response::Item {
                    item: crate::HistoryItem::Text(Box::new(
                        self.store()
                            .text_create(crate::TextEditRecord::new(
                                id,
                                text,
                                mode,
                                super::runtime::now_ms(),
                            )?)
                            .await?,
                    )),
                })
            }
            Request::FinishLegacyImport => {
                self.store().finish_legacy_import().await?;
                self.settings_changed();
                return Ok(Response::Imported {
                    report: Default::default(),
                });
            }
            Request::ImportLegacy { entries } => {
                return Ok(Response::Imported {
                    report: self.store().import_legacy(entries).await?,
                })
            }
            Request::History { cursor, limit } => {
                return Ok(Response::History {
                    page: self.store().history(cursor, limit).await?,
                })
            }
            Request::Item { id } => {
                return Ok(Response::Item {
                    item: self.store().item(&id).await?,
                })
            }
            Request::ItemChange { id, change } => {
                return Ok(Response::Item {
                    item: self.change_item(&id, change).await?,
                })
            }
            Request::CleanupPreview => {
                let plan = crate::CleanupPlan {
                    policy: self.retention_policy()?,
                    as_of_ms: super::runtime::now_ms(),
                };
                let report = self.cleanup_history(&plan, false).await?;
                let presentation = report.presentation();
                return Ok(Response::Cleanup {
                    report,
                    presentation,
                    plan,
                });
            }
            Request::CleanupApply { confirmation, plan } => {
                anyhow::ensure!(confirmation, "dictate.cleanup_confirmation_required");
                let report = self.cleanup_history(&plan, true).await?;
                let presentation = report.presentation();
                return Ok(Response::Cleanup {
                    report,
                    presentation,
                    plan,
                });
            }
            Request::SubscribeHistory => {
                anyhow::bail!("History subscription requires an event connection")
            }
            Request::ShortcutModes => {
                return Ok(Response::ShortcutModes {
                    choices: crate::ShortcutMode::choices(),
                })
            }
            Request::TranscriptionModes => {
                return Ok(Response::TranscriptionModes {
                    choices: crate::TranscriptionMode::choices(),
                })
            }
            Request::RefineText {
                operation_id,
                text,
                context,
                mode,
                scope,
                project,
            } => {
                let result = self
                    .refine_text(polar_transcreation::EditRequest {
                        project,
                        scope,
                        operation_id,
                        base_revision: 0,
                        original: text,
                        edit: mode.edit(context),
                    })
                    .await?;
                return Ok(Response::Refined {
                    operation_id: result.operation_id,
                    text: result.text,
                });
            }
            Request::Wait { id, stage } => self.wait(&id, stage).await?,
            Request::Start {
                project,
                id,
                now_ms,
                is_test,
                cleanup,
                context,
                reference,
            } => {
                anyhow::ensure!(
                    context
                        .as_ref()
                        .is_none_or(|s| s.len() <= 16 * 1024 && !s.contains('\0')),
                    "dictate.context_limit"
                );
                let mut record = Recording::new(id, now_ms, is_test)?;
                record.set_project(project)?;
                record.set_reference(reference)?;
                record.apply(Change::Configure { cleanup, context })?;
                self.create_configured(record).await?
            }
            Request::Feed { id, pcm } => {
                return Ok(Response::Saved {
                    receipt: self.feed(&id, pcm).await?,
                })
            }
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
            Request::UseAvailable { id } => {
                return Ok(Response::Item { item: self.use_available(&id).await? })
            }
            Request::PrepareTextDelivery {
                id,
                token,
                text,
                edited,
            } => {
                self.change(
                    &id,
                    Change::PrepareTextDelivery {
                        token,
                        text,
                        edited,
                    },
                )
                .await?
            }
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
            Request::MarkCopied { id, now_ms } => {
                self.change(&id, Change::MarkCopied { now_ms }).await?
            }
            Request::ReopenCopy { id } => self.change(&id, Change::ReopenCopy).await?,
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

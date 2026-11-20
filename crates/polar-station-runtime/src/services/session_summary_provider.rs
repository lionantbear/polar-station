//! Optional natural-language summaries for bounded Session activity.
//!
//! The deterministic local classifier is always authoritative for attention
//! state. This adapter may replace only the privacy-safe summary sections. It
//! delegates temporary read-only turns to the shared Codex backend.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use polar_station_protocol::SessionAttentionStateV1;
use polar_station_settings::SessionSummarySettingsV1;
use serde_json::{json, Value};
use tokio::sync::Semaphore;

use super::terminal_text::sanitize_terminal_output;

const MAX_SUMMARY_SECTIONS: usize = 3;
const MAX_SUMMARY_SECTION_BYTES: usize = 128;

pub type SessionSummaryFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<String>, String>> + Send + 'a>>;
pub type SessionDraftFuture<'a> = Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>>;
pub type SessionAnalysisFuture<'a> =
    Pin<Box<dyn Future<Output = Result<SessionAnalysis, String>> + Send + 'a>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionAnalysis {
    pub state: SessionAttentionStateV1,
    pub sections: Vec<String>,
}

pub trait SessionSummaryProvider: std::fmt::Debug + Send + Sync {
    fn summarize<'a>(&'a self, sections: &'a [String]) -> SessionSummaryFuture<'a>;
    fn analyze<'a>(&'a self, sections: &'a [String]) -> SessionAnalysisFuture<'a> {
        Box::pin(async move {
            self.summarize(sections)
                .await
                .map(|sections| SessionAnalysis {
                    state: SessionAttentionStateV1::Unknown,
                    sections,
                })
        })
    }
    fn draft<'a>(&'a self, _summary: &'a str, _reply: &'a str) -> SessionDraftFuture<'a> {
        Box::pin(async { Err("This summary provider does not support reply drafts".to_owned()) })
    }
}

#[derive(Debug, Clone)]
pub struct CodexAppServerSummaryProvider {
    model: Option<String>,
    timeout: Duration,
    concurrency: Arc<Semaphore>,
}

impl CodexAppServerSummaryProvider {
    pub fn from_settings(
        settings: &SessionSummarySettingsV1,
    ) -> Result<Option<Arc<dyn SessionSummaryProvider>>, String> {
        settings.validate().map_err(|error| error.to_string())?;
        if !settings.enabled {
            return Ok(None);
        }
        Ok(Some(Arc::new(Self {
            model: settings.model.clone(),
            timeout: Duration::from_millis(settings.timeout_ms),
            concurrency: Arc::new(Semaphore::new(1)),
        })))
    }

    pub async fn test_settings(settings: &SessionSummarySettingsV1) -> Result<String, String> {
        let provider = Self::from_settings(settings)?
            .ok_or_else(|| "Enable AI summaries before testing the provider".to_owned())?;
        let sections = provider
            .summarize(&["Privacy-safe provider readiness check".to_owned()])
            .await?;
        Ok(sections.join(" · "))
    }
}

impl SessionSummaryProvider for CodexAppServerSummaryProvider {
    fn summarize<'a>(&'a self, sections: &'a [String]) -> SessionSummaryFuture<'a> {
        let model = self.model.clone();
        let timeout = self.timeout;
        let sections = sections.to_vec();
        let concurrency = self.concurrency.clone();
        Box::pin(async move {
            tokio::time::timeout(timeout, async move {
                let _permit = concurrency
                    .acquire_owned()
                    .await
                    .map_err(|_| "Codex app-server summary adapter stopped".to_owned())?;
                generate_once(model.as_deref(), &sections, GenerationTask::Summary)
                    .await
                    .map(|output| output.sections)
            })
            .await
            .map_err(|_| format!("Codex app-server summary timed out after {timeout:?}"))?
        })
    }

    fn analyze<'a>(&'a self, sections: &'a [String]) -> SessionAnalysisFuture<'a> {
        let model = self.model.clone();
        let timeout = self.timeout;
        let sections = sections.to_vec();
        let concurrency = self.concurrency.clone();
        Box::pin(async move {
            let output = tokio::time::timeout(timeout, async move {
                let _permit = concurrency
                    .acquire_owned()
                    .await
                    .map_err(|_| "Codex app-server analysis adapter stopped".to_owned())?;
                generate_once(model.as_deref(), &sections, GenerationTask::Analysis).await
            })
            .await
            .map_err(|_| format!("Codex app-server analysis timed out after {timeout:?}"))??;
            Ok(SessionAnalysis {
                state: output.state.ok_or_else(|| {
                    "Codex app-server analysis returned no Agent state".to_owned()
                })?,
                sections: output.sections,
            })
        })
    }

    fn draft<'a>(&'a self, summary: &'a str, reply: &'a str) -> SessionDraftFuture<'a> {
        let model = self.model.clone();
        let timeout = self.timeout;
        let input = vec![summary.to_owned(), reply.to_owned()];
        let concurrency = self.concurrency.clone();
        Box::pin(async move {
            let output = tokio::time::timeout(timeout, async move {
                let _permit = concurrency
                    .acquire_owned()
                    .await
                    .map_err(|_| "Codex app-server draft adapter stopped".to_owned())?;
                generate_once(model.as_deref(), &input, GenerationTask::ReplyDraft).await
            })
            .await
            .map_err(|_| format!("Codex app-server draft timed out after {timeout:?}"))??;
            output
                .sections
                .into_iter()
                .next()
                .ok_or_else(|| "Codex app-server returned no draft".to_owned())
        })
    }
}

#[derive(Clone, Copy)]
enum GenerationTask {
    Summary,
    Analysis,
    ReplyDraft,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProviderOutput {
    state: Option<SessionAttentionStateV1>,
    sections: Vec<String>,
}

async fn generate_once(
    model: Option<&str>,
    sections: &[String],
    task: GenerationTask,
) -> Result<ProviderOutput, String> {
    let instructions = match task {
                    GenerationTask::Summary => "Summarize only the supplied terminal excerpt. Do not call tools, infer secrets, or add facts. Return one to three concise sections.",
                    GenerationTask::Analysis => "Classify only the supplied bounded terminal snapshot as working, review_ready, needs_attention, approval_required, decision_required, permission_required, failed, or unknown and return one to three concise evidence-grounded sections. Do not call tools, infer secrets, or add facts.",
                    GenerationTask::ReplyDraft => "Create one concise terminal-input draft answering the human reply using only the supplied privacy-safe summary. Do not call tools, add facts, or include a trailing Return. Return the draft as the only sections item.",
                };
    let excerpt = sections.join("\n\n");
    let prompt = match task {
        GenerationTask::Summary => format!(
            "Return JSON with a sections array summarizing this bounded terminal excerpt. Preserve no escape sequences and include no more than three short strings.\n\n{excerpt}"
        ),
        GenerationTask::Analysis => format!(
            "Return JSON with one state and a sections array for this bounded terminal snapshot. Use only working, review_ready, needs_attention, approval_required, decision_required, permission_required, failed, or unknown. Preserve no escape sequences and include no more than three short strings.\n\n{excerpt}"
        ),
        GenerationTask::ReplyDraft => format!(
            "The first block is a privacy-safe terminal summary. The second block is the human reply. Return JSON with a sections array containing exactly one editable terminal-input draft and no trailing Return.\n\n{excerpt}"
        ),
    };
    let schema = json!({
        "type": "object",
        "properties": {
            "state": {
                "type": "string",
                "enum": ["working", "review_ready", "needs_attention", "approval_required", "decision_required", "permission_required", "failed", "unknown"]
            },
            "sections": {
                "type": "array",
                "items": {"type": "string"},
                "minItems": 1,
                "maxItems": if matches!(task, GenerationTask::ReplyDraft) { 1 } else { 3 }
            }
        },
        "required": if matches!(task, GenerationTask::Analysis) { vec!["state", "sections"] } else { vec!["sections"] },
        "additionalProperties": false
    });
    let message = polar_inhouse_backend_codex::client::run_read_only_turn(
        model,
        "polar_station_session_summary",
        instructions,
        prompt,
        schema,
    )
    .await
    .map_err(|error| format!("Codex generation failed: {error:#}"))?;
    parse_provider_output(&message, task)
}

fn parse_provider_output(message: &str, task: GenerationTask) -> Result<ProviderOutput, String> {
    let trimmed = message
        .trim()
        .strip_prefix("```json")
        .or_else(|| message.trim().strip_prefix("```"))
        .unwrap_or(message.trim())
        .strip_suffix("```")
        .unwrap_or(message.trim())
        .trim();
    let value: Value = serde_json::from_str(trimmed)
        .map_err(|error| format!("Codex app-server summary was not JSON: {error}"))?;
    let sections = value
        .get("sections")
        .and_then(Value::as_array)
        .ok_or_else(|| "Codex app-server summary had no sections array".to_owned())?
        .iter()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let sections = normalize_summary_sections(sections);
    if sections.is_empty() {
        return Err("Codex app-server summary contained no usable sections".to_owned());
    }
    let state = if matches!(task, GenerationTask::Analysis) {
        Some(parse_analysis_state(
            value
                .get("state")
                .and_then(Value::as_str)
                .ok_or_else(|| "Codex app-server analysis had no state".to_owned())?,
        )?)
    } else {
        None
    };
    Ok(ProviderOutput { state, sections })
}

fn parse_analysis_state(value: &str) -> Result<SessionAttentionStateV1, String> {
    match value {
        "working" => Ok(SessionAttentionStateV1::Working),
        "review_ready" => Ok(SessionAttentionStateV1::ReviewReady),
        "needs_attention" => Ok(SessionAttentionStateV1::NeedsAttention),
        "approval_required" => Ok(SessionAttentionStateV1::ApprovalRequired),
        "decision_required" => Ok(SessionAttentionStateV1::DecisionRequired),
        "permission_required" => Ok(SessionAttentionStateV1::PermissionRequired),
        "failed" => Ok(SessionAttentionStateV1::Failed),
        "unknown" => Ok(SessionAttentionStateV1::Unknown),
        _ => Err("Codex app-server analysis returned an unsupported state".to_owned()),
    }
}

pub fn normalize_summary_sections(sections: impl IntoIterator<Item = String>) -> Vec<String> {
    sections
        .into_iter()
        .map(|section| {
            sanitize_terminal_output(section.as_bytes())
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|section| !section.is_empty())
        .map(|section| truncate_utf8(&section, MAX_SUMMARY_SECTION_BYTES))
        .take(MAX_SUMMARY_SECTIONS)
        .collect()
}

fn truncate_utf8(value: &str, maximum_bytes: usize) -> String {
    if value.len() <= maximum_bytes {
        return value.to_owned();
    }
    let mut end = maximum_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}



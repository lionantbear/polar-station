use polar_inhouse_model::ReplyGenerationState;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind {
    Waiting,
    Thinking,
    Tool,
    Replying,
    Committing,
    Completed,
    Failed,
    Cancelled,
}

/// Human-facing activity, independent of accumulated answer text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityPresentation {
    pub kind: ActivityKind,
    pub label: String,
    pub status_text: String,
}

impl ActivityPresentation {
    pub fn project(state: ReplyGenerationState, phase: &str, status: &str) -> Self {
        use ActivityKind as A;
        use ReplyGenerationState as S;
        let kind = match state {
            S::Waiting => A::Waiting,
            S::Committing => A::Committing,
            S::Completed => A::Completed,
            S::Failed => A::Failed,
            S::Cancelled => A::Cancelled,
            S::Generating => match phase {
                "tool" | "tool_start" => A::Tool,
                "answer" => A::Replying,
                _ => A::Thinking,
            },
        };
        let label = match kind {
            A::Waiting => "Waiting…",
            A::Thinking => "Thinking…",
            A::Tool => "Tool activity",
            A::Replying => "Replying…",
            A::Committing => "Saving reply…",
            A::Completed => "Completed",
            A::Failed => "Failed",
            A::Cancelled => "Cancelled",
        };
        Self {
            kind,
            label: label.into(),
            status_text: if matches!(kind, A::Tool | A::Failed | A::Cancelled) {
                status.into()
            } else {
                String::new()
            },
        }
    }
}

pub fn tool_state_label(state: polar_inhouse_model::ReplyToolState) -> &'static str {
    use polar_inhouse_model::ReplyToolState;
    match state {
        ReplyToolState::Running => "Running",
        ReplyToolState::Completed => "Completed",
        ReplyToolState::Failed => "Failed",
        ReplyToolState::Cancelled => "Cancelled",
        ReplyToolState::Interrupted => "Interrupted",
    }
}



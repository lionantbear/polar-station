//! Shared turn assembly. Kernel and legacy callers supply identity and already
//! compiled blocks; the prompt crate owns projection and compaction semantics.
use anyhow::{Context, Result};
use polar_inhouse_contract::{
    InhouseBackendCapabilities, InhouseBackendToolSpec, InhouseBackendTurnRef,
    InhouseBackendTurnRequest, InhouseDynamicToolSpec, InhousePromptBlock, InhouseToolPlan,
    InhouseTurnOptions, InhouseTurnRequirements,
};
use crate::{
    build_compacted_inhouse_prompt_envelope, build_inhouse_prompt_envelope,
};

pub fn assemble_backend_turn_request(
    turn_ref: InhouseBackendTurnRef,
    options: &InhouseTurnOptions,
    mut blocks: Vec<InhousePromptBlock>,
    dynamic_tool_specs: Vec<InhouseDynamicToolSpec>,
    available_capabilities: InhouseBackendCapabilities,
) -> Result<InhouseBackendTurnRequest> {
    if let Some(memory) = &options.thread_memory_context {
        blocks.push(InhousePromptBlock {
            block_id: "polar-thread-memory-v1".into(),
            source_ref: format!("polar-memory:{}:{}", turn_ref.owner_id, turn_ref.thread_id),
            layer: polar_inhouse_contract::InhousePromptLayer::TurnAugment,
            body_text: format!("Thread memory (derived reference data; current user corrections take precedence):\n{memory}"),
        });
    }
    let current_message = Some(turn_ref.request_message_id.as_str());
    let prompt_envelope = if let Some(compaction) = options
        .context_compaction
        .as_ref()
        .filter(|_| options.thread_memory_context.is_none())
    {
        build_compacted_inhouse_prompt_envelope(
            blocks,
            compaction,
            &options.canonical_thread_messages,
            current_message,
        )
        .map_err(anyhow::Error::msg)?
    } else {
        build_inhouse_prompt_envelope(
            blocks,
            &options.canonical_thread_messages,
            current_message,
            0,
        )
    };
    let message_projection = prompt_envelope
        .message_projection
        .clone()
        .context("shared prompt envelope did not include an LGP projection")?;
    let tool_plan = InhouseToolPlan {
        tool_plan_id: format!(
            "inhouse-tool-plan:{}:{}",
            turn_ref.backend_profile_id, turn_ref.request_message_id
        ),
        tools: dynamic_tool_specs
            .into_iter()
            .map(InhouseBackendToolSpec::from)
            .collect(),
    };
    let requested_capabilities = InhouseTurnRequirements::from_tool_plan(&tool_plan).into();
    Ok(InhouseBackendTurnRequest {
        turn_ref,
        scheduling_priority: options.scheduling_priority,
        prompt_envelope,
        message_projection,
        tool_plan,
        requested_capabilities,
        effective_capabilities: available_capabilities,
        extensions: Vec::new(),
    })
}



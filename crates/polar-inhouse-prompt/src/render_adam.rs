use crate::model::{
    AdamThreadPromptRender, AdamTurnPromptRender, CompiledPromptSection,
    CompiledThreadPromptBundle, CompiledTurnPromptBundle,
};
use polar_inhouse_contract::{InhousePromptBlock, InhousePromptEnvelope, InhousePromptLayer};

fn join_sections(sections: &[CompiledPromptSection]) -> Option<String> {
    let joined = sections
        .iter()
        .map(|section| format!("## {}\n{}", section.title.trim(), section.body_text.trim()))
        .collect::<Vec<_>>()
        .join("\n\n");
    (!joined.trim().is_empty()).then_some(joined)
}

pub fn render_adam_thread_prompt(bundle: &CompiledThreadPromptBundle) -> AdamThreadPromptRender {
    let base_instructions = join_sections(&bundle.ir.base_identity);
    let developer_sections = bundle
        .ir
        .developer_contract
        .iter()
        .chain(bundle.ir.tool_usage_protocol.iter())
        .cloned()
        .collect::<Vec<_>>();
    let developer_instructions = join_sections(&developer_sections);
    let user_project_context = bundle
        .ir
        .user_project_context
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    AdamThreadPromptRender {
        prompt_override_enabled: base_instructions.is_some()
            || developer_instructions.is_some()
            || user_project_context.is_some(),
        base_instructions,
        developer_instructions,
        user_project_context,
    }
}

pub fn render_adam_turn_prompt(bundle: &CompiledTurnPromptBundle) -> AdamTurnPromptRender {
    AdamTurnPromptRender {
        developer_instructions: join_sections(&bundle.ir.tool_usage_protocol),
        input_prefix: join_sections(&bundle.ir.turn_augment),
    }
}

pub fn render_adam_prompt_envelope(envelope: &InhousePromptEnvelope) -> AdamThreadPromptRender {
    let base_instructions = join_envelope_blocks(&envelope.blocks, &[InhousePromptLayer::System]);
    let developer_instructions = join_envelope_blocks(
        &envelope.blocks,
        &[
            InhousePromptLayer::Developer,
            InhousePromptLayer::ToolUsageProtocol,
        ],
    );
    let user_project_context =
        join_envelope_blocks(&envelope.blocks, &[InhousePromptLayer::UserProjectContext]);
    AdamThreadPromptRender {
        prompt_override_enabled: base_instructions.is_some()
            || developer_instructions.is_some()
            || user_project_context.is_some(),
        base_instructions,
        developer_instructions,
        user_project_context,
    }
}

pub fn render_adam_turn_prompt_envelope(envelope: &InhousePromptEnvelope) -> AdamTurnPromptRender {
    AdamTurnPromptRender {
        developer_instructions: join_envelope_blocks(
            &envelope.blocks,
            &[InhousePromptLayer::ToolUsageProtocol],
        ),
        input_prefix: join_envelope_blocks(&envelope.blocks, &[InhousePromptLayer::TurnAugment]),
    }
}

fn join_envelope_blocks(
    blocks: &[InhousePromptBlock],
    layers: &[InhousePromptLayer],
) -> Option<String> {
    let joined = blocks
        .iter()
        .filter(|block| layers.contains(&block.layer))
        .map(|block| format!("## {}\n{}", block.block_id.trim(), block.body_text.trim()))
        .collect::<Vec<_>>()
        .join("\n\n");
    (!joined.trim().is_empty()).then_some(joined)
}

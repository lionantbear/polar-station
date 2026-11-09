use crate::model::{
    CodexThreadPromptRender, CodexTurnPromptRender, CompiledThreadPromptBundle,
    CompiledTurnPromptBundle,
};
use polar_inhouse_contract::{InhousePromptBlock, InhousePromptEnvelope, InhousePromptLayer};

fn join_sections(sections: &[crate::model::CompiledPromptSection]) -> Option<String> {
    let joined = sections
        .iter()
        .map(|section| format!("## {}\n{}", section.title.trim(), section.body_text.trim()))
        .collect::<Vec<_>>()
        .join("\n\n");
    (!joined.trim().is_empty()).then_some(joined)
}

pub fn render_codex_thread_prompt(bundle: &CompiledThreadPromptBundle) -> CodexThreadPromptRender {
    let base_instructions = join_sections(&bundle.ir.base_identity);
    let developer_sections = bundle
        .ir
        .developer_contract
        .iter()
        .chain(bundle.ir.tool_usage_protocol.iter())
        .cloned()
        .collect::<Vec<_>>();
    let developer_instructions = join_sections(&developer_sections);
    let user_instructions = bundle
        .ir
        .user_project_context
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    CodexThreadPromptRender {
        prompt_override_enabled: base_instructions.is_some()
            || developer_instructions.is_some()
            || user_instructions.is_some(),
        base_instructions,
        developer_instructions,
        user_instructions,
    }
}

pub fn render_codex_turn_prompt(bundle: &CompiledTurnPromptBundle) -> CodexTurnPromptRender {
    CodexTurnPromptRender {
        developer_instructions: join_sections(&bundle.ir.tool_usage_protocol),
        input_prefix: join_sections(&bundle.ir.turn_augment),
    }
}

pub fn render_codex_prompt_envelope(envelope: &InhousePromptEnvelope) -> CodexThreadPromptRender {
    let base_instructions = join_envelope_blocks(&envelope.blocks, &[InhousePromptLayer::System]);
    let developer_instructions = join_envelope_blocks(
        &envelope.blocks,
        &[
            InhousePromptLayer::Developer,
            InhousePromptLayer::ToolUsageProtocol,
        ],
    );
    let user_instructions =
        join_envelope_blocks(&envelope.blocks, &[InhousePromptLayer::UserProjectContext]);
    CodexThreadPromptRender {
        prompt_override_enabled: base_instructions.is_some()
            || developer_instructions.is_some()
            || user_instructions.is_some(),
        base_instructions,
        developer_instructions,
        user_instructions,
    }
}

pub fn render_codex_turn_prompt_envelope(
    envelope: &InhousePromptEnvelope,
) -> CodexTurnPromptRender {
    CodexTurnPromptRender {
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

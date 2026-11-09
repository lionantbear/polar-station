use crate::model::{
    CompiledThreadPromptBundle, CompiledTurnPromptBundle, PiThreadPromptRender, PiTurnPromptRender,
};
use polar_inhouse_contract::{InhousePromptBlock, InhousePromptEnvelope, InhousePromptLayer};

fn render_section_group(title: &str, bodies: Vec<String>) -> Option<String> {
    if bodies.is_empty() {
        return None;
    }
    Some(format!("## {title}\n\n{}", bodies.join("\n\n")))
}

fn render_named_sections(sections: &[crate::model::CompiledPromptSection]) -> Vec<String> {
    sections
        .iter()
        .map(|section| format!("### {}\n{}", section.title.trim(), section.body_text.trim()))
        .collect()
}

pub fn render_pi_thread_prompt(bundle: &CompiledThreadPromptBundle) -> PiThreadPromptRender {
    let mut sections = Vec::new();
    if let Some(group) = render_section_group(
        "Base Identity",
        render_named_sections(&bundle.ir.base_identity),
    ) {
        sections.push(group);
    }
    let developer_sections = bundle
        .ir
        .developer_contract
        .iter()
        .chain(bundle.ir.tool_usage_protocol.iter())
        .cloned()
        .collect::<Vec<_>>();
    if let Some(group) = render_section_group(
        "Developer Contract",
        render_named_sections(&developer_sections),
    ) {
        sections.push(group);
    }
    if let Some(user_project_context) = bundle
        .ir
        .user_project_context
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        sections.push(format!("## User Project Context\n\n{user_project_context}"));
    }
    PiThreadPromptRender {
        system_prompt: sections.join("\n\n"),
    }
}

pub fn render_pi_turn_prompt(bundle: &CompiledTurnPromptBundle) -> PiTurnPromptRender {
    let mut sections = Vec::new();
    if let Some(group) = render_section_group(
        "Tool Usage Protocol",
        render_named_sections(&bundle.ir.tool_usage_protocol),
    ) {
        sections.push(group);
    }
    if let Some(group) = render_section_group(
        "Turn Augment",
        render_named_sections(&bundle.ir.turn_augment),
    ) {
        sections.push(group);
    }
    PiTurnPromptRender {
        input_prefix: sections.join("\n\n"),
    }
}

pub fn render_pi_prompt_envelope(envelope: &InhousePromptEnvelope) -> PiThreadPromptRender {
    let mut sections = Vec::new();
    if let Some(group) = render_envelope_group(
        "Base Identity",
        &envelope.blocks,
        &[InhousePromptLayer::System],
    ) {
        sections.push(group);
    }
    if let Some(group) = render_envelope_group(
        "Developer Contract",
        &envelope.blocks,
        &[
            InhousePromptLayer::Developer,
            InhousePromptLayer::ToolUsageProtocol,
        ],
    ) {
        sections.push(group);
    }
    if let Some(group) = render_envelope_group(
        "User Project Context",
        &envelope.blocks,
        &[InhousePromptLayer::UserProjectContext],
    ) {
        sections.push(group);
    }
    PiThreadPromptRender {
        system_prompt: sections.join("\n\n"),
    }
}

pub fn render_pi_turn_prompt_envelope(envelope: &InhousePromptEnvelope) -> PiTurnPromptRender {
    let mut sections = Vec::new();
    if let Some(group) = render_envelope_group(
        "Tool Usage Protocol",
        &envelope.blocks,
        &[InhousePromptLayer::ToolUsageProtocol],
    ) {
        sections.push(group);
    }
    if let Some(group) = render_envelope_group(
        "Turn Augment",
        &envelope.blocks,
        &[InhousePromptLayer::TurnAugment],
    ) {
        sections.push(group);
    }
    PiTurnPromptRender {
        input_prefix: sections.join("\n\n"),
    }
}

fn render_envelope_group(
    title: &str,
    blocks: &[InhousePromptBlock],
    layers: &[InhousePromptLayer],
) -> Option<String> {
    let bodies = blocks
        .iter()
        .filter(|block| layers.contains(&block.layer))
        .map(|block| format!("### {}\n{}", block.block_id.trim(), block.body_text.trim()))
        .collect::<Vec<_>>();
    render_section_group(title, bodies)
}

use polar_inhouse_contract::{
    InhouseCanonicalThreadMessage, InhouseMessageProjection, InhousePromptBlock,
    InhousePromptEnvelope, InhousePromptLayer, InhousePromptSegment, PolarPromptProjection,
};
use polar_lgp_model::{
    ContextCompactionRecord, PromptActivationRecord, PromptAssetClass, PromptAssetRecord,
    PromptBackend, PromptLifecycle,
};
use tracing::debug;

use crate::model::{
    CompiledPromptSection, CompiledThreadPromptBundle, CompiledThreadPromptIr,
    CompiledTurnPromptBundle, CompiledTurnPromptIr, PromptCompileContext, PromptSemanticLayer,
};

fn section_order(asset_class: PromptAssetClass) -> usize {
    match asset_class {
        PromptAssetClass::BaseIdentity => 0,
        PromptAssetClass::ManagementSemantics => 1,
        PromptAssetClass::ChatBridgeGuide => 2,
        PromptAssetClass::SystemOps => 3,
        PromptAssetClass::PromptAuthor => 4,
        PromptAssetClass::BackendOverrideCodex => 5,
        PromptAssetClass::BackendOverridePi => 6,
        PromptAssetClass::BackendOverrideAdam => 7,
    }
}

fn prompt_layer_order(layer: InhousePromptLayer) -> usize {
    match layer {
        InhousePromptLayer::System => 0,
        InhousePromptLayer::Developer => 1,
        InhousePromptLayer::ToolUsageProtocol => 2,
        InhousePromptLayer::UserProjectContext => 3,
        InhousePromptLayer::TurnAugment => 4,
    }
}

/// Builds the backend-neutral prompt envelope consumed by Adam, Codex, and Pi.
///
/// Canonical LGP messages are rendered once into stable segments. Changing which
/// message is the current turn therefore changes only the projection pointer,
/// not the bytes or hash of any existing message segment.
pub fn build_inhouse_prompt_envelope(
    blocks: Vec<InhousePromptBlock>,
    messages: &[InhouseCanonicalThreadMessage],
    current_turn_message_id: Option<&str>,
    compaction_epoch: i64,
) -> InhousePromptEnvelope {
    let segments = messages
        .iter()
        .map(|message| {
            InhousePromptSegment::from_lgp_message(
                message.message_id.clone(),
                message.role.clone(),
                "canonical",
                &message.text,
                compaction_epoch,
            )
        })
        .collect::<Vec<_>>();
    build_prompt_envelope_from_segments(blocks, segments, current_turn_message_id)
}

/// Builds the final shared backend envelope from canonical LGP compaction
/// truth. All semantic message segments are compiled once here; Adam, Codex,
/// and Pi may only map this envelope to provider wire fields.
pub fn build_compacted_inhouse_prompt_envelope(
    blocks: Vec<InhousePromptBlock>,
    compaction: &ContextCompactionRecord,
    canonical_messages: &[InhouseCanonicalThreadMessage],
    current_turn_message_id: Option<&str>,
) -> Result<InhousePromptEnvelope, String> {
    let projection = compile_compacted_prompt_projection(
        "",
        compaction,
        canonical_messages,
        current_turn_message_id,
    )?;
    Ok(build_prompt_envelope_from_segments(
        blocks,
        projection.segments,
        current_turn_message_id,
    ))
}

fn build_prompt_envelope_from_segments(
    mut blocks: Vec<InhousePromptBlock>,
    segments: Vec<InhousePromptSegment>,
    current_turn_message_id: Option<&str>,
) -> InhousePromptEnvelope {
    blocks.sort_by_key(|block| prompt_layer_order(block.layer));
    let compaction_epoch = segments
        .iter()
        .map(|segment| segment.compaction_epoch)
        .max()
        .unwrap_or_default();
    let projection =
        PolarPromptProjection::from_segments("", segments.clone(), current_turn_message_id);
    let message_projection = InhouseMessageProjection {
        projection_id: projection.projection_id,
        current_turn_message_id: current_turn_message_id.map(ToString::to_string),
        segments,
    };
    let block_identity = blocks
        .iter()
        .map(|block| {
            format!(
                "{}\u{1f}{}\u{1f}{}\u{1f}{}",
                prompt_layer_order(block.layer),
                block.block_id,
                block.source_ref,
                block.body_text
            )
        })
        .collect::<Vec<_>>()
        .join("\u{1e}");
    let block_hash = InhousePromptSegment::from_rendered_source(
        "prompt-envelope-blocks",
        "runtime:prompt-envelope-blocks",
        "system",
        block_identity,
        compaction_epoch,
    )
    .segment_hash;
    InhousePromptEnvelope {
        envelope_id: format!(
            "inhouse-prompt-envelope:{}:{}",
            block_hash, message_projection.projection_id
        ),
        blocks,
        message_projection: Some(message_projection),
    }
}

/// Projects canonical LGP compaction truth plus the retained message tail into
/// deterministic model-visible bytes.
///
/// Messages preceding `first_kept_message_id` are never rendered. The current
/// turn remains an out-of-band pointer appended by `PolarPromptProjection`, so
/// it cannot change the stable hash of retained LGP segments.
pub fn compile_compacted_prompt_projection(
    prefix: impl AsRef<str>,
    compaction: &ContextCompactionRecord,
    canonical_messages: &[InhouseCanonicalThreadMessage],
    current_turn_message_id: Option<&str>,
) -> Result<PolarPromptProjection, String> {
    compaction.validate().map_err(|error| error.to_string())?;
    let first_kept_index = canonical_messages
        .iter()
        .position(|message| message.message_id == compaction.first_kept_message_id)
        .ok_or_else(|| {
            format!(
                "first kept LGP message {} is absent from canonical input",
                compaction.first_kept_message_id
            )
        })?;
    let retained_tail = &canonical_messages[first_kept_index..];
    if let Some(current_turn_message_id) = current_turn_message_id {
        if !retained_tail
            .iter()
            .any(|message| message.message_id == current_turn_message_id)
        {
            return Err(format!(
                "current turn {current_turn_message_id} is outside the retained LGP tail"
            ));
        }
    }

    let rendered_compaction = format!(
        "<lgp-compaction id=\"{}\" epoch=\"{}\">\n{}\n</lgp-compaction>",
        escape_prompt_attribute(&compaction.compaction_id),
        compaction.context_compaction_epoch,
        compaction.summary_text.trim()
    );
    let mut segments = vec![InhousePromptSegment::from_rendered_source(
        format!("lgp-compaction:{}", compaction.compaction_id),
        format!("lgp:compaction:{}", compaction.compaction_id),
        "system",
        rendered_compaction,
        compaction.context_compaction_epoch,
    )];
    segments.extend(retained_tail.iter().map(|message| {
        InhousePromptSegment::from_lgp_message(
            message.message_id.clone(),
            message.role.clone(),
            "canonical",
            &message.text,
            compaction.context_compaction_epoch,
        )
    }));

    Ok(PolarPromptProjection::from_segments(
        prefix,
        segments,
        current_turn_message_id,
    ))
}

fn escape_prompt_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn backend_override_class(backend: PromptBackend) -> PromptAssetClass {
    match backend {
        PromptBackend::Codex => PromptAssetClass::BackendOverrideCodex,
        PromptBackend::Pi => PromptAssetClass::BackendOverridePi,
        PromptBackend::Adam => PromptAssetClass::BackendOverrideAdam,
    }
}

fn activation_matches(
    activation: &PromptActivationRecord,
    context: &PromptCompileContext,
    backend: PromptBackend,
) -> bool {
    activation
        .target_backend
        .is_none_or(|value| value == backend)
        && activation
            .target_inhouse_polar_id
            .as_deref()
            .is_none_or(|value| value == context.inhouse_polar_id.as_str())
        && activation
            .target_system_role
            .is_none_or(|value| Some(value) == context.system_role)
        && activation
            .target_management_class
            .as_deref()
            .is_none_or(|value| Some(value) == context.management_class.as_deref())
        && activation
            .target_channel_id
            .as_deref()
            .is_none_or(|value| Some(value) == context.channel_id.as_deref())
        && activation
            .target_thread_id
            .as_deref()
            .is_none_or(|value| Some(value) == context.thread_id.as_deref())
}

fn activation_specificity(activation: &PromptActivationRecord) -> usize {
    usize::from(activation.target_backend.is_some())
        + usize::from(activation.target_management_class.is_some()) * 2
        + usize::from(activation.target_system_role.is_some()) * 4
        + usize::from(activation.target_inhouse_polar_id.is_some()) * 8
        + usize::from(activation.target_channel_id.is_some()) * 16
        + usize::from(activation.target_thread_id.is_some()) * 32
}

fn dynamic_tool_section(context: &PromptCompileContext) -> Option<CompiledPromptSection> {
    if context.available_dynamic_tools.is_empty() {
        return None;
    }
    let has_bundle_tools = context
        .available_dynamic_tools
        .iter()
        .any(|tool| tool.name.starts_with("bundle_"));
    let lines = context
        .available_dynamic_tools
        .iter()
        .map(|tool| format!("- {}: {}", tool.name, tool.description))
        .collect::<Vec<_>>()
        .join("\n");
    let bundle_authoring_protocol = has_bundle_tools.then(|| {
        "\n\nBundle authoring protocol:\n\
- Call bundle_validate before committing generated bundles.\n\
- Treat ok:false validation output as path-addressed repair instructions; repair the same draft and call bundle_validate again.\n\
- Call bundle_preview before commit/apply and treat preview warnings as semantic repair feedback.\n\
- Call bundle_commit_attachment exactly once with the final full bundle; it commits a real LGP message attachment. Use the returned bundle_attachment_ref plus bundle_sha256 only in tool calls and machine reports, not in human-facing final text.\n\
- If a committed bundle must be used from a different prompt-admin thread, call bundle_forward_attachment_ref and use the returned target-thread ref.\n\
- Prefer bundle_apply_local_checked_ref for committed local apply; do not repeat full bundle JSON or raw attref values in final replies.\n\
- Treat bundle_create_draft as an empty PolarBundle shape helper with field-contract notes, not a product-specific example.\n\
- Do not invent system_role values; public product actors usually omit system_role.\n\
- Preserve exact product ids/names given by the human, including bundle_id, inhouse_polar_id, channel_id, compile target name, deployment target name, and node name. Do not replace literal ids with prettier ids.\n\
- Use exact PolarBundle field names. backend_profiles[] uses backend_profile_id, display_name, backend_driver, backend_config_json; inhouse_polars[] uses inhouse_polar_id, polar_name, active_backend_profile_id, and optional system_role; do not use id/name/display_name/type on inhouse_polars.\n\
- channels[] uses channel_id, title, partner_inhouse_polar_id, and access_plane; do not use id/name/channel_type/semantic.\n\
- prompts.assets[] and prompts.activations[] require updated_by_inhouse_polar_id. lifecycle is thread_init or turn_augment. asset_class is base_identity, management_semantics, chat_bridge_guide, system_ops, prompt_author, backend_override_codex, backend_override_pi, or backend_override_adam. Use target_inhouse_polar_id for an in-house Polar profile layer: name, basic mission, personality, and default behavior that must follow the actor across any thread. Use target_management_class for thread or channel scene semantics. For behavior, safety, admin, or permission boundaries, create a separate turn_augment management_semantics prompt asset and activation; do not store those boundaries only in service_contract or only in base_identity.\n\
- prompts.compile_targets[] uses name, inhouse_polar_id, management_class, and backend. backend is codex, pi, or adam, not default.\n\
- Deployment targets are objects with name, node, and role."
            .to_string()
    });
    Some(CompiledPromptSection {
        asset_class: backend_override_class(context.backend),
        asset_id: None,
        semantic_layer: PromptSemanticLayer::ToolUsageProtocol,
        title: "Dynamic Polar Tools".to_string(),
        body_text: format!(
            "Dynamic Polar tools available for this turn:\n{lines}\nUse Polar tools first when the operator asks about Polar channels, threads, sessions, requests, prompts, management policy, or maintenance state.{}",
            bundle_authoring_protocol.unwrap_or_default()
        ),
    })
}

fn semantic_layer_for_asset(
    lifecycle: PromptLifecycle,
    asset_class: PromptAssetClass,
) -> PromptSemanticLayer {
    match lifecycle {
        PromptLifecycle::ThreadInit => match asset_class {
            PromptAssetClass::BaseIdentity => PromptSemanticLayer::BaseIdentity,
            _ => PromptSemanticLayer::DeveloperContract,
        },
        PromptLifecycle::TurnAugment => match asset_class {
            PromptAssetClass::BackendOverrideCodex
            | PromptAssetClass::BackendOverridePi
            | PromptAssetClass::BackendOverrideAdam => PromptSemanticLayer::ToolUsageProtocol,
            _ => PromptSemanticLayer::TurnAugment,
        },
    }
}

fn thread_prompt_ir(
    sections: &[CompiledPromptSection],
    context: &PromptCompileContext,
) -> CompiledThreadPromptIr {
    CompiledThreadPromptIr {
        base_identity: sections
            .iter()
            .filter(|section| section.semantic_layer == PromptSemanticLayer::BaseIdentity)
            .cloned()
            .collect(),
        developer_contract: sections
            .iter()
            .filter(|section| section.semantic_layer == PromptSemanticLayer::DeveloperContract)
            .cloned()
            .collect(),
        tool_usage_protocol: sections
            .iter()
            .filter(|section| section.semantic_layer == PromptSemanticLayer::ToolUsageProtocol)
            .cloned()
            .collect(),
        user_project_context: {
            let parts = sections
                .iter()
                .filter(|section| section.semantic_layer == PromptSemanticLayer::UserProjectContext)
                .map(|section| section.body_text.trim())
                .chain(context.user_project_context.as_deref())
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>();
            (!parts.is_empty()).then(|| parts.join("\n\n"))
        },
    }
}

fn turn_prompt_ir(sections: &[CompiledPromptSection]) -> CompiledTurnPromptIr {
    CompiledTurnPromptIr {
        tool_usage_protocol: sections
            .iter()
            .filter(|section| section.semantic_layer == PromptSemanticLayer::ToolUsageProtocol)
            .cloned()
            .collect(),
        turn_augment: sections
            .iter()
            .filter(|section| section.semantic_layer == PromptSemanticLayer::TurnAugment)
            .cloned()
            .collect(),
    }
}

fn prompt_debug_enabled() -> bool {
    matches!(
        std::env::var("POLAR_DEBUG_PROMPTS"),
        Ok(value)
            if value == "1"
                || value.eq_ignore_ascii_case("true")
                || value.eq_ignore_ascii_case("yes")
    )
}

fn compile_sections_for_lifecycle(
    assets: &[PromptAssetRecord],
    activations: &[PromptActivationRecord],
    context: &PromptCompileContext,
    lifecycle: PromptLifecycle,
    include_dynamic_tools: bool,
) -> Vec<CompiledPromptSection> {
    let mut sections = Vec::new();
    let backend_override_class = backend_override_class(context.backend);

    if lifecycle == PromptLifecycle::ThreadInit {
        sections.extend(context.session_prompt_blocks.iter().filter_map(|block| {
            let semantic_layer = match block.layer {
                InhousePromptLayer::System => PromptSemanticLayer::BaseIdentity,
                InhousePromptLayer::Developer => PromptSemanticLayer::DeveloperContract,
                InhousePromptLayer::ToolUsageProtocol => PromptSemanticLayer::ToolUsageProtocol,
                InhousePromptLayer::UserProjectContext => PromptSemanticLayer::UserProjectContext,
                InhousePromptLayer::TurnAugment => return None,
            };
            Some(CompiledPromptSection {
                asset_class: backend_override_class,
                asset_id: Some(block.source_ref.clone()),
                semantic_layer,
                title: block.block_id.clone(),
                body_text: block.body_text.trim().to_string(),
            })
        }));
    }

    for asset_class in [
        PromptAssetClass::BaseIdentity,
        PromptAssetClass::ManagementSemantics,
        PromptAssetClass::ChatBridgeGuide,
        PromptAssetClass::SystemOps,
        PromptAssetClass::PromptAuthor,
        backend_override_class,
    ] {
        let activation = activations
            .iter()
            .filter(|row| row.lifecycle == lifecycle)
            .filter(|row| row.asset_class == asset_class)
            .filter(|row| activation_matches(row, context, context.backend))
            .max_by_key(|row| activation_specificity(row));
        let Some(activation) = activation else {
            if prompt_debug_enabled() {
                debug!(
                    event = "prompt_compile_skipped_class",
                    lifecycle = lifecycle.as_str(),
                    backend = context.backend.as_str(),
                    inhouse_polar_id = %context.inhouse_polar_id,
                    system_role = ?context.system_role.map(|value| value.as_str()),
                    management_class = ?context.management_class,
                    asset_class = asset_class.as_str(),
                    reason = "no_matching_activation",
                );
            }
            continue;
        };
        let Some(asset) = assets.iter().find(|row| {
            row.asset_id == activation.asset_id
                && row.lifecycle == lifecycle
                && row.asset_class == activation.asset_class
        }) else {
            if prompt_debug_enabled() {
                debug!(
                    event = "prompt_compile_skipped_class",
                    lifecycle = lifecycle.as_str(),
                    backend = context.backend.as_str(),
                    inhouse_polar_id = %context.inhouse_polar_id,
                    system_role = ?context.system_role.map(|value| value.as_str()),
                    management_class = ?context.management_class,
                    asset_class = asset_class.as_str(),
                    activation_asset_id = %activation.asset_id,
                    reason = "missing_matching_asset",
                );
            }
            continue;
        };
        if prompt_debug_enabled() {
            debug!(
                event = "prompt_compile_selected_asset",
                lifecycle = lifecycle.as_str(),
                backend = context.backend.as_str(),
                inhouse_polar_id = %context.inhouse_polar_id,
                system_role = ?context.system_role.map(|value| value.as_str()),
                management_class = ?context.management_class,
                asset_class = asset_class.as_str(),
                asset_id = %asset.asset_id,
            );
        }
        sections.push(CompiledPromptSection {
            asset_class,
            asset_id: Some(asset.asset_id.clone()),
            semantic_layer: semantic_layer_for_asset(lifecycle, asset_class),
            title: asset.title.clone(),
            body_text: asset.body_text.trim().to_string(),
        });
    }

    if lifecycle == PromptLifecycle::TurnAugment {
        sections.extend(context.supplemental_turn_sections.iter().map(|section| {
            CompiledPromptSection {
                asset_class: section.asset_class,
                asset_id: None,
                semantic_layer: PromptSemanticLayer::TurnAugment,
                title: section.title.clone(),
                body_text: section.body_text.trim().to_string(),
            }
        }));
    }

    if include_dynamic_tools {
        if let Some(dynamic_section) = dynamic_tool_section(context) {
            sections.push(dynamic_section);
        }
    }

    sections.sort_by_key(|section| section_order(section.asset_class));
    if prompt_debug_enabled() {
        let section_order = sections
            .iter()
            .map(|section| format!("{}:{}", section.asset_class.as_str(), section.title))
            .collect::<Vec<_>>();
        debug!(
            event = if lifecycle == PromptLifecycle::ThreadInit {
                "prompt_compile_thread_bundle"
            } else {
                "prompt_compile_turn_bundle"
            },
            lifecycle = lifecycle.as_str(),
            backend = context.backend.as_str(),
            inhouse_polar_id = %context.inhouse_polar_id,
            system_role = ?context.system_role.map(|value| value.as_str()),
            management_class = ?context.management_class,
            section_order = ?section_order,
        );
    }
    sections
}

pub fn compile_thread_prompt_bundle(
    assets: &[PromptAssetRecord],
    activations: &[PromptActivationRecord],
    context: &PromptCompileContext,
) -> CompiledThreadPromptBundle {
    let sections = compile_sections_for_lifecycle(
        assets,
        activations,
        context,
        PromptLifecycle::ThreadInit,
        false,
    );
    let combined_text = sections
        .iter()
        .map(|section| format!("## {}\n{}", section.title.trim(), section.body_text.trim()))
        .collect::<Vec<_>>()
        .join("\n\n");
    let ir = thread_prompt_ir(&sections, context);

    CompiledThreadPromptBundle {
        sections,
        ir,
        combined_text,
    }
}

pub fn compile_turn_prompt_bundle(
    assets: &[PromptAssetRecord],
    activations: &[PromptActivationRecord],
    context: &PromptCompileContext,
) -> CompiledTurnPromptBundle {
    let sections = compile_sections_for_lifecycle(
        assets,
        activations,
        context,
        PromptLifecycle::TurnAugment,
        true,
    );
    let combined_text = sections
        .iter()
        .map(|section| format!("## {}\n{}", section.title.trim(), section.body_text.trim()))
        .collect::<Vec<_>>()
        .join("\n\n");
    let ir = turn_prompt_ir(&sections);

    CompiledTurnPromptBundle {
        sections,
        ir,
        combined_text,
    }
}



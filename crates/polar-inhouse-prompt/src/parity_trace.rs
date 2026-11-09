use polar_inhouse_contract::{
    InhousePromptAdapterReceipt, InhousePromptEnvelope, InhousePromptParityRendererHashes,
    InhousePromptParityTrace, InhousePromptRendererHashes,
};

use crate::{
    render_adam_prompt_envelope, render_adam_turn_prompt_envelope, render_codex_prompt_envelope,
    render_codex_turn_prompt_envelope, render_pi_prompt_envelope, render_pi_turn_prompt_envelope,
};

fn render_hash<T: serde::Serialize>(value: &T) -> String {
    let bytes = serde_json::to_vec(value).expect("prompt render types are serializable");
    format!("blake3:{}", blake3::hash(&bytes).to_hex())
}

fn renderer_hashes<T: serde::Serialize, U: serde::Serialize>(
    thread: &T,
    turn: &U,
) -> InhousePromptRendererHashes {
    InhousePromptRendererHashes {
        thread_render_hash: render_hash(thread),
        turn_render_hash: render_hash(turn),
    }
}

/// Build redacted cross-backend parity evidence from the exact envelope used
/// by a runtime turn. Renderer bytes never leave this function.
pub fn redacted_prompt_parity_trace(envelope: &InhousePromptEnvelope) -> InhousePromptParityTrace {
    let mut canonical_source_refs = Vec::new();
    for source_ref in envelope.blocks.iter().map(|block| &block.source_ref).chain(
        envelope.message_projection.iter().flat_map(|projection| {
            projection
                .segments
                .iter()
                .map(|segment| &segment.source_ref)
        }),
    ) {
        if !source_ref.trim().is_empty() && !canonical_source_refs.contains(source_ref) {
            canonical_source_refs.push(source_ref.clone());
        }
    }

    let adam_thread = render_adam_prompt_envelope(envelope);
    let adam_turn = render_adam_turn_prompt_envelope(envelope);
    let codex_thread = render_codex_prompt_envelope(envelope);
    let codex_turn = render_codex_turn_prompt_envelope(envelope);
    let pi_thread = render_pi_prompt_envelope(envelope);
    let pi_turn = render_pi_turn_prompt_envelope(envelope);
    InhousePromptParityTrace {
        envelope_id: envelope.envelope_id.clone(),
        canonical_source_ref_count: canonical_source_refs.len(),
        canonical_source_refs,
        renderer_hashes: InhousePromptParityRendererHashes {
            adam: renderer_hashes(&adam_thread, &adam_turn),
            codex: renderer_hashes(&codex_thread, &codex_turn),
            pi: renderer_hashes(&pi_thread, &pi_turn),
        },
    }
}

/// Build a redacted receipt for the exact backend adapter selected at runtime.
/// The selected renderer hash is copied from the same cross-backend trace so
/// the receipt cannot drift from the envelope-level parity evidence.
pub fn redacted_prompt_adapter_receipt(
    envelope: &InhousePromptEnvelope,
    backend_driver: &str,
    dispatch_status: &str,
    provider_contact_attempted: Option<bool>,
) -> Result<InhousePromptAdapterReceipt, String> {
    let backend_driver = backend_driver.trim();
    let trace = redacted_prompt_parity_trace(envelope);
    let renderer_hashes = match backend_driver {
        "adam" => trace.renderer_hashes.adam.clone(),
        "codex" => trace.renderer_hashes.codex.clone(),
        "pi" => trace.renderer_hashes.pi.clone(),
        unsupported => {
            return Err(format!(
                "unsupported prompt adapter receipt backend '{unsupported}'"
            ))
        }
    };
    Ok(InhousePromptAdapterReceipt {
        schema: "polar.inhouse-prompt-adapter-receipt.v1".to_string(),
        backend_driver: backend_driver.to_string(),
        dispatch_status: dispatch_status.to_string(),
        envelope_id: trace.envelope_id,
        canonical_source_refs: trace.canonical_source_refs,
        canonical_source_ref_count: trace.canonical_source_ref_count,
        renderer_hashes,
        provider_contact_attempted,
    })
}

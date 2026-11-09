//! Deterministic lifecycle-specific prompt compilation. No persistence, tool execution,
//! model discovery or session lifecycle. Base blocks remain source-owned.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{bail, ensure, Context, Result};
use polar_inhouse_contract::InhousePromptBlock;
use polar_lgp_model::{
    InhousePromptLayer, PromptAssetRevision, PromptBindingRecord, PromptBindingTargetKind,
    PromptLifecycle,
};
pub use polar_lgp_model::{PromptAssetKey, PromptTarget};

pub use polar_inhouse_contract::BoundPromptTool;

/// Scene identities and model identity are resolved by their owning domains.
/// Lifecycle is explicit; this is not a session or tool refresh mechanism.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptPreparationContext {
    pub lifecycle: PromptLifecycle,
    pub scene_targets: Vec<PromptTarget>,
    pub backend_driver: String,
    pub model_provider: Option<String>,
    pub model_id: Option<String>,
    pub tools: Vec<BoundPromptTool>,
    pub base_blocks: Vec<InhousePromptBlock>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PromptSnapshot {
    pub bindings: Vec<PromptBindingRecord>,
    pub assets: Vec<PromptAssetRevision>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptExclusionReason {
    Disabled,
    Lifecycle,
    Target,
    RequiredTools(Vec<String>),
    Backend,
    Provider,
    Model,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptExclusion {
    pub binding_id: String,
    pub reason: PromptExclusionReason,
}

/// Full binding preserves the target, ordering and model conditions; tools
/// explain which actual contributions made a guide applicable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptOrigin {
    pub binding: PromptBindingRecord,
    pub asset_ref: polar_lgp_model::PromptAssetRef,
    pub contributing_tool_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptBlockProvenance {
    pub block_id: String,
    pub base_source_ref: Option<String>,
    /// One root-to-leaf chain per surviving contribution before asset dedup.
    pub chains: Vec<Vec<PromptOrigin>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PreparedPrompt {
    pub blocks: Vec<InhousePromptBlock>,
    /// Same order as blocks, including source-owned base blocks.
    pub provenance: Vec<PromptBlockProvenance>,
    pub excluded: Vec<PromptExclusion>,
}

fn layer_rank(layer: InhousePromptLayer) -> u8 {
    match layer {
        InhousePromptLayer::System => 0,
        InhousePromptLayer::Developer => 1,
        InhousePromptLayer::ToolUsageProtocol => 2,
        InhousePromptLayer::UserProjectContext => 3,
        InhousePromptLayer::TurnAugment => 4,
    }
}

fn source_rank(kind: PromptBindingTargetKind) -> u8 {
    match kind {
        PromptBindingTargetKind::Role => 1,
        PromptBindingTargetKind::InhousePolar => 2,
        PromptBindingTargetKind::Channel => 3,
        PromptBindingTargetKind::Thread => 4,
        PromptBindingTargetKind::Tool
        | PromptBindingTargetKind::ToolFamily
        | PromptBindingTargetKind::Gear => 5,
    }
}

impl PromptPreparationContext {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.backend_driver.trim().is_empty(),
            "backend_driver is required"
        );
        for value in [&self.model_provider, &self.model_id].into_iter().flatten() {
            ensure!(!value.trim().is_empty(), "empty resolved model identity");
        }
        ensure!(
            self.model_id.is_none() || self.model_provider.is_some(),
            "resolved model_id requires model_provider"
        );
        for target in &self.scene_targets {
            target.validate()?;
            ensure!(
                source_rank(target.kind) < 5,
                "tool targets must come from actual tool contributions"
            );
        }
        for tool in &self.tools {
            ensure!(
                !tool.ai_name.is_empty()
                    && tool.ai_name.len() <= 64
                    && tool
                        .ai_name
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || c == b'_'),
                "invalid bound tool AI name"
            );
            for id in std::iter::once(&tool.tool_id)
                .chain(&tool.family_ids)
                .chain(&tool.gear_ids)
            {
                ensure!(
                    !id.trim().is_empty(),
                    "empty bound tool contribution identity"
                );
            }
        }
        let mut ids = BTreeSet::new();
        for block in &self.base_blocks {
            ensure!(
                self.lifecycle != PromptLifecycle::ThreadInit
                    || block.layer != InhousePromptLayer::TurnAugment,
                "initial base block cannot be turn_augment: {}",
                block.block_id
            );
            ensure!(
                !block.block_id.trim().is_empty() && ids.insert(&block.block_id),
                "empty/duplicate base block id: {}",
                block.block_id
            );
        }
        Ok(())
    }

    /// Targeted repository queries only, with owner intact. This is not a tool
    /// catalogue: the caller already supplied the actual bound contributions.
    pub fn targets(&self) -> Vec<PromptTarget> {
        let mut targets: BTreeSet<_> = self.scene_targets.iter().cloned().collect();
        for tool in &self.tools {
            targets.insert(PromptTarget::new(
                PromptBindingTargetKind::Tool,
                None,
                &tool.tool_id,
            ));
            for family in &tool.family_ids {
                targets.insert(PromptTarget::new(
                    PromptBindingTargetKind::ToolFamily,
                    None,
                    family,
                ));
            }
            for gear in &tool.gear_ids {
                targets.insert(PromptTarget::new(PromptBindingTargetKind::Gear, None, gear));
            }
        }
        targets.into_iter().collect()
    }
}

fn contributing_tools(
    ctx: &PromptPreparationContext,
    binding: &PromptBindingRecord,
) -> Vec<String> {
    ctx.tools
        .iter()
        .filter(|tool| match binding.target_kind {
            PromptBindingTargetKind::Tool => tool.tool_id == binding.target_id,
            PromptBindingTargetKind::ToolFamily => tool.family_ids.contains(&binding.target_id),
            PromptBindingTargetKind::Gear => tool.gear_ids.contains(&binding.target_id),
            _ => false,
        })
        .map(|tool| tool.tool_id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn exclusion(
    ctx: &PromptPreparationContext,
    binding: &PromptBindingRecord,
) -> Result<Option<PromptExclusionReason>> {
    use PromptExclusionReason as Why;
    if !binding.enabled {
        return Ok(Some(Why::Disabled));
    }
    if binding.lifecycle != ctx.lifecycle {
        return Ok(Some(Why::Lifecycle));
    }
    let target_matches = if source_rank(binding.target_kind) == 5 {
        binding.target_owner_id.is_none() && !contributing_tools(ctx, binding).is_empty()
    } else {
        ctx.scene_targets.contains(&binding.target())
    };
    if !target_matches {
        return Ok(Some(Why::Target));
    }
    let missing: Vec<_> = binding
        .required_tool_ids
        .iter()
        .filter(|id| !ctx.tools.iter().any(|tool| &tool.tool_id == *id))
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if !missing.is_empty() {
        return Ok(Some(Why::RequiredTools(missing)));
    }
    if binding
        .backend_driver
        .as_ref()
        .is_some_and(|driver| driver != &ctx.backend_driver)
    {
        return Ok(Some(Why::Backend));
    }
    // Check every known exclusion before reporting an unknown model condition.
    if let (Some(wanted), Some(actual)) = (&binding.model_provider, &ctx.model_provider) {
        if wanted != actual {
            return Ok(Some(Why::Provider));
        }
    }
    if let (Some(wanted), Some(actual)) = (&binding.model_id, &ctx.model_id) {
        if wanted != actual {
            return Ok(Some(Why::Model));
        }
    }
    ensure!(
        binding.model_provider.is_none() || ctx.model_provider.is_some(),
        "binding {}: model_provider cannot be decided",
        binding.binding_id
    );
    ensure!(
        binding.model_id.is_none() || ctx.model_id.is_some(),
        "binding {}: model_id cannot be decided",
        binding.binding_id
    );
    Ok(None)
}

/// Selection is also used by the service to load only exact selected versions.
/// Returns unique contributions in deterministic semantic/source/declared order.
pub fn select_prompt_bindings(
    ctx: &PromptPreparationContext,
    bindings: &[PromptBindingRecord],
) -> Result<(Vec<PromptBindingRecord>, Vec<PromptExclusion>)> {
    ctx.validate()?;
    let mut unique = BTreeMap::new();
    for binding in bindings {
        binding
            .validate()
            .with_context(|| format!("binding {}", binding.binding_id))?;
        if let Some(previous) = unique.insert(&binding.binding_id, binding) {
            ensure!(
                previous == binding,
                "conflicting discoveries of binding {}",
                binding.binding_id
            );
        }
    }
    let mut selected = Vec::new();
    let mut excluded = Vec::new();
    for binding in unique.into_values() {
        if let Some(reason) = exclusion(ctx, binding)? {
            excluded.push(PromptExclusion {
                binding_id: binding.binding_id.clone(),
                reason,
            });
        } else {
            selected.push(binding.clone());
        }
    }
    selected.sort_by(|a, b| {
        (
            layer_rank(a.prompt_layer),
            source_rank(a.target_kind),
            a.sort_order,
            &a.binding_id,
        )
            .cmp(&(
                layer_rank(b.prompt_layer),
                source_rank(b.target_kind),
                b.sort_order,
                &b.binding_id,
            ))
    });
    Ok((selected, excluded))
}

pub fn compile_prompt(
    ctx: &PromptPreparationContext,
    snapshot: &PromptSnapshot,
) -> Result<PreparedPrompt> {
    let (selected, excluded) = select_prompt_bindings(ctx, &snapshot.bindings)?;
    let mut assets = BTreeMap::new();
    for asset in &snapshot.assets {
        let key = PromptAssetKey {
            asset_id: asset.asset_id.clone(),
            revision: asset.revision,
        };
        if let Some(previous) = assets.insert(key, asset) {
            ensure!(
                previous == asset,
                "conflicting asset snapshot {}@{}",
                asset.asset_id,
                asset.revision
            );
        }
    }
    let mut slots: Vec<Vec<PromptOrigin>> = Vec::new();
    let mut roots: BTreeMap<String, usize> = BTreeMap::new();
    let mut replaced = BTreeSet::new();
    for binding in selected {
        let asset = assets.get(&binding.asset_key()).with_context(|| {
            format!(
                "binding {}: missing asset {}@{}",
                binding.binding_id, binding.asset_id, binding.asset_revision
            )
        })?;
        asset.validate().with_context(|| {
            format!(
                "binding {}: asset {}@{}",
                binding.binding_id, asset.asset_id, asset.revision
            )
        })?;
        let root = if let Some(target) = &binding.replaces_binding_id {
            let Some(&root) = roots.get(target) else {
                bail!("binding {}: replacement {} must be selected and earlier (no forward/cyclic replacement)", binding.binding_id, target);
            };
            let previous = slots[root].last().expect("populated root slot");
            ensure!(
                previous.binding.prompt_layer == binding.prompt_layer
                    && previous.binding.lifecycle == binding.lifecycle,
                "binding {}: replacement {} crosses layer/lifecycle",
                binding.binding_id,
                target
            );
            ensure!(
                replaced.insert(target.clone()),
                "binding {}: competing replacement of {}",
                binding.binding_id,
                target
            );
            root
        } else {
            slots.push(Vec::new());
            slots.len() - 1
        };
        roots.insert(binding.binding_id.clone(), root);
        let contributing_tool_ids = contributing_tools(ctx, &binding);
        slots[root].push(PromptOrigin {
            binding,
            asset_ref: asset.reference(),
            contributing_tool_ids,
        });
    }
    let mut output = PreparedPrompt {
        excluded,
        ..PreparedPrompt::default()
    };
    // Bases are never in replacement slots or the asset dedup map.
    for rank in 0..=4 {
        for block in ctx
            .base_blocks
            .iter()
            .filter(|block| layer_rank(block.layer) == rank)
        {
            output.provenance.push(PromptBlockProvenance {
                block_id: block.block_id.clone(),
                base_source_ref: Some(block.source_ref.clone()),
                chains: vec![],
            });
            output.blocks.push(block.clone());
        }
        let mut dedup = BTreeMap::new();
        for chain in slots
            .iter()
            .filter(|chain| layer_rank(chain[0].binding.prompt_layer) == rank)
        {
            let leaf = chain.last().expect("populated root slot");
            let key = (
                &leaf.asset_ref.asset_id,
                leaf.asset_ref.revision,
                &leaf.asset_ref.digest,
            );
            if let Some(&index) = dedup.get(&key) {
                let provenance: &mut PromptBlockProvenance = &mut output.provenance[index];
                provenance.chains.push(chain.clone());
            } else {
                let block_id = format!("binding:{}", chain[0].binding.binding_id);
                ensure!(
                    !ctx.base_blocks
                        .iter()
                        .any(|block| block.block_id == block_id),
                    "binding output collides with base block id: {block_id}"
                );
                let asset = assets[&leaf.binding.asset_key()];
                dedup.insert(key, output.blocks.len());
                output.blocks.push(InhousePromptBlock {
                    block_id: block_id.clone(),
                    layer: leaf.binding.prompt_layer,
                    source_ref: format!("{}@{}#{}", asset.asset_id, asset.revision, asset.digest),
                    body_text: render_tool_symbols(&asset.body_text, &ctx.tools)?,
                });
                output.provenance.push(PromptBlockProvenance {
                    block_id,
                    base_source_ref: None,
                    chains: vec![chain.clone()],
                });
            }
        }
    }
    Ok(output)
}

fn render_tool_symbols(body: &str, tools: &[BoundPromptTool]) -> Result<String> {
    let mut rendered = String::with_capacity(body.len());
    let mut rest = body;
    while let Some((prefix, tail)) = rest.split_once("{{tool:") {
        rendered.push_str(prefix);
        let (id, tail) = tail
            .split_once("}}")
            .context("unterminated tool reference")?;
        let tool = tools
            .iter()
            .find(|tool| tool.tool_id == id)
            .with_context(|| format!("instruction tool reference is not bound: {id}"))?;
        rendered.push_str(&tool.ai_name);
        rest = tail;
    }
    rendered.push_str(rest);
    Ok(rendered)
}

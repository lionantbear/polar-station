//! Versioned capability instructions. File access is supplied by the owning
//! native bundle or validated plugin package; no database or runtime discovery.
use crate::{
    direct_compiler::PromptSnapshot,
    service::{binding_record, PromptMutationContext},
};
use anyhow::{ensure, Context, Result};
use polar_lgp_model::{
    PackagedPromptMaterial, PromptAssetRevision, PromptBindingTargetKind, PromptChange,
};
use std::collections::{BTreeMap, BTreeSet};

/// Join declaration-provider shards of the same immutable package. Equal records
/// are shared; disagreement never becomes an implicit override. Target selection
/// remains in the prompt compiler, not the tool registry.
pub fn merge_materials(
    materials: &[PackagedPromptMaterial],
) -> Result<Vec<PackagedPromptMaterial>> {
    let mut sources: BTreeMap<String, PackagedPromptMaterial> = BTreeMap::new();
    for material in materials {
        ensure!(
            material.prompts.delete_assets.is_empty()
                && material.prompts.delete_binding_ids.is_empty(),
            "packaged instructions cannot delete runtime prompts"
        );
        let merged = sources
            .entry(material.source_id.clone())
            .or_insert_with(|| PackagedPromptMaterial {
                source_id: material.source_id.clone(),
                prompts: PromptChange::default(),
            });
        for asset in &material.prompts.assets {
            if let Some(existing) = merged
                .prompts
                .assets
                .iter()
                .find(|a| a.asset_id == asset.asset_id && a.revision == asset.revision)
            {
                ensure!(
                    existing == asset,
                    "conflicting packaged asset {}@{}",
                    asset.asset_id,
                    asset.revision
                );
            } else {
                merged.prompts.assets.push(asset.clone());
            }
        }
        for binding in &material.prompts.bindings {
            if let Some(existing) = merged
                .prompts
                .bindings
                .iter()
                .find(|b| b.binding_id == binding.binding_id)
            {
                ensure!(
                    existing == binding,
                    "conflicting packaged binding {}",
                    binding.binding_id
                );
            } else {
                merged.prompts.bindings.push(binding.clone());
            }
        }
    }
    let merged: Vec<_> = sources.into_values().collect();
    material_snapshot(&merged)?;
    Ok(merged)
}

pub fn validate_instruction_path(path: &str) -> Result<()> {
    ensure!(
        !path.is_empty()
            && !path.contains(['\\', ':'])
            && path
                .split('/')
                .all(|part| !part.is_empty() && part != "." && part != ".."),
        "instructions path must be package-relative: {path}"
    );
    Ok(())
}

/// TOML uses existing PromptChange fields, except asset body_path names a
/// packaged UTF-8 Markdown file instead of copying its body into the manifest.
pub fn load_instructions(
    source_id: &str,
    config: &str,
    mut read: impl FnMut(&str) -> Result<String>,
) -> Result<PackagedPromptMaterial> {
    let mut document: toml::Value =
        toml::from_str(config).context("parse instructions configuration")?;
    if let Some(assets) = document
        .get_mut("assets")
        .and_then(toml::Value::as_array_mut)
    {
        for asset in assets {
            let asset = asset
                .as_table_mut()
                .context("instructions asset must be a table")?;
            ensure!(
                !asset.contains_key("body_text"),
                "packaged instructions use body_path, not body_text"
            );
            let path = asset
                .remove("body_path")
                .context("instructions asset requires body_path")?;
            let path = path.as_str().context("body_path must be a string")?;
            validate_instruction_path(path)?;
            let body = read(path).with_context(|| format!("read instructions {path}"))?;
            ensure!(body.len() <= 1024 * 1024, "instructions body exceeds 1 MiB");
            asset.insert("body_text".into(), toml::Value::String(body));
        }
    }
    let prompts: PromptChange = document
        .try_into()
        .context("decode instructions Asset/Binding")?;
    let material = PackagedPromptMaterial {
        source_id: source_id.into(),
        prompts,
    };
    material_snapshot(std::slice::from_ref(&material))?;
    Ok(material)
}

pub fn material_snapshot(materials: &[PackagedPromptMaterial]) -> Result<PromptSnapshot> {
    let mut snapshot = PromptSnapshot::default();
    let mut sources = BTreeSet::new();
    let mut asset_ids = BTreeSet::new();
    let mut binding_ids = BTreeSet::new();
    for material in materials {
        ensure!(
            !material.source_id.trim().is_empty() && sources.insert(&material.source_id),
            "empty or duplicate instructions source {}",
            material.source_id
        );
        ensure!(
            material.prompts.delete_assets.is_empty()
                && material.prompts.delete_binding_ids.is_empty(),
            "packaged instructions cannot delete runtime prompts"
        );
        // Deterministic provenance for release material, not an installation
        // timestamp or a fabricated user mutation; no records are persisted.
        let provenance = PromptMutationContext {
            actor_id: material.source_id.clone(),
            now: 0,
        };
        for input in &material.prompts.assets {
            ensure!(
                asset_ids.insert((input.asset_id.clone(), input.revision)),
                "duplicate packaged asset {}@{}",
                input.asset_id,
                input.revision
            );
            snapshot.assets.push(PromptAssetRevision::new(
                &input.asset_id,
                input.revision,
                &input.title,
                &input.body_text,
                &material.source_id,
                0,
            )?);
        }
        for input in &material.prompts.bindings {
            let mut ancestors = BTreeSet::new();
            let mut current = input;
            while let Some(parent) = &current.replaces_binding_id {
                ensure!(
                    ancestors.insert(&current.binding_id),
                    "packaged instructions replacement cycle"
                );
                current = material.prompts.bindings.iter()
                    .find(|binding| &binding.binding_id == parent)
                    .context("packaged instructions replacement must reference a binding in the same source")?;
            }
            ensure!(
                matches!(
                    input.target.kind,
                    PromptBindingTargetKind::Tool
                        | PromptBindingTargetKind::ToolFamily
                        | PromptBindingTargetKind::Gear
                ),
                "packaged instructions must target Tool, Tool Family or Gear"
            );
            ensure!(
                binding_ids.insert(input.binding_id.clone()),
                "duplicate packaged binding {}",
                input.binding_id
            );
            ensure!(
                material
                    .prompts
                    .assets
                    .iter()
                    .any(|a| a.asset_id == input.asset.asset_id
                        && a.revision == input.asset.revision),
                "packaged binding {} references missing asset",
                input.binding_id
            );
            let binding = binding_record(input, &provenance, None);
            binding.validate().map_err(anyhow::Error::msg)?;
            snapshot.bindings.push(binding);
        }
    }
    Ok(snapshot)
}

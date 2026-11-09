//! One domain entry for installation, management and prompt preparation.
//! Surface adapters supply intent and trusted context, never persistence rows.

use std::collections::BTreeSet;

use anyhow::{ensure, Context, Result};
use polar_lgp_model::{PromptAssetKey, PromptAssetRevision, PromptBindingRecord, PromptTarget};
use polar_lgp_store::prompt_repository::{PromptRead, PromptRepository, PromptTransaction};

use crate::direct_compiler::{
    compile_prompt, select_prompt_bindings, PreparedPrompt, PromptPreparationContext,
    PromptSnapshot,
};

/// Trusted caller identity/time, not fields decoded from a user payload.
#[derive(Debug, Clone)]
pub struct PromptMutationContext {
    pub actor_id: String,
    pub now: i64,
}

pub use polar_lgp_model::{PromptAssetInput, PromptBindingSpec, PromptChange};

pub(crate) fn binding_record(
    spec: &PromptBindingSpec,
    context: &PromptMutationContext,
    existing: Option<&PromptBindingRecord>,
) -> PromptBindingRecord {
    PromptBindingRecord {
        binding_id: spec.binding_id.clone(),
        target_kind: spec.target.kind,
        target_owner_id: spec.target.owner_id.clone(),
        target_id: spec.target.id.clone(),
        asset_id: spec.asset.asset_id.clone(),
        asset_revision: spec.asset.revision,
        lifecycle: spec.lifecycle,
        backend_driver: spec.backend_driver.clone(),
        model_provider: spec.model_provider.clone(),
        model_id: spec.model_id.clone(),
        prompt_layer: spec.prompt_layer,
        sort_order: spec.sort_order,
        replaces_binding_id: spec.replaces_binding_id.clone(),
        required_tool_ids: spec.required_tool_ids.clone(),
        enabled: spec.enabled,
        created_by: existing.map_or_else(
            || context.actor_id.clone(),
            |record| record.created_by.clone(),
        ),
        created_at: existing.map_or(context.now, |record| record.created_at),
        updated_by: context.actor_id.clone(),
        updated_at: context.now,
    }
}

pub struct PromptService<R> {
    repository: R,
}

/// Object-safe domain port for shared runtime resources and surface adapters.
/// The concrete service may own an Arc repository; callers never need SQLite
/// types or the generic repository's transaction callback in their API.
#[async_trait::async_trait]
pub trait PromptOperations: Send + Sync {
    async fn asset(&self, key: &PromptAssetKey) -> Result<Option<PromptAssetRevision>>;
    async fn bindings(&self, target: &PromptTarget) -> Result<Vec<PromptBindingRecord>>;
    async fn apply(&self, context: &PromptMutationContext, change: &PromptChange) -> Result<()>;
    async fn prepare(&self, context: &PromptPreparationContext) -> Result<PreparedPrompt>;
    async fn prepare_with_material(
        &self,
        context: &PromptPreparationContext,
        materials: &[polar_lgp_model::PackagedPromptMaterial],
    ) -> Result<PreparedPrompt> {
        ensure!(
            materials.is_empty(),
            "prompt service does not support packaged instructions"
        );
        self.prepare(context).await
    }
}

#[async_trait::async_trait]
impl<R: PromptRepository> PromptOperations for PromptService<R> {
    async fn asset(&self, key: &PromptAssetKey) -> Result<Option<PromptAssetRevision>> {
        PromptService::asset(self, key).await
    }
    async fn bindings(&self, target: &PromptTarget) -> Result<Vec<PromptBindingRecord>> {
        PromptService::bindings(self, target).await
    }
    async fn apply(&self, context: &PromptMutationContext, change: &PromptChange) -> Result<()> {
        PromptService::apply(self, context, change).await
    }
    async fn prepare(&self, context: &PromptPreparationContext) -> Result<PreparedPrompt> {
        PromptService::prepare(self, context).await
    }

    async fn prepare_with_material(
        &self,
        context: &PromptPreparationContext,
        materials: &[polar_lgp_model::PackagedPromptMaterial],
    ) -> Result<PreparedPrompt> {
        PromptService::prepare_with_material(self, context, materials).await
    }
}

impl<R: PromptRepository> PromptService<R> {
    pub fn new(repository: R) -> Self {
        Self { repository }
    }

    pub async fn asset(&self, key: &PromptAssetKey) -> Result<Option<PromptAssetRevision>> {
        ensure!(!key.asset_id.trim().is_empty(), "asset_id is required");
        let key = key.clone();
        self.repository.read(move |view| view.asset(&key)).await
    }

    pub async fn bindings(&self, target: &PromptTarget) -> Result<Vec<PromptBindingRecord>> {
        target.validate()?;
        let target = target.clone();
        self.repository
            .read(move |view| {
                let mut records = view.bindings_for_target(&target)?;
                records.sort_by(|a, b| a.binding_id.cmp(&b.binding_id));
                Ok(records)
            })
            .await
    }

    pub async fn apply(
        &self,
        context: &PromptMutationContext,
        change: &PromptChange,
    ) -> Result<()> {
        let context = context.clone();
        let change = change.clone();
        let targets = change
            .bindings
            .iter()
            .map(|binding| binding.target.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        self.repository
            .transaction(targets, move |tx| {
                apply_in_transaction(tx, &context, &change)
            })
            .await
    }

    /// Preview, initial-session and existing turn augmentation use this read-only
    /// path. Repository reads finish before the pure final compilation.
    pub async fn prepare(&self, context: &PromptPreparationContext) -> Result<PreparedPrompt> {
        self.prepare_with_material(context, &[]).await
    }

    pub async fn prepare_with_material(
        &self,
        context: &PromptPreparationContext,
        materials: &[polar_lgp_model::PackagedPromptMaterial],
    ) -> Result<PreparedPrompt> {
        context.validate()?;
        let packaged = crate::packaged::material_snapshot(materials)?;
        let resolved = context.clone();
        let snapshot = self
            .repository
            .read(move |view| {
                let context = &resolved;
                let mut bindings = packaged.bindings;
                for target in context.targets() {
                    bindings.extend(view.bindings_for_target(&target)?);
                }
                let (selected, _) = select_prompt_bindings(context, &bindings)?;
                let keys: BTreeSet<_> = selected
                    .iter()
                    .map(PromptBindingRecord::asset_key)
                    .collect();
                let mut assets = Vec::new();
                for key in keys {
                    // Missing versions are diagnosed by the compiler with binding ID.
                    let stored = view.asset(&key)?;
                    let shipped = packaged.assets.iter().find(|asset| {
                        asset.asset_id == key.asset_id && asset.revision == key.revision
                    });
                    ensure!(
                        stored.is_none() || shipped.is_none(),
                        "stored and packaged asset identity conflict: {}@{}",
                        key.asset_id,
                        key.revision
                    );
                    if let Some(asset) = shipped.cloned().or(stored) {
                        assets.push(asset);
                    }
                }
                Ok(PromptSnapshot { bindings, assets })
            })
            .await?;
        compile_prompt(context, &snapshot)
    }
}

fn validate_change_ids(change: &PromptChange) -> Result<()> {
    let mut assets = BTreeSet::new();
    for input in &change.assets {
        ensure!(
            assets.insert((&input.asset_id, input.revision)),
            "duplicate submitted asset {}@{}",
            input.asset_id,
            input.revision
        );
    }
    for key in &change.delete_assets {
        ensure!(!key.asset_id.trim().is_empty(), "empty deleted asset id");
        ensure!(
            assets.insert((&key.asset_id, key.revision)),
            "conflicting/duplicate asset change {}@{}",
            key.asset_id,
            key.revision
        );
    }
    let mut bindings = BTreeSet::new();
    for id in change
        .bindings
        .iter()
        .map(|spec| &spec.binding_id)
        .chain(&change.delete_binding_ids)
    {
        ensure!(
            !id.trim().is_empty() && bindings.insert(id),
            "empty/conflicting/duplicate binding change {id}"
        );
    }
    Ok(())
}

fn validate_references(view: &dyn PromptRead, binding: &PromptBindingRecord) -> Result<()> {
    let mut current = binding.clone();
    let mut visited = BTreeSet::new();
    loop {
        current
            .validate()
            .with_context(|| format!("binding {}", current.binding_id))?;
        ensure!(
            visited.insert(current.binding_id.clone()),
            "binding {}: replacement cycle at {}",
            binding.binding_id,
            current.binding_id
        );
        let asset = view.asset(&current.asset_key())?.with_context(|| {
            format!(
                "binding {}: missing asset {}@{}",
                current.binding_id, current.asset_id, current.asset_revision
            )
        })?;
        asset
            .validate()
            .with_context(|| format!("binding {}: invalid referenced asset", current.binding_id))?;
        let Some(target) = &current.replaces_binding_id else {
            return Ok(());
        };
        let previous = view.binding(target)?.with_context(|| {
            format!(
                "binding {}: missing replacement {target}",
                current.binding_id
            )
        })?;
        ensure!(
            previous.lifecycle == current.lifecycle
                && previous.prompt_layer == current.prompt_layer,
            "binding {}: replacement {target} crosses layer/lifecycle",
            current.binding_id
        );
        current = previous;
    }
}

/// Participate in an existing LGP installation transaction. The caller owns
/// scene validation and MUST propagate errors to roll back the entire batch.
pub fn apply_in_transaction(
    tx: &mut dyn PromptTransaction,
    context: &PromptMutationContext,
    change: &PromptChange,
) -> Result<()> {
    ensure!(
        !context.actor_id.trim().is_empty() && context.now >= 0,
        "invalid trusted prompt mutation context"
    );
    validate_change_ids(change)?;
    for input in &change.assets {
        let asset = PromptAssetRevision::new(
            &input.asset_id,
            input.revision,
            &input.title,
            &input.body_text,
            &context.actor_id,
            context.now,
        )?;
        let key = PromptAssetKey {
            asset_id: input.asset_id.clone(),
            revision: input.revision,
        };
        if let Some(existing) = tx.asset(&key)? {
            existing
                .validate()
                .with_context(|| format!("stored asset {}@{}", key.asset_id, key.revision))?;
            ensure!(
                existing.digest == asset.digest,
                "immutable asset conflict: {}@{}",
                key.asset_id,
                key.revision
            );
        } else {
            tx.insert_asset(&asset)?;
        }
    }
    for spec in &change.bindings {
        let existing = tx.binding(&spec.binding_id)?;
        let record = binding_record(spec, context, existing.as_ref());
        record
            .validate()
            .with_context(|| format!("binding {}", spec.binding_id))?;
        ensure!(
            tx.target_exists(&spec.target)?,
            "binding {}: missing target {:?}",
            spec.binding_id,
            spec.target
        );
        if existing.as_ref().map(PromptBindingSpec::from).as_ref() != Some(spec) {
            tx.put_binding(&record)?;
        }
    }
    for id in &change.delete_binding_ids {
        tx.delete_binding(id)?;
    }
    // Validate final transaction state, including reverse dependencies when
    // a target binding was moved, re-layered, replaced or explicitly removed.
    let touched: BTreeSet<_> = change
        .bindings
        .iter()
        .map(|spec| &spec.binding_id)
        .chain(&change.delete_binding_ids)
        .collect();
    for id in touched {
        if let Some(binding) = tx.binding(id)? {
            validate_references(tx, &binding)?;
        }
        for dependent in tx.bindings_replacing(id)? {
            validate_references(tx, &dependent)?;
        }
    }
    for key in &change.delete_assets {
        ensure!(
            tx.bindings_using_asset(key)?.is_empty(),
            "asset {}@{} is still referenced",
            key.asset_id,
            key.revision
        );
        tx.delete_asset(key)?;
    }
    Ok(())
}

//! Deterministic InquiryGraph runtime.
//!
//! This crate owns package validation, the session reducer, surface-neutral
//! projection, and result artifact generation. It intentionally has no Android,
//! WebUI, connector, transport, or LGP store dependency.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use anyhow::{anyhow, ensure, Result};
use polar_inquiry_graph_model::{InquiryGraphPackageProviderRef, *};
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};

pub fn validate_package(package: &InquiryGraphPackage) -> InquiryGraphValidationReport {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let mut node_ids = BTreeSet::new();
    let mut capabilities = BTreeSet::new();

    if package.manifest.schema_version != INQUIRY_GRAPH_PACKAGE_SCHEMA_VERSION {
        errors.push(format!(
            "unsupported schema_version {}; expected {}",
            package.manifest.schema_version, INQUIRY_GRAPH_PACKAGE_SCHEMA_VERSION
        ));
    }
    if package.manifest.package_id.trim().is_empty() {
        errors.push("manifest.package_id must not be empty".to_string());
    }
    if package.manifest.package_version.trim().is_empty() {
        errors.push("manifest.package_version must not be empty".to_string());
    }
    if package.nodes.is_empty() {
        errors.push("package must contain at least one node".to_string());
    }

    for node in &package.nodes {
        if node.node_id.trim().is_empty() {
            errors.push("node.node_id must not be empty".to_string());
            continue;
        }
        if !node_ids.insert(node.node_id.clone()) {
            errors.push(format!("duplicate node_id {}", node.node_id));
        }
        capabilities.insert(node.kind.required_capability());
        validate_node(node, &mut errors, &mut warnings);
    }

    if !package.manifest.entry_node_id.is_empty()
        && !node_ids.contains(&package.manifest.entry_node_id)
    {
        errors.push(format!(
            "entry_node_id {} is not present in nodes",
            package.manifest.entry_node_id
        ));
    }

    let edge_ids = package
        .edges
        .iter()
        .map(|edge| edge.edge_id.clone())
        .collect::<Vec<_>>();
    let mut seen_edge_ids = BTreeSet::new();
    for edge in &package.edges {
        if edge.edge_id.trim().is_empty() {
            errors.push("edge.edge_id must not be empty".to_string());
        } else if !seen_edge_ids.insert(edge.edge_id.clone()) {
            errors.push(format!("duplicate edge_id {}", edge.edge_id));
        }
        if !node_ids.contains(&edge.from_node_id) {
            errors.push(format!(
                "edge {} references missing from_node_id {}",
                edge.edge_id, edge.from_node_id
            ));
        }
        if !node_ids.contains(&edge.to_node_id) {
            errors.push(format!(
                "edge {} references missing to_node_id {}",
                edge.edge_id, edge.to_node_id
            ));
        }
        validate_edge_condition(edge, &node_ids, &mut errors);
    }

    for node in &package.nodes {
        let Some(resolution) = &node.resolution else {
            continue;
        };
        let mut dependency_ids = BTreeSet::new();
        for dependency in resolution
            .depends_on
            .iter()
            .chain(resolution.optional_depends_on.iter())
        {
            if !node_ids.contains(dependency) {
                errors.push(format!(
                    "node {} resolution references missing dependency {}",
                    node.node_id, dependency
                ));
            }
            if dependency == &node.node_id {
                errors.push(format!(
                    "node {} resolution cannot depend on itself",
                    node.node_id
                ));
            }
            if !dependency_ids.insert(dependency) {
                errors.push(format!(
                    "node {} resolution repeats dependency {}",
                    node.node_id, dependency
                ));
            }
        }
        let slots = resolution
            .output
            .choice_slots
            .iter()
            .map(|slot| slot.choice_id.as_str())
            .collect::<BTreeSet<_>>();
        for edge in &package.edges {
            let referenced = match &edge.condition {
                InquiryGraphEdgeCondition::AnswerEquals { node_id, choice_id }
                    if node_id == &node.node_id =>
                {
                    vec![choice_id.as_str()]
                }
                InquiryGraphEdgeCondition::AnswerIn {
                    node_id,
                    choice_ids,
                } if node_id == &node.node_id => choice_ids.iter().map(String::as_str).collect(),
                _ => Vec::new(),
            };
            for choice_id in referenced {
                if !slots.contains(choice_id) {
                    errors.push(format!(
                        "edge {} references undeclared dynamic choice slot {} on node {}",
                        edge.edge_id, choice_id, node.node_id
                    ));
                }
            }
        }
    }

    if package.edges.is_empty() && package.nodes.len() > 1 {
        warnings
            .push("multi-node package has no edges; only entry node can be reached".to_string());
    }
    if !edge_ids.is_empty() {
        let reachable = reachable_nodes(package);
        for node_id in &node_ids {
            if !reachable.contains(node_id) {
                warnings.push(format!("node {node_id} is not reachable from entry node"));
            }
        }
    }

    InquiryGraphValidationReport {
        ok: errors.is_empty(),
        errors,
        warnings,
        capabilities_detected: capabilities.into_iter().collect(),
    }
}

fn validate_node(node: &InquiryGraphNode, errors: &mut Vec<String>, warnings: &mut Vec<String>) {
    if node.title.trim().is_empty() {
        errors.push(format!("node {} title must not be empty", node.node_id));
    }
    let choice_count = node
        .choices
        .iter()
        .filter(|choice| !choice.is_unknown)
        .count();
    match node.kind {
        InquiryGraphNodeKind::BinaryChoice
        | InquiryGraphNodeKind::PairwisePreference
        | InquiryGraphNodeKind::ResponseAb => {
            if choice_count != 2 {
                errors.push(format!(
                    "node {} kind {:?} requires exactly two non-unknown choices",
                    node.node_id, node.kind
                ));
            }
        }
        InquiryGraphNodeKind::MaxDiff | InquiryGraphNodeKind::CardSort => {
            if choice_count < 3 {
                errors.push(format!(
                    "node {} kind {:?} requires at least three non-unknown choices",
                    node.node_id, node.kind
                ));
            }
        }
        InquiryGraphNodeKind::Consent => {
            if node.choices.is_empty() {
                warnings.push(format!(
                    "consent node {} has no explicit choices; renderer must provide canonical accept/decline",
                    node.node_id
                ));
            }
        }
        InquiryGraphNodeKind::IdentityCapture => {
            if node.fields.is_empty() {
                errors.push(format!(
                    "identity capture node {} requires one or more fields",
                    node.node_id
                ));
            }
            let mut seen_field_ids = BTreeSet::new();
            for field in &node.fields {
                if field.field_id.trim().is_empty() {
                    errors.push(format!(
                        "identity capture node {} has empty field_id",
                        node.node_id
                    ));
                } else if !seen_field_ids.insert(field.field_id.clone()) {
                    errors.push(format!(
                        "identity capture node {} has duplicate field_id {}",
                        node.node_id, field.field_id
                    ));
                }
                if field.label.trim().is_empty() {
                    errors.push(format!(
                        "identity capture node {} field {} label must not be empty",
                        node.node_id, field.field_id
                    ));
                }
            }
        }
        InquiryGraphNodeKind::ShortText | InquiryGraphNodeKind::LongText => {
            if !node.choices.is_empty() {
                warnings.push(format!(
                    "text node {} has choices; renderer should ignore choices unless metadata explains them",
                    node.node_id
                ));
            }
        }
        InquiryGraphNodeKind::Section
        | InquiryGraphNodeKind::ScenarioChoice
        | InquiryGraphNodeKind::ReviewConfirm
        | InquiryGraphNodeKind::ResultReview => {}
    }

    let mut seen_choice_ids = BTreeSet::new();
    let unknown_count = node
        .choices
        .iter()
        .filter(|choice| choice.is_unknown)
        .count();
    if unknown_count > 1 {
        errors.push(format!(
            "node {} has multiple unknown choices; use one unknown/skip choice",
            node.node_id
        ));
    }
    for choice in &node.choices {
        if choice.choice_id.trim().is_empty() {
            errors.push(format!("node {} has empty choice_id", node.node_id));
        } else if !seen_choice_ids.insert(choice.choice_id.clone()) {
            errors.push(format!(
                "node {} has duplicate choice_id {}",
                node.node_id, choice.choice_id
            ));
        }
        if choice.label.trim().is_empty() {
            errors.push(format!(
                "node {} choice {} label must not be empty",
                node.node_id, choice.choice_id
            ));
        }
    }

    if let Some(resolution) = &node.resolution {
        if resolution.kind != "plugin" {
            errors.push(format!(
                "node {} resolution kind must be plugin",
                node.node_id
            ));
        }
        if resolution.trigger != "on_enter" {
            errors.push(format!(
                "node {} resolution trigger must be on_enter",
                node.node_id
            ));
        }
        if resolution.resolver_id.trim().is_empty() {
            errors.push(format!(
                "node {} resolver_id must not be empty",
                node.node_id
            ));
        }
        if resolution.depends_on.is_empty() {
            errors.push(format!(
                "node {} dynamic resolution requires dependencies",
                node.node_id
            ));
        }
        let slots = &resolution.output.choice_slots;
        let mut slot_ids = BTreeSet::new();
        for slot in slots {
            if slot.choice_id.trim().is_empty() {
                errors.push(format!(
                    "node {} has empty dynamic choice slot",
                    node.node_id
                ));
            } else if !slot_ids.insert(slot.choice_id.clone()) {
                errors.push(format!(
                    "node {} has duplicate dynamic choice slot {}",
                    node.node_id, slot.choice_id
                ));
            }
        }
        let required_slot_count = slots.iter().filter(|slot| slot.required).count();
        if resolution.output.kind != "choice_hydration"
            || resolution.output.min_choices > resolution.output.max_choices
            || slots.len() < resolution.output.max_choices
            || required_slot_count > resolution.output.max_choices
        {
            errors.push(format!(
                "node {} has invalid dynamic choice output bounds",
                node.node_id
            ));
        }
        if resolution.output.result_namespace.trim().is_empty() {
            errors.push(format!(
                "node {} resolution result_namespace must not be empty",
                node.node_id
            ));
        }
    }
}

fn validate_edge_condition(
    edge: &InquiryGraphEdge,
    node_ids: &BTreeSet<String>,
    errors: &mut Vec<String>,
) {
    match &edge.condition {
        InquiryGraphEdgeCondition::AnswerEquals { node_id, choice_id } => {
            if !node_ids.contains(node_id) {
                errors.push(format!(
                    "edge {} condition references missing node_id {}",
                    edge.edge_id, node_id
                ));
            }
            if choice_id.trim().is_empty() {
                errors.push(format!(
                    "edge {} condition choice_id is empty",
                    edge.edge_id
                ));
            }
        }
        InquiryGraphEdgeCondition::AnswerIn {
            node_id,
            choice_ids,
        } => {
            if !node_ids.contains(node_id) {
                errors.push(format!(
                    "edge {} condition references missing node_id {}",
                    edge.edge_id, node_id
                ));
            }
            if choice_ids.is_empty() {
                errors.push(format!("edge {} AnswerIn condition is empty", edge.edge_id));
            }
        }
        InquiryGraphEdgeCondition::Unknown { node_id } => {
            if !node_ids.contains(node_id) {
                errors.push(format!(
                    "edge {} condition references missing node_id {}",
                    edge.edge_id, node_id
                ));
            }
        }
        InquiryGraphEdgeCondition::JsonLogic { expression } => {
            if expression.is_null() {
                errors.push(format!(
                    "edge {} JsonLogic expression must not be null",
                    edge.edge_id
                ));
            }
        }
        InquiryGraphEdgeCondition::Always => {}
    }
}

fn reachable_nodes(package: &InquiryGraphPackage) -> BTreeSet<String> {
    let mut reachable = BTreeSet::new();
    let mut queue = vec![package.manifest.entry_node_id.clone()];
    while let Some(node_id) = queue.pop() {
        if !reachable.insert(node_id.clone()) {
            continue;
        }
        for edge in package
            .edges
            .iter()
            .filter(|edge| edge.from_node_id == node_id)
        {
            queue.push(edge.to_node_id.clone());
        }
    }
    reachable
}

pub fn open_session(
    package: &InquiryGraphPackage,
    session_id: impl Into<String>,
    now_secs: i64,
) -> Result<InquiryGraphSessionState> {
    open_session_with_context(package, session_id, now_secs, Default::default(), None)
}

pub fn open_session_with_context(
    package: &InquiryGraphPackage,
    session_id: impl Into<String>,
    now_secs: i64,
    interaction_mode: InquiryGraphSessionInteractionMode,
    source: Option<InquiryGraphSessionSource>,
) -> Result<InquiryGraphSessionState> {
    let report = validate_package(package);
    ensure!(
        report.ok,
        "invalid InquiryGraph package: {}",
        report.errors.join("; ")
    );
    ensure!(
        interaction_mode != InquiryGraphSessionInteractionMode::ExternalDeferredSubmit
            || !package_requires_dynamic_resolution(package),
        "InquiryGraph packages with plugin-resolved nodes require interactive_stepwise mode"
    );
    Ok(InquiryGraphSessionState {
        session_id: session_id.into(),
        alias: None,
        package_id: package.manifest.package_id.clone(),
        package_version: package.manifest.package_version.clone(),
        package_hash: None,
        provider_ref: None,
        revision: 0,
        interaction_mode,
        source,
        entry_node_id: package.manifest.entry_node_id.clone(),
        current_node_id: Some(package.manifest.entry_node_id.clone()),
        status: InquiryGraphSessionStatus::Active,
        answers: Vec::new(),
        node_resolution_refs: BTreeMap::new(),
        processed_answer_operations: BTreeMap::new(),
        processed_answer_operation_hashes: BTreeMap::new(),
        visited_node_ids: vec![package.manifest.entry_node_id.clone()],
        started_at_secs: now_secs,
        updated_at_secs: now_secs,
        result_target: package.manifest.result_target.clone(),
        consent: InquiryGraphConsentState::default(),
        submitted_client_submission_id: None,
    })
}

pub fn package_requires_dynamic_resolution(package: &InquiryGraphPackage) -> bool {
    package.nodes.iter().any(|node| node.resolution.is_some())
}

pub fn package_sha256(package: &InquiryGraphPackage) -> Result<String> {
    let bytes = serde_json::to_vec(package)?;
    let digest = Sha256::digest(bytes);
    let hex = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(format!("sha256:{hex}"))
}

pub fn open_session_with_provider(
    package: &InquiryGraphPackage,
    alias: impl Into<String>,
    provider_ref: InquiryGraphPackageProviderRef,
    session_id: impl Into<String>,
    now_secs: i64,
    interaction_mode: InquiryGraphSessionInteractionMode,
    source: Option<InquiryGraphSessionSource>,
) -> Result<InquiryGraphSessionState> {
    let package_hash = package_sha256(package)?;
    let mut session =
        open_session_with_context(package, session_id, now_secs, interaction_mode, source)?;
    session.alias = Some(alias.into());
    session.package_hash = Some(package_hash);
    session.provider_ref = Some(provider_ref);
    Ok(session)
}

#[derive(Debug, Clone, PartialEq)]
pub enum InquiryGraphExecutionEffect {
    ResolveCurrentNode(InquiryGraphNodeResolutionRequest),
}

#[derive(Debug, Clone, PartialEq)]
pub struct InquiryGraphTransition {
    pub session: InquiryGraphSessionState,
    pub effects: Vec<InquiryGraphExecutionEffect>,
}

pub type InquiryGraphNodeResolutionArtifacts = BTreeMap<String, InquiryGraphNodeResolutionArtifact>;

pub fn apply_answer(
    package: &InquiryGraphPackage,
    session: InquiryGraphSessionState,
    answer: InquiryGraphAnswer,
    now_secs: i64,
) -> Result<InquiryGraphSessionState> {
    Ok(apply_answer_transition_with_resolutions(
        package,
        session,
        answer,
        now_secs,
        &BTreeMap::new(),
    )?
    .session)
}

pub fn apply_answer_transition(
    package: &InquiryGraphPackage,
    session: InquiryGraphSessionState,
    answer: InquiryGraphAnswer,
    now_secs: i64,
) -> Result<InquiryGraphTransition> {
    apply_answer_transition_with_resolutions(package, session, answer, now_secs, &BTreeMap::new())
}

pub fn apply_answer_transition_with_resolutions(
    package: &InquiryGraphPackage,
    mut session: InquiryGraphSessionState,
    mut answer: InquiryGraphAnswer,
    now_secs: i64,
    artifacts: &InquiryGraphNodeResolutionArtifacts,
) -> Result<InquiryGraphTransition> {
    ensure!(
        session.status == InquiryGraphSessionStatus::Active,
        "cannot answer InquiryGraph session in status {:?}",
        session.status
    );
    let current_node_id = session
        .current_node_id
        .clone()
        .ok_or_else(|| anyhow!("active session has no current_node_id"))?;
    ensure!(
        answer.node_id == current_node_id,
        "answer node {} does not match current node {}",
        answer.node_id,
        current_node_id
    );
    let node = effective_node_by_id(package, &session, &answer.node_id, artifacts)?;
    validate_answer_for_node(&node, &answer)?;

    if let Some(resolution) = &node.resolution {
        let artifact = active_resolution_artifact(&session, &node.node_id, artifacts)
            .ok_or_else(|| anyhow!("dynamic node {} is not resolved", node.node_id))?;
        let choice_id = answer
            .choice_ids
            .first()
            .ok_or_else(|| anyhow!("dynamic node {} requires one choice_id", node.node_id))?;
        let choice = artifact
            .choices
            .iter()
            .find(|choice| &choice.choice_id == choice_id)
            .ok_or_else(|| {
                anyhow!(
                    "dynamic node {} choice {} is not authoritative",
                    node.node_id,
                    choice_id
                )
            })?;
        answer.metadata = serde_json::json!({
            "node_resolution": {
                "resolver_id": resolution.resolver_id,
                "input_hash": artifact.input_hash,
                "choice_id": choice.choice_id,
                "choice_value": choice.value,
                "choice_metadata": choice.metadata,
            }
        });
    }

    if node.kind == InquiryGraphNodeKind::Consent {
        apply_consent_answer(&mut session, &answer);
    }

    session
        .answers
        .retain(|existing| existing.node_id != answer.node_id);
    session.answers.push(answer.clone());
    session.updated_at_secs = now_secs;
    session.revision = session.revision.saturating_add(1);

    let next_node_id = next_node_id(package, &session, &answer.node_id);
    match next_node_id {
        Some(next) => {
            session.current_node_id = Some(next.clone());
            if !session.visited_node_ids.contains(&next) {
                session.visited_node_ids.push(next);
            }
        }
        None => {
            session.current_node_id = None;
            session.status = InquiryGraphSessionStatus::Completed;
        }
    }
    transition_for_current_node_at(package, session, now_secs)
}

pub fn transition_for_current_node(
    package: &InquiryGraphPackage,
    session: InquiryGraphSessionState,
) -> Result<InquiryGraphTransition> {
    let now_secs = session.updated_at_secs;
    transition_for_current_node_at(package, session, now_secs)
}

pub fn transition_for_current_node_at(
    package: &InquiryGraphPackage,
    mut session: InquiryGraphSessionState,
    now_secs: i64,
) -> Result<InquiryGraphTransition> {
    let Some(node_id) = session.current_node_id.clone() else {
        return Ok(InquiryGraphTransition {
            session,
            effects: Vec::new(),
        });
    };
    let node = node_by_id(package, &node_id)?;
    let Some(spec) = &node.resolution else {
        return Ok(InquiryGraphTransition {
            session,
            effects: Vec::new(),
        });
    };
    let initial_request = build_node_resolution_request(package, &session, node, spec)?;
    let existing = session.node_resolution_refs.get(&node_id);
    if existing.is_some_and(|reference| {
        reference.input_hash == initial_request.input_hash
            && reference.resolver_id == initial_request.resolver_id
            && reference.provider == initial_request.provider
            && reference.status == InquiryGraphNodeResolutionStatus::Resolved
    }) {
        return Ok(InquiryGraphTransition {
            session,
            effects: Vec::new(),
        });
    }
    if existing.is_some_and(|reference| {
        reference.input_hash == initial_request.input_hash
            && reference.resolver_id == initial_request.resolver_id
            && reference.provider == initial_request.provider
            && reference.status == InquiryGraphNodeResolutionStatus::Pending
    }) {
        return Ok(InquiryGraphTransition {
            session,
            effects: vec![InquiryGraphExecutionEffect::ResolveCurrentNode(
                initial_request,
            )],
        });
    }

    // Publishing a new pending/failed-retry projection is an observable domain
    // transition. Give it its own revision before constructing the request so
    // the resolver echo and subsequent commit CAS bind to that revision.
    session.revision = session.revision.saturating_add(1);
    session.updated_at_secs = now_secs;
    let request = build_node_resolution_request(package, &session, node, spec)?;
    session.node_resolution_refs.insert(
        node_id.clone(),
        InquiryGraphNodeResolutionRef {
            node_id,
            resolver_id: spec.resolver_id.clone(),
            input_hash: request.input_hash.clone(),
            output_hash: None,
            provider: request.provider.clone(),
            status: InquiryGraphNodeResolutionStatus::Pending,
        },
    );
    Ok(InquiryGraphTransition {
        session,
        effects: vec![InquiryGraphExecutionEffect::ResolveCurrentNode(request)],
    })
}

pub fn apply_answer_batch(
    package: &InquiryGraphPackage,
    mut session: InquiryGraphSessionState,
    answers: Vec<InquiryGraphAnswer>,
    now_secs: i64,
) -> Result<InquiryGraphSessionState> {
    ensure!(
        !answers.is_empty(),
        "InquiryGraph submit requires at least one answer"
    );
    for answer in answers {
        session = apply_answer(package, session, answer, now_secs)?;
    }
    ensure!(
        session.status == InquiryGraphSessionStatus::Completed,
        "InquiryGraph submit answers did not complete the session"
    );
    Ok(session)
}

/// Replace an earlier dependency answer and invalidate every downstream answer
/// and active resolution derived from it. Historical resolution rows remain in
/// the store; only session-active references are removed here.
pub fn replace_dependency_answer_and_invalidate(
    package: &InquiryGraphPackage,
    mut session: InquiryGraphSessionState,
    replacement: InquiryGraphAnswer,
    now_secs: i64,
) -> Result<InquiryGraphTransition> {
    let answer_index = session
        .answers
        .iter()
        .position(|answer| answer.node_id == replacement.node_id)
        .ok_or_else(|| anyhow!("cannot replace unanswered node {}", replacement.node_id))?;
    session.answers.truncate(answer_index);
    if let Some(visited_index) = session
        .visited_node_ids
        .iter()
        .position(|node_id| node_id == &replacement.node_id)
    {
        session.visited_node_ids.truncate(visited_index + 1);
    }
    let invalid_resolution_nodes = package
        .nodes
        .iter()
        .filter_map(|node| {
            node.resolution.as_ref().and_then(|spec| {
                (spec.depends_on.contains(&replacement.node_id)
                    || spec.optional_depends_on.contains(&replacement.node_id))
                .then(|| node.node_id.clone())
            })
        })
        .collect::<BTreeSet<_>>();
    session
        .node_resolution_refs
        .retain(|node_id, _| !invalid_resolution_nodes.contains(node_id));
    session.status = InquiryGraphSessionStatus::Active;
    session.current_node_id = Some(replacement.node_id.clone());
    apply_answer_transition(package, session, replacement, now_secs)
}

fn node_by_id<'a>(package: &'a InquiryGraphPackage, node_id: &str) -> Result<&'a InquiryGraphNode> {
    package
        .nodes
        .iter()
        .find(|node| node.node_id == node_id)
        .ok_or_else(|| anyhow!("node {node_id} not found"))
}

fn build_node_resolution_request(
    package: &InquiryGraphPackage,
    session: &InquiryGraphSessionState,
    node: &InquiryGraphNode,
    spec: &InquiryGraphNodeResolutionSpec,
) -> Result<InquiryGraphNodeResolutionRequest> {
    let package_hash = session
        .package_hash
        .clone()
        .unwrap_or(package_sha256(package)?);
    let provider = match session.provider_ref.as_ref() {
        Some(InquiryGraphPackageProviderRef::Plugin {
            plugin_id,
            plugin_version,
            ..
        }) => InquiryGraphNodeResolutionProviderRef {
            plugin_id: plugin_id.clone(),
            plugin_version: plugin_version.clone(),
        },
        None => {
            return Err(anyhow!(
                "dynamic node resolution requires a pinned plugin provider"
            ))
        }
    };
    let answers_by_node = session
        .answers
        .iter()
        .map(|answer| (answer.node_id.as_str(), answer))
        .collect::<HashMap<_, _>>();
    let mut answers = Vec::with_capacity(spec.depends_on.len() + spec.optional_depends_on.len());
    for dependency in &spec.depends_on {
        let answer = answers_by_node.get(dependency.as_str()).ok_or_else(|| {
            anyhow!(
                "dynamic node {} dependency {} has no answer",
                node.node_id,
                dependency
            )
        })?;
        answers.push(normalize_resolution_dependency_answer(answer));
    }
    for dependency in &spec.optional_depends_on {
        if let Some(answer) = answers_by_node.get(dependency.as_str()) {
            answers.push(normalize_resolution_dependency_answer(answer));
        }
    }
    let hash_input = serde_json::json!({
        "package_hash": package_hash,
        "provider": provider,
        "node_id": node.node_id,
        "resolver_id": spec.resolver_id,
        "depends_on": spec.depends_on,
        "optional_depends_on": spec.optional_depends_on,
        "answers": answers,
    });
    let input_hash = sha256_canonical_json(&hash_input)?;
    let invocation_id = format!(
        "inquiry-resolve:{}:{}:{}",
        session.session_id,
        node.node_id,
        input_hash.trim_start_matches("sha256:")
    );
    Ok(InquiryGraphNodeResolutionRequest {
        schema_version: INQUIRY_GRAPH_NODE_RESOLUTION_REQUEST_SCHEMA_VERSION.to_string(),
        invocation_id,
        session_id: session.session_id.clone(),
        session_revision: session.revision,
        package_id: session.package_id.clone(),
        package_version: session.package_version.clone(),
        package_hash,
        provider,
        node_id: node.node_id.clone(),
        resolver_id: spec.resolver_id.clone(),
        depends_on: spec.depends_on.clone(),
        optional_depends_on: spec.optional_depends_on.clone(),
        answers,
        prior_resolutions: session
            .node_resolution_refs
            .values()
            .filter(|reference| reference.node_id != node.node_id)
            .cloned()
            .collect(),
        input_hash,
    })
}

pub fn node_resolution_request_for_current(
    package: &InquiryGraphPackage,
    session: &InquiryGraphSessionState,
) -> Result<Option<InquiryGraphNodeResolutionRequest>> {
    let Some(node_id) = session.current_node_id.as_deref() else {
        return Ok(None);
    };
    let node = node_by_id(package, node_id)?;
    node.resolution
        .as_ref()
        .map(|spec| build_node_resolution_request(package, session, node, spec))
        .transpose()
}

fn normalize_resolution_dependency_answer(answer: &InquiryGraphAnswer) -> InquiryGraphAnswer {
    let mut normalized = answer.clone();
    normalized.answered_at_secs = 0;
    normalized.metadata = answer
        .metadata
        .get("node_resolution")
        .cloned()
        .map(|value| serde_json::json!({ "node_resolution": value }))
        .unwrap_or_else(|| serde_json::json!({}));
    normalized
}

fn sha256_canonical_json(value: &JsonValue) -> Result<String> {
    let canonical = canonicalize_json(value);
    let digest = Sha256::digest(serde_json::to_vec(&canonical)?);
    Ok(format!(
        "sha256:{}",
        digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    ))
}

fn canonicalize_json(value: &JsonValue) -> JsonValue {
    match value {
        JsonValue::Object(object) => {
            let sorted = object
                .iter()
                .map(|(key, value)| (key.clone(), canonicalize_json(value)))
                .collect::<BTreeMap<_, _>>();
            serde_json::to_value(sorted).expect("BTreeMap JSON serialization")
        }
        JsonValue::Array(values) => {
            JsonValue::Array(values.iter().map(canonicalize_json).collect())
        }
        scalar => scalar.clone(),
    }
}

pub fn validate_node_resolution_artifact(
    package: &InquiryGraphPackage,
    request: &InquiryGraphNodeResolutionRequest,
    artifact: &InquiryGraphNodeResolutionArtifact,
) -> Result<()> {
    ensure!(
        artifact.schema_version == INQUIRY_GRAPH_NODE_RESOLUTION_SCHEMA_VERSION,
        "unsupported node-resolution artifact schema_version"
    );
    ensure!(
        artifact.session_id == request.session_id,
        "resolution session_id mismatch"
    );
    ensure!(
        artifact.session_revision == request.session_revision,
        "resolution session_revision mismatch"
    );
    ensure!(
        artifact.package_hash == request.package_hash,
        "resolution package_hash mismatch"
    );
    ensure!(
        artifact.node_id == request.node_id,
        "resolution node_id mismatch"
    );
    ensure!(
        artifact.resolver_id == request.resolver_id,
        "resolution resolver_id mismatch"
    );
    ensure!(
        artifact.input_hash == request.input_hash,
        "resolution input_hash mismatch"
    );
    let node = node_by_id(package, &request.node_id)?;
    let spec = node
        .resolution
        .as_ref()
        .ok_or_else(|| anyhow!("node {} is not dynamically resolved", node.node_id))?;
    ensure!(
        artifact.choices.len() >= spec.output.min_choices
            && artifact.choices.len() <= spec.output.max_choices,
        "resolution choice count is outside package bounds"
    );
    let declared = spec
        .output
        .choice_slots
        .iter()
        .map(|slot| slot.choice_id.as_str())
        .collect::<BTreeSet<_>>();
    let actual = artifact
        .choices
        .iter()
        .map(|choice| choice.choice_id.as_str())
        .collect::<BTreeSet<_>>();
    ensure!(
        actual.len() == artifact.choices.len(),
        "resolution choices contain duplicate choice_id"
    );
    ensure!(
        actual.is_subset(&declared),
        "resolution choices include undeclared stable slots"
    );
    let required = spec
        .output
        .choice_slots
        .iter()
        .filter(|slot| slot.required)
        .map(|slot| slot.choice_id.as_str())
        .collect::<BTreeSet<_>>();
    ensure!(
        required.is_subset(&actual),
        "resolution choices omit required stable slots"
    );
    for choice in &artifact.choices {
        ensure!(
            !choice.label.trim().is_empty(),
            "resolved choice label must not be empty"
        );
    }
    if let Some(title) = artifact.title.as_deref() {
        ensure!(
            !title.trim().is_empty(),
            "resolved node title must not be empty"
        );
    }
    if let Some(prompt) = artifact.prompt.as_deref() {
        ensure!(
            !prompt.trim().is_empty(),
            "resolved node prompt must not be empty"
        );
    }
    ensure!(
        serde_json::to_vec(artifact)?.len() <= 1_048_576,
        "node-resolution artifact exceeds 1 MiB"
    );
    Ok(())
}

fn active_resolution_artifact<'a>(
    session: &InquiryGraphSessionState,
    node_id: &str,
    artifacts: &'a InquiryGraphNodeResolutionArtifacts,
) -> Option<&'a InquiryGraphNodeResolutionArtifact> {
    let reference = session.node_resolution_refs.get(node_id)?;
    if reference.status != InquiryGraphNodeResolutionStatus::Resolved {
        return None;
    }
    artifacts.get(node_id).filter(|artifact| {
        artifact.input_hash == reference.input_hash && artifact.resolver_id == reference.resolver_id
    })
}

pub fn effective_node_by_id(
    package: &InquiryGraphPackage,
    session: &InquiryGraphSessionState,
    node_id: &str,
    artifacts: &InquiryGraphNodeResolutionArtifacts,
) -> Result<InquiryGraphNode> {
    let mut node = node_by_id(package, node_id)?.clone();
    if node.resolution.is_some() {
        if let Some(artifact) = active_resolution_artifact(session, node_id, artifacts) {
            node.choices = artifact.choices.clone();
            if let Some(title) = artifact.title.as_ref() {
                node.title = title.clone();
            }
            if let Some(prompt) = artifact.prompt.as_ref() {
                node.prompt = Some(prompt.clone());
            }
            if !artifact.metadata.is_null() {
                node.metadata = artifact.metadata.clone();
            }
        }
    }
    Ok(node)
}

pub fn attach_node_resolution(
    package: &InquiryGraphPackage,
    mut session: InquiryGraphSessionState,
    request: &InquiryGraphNodeResolutionRequest,
    artifact: &InquiryGraphNodeResolutionArtifact,
    now_secs: i64,
) -> Result<InquiryGraphSessionState> {
    ensure!(
        session.status == InquiryGraphSessionStatus::Active,
        "cannot attach resolution to non-active session"
    );
    ensure!(
        session.revision == request.session_revision
            && session.current_node_id.as_deref() == Some(request.node_id.as_str()),
        "late node-resolution response does not match current session revision/node"
    );
    let reference = session
        .node_resolution_refs
        .get(&request.node_id)
        .ok_or_else(|| anyhow!("node-resolution reservation is missing"))?;
    ensure!(
        reference.input_hash == request.input_hash
            && reference.status == InquiryGraphNodeResolutionStatus::Pending,
        "late node-resolution response does not match active input_hash"
    );
    validate_node_resolution_artifact(package, request, artifact)?;
    let output_hash = sha256_canonical_json(&serde_json::to_value(artifact)?)?;
    session.node_resolution_refs.insert(
        request.node_id.clone(),
        InquiryGraphNodeResolutionRef {
            node_id: request.node_id.clone(),
            resolver_id: request.resolver_id.clone(),
            input_hash: request.input_hash.clone(),
            output_hash: Some(output_hash),
            provider: request.provider.clone(),
            status: InquiryGraphNodeResolutionStatus::Resolved,
        },
    );
    session.revision = session.revision.saturating_add(1);
    session.updated_at_secs = now_secs;
    Ok(session)
}

pub fn mark_node_resolution_failed(
    mut session: InquiryGraphSessionState,
    request: &InquiryGraphNodeResolutionRequest,
    now_secs: i64,
) -> Result<InquiryGraphSessionState> {
    ensure!(
        session.revision == request.session_revision
            && session.current_node_id.as_deref() == Some(request.node_id.as_str()),
        "late node-resolution failure does not match current session"
    );
    let reference = session
        .node_resolution_refs
        .get_mut(&request.node_id)
        .ok_or_else(|| anyhow!("node-resolution reservation is missing"))?;
    ensure!(
        reference.input_hash == request.input_hash,
        "resolution input_hash mismatch"
    );
    ensure!(
        reference.status == InquiryGraphNodeResolutionStatus::Pending,
        "only a pending node resolution may fail"
    );
    reference.status = InquiryGraphNodeResolutionStatus::Failed;
    reference.output_hash = None;
    session.revision = session.revision.saturating_add(1);
    session.updated_at_secs = now_secs;
    Ok(session)
}

fn validate_answer_for_node(node: &InquiryGraphNode, answer: &InquiryGraphAnswer) -> Result<()> {
    if answer.unknown {
        ensure!(
            node.allow_unknown
                || node.choices.iter().any(|choice| choice.is_unknown)
                || answer.answer_kind == InquiryGraphAnswerKind::Skip,
            "node {} does not allow unknown/skip answer",
            node.node_id
        );
        return Ok(());
    }
    let choice_ids = node
        .choices
        .iter()
        .map(|choice| choice.choice_id.as_str())
        .collect::<BTreeSet<_>>();
    match node.kind {
        InquiryGraphNodeKind::BinaryChoice
        | InquiryGraphNodeKind::PairwisePreference
        | InquiryGraphNodeKind::ScenarioChoice
        | InquiryGraphNodeKind::ResponseAb
        | InquiryGraphNodeKind::ReviewConfirm
        | InquiryGraphNodeKind::Consent => {
            ensure!(
                answer.choice_ids.len() == 1,
                "node {} requires exactly one choice answer",
                node.node_id
            );
            ensure!(
                choice_ids.contains(answer.choice_ids[0].as_str()),
                "node {} answer references invalid choice {}",
                node.node_id,
                answer.choice_ids[0]
            );
        }
        InquiryGraphNodeKind::ResultReview => {
            ensure!(
                answer.answer_kind == InquiryGraphAnswerKind::ReviewDecision
                    || answer.answer_kind == InquiryGraphAnswerKind::Choice,
                "result review node {} requires a review decision answer",
                node.node_id
            );
            if !node.choices.is_empty() {
                ensure!(
                    answer.choice_ids.len() == 1,
                    "result review node {} requires exactly one choice answer",
                    node.node_id
                );
                ensure!(
                    choice_ids.contains(answer.choice_ids[0].as_str()),
                    "result review node {} answer references invalid choice {}",
                    node.node_id,
                    answer.choice_ids[0]
                );
            }
        }
        InquiryGraphNodeKind::IdentityCapture => {
            ensure!(
                answer.answer_kind == InquiryGraphAnswerKind::IdentityCapture
                    || answer.answer_kind == InquiryGraphAnswerKind::Text,
                "identity capture node {} requires an identity_capture answer",
                node.node_id
            );
            validate_identity_capture_answer(node, answer)?;
        }
        InquiryGraphNodeKind::MaxDiff | InquiryGraphNodeKind::CardSort => {
            ensure!(
                !answer.choice_ids.is_empty(),
                "node {} requires one or more choices",
                node.node_id
            );
            for choice_id in &answer.choice_ids {
                ensure!(
                    choice_ids.contains(choice_id.as_str()),
                    "node {} answer references invalid choice {}",
                    node.node_id,
                    choice_id
                );
            }
        }
        InquiryGraphNodeKind::ShortText | InquiryGraphNodeKind::LongText => {
            ensure!(
                answer
                    .text
                    .as_deref()
                    .is_some_and(|text| !text.trim().is_empty()),
                "node {} requires non-empty text",
                node.node_id
            );
        }
        InquiryGraphNodeKind::Section => {
            ensure!(
                answer.answer_kind == InquiryGraphAnswerKind::Skip
                    || answer.answer_kind == InquiryGraphAnswerKind::Choice,
                "section node {} only accepts skip/continue style answers",
                node.node_id
            );
        }
    }
    Ok(())
}

fn validate_identity_capture_answer(
    node: &InquiryGraphNode,
    answer: &InquiryGraphAnswer,
) -> Result<()> {
    let fields = answer
        .metadata
        .get("fields")
        .and_then(JsonValue::as_object)
        .ok_or_else(|| {
            anyhow!(
                "identity capture node {} requires metadata.fields",
                node.node_id
            )
        })?;
    for field in &node.fields {
        let value = fields
            .get(&field.field_id)
            .and_then(JsonValue::as_str)
            .map(str::trim)
            .unwrap_or("");
        ensure!(
            !field.required || !value.is_empty(),
            "identity capture node {} requires field {}",
            node.node_id,
            field.field_id
        );
        if let Some(max_length) = field.max_length {
            ensure!(
                value.chars().count() <= max_length as usize,
                "identity capture node {} field {} exceeds max_length {}",
                node.node_id,
                field.field_id,
                max_length
            );
        }
    }
    Ok(())
}

fn apply_consent_answer(session: &mut InquiryGraphSessionState, answer: &InquiryGraphAnswer) {
    let accept = answer
        .choice_ids
        .iter()
        .any(|choice| choice == "accept" || choice == "allow" || choice == "yes");
    let decline = answer
        .choice_ids
        .iter()
        .any(|choice| choice == "decline" || choice == "deny" || choice == "no");
    if accept {
        session.consent.allow_remote_processing = true;
    }
    if decline {
        session.consent.allow_remote_processing = false;
        session.consent.allow_memory_write = "denied".to_string();
        session.consent.allow_raw_answer_retention = false;
    }
}

fn next_node_id(
    package: &InquiryGraphPackage,
    session: &InquiryGraphSessionState,
    current_node_id: &str,
) -> Option<String> {
    let answers = session
        .answers
        .iter()
        .map(|answer| (answer.node_id.as_str(), answer))
        .collect::<HashMap<_, _>>();
    package
        .edges
        .iter()
        .filter(|edge| edge.from_node_id == current_node_id)
        .find(|edge| condition_matches(&edge.condition, &answers))
        .map(|edge| edge.to_node_id.clone())
}

fn condition_matches(
    condition: &InquiryGraphEdgeCondition,
    answers: &HashMap<&str, &InquiryGraphAnswer>,
) -> bool {
    match condition {
        InquiryGraphEdgeCondition::Always => true,
        InquiryGraphEdgeCondition::AnswerEquals { node_id, choice_id } => answers
            .get(node_id.as_str())
            .is_some_and(|answer| answer.choice_ids.iter().any(|choice| choice == choice_id)),
        InquiryGraphEdgeCondition::AnswerIn {
            node_id,
            choice_ids,
        } => answers.get(node_id.as_str()).is_some_and(|answer| {
            answer
                .choice_ids
                .iter()
                .any(|choice| choice_ids.contains(choice))
        }),
        InquiryGraphEdgeCondition::Unknown { node_id } => {
            answers.get(node_id.as_str()).is_some_and(|answer| {
                answer.unknown || answer.answer_kind == InquiryGraphAnswerKind::Unknown
            })
        }
        InquiryGraphEdgeCondition::JsonLogic { .. } => false,
    }
}

pub fn project_session(
    package: &InquiryGraphPackage,
    session: &InquiryGraphSessionState,
) -> Result<InquiryGraphSessionProjection> {
    project_session_with_resolutions(package, session, &BTreeMap::new())
}

pub fn project_session_with_resolutions(
    package: &InquiryGraphPackage,
    session: &InquiryGraphSessionState,
    artifacts: &InquiryGraphNodeResolutionArtifacts,
) -> Result<InquiryGraphSessionProjection> {
    let current_node = match session.current_node_id.as_deref() {
        Some(node_id) => Some(effective_node_by_id(package, session, node_id, artifacts)?),
        None => None,
    };
    let resolution = current_node
        .as_ref()
        .and_then(|node| node.resolution.as_ref().map(|spec| (node, spec)))
        .map(|(node, spec)| {
            let reference = session.node_resolution_refs.get(&node.node_id);
            let state = match reference.map(|reference| reference.status) {
                Some(InquiryGraphNodeResolutionStatus::Pending) => {
                    InquiryGraphResolutionState::Pending
                }
                Some(InquiryGraphNodeResolutionStatus::Resolved)
                    if active_resolution_artifact(session, &node.node_id, artifacts).is_some() =>
                {
                    InquiryGraphResolutionState::Resolved
                }
                Some(InquiryGraphNodeResolutionStatus::Failed) => {
                    InquiryGraphResolutionState::Failed
                }
                _ => InquiryGraphResolutionState::Pending,
            };
            InquiryGraphResolutionProjection {
                state,
                resolver_id: Some(spec.resolver_id.clone()),
                input_hash: reference.map(|reference| reference.input_hash.clone()),
                retryable: matches!(
                    state,
                    InquiryGraphResolutionState::Pending | InquiryGraphResolutionState::Failed
                ),
                error: None,
            }
        })
        .unwrap_or_default();
    let estimated_total_steps = package.nodes.len().max(1);
    let answered_node_count = session.answers.len();
    let current_step = answered_node_count.saturating_add(usize::from(current_node.is_some()));
    let progress_percent = match session.status {
        InquiryGraphSessionStatus::Completed => 100,
        _ => (((answered_node_count as f32 / estimated_total_steps as f32) * 100.0).round() as u8)
            .min(99),
    };
    Ok(InquiryGraphSessionProjection {
        session_id: session.session_id.clone(),
        revision: session.revision,
        package_title: package.manifest.title.clone(),
        package_kind: package.manifest.package_kind.clone(),
        current_node: current_node.clone(),
        resolution,
        current_step,
        estimated_total_steps,
        answered_node_count,
        progress_percent,
        can_go_back: session.answers.len() > 1,
        can_skip: current_node
            .as_ref()
            .is_some_and(|node| node.skip_allowed || node.allow_unknown),
        can_submit: session.status == InquiryGraphSessionStatus::Completed,
        status: session.status,
    })
}

pub fn build_result(
    package: &InquiryGraphPackage,
    session: &InquiryGraphSessionState,
    completed_at_secs: i64,
) -> Result<InquiryGraphResultArtifact> {
    build_result_with_resolutions(package, session, completed_at_secs, &BTreeMap::new())
}

pub fn build_result_with_resolutions(
    package: &InquiryGraphPackage,
    session: &InquiryGraphSessionState,
    completed_at_secs: i64,
    artifacts: &InquiryGraphNodeResolutionArtifacts,
) -> Result<InquiryGraphResultArtifact> {
    ensure!(
        session.status == InquiryGraphSessionStatus::Completed,
        "cannot build result for non-completed InquiryGraph session"
    );
    ensure!(
        session.package_id == package.manifest.package_id
            && session.package_version == package.manifest.package_version,
        "session package identity does not match package"
    );
    for node in package
        .nodes
        .iter()
        .filter(|node| node.resolution.is_some())
        .filter(|node| session.visited_node_ids.contains(&node.node_id))
    {
        ensure!(
            active_resolution_artifact(session, &node.node_id, artifacts).is_some(),
            "completed session is missing active node-resolution artifact for {}",
            node.node_id
        );
    }
    let node_resolutions = package
        .nodes
        .iter()
        .filter_map(|node| active_resolution_artifact(session, &node.node_id, artifacts).cloned())
        .collect::<Vec<_>>();
    Ok(InquiryGraphResultArtifact {
        schema_version: INQUIRY_GRAPH_RESULT_SCHEMA_VERSION.to_string(),
        session_id: session.session_id.clone(),
        package_id: session.package_id.clone(),
        package_version: session.package_version.clone(),
        completed_at_secs,
        answers: session.answers.clone(),
        node_resolutions,
        derived_profile: derive_profile_json(package, session, artifacts),
        calibration_patch: build_calibration_patch(package, session),
        learning_summary: build_learning_summary(package, session),
        consent: session.consent.clone(),
        result_target: session.result_target.clone(),
    })
}

pub fn apply_review_decision(
    mut review: InquiryGraphReviewRecord,
    decision: InquiryGraphReviewDecision,
    updated_at_secs: i64,
) -> Result<InquiryGraphReviewRecord> {
    ensure!(
        review.status == InquiryGraphReviewStatus::Pending,
        "cannot decide InquiryGraph review {} in status {:?}",
        review.review_id,
        review.status
    );
    ensure!(
        decision.reviewer_ref == review.reviewer_ref,
        "reviewer_ref {} cannot decide InquiryGraph review {} for reviewer_ref {}",
        decision.reviewer_ref,
        review.review_id,
        review.reviewer_ref
    );
    if !review.policy.allowed_decisions.is_empty() {
        ensure!(
            review.policy.allowed_decisions.contains(&decision.decision),
            "decision {} is not allowed for InquiryGraph review {}",
            decision.decision.as_str(),
            review.review_id
        );
    }
    if let Some(expires_at_secs) = review.policy.expires_at_secs {
        ensure!(
            decision.decided_at_secs <= expires_at_secs,
            "InquiryGraph review {} expired before decision",
            review.review_id
        );
    }

    review.status = decision.decision.review_status();
    review.decided_at_secs = Some(decision.decided_at_secs);
    review.latest_decision = Some(decision);
    review.updated_at_secs = updated_at_secs;
    Ok(review)
}

fn derive_profile_json(
    package: &InquiryGraphPackage,
    session: &InquiryGraphSessionState,
    artifacts: &InquiryGraphNodeResolutionArtifacts,
) -> JsonValue {
    let answer_values = session
        .answers
        .iter()
        .map(|answer| {
            let node = effective_node_by_id(package, session, &answer.node_id, artifacts).ok();
            let labels = node
                .as_ref()
                .map(|node| {
                    answer
                        .choice_ids
                        .iter()
                        .filter_map(|choice_id| {
                            node.choices
                                .iter()
                                .find(|choice| choice.choice_id == *choice_id)
                                .map(|choice| choice.label.clone())
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            serde_json::json!({
                "node_id": answer.node_id.clone(),
                "choice_ids": answer.choice_ids.clone(),
                "choice_labels": labels,
                "text": answer.text.clone(),
                "unknown": answer.unknown,
                "metadata": answer.metadata.clone(),
            })
        })
        .collect::<Vec<_>>();
    let mut profile = serde_json::Map::from_iter([
        (
            "package_kind".to_string(),
            JsonValue::String(package.manifest.package_kind.clone()),
        ),
        ("answers".to_string(), JsonValue::Array(answer_values)),
    ]);
    for node in &package.nodes {
        let Some(spec) = &node.resolution else {
            continue;
        };
        let Some(artifact) = active_resolution_artifact(session, &node.node_id, artifacts) else {
            continue;
        };
        let mut fragment = match artifact.result_fragment.clone() {
            JsonValue::Object(object) => object,
            value => serde_json::Map::from_iter([("value".to_string(), value)]),
        };
        fragment.entry("items".to_string()).or_insert_with(|| {
            JsonValue::Array(
                artifact
                    .choices
                    .iter()
                    .map(|choice| choice.value.clone())
                    .collect(),
            )
        });
        if let Some(selected_choice_id) = session
            .answers
            .iter()
            .find(|answer| answer.node_id == node.node_id)
            .and_then(|answer| answer.choice_ids.first())
        {
            fragment.insert(
                "selected_choice_id".to_string(),
                JsonValue::String(selected_choice_id.clone()),
            );
            if let Some(choice) = artifact
                .choices
                .iter()
                .find(|choice| &choice.choice_id == selected_choice_id)
            {
                fragment.insert("selected_candidate".to_string(), choice.value.clone());
            }
        }
        profile.insert(
            spec.output.result_namespace.clone(),
            JsonValue::Object(fragment),
        );
    }
    JsonValue::Object(profile)
}

fn build_learning_summary(
    package: &InquiryGraphPackage,
    session: &InquiryGraphSessionState,
) -> InquiryGraphLearningSummary {
    let mut summary = InquiryGraphLearningSummary {
        title: Some("This time Familiar learned".to_string()),
        ..InquiryGraphLearningSummary::default()
    };

    for answer in &session.answers {
        let Some(node) = package
            .nodes
            .iter()
            .find(|node| node.node_id == answer.node_id)
        else {
            continue;
        };

        if answer.unknown
            || matches!(
                answer.answer_kind,
                InquiryGraphAnswerKind::Unknown | InquiryGraphAnswerKind::Skip
            )
        {
            let statement = metadata_string(&node.metadata, "uncertain_statement")
                .unwrap_or_else(|| format!("Still uncertain: {}", node.title));
            summary.uncertain_items.push(InquiryGraphUncertainItem {
                item_id: format!("uncertain.{}", node.node_id),
                statement,
                evidence_node_ids: vec![node.node_id.clone()],
                suggested_followup: metadata_string(&node.metadata, "suggested_followup"),
            });
            continue;
        }

        if node.emits.is_empty() {
            continue;
        }

        let learned_statement = metadata_string(&node.metadata, "learned_statement")
            .unwrap_or_else(|| answer_summary(node, answer));
        let proposed_behavior = metadata_string(&node.metadata, "proposed_behavior");
        let target = metadata_string(&node.metadata, "result_target").or_else(|| {
            node.emits
                .first()
                .map(|target| format!("{:?}", target).to_ascii_lowercase())
        });
        let persistence = metadata_string(&node.metadata, "persistence")
            .or_else(|| Some(package.manifest.privacy.memory_write_policy.clone()));

        summary.learned_items.push(InquiryGraphLearnedItem {
            item_id: format!("learn.{}", node.node_id),
            statement: learned_statement.clone(),
            evidence_node_ids: vec![node.node_id.clone()],
            proposed_behavior: proposed_behavior.clone(),
            target: target.clone(),
            persistence,
        });

        if let Some(description) = proposed_behavior {
            summary.behavior_changes.push(InquiryGraphBehaviorChange {
                change_id: format!("behavior.{}", node.node_id),
                description,
                target,
                requires_user_confirmation: package.manifest.privacy.memory_write_policy
                    == "requires_user_confirmation",
            });
        }
    }

    summary.memory_candidates = build_calibration_patch(package, session).memory_candidates;
    summary
}

fn metadata_string(metadata: &JsonValue, key: &str) -> Option<String> {
    metadata
        .get(key)
        .and_then(|value| value.as_str())
        .map(|value| value.to_string())
}

fn build_calibration_patch(
    package: &InquiryGraphPackage,
    session: &InquiryGraphSessionState,
) -> InquiryGraphCalibrationPatch {
    let mut patch = InquiryGraphCalibrationPatch::default();
    for answer in &session.answers {
        let Some(node) = package
            .nodes
            .iter()
            .find(|node| node.node_id == answer.node_id)
        else {
            continue;
        };
        for target in &node.emits {
            match target {
                InquiryGraphPatchTarget::PromptProfile
                | InquiryGraphPatchTarget::StylePreference
                | InquiryGraphPatchTarget::ActivationIdentity => {
                    patch.prompt_fragments.push(InquiryGraphPromptFragment {
                        key: format!("{}.{}", package.manifest.package_id, node.node_id),
                        text: answer_summary(node, answer),
                        source_node_id: node.node_id.clone(),
                    });
                }
                InquiryGraphPatchTarget::MemoryCandidate => {
                    patch.memory_candidates.push(InquiryGraphMemoryCandidate {
                        key: format!("{}.{}", package.manifest.package_id, node.node_id),
                        value: answer_summary(node, answer),
                        source_node_id: node.node_id.clone(),
                        confidence: if answer.unknown { "low" } else { "medium" }.to_string(),
                    });
                }
                InquiryGraphPatchTarget::KnowledgeSeed => {
                    if let Some(text) = answer.text.as_ref() {
                        patch.knowledge_seed_refs.push(text.clone());
                    }
                }
                InquiryGraphPatchTarget::ToolPreference
                | InquiryGraphPatchTarget::AgentBehavior
                | InquiryGraphPatchTarget::ConsentPolicy
                | InquiryGraphPatchTarget::ProvisioningRequiredFields => {
                    patch.preference_updates.push(InquiryGraphPreferenceUpdate {
                        key: format!("{}.{}", package.manifest.package_id, node.node_id),
                        value: serde_json::json!({
                            "choice_ids": answer.choice_ids.clone(),
                            "text": answer.text.clone(),
                            "unknown": answer.unknown,
                        }),
                        source_node_id: node.node_id.clone(),
                    });
                }
            }
        }
    }
    patch
}

fn answer_summary(node: &InquiryGraphNode, answer: &InquiryGraphAnswer) -> String {
    if answer.unknown {
        return format!("{}: unknown", node.title);
    }
    if let Some(text) = answer.text.as_ref() {
        return format!("{}: {}", node.title, text);
    }
    let labels = answer
        .choice_ids
        .iter()
        .filter_map(|choice_id| {
            node.choices
                .iter()
                .find(|choice| choice.choice_id == *choice_id)
                .map(|choice| choice.label.clone())
        })
        .collect::<Vec<_>>();
    format!("{}: {}", node.title, labels.join(", "))
}

pub fn cancel_session(
    mut session: InquiryGraphSessionState,
    now_secs: i64,
) -> InquiryGraphSessionState {
    session.status = InquiryGraphSessionStatus::Cancelled;
    session.revision = session.revision.saturating_add(1);
    session.updated_at_secs = now_secs;
    session
}

pub fn answer_choice(
    node_id: impl Into<String>,
    choice_id: impl Into<String>,
    answered_at_secs: i64,
) -> InquiryGraphAnswer {
    InquiryGraphAnswer {
        node_id: node_id.into(),
        answer_kind: InquiryGraphAnswerKind::Choice,
        choice_ids: vec![choice_id.into()],
        text: None,
        number: None,
        unknown: false,
        answered_at_secs,
        metadata: JsonValue::Null,
    }
}

pub fn answer_unknown(node_id: impl Into<String>, answered_at_secs: i64) -> InquiryGraphAnswer {
    InquiryGraphAnswer {
        node_id: node_id.into(),
        answer_kind: InquiryGraphAnswerKind::Unknown,
        choice_ids: Vec::new(),
        text: None,
        number: None,
        unknown: true,
        answered_at_secs,
        metadata: JsonValue::Null,
    }
}

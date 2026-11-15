use crate::model::{
    ManagementPolicyPatch, PromptCandidateResponse, PromptCandidateValidationScope,
    PromptCandidateValidationTarget, PromptPackage,
};
use polar_lgp_model::{PromptAssetClass, PromptBackend};
use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptPackageValidationError {
    pub issues: Vec<PromptPackageValidationIssue>,
}

impl Display for PromptPackageValidationError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let message = self
            .issues
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ");
        write!(f, "{message}")
    }
}

impl std::error::Error for PromptPackageValidationError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptCandidateValidationError {
    pub issues: Vec<PromptCandidateValidationIssue>,
}

impl Display for PromptCandidateValidationError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let message = self
            .issues
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ");
        write!(f, "{message}")
    }
}

impl std::error::Error for PromptCandidateValidationError {}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PromptPackageValidationIssue {
    pub code: String,
    pub path: String,
    pub message: String,
}

impl Display for PromptPackageValidationIssue {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} at {}: {}", self.code, self.path, self.message)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PromptCandidateValidationIssue {
    pub code: String,
    pub path: String,
    pub message: String,
}

impl Display for PromptCandidateValidationIssue {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} at {}: {}", self.code, self.path, self.message)
    }
}

pub fn validate_prompt_candidate_response(
    response: &PromptCandidateResponse,
    scope: &PromptCandidateValidationScope,
) -> Vec<PromptCandidateValidationIssue> {
    let mut issues = Vec::new();
    if response.candidates.is_empty() {
        issues.push(candidate_issue(
            "missing_candidates",
            "candidates",
            "candidate response must include at least one candidate".to_string(),
        ));
        return issues;
    }

    for (candidate_index, candidate) in response.candidates.iter().enumerate() {
        let candidate_path = format!("candidates[{candidate_index}]");
        if candidate.candidate_id.trim().is_empty() {
            issues.push(candidate_issue(
                "missing_candidate_id",
                &format!("{candidate_path}.candidate_id"),
                "candidate_id must not be empty".to_string(),
            ));
        }
        if candidate.rationale.trim().is_empty() {
            issues.push(candidate_issue(
                "missing_rationale",
                &format!("{candidate_path}.rationale"),
                "rationale must not be empty".to_string(),
            ));
        }
        if candidate.risk.trim().is_empty() {
            issues.push(candidate_issue(
                "missing_risk",
                &format!("{candidate_path}.risk"),
                "risk must not be empty".to_string(),
            ));
        }
        if candidate.patches.is_empty() {
            issues.push(candidate_issue(
                "missing_patches",
                &format!("{candidate_path}.patches"),
                "candidate must include at least one patch".to_string(),
            ));
            continue;
        }

        let mut targets_alpha_builder = false;
        let mut mentions_bundle_validate = false;
        let mut mentions_bundle_preview = false;
        for (patch_index, patch) in candidate.patches.iter().enumerate() {
            let patch_path = format!("{candidate_path}.patches[{patch_index}]");
            if patch.asset_id.trim().is_empty() {
                issues.push(candidate_issue(
                    "missing_asset_id",
                    &format!("{patch_path}.asset_id"),
                    "patch asset_id must not be empty".to_string(),
                ));
            }
            if patch.body_text.trim().is_empty() {
                issues.push(candidate_issue(
                    "missing_body_text",
                    &format!("{patch_path}.body_text"),
                    "patch body_text must not be empty".to_string(),
                ));
            }

            let lowered = patch.body_text.to_ascii_lowercase();
            mentions_bundle_validate |= lowered.contains("bundle_validate");
            mentions_bundle_preview |= lowered.contains("bundle_preview");
            if lowered.contains("summary")
                && lowered.contains("proposed_assets")
                && !lowered.contains("do not return summary")
            {
                issues.push(candidate_issue(
                    "legacy_optimizer_shape",
                    &format!("{patch_path}.body_text"),
                    "patch body reintroduces legacy summary/proposed_assets language".to_string(),
                ));
            }
            if scope.forbid_activation_attempts && contains_activation_attempt(&lowered) {
                issues.push(candidate_issue(
                    "activation_attempt",
                    &format!("{patch_path}.body_text"),
                    "candidate patch must not request direct activation or apply actions"
                        .to_string(),
                ));
            }

            match scope.expected_target {
                PromptCandidateValidationTarget::SystemAlphaBuilder => {
                    if patch.asset_id == "prompt.alpha.prompt-admin" {
                        targets_alpha_builder = true;
                    }
                    if !patch.asset_id.starts_with("prompt.alpha.") {
                        issues.push(candidate_issue(
                            "candidate_target_mismatch",
                            &format!("{patch_path}.asset_id"),
                            "alpha builder candidates must target prompt.alpha.* assets"
                                .to_string(),
                        ));
                    }
                }
            }
        }

        if matches!(
            scope.expected_target,
            PromptCandidateValidationTarget::SystemAlphaBuilder
        ) && !targets_alpha_builder
        {
            issues.push(candidate_issue(
                "candidate_target_mismatch",
                &format!("{candidate_path}.patches"),
                "alpha builder candidates must include a patch for prompt.alpha.prompt-admin"
                    .to_string(),
            ));
        }
        if scope.require_bundle_authoring_protocol && !mentions_bundle_validate {
            issues.push(candidate_issue(
                "missing_bundle_validate_mention",
                &candidate_path,
                "candidate patches must mention bundle_validate for builder optimization"
                    .to_string(),
            ));
        }
        if scope.require_bundle_authoring_protocol && !mentions_bundle_preview {
            issues.push(candidate_issue(
                "missing_bundle_preview_mention",
                &candidate_path,
                "candidate patches must mention bundle_preview for builder optimization"
                    .to_string(),
            ));
        }
    }

    issues
}

pub fn ensure_valid_prompt_candidate_response(
    response: &PromptCandidateResponse,
    scope: &PromptCandidateValidationScope,
) -> Result<(), PromptCandidateValidationError> {
    let issues = validate_prompt_candidate_response(response, scope);
    if issues.is_empty() {
        Ok(())
    } else {
        Err(PromptCandidateValidationError { issues })
    }
}

pub fn validate_prompt_package(
    package: &PromptPackage,
    policy_patch: &ManagementPolicyPatch,
) -> Vec<PromptPackageValidationIssue> {
    let mut issues = Vec::new();
    let validation = &package.validation;
    let assets = package
        .assets
        .iter()
        .map(|asset| (asset.asset_id.as_str(), asset))
        .collect::<BTreeMap<_, _>>();

    for activation in &package.activations {
        let activation_path = format!(
            "activations[{}/{}/{}]",
            activation.lifecycle.as_str(),
            activation.asset_class.as_str(),
            activation.asset_id
        );
        let Some(asset) = assets.get(activation.asset_id.as_str()).copied() else {
            if validation.require_asset_references {
                issues.push(issue(
                    "missing_asset",
                    &activation_path,
                    format!(
                        "activation references unknown asset_id {}",
                        activation.asset_id
                    ),
                ));
            }
            continue;
        };

        if validation.require_lifecycle_class_match
            && (asset.lifecycle != activation.lifecycle
                || asset.asset_class != activation.asset_class)
        {
            issues.push(issue(
                "asset_activation_mismatch",
                &activation_path,
                format!(
                    "activation lifecycle/class {}:{} does not match asset {}:{}",
                    activation.lifecycle.as_str(),
                    activation.asset_class.as_str(),
                    asset.lifecycle.as_str(),
                    asset.asset_class.as_str()
                ),
            ));
        }

        if validation.require_target_consistency {
            require_matching_target(
                &mut issues,
                &activation_path,
                "target_inhouse_polar_id",
                asset.target_inhouse_polar_id.clone(),
                activation.target_inhouse_polar_id.clone(),
            );
            require_matching_target(
                &mut issues,
                &activation_path,
                "target_system_role",
                asset
                    .target_system_role
                    .map(|value| value.as_str().to_string()),
                activation
                    .target_system_role
                    .map(|value| value.as_str().to_string()),
            );
            require_matching_target(
                &mut issues,
                &activation_path,
                "target_management_class",
                asset.target_management_class.clone(),
                activation.target_management_class.clone(),
            );
            require_matching_target(
                &mut issues,
                &activation_path,
                "target_channel_id",
                asset.target_channel_id.clone(),
                activation.target_channel_id.clone(),
            );
            require_matching_target(
                &mut issues,
                &activation_path,
                "target_thread_id",
                asset.target_thread_id.clone(),
                activation.target_thread_id.clone(),
            );
            require_matching_target(
                &mut issues,
                &activation_path,
                "target_backend",
                asset.target_backend.map(|value| value.as_str().to_string()),
                activation
                    .target_backend
                    .map(|value| value.as_str().to_string()),
            );
        }
    }

    if validation.require_backend_override_target_match {
        for asset in &package.assets {
            let expected_backend = match asset.asset_class {
                PromptAssetClass::BackendOverrideCodex => Some(PromptBackend::Codex),
                PromptAssetClass::BackendOverridePi => Some(PromptBackend::Pi),
                PromptAssetClass::BackendOverrideAdam => Some(PromptBackend::Adam),
                _ => None,
            };
            if let Some(expected_backend) = expected_backend {
                if asset.target_backend != Some(expected_backend) {
                    issues.push(issue(
                        "backend_override_target_mismatch",
                        &format!("assets[{}].target_backend", asset.asset_id),
                        format!(
                            "backend override asset {} must target backend {}",
                            asset.asset_id,
                            expected_backend.as_str()
                        ),
                    ));
                }
            }
        }
    }

    if validation.require_management_class_alignment {
        for target in &package.compile_targets {
            let Some(management_class) = target.management_class.as_deref() else {
                continue;
            };
            let path = format!("compile_targets[{}].management_class", target.target_id);
            let aligned = match (target.channel_id.as_deref(), target.thread_id.as_deref()) {
                (Some(channel_id), Some(thread_id)) => policy_patch.threads.iter().any(|thread| {
                    thread.channel_id == channel_id
                        && thread.thread_id == thread_id
                        && thread.management_class == management_class
                }),
                _ => policy_patch
                    .threads
                    .iter()
                    .any(|thread| thread.management_class == management_class),
            };
            if !aligned {
                issues.push(issue(
                    "management_class_mismatch",
                    &path,
                    format!(
                        "compile target management class {} is not aligned with management_policy_patch threads",
                        management_class
                    ),
                ));
            }
        }
    }

    issues
}

pub fn ensure_valid_prompt_package(
    package: &PromptPackage,
    policy_patch: &ManagementPolicyPatch,
) -> Result<(), PromptPackageValidationError> {
    let issues = validate_prompt_package(package, policy_patch);
    if issues.is_empty() {
        Ok(())
    } else {
        Err(PromptPackageValidationError { issues })
    }
}

fn require_matching_target(
    issues: &mut Vec<PromptPackageValidationIssue>,
    base_path: &str,
    field_name: &str,
    asset_value: Option<String>,
    activation_value: Option<String>,
) {
    if asset_value.as_deref().is_none() || asset_value == activation_value {
        return;
    }
    issues.push(issue(
        "target_conflict",
        &format!("{base_path}.{field_name}"),
        format!(
            "asset target {:?} must be empty or equal activation target {:?}",
            asset_value, activation_value
        ),
    ));
}

fn issue(code: &str, path: &str, message: String) -> PromptPackageValidationIssue {
    PromptPackageValidationIssue {
        code: code.to_string(),
        path: path.to_string(),
        message,
    }
}

fn candidate_issue(code: &str, path: &str, message: String) -> PromptCandidateValidationIssue {
    PromptCandidateValidationIssue {
        code: code.to_string(),
        path: path.to_string(),
        message,
    }
}

fn contains_activation_attempt(lowered: &str) -> bool {
    let safely_negated_activation = lowered.contains("do not activate production prompt")
        || lowered.contains("must not activate production prompt")
        || lowered.contains("never activate production prompt");
    let safely_negated_apply = lowered.contains("do not apply the bundle")
        || lowered.contains("must not apply the bundle")
        || lowered.contains("never apply the bundle");

    lowered.contains("prompt_bundle_activate")
        || lowered.contains("bundle_apply_local")
        || lowered.contains("bundle_apply_remote")
        || lowered.contains("activate the new prompt asset")
        || (lowered.contains("activate production prompt") && !safely_negated_activation)
        || (lowered.contains("apply the bundle") && !safely_negated_apply)
        || (lowered.contains("activate")
            && lowered.contains("production")
            && !lowered.contains("do not activate")
            && !lowered.contains("must not activate")
            && !lowered.contains("never activate"))
}



use polar_identity::UnixTimestamp;
use serde::{Deserialize, Serialize};

pub const BACKEND_FAMILY_CODEX: &str = "codex";
pub const LOGICAL_ROOT_WORKSPACE: &str = "workspace";
pub const REPLACEMENT_PROFILE_WORKSPACE_DEFAULT: &str = "workspace-default";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolReplacementMode {
    Disabled,
    Preferred,
    Required,
    Experimental,
}

impl ToolReplacementMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Preferred => "preferred",
            Self::Required => "required",
            Self::Experimental => "experimental",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "disabled" => Some(Self::Disabled),
            "preferred" => Some(Self::Preferred),
            "required" => Some(Self::Required),
            "experimental" => Some(Self::Experimental),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectiveReplacementAction {
    KeepNative,
    ReplaceWithPolar,
    ReplaceWithPolarExperimental,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolReplacementPolicyRecord {
    pub policy_id: String,
    pub backend_family: String,
    pub native_tool_name: String,
    pub replacement_tool_id: String,
    pub replacement_profile: Option<String>,
    pub inhouse_polar_id: Option<String>,
    pub runtime_profile: Option<String>,
    pub replacement_mode: ToolReplacementMode,
    pub enabled: bool,
    pub updated_by_inhouse_polar_id: String,
    #[serde(with = "unix_timestamp_serde")]
    pub updated_at: UnixTimestamp,
}

impl ToolReplacementPolicyRecord {
    pub fn specificity_score(&self) -> u8 {
        u8::from(self.inhouse_polar_id.is_some()) + u8::from(self.runtime_profile.is_some())
    }

    pub fn matches_scope(
        &self,
        inhouse_polar_id: Option<&str>,
        runtime_profile: Option<&str>,
    ) -> bool {
        scope_matches(self.inhouse_polar_id.as_deref(), inhouse_polar_id)
            && scope_matches(self.runtime_profile.as_deref(), runtime_profile)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolReplacementPolicyAuditRecord {
    pub audit_event_id: String,
    pub policy_id: String,
    pub action: String,
    pub before_json: Option<String>,
    pub after_json: String,
    pub updated_by_inhouse_polar_id: String,
    #[serde(with = "unix_timestamp_serde")]
    pub updated_at: UnixTimestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolReplacementCacheStateRecord {
    pub cache_key: String,
    pub state_json: String,
    #[serde(with = "unix_timestamp_serde")]
    pub updated_at: UnixTimestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetToolReplacementPolicyInput {
    pub backend_family: String,
    pub native_tool_name: String,
    pub replacement_tool_id: String,
    pub replacement_profile: Option<String>,
    pub inhouse_polar_id: Option<String>,
    pub runtime_profile: Option<String>,
    pub replacement_mode: ToolReplacementMode,
    pub enabled: bool,
    pub updated_by_inhouse_polar_id: String,
}

impl SetToolReplacementPolicyInput {
    pub fn policy_id(&self) -> String {
        build_tool_replacement_policy_id(
            &self.backend_family,
            self.inhouse_polar_id.as_deref(),
            self.runtime_profile.as_deref(),
            &self.native_tool_name,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolReplacementPolicyView {
    pub policy_id: String,
    pub backend_family: String,
    pub native_tool_name: String,
    pub replacement_tool_id: String,
    pub replacement_profile: Option<String>,
    pub inhouse_polar_id: Option<String>,
    pub runtime_profile: Option<String>,
    pub replacement_mode: ToolReplacementMode,
    pub enabled: bool,
    pub updated_by_inhouse_polar_id: String,
    pub updated_at: u64,
}

impl From<ToolReplacementPolicyRecord> for ToolReplacementPolicyView {
    fn from(record: ToolReplacementPolicyRecord) -> Self {
        Self {
            policy_id: record.policy_id,
            backend_family: record.backend_family,
            native_tool_name: record.native_tool_name,
            replacement_tool_id: record.replacement_tool_id,
            replacement_profile: record.replacement_profile,
            inhouse_polar_id: record.inhouse_polar_id,
            runtime_profile: record.runtime_profile,
            replacement_mode: record.replacement_mode,
            enabled: record.enabled,
            updated_by_inhouse_polar_id: record.updated_by_inhouse_polar_id,
            updated_at: record.updated_at.as_secs(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectiveToolReplacementView {
    pub backend_family: String,
    pub native_tool_name: String,
    pub scope_inhouse_polar_id: Option<String>,
    pub scope_runtime_profile: Option<String>,
    pub policy_id: Option<String>,
    pub replacement_tool_id: Option<String>,
    pub replacement_profile: Option<String>,
    pub replacement_mode: Option<ToolReplacementMode>,
    pub enabled: bool,
    pub effective_action: EffectiveReplacementAction,
    pub reason: String,
    pub updated_by_inhouse_polar_id: Option<String>,
    pub updated_at: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolReplacementExplainView {
    pub backend_family: String,
    pub native_tool_name: String,
    pub scope_inhouse_polar_id: Option<String>,
    pub scope_runtime_profile: Option<String>,
    pub effective: EffectiveToolReplacementView,
    pub candidates: Vec<ToolReplacementPolicyView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogicalPath {
    pub root: String,
    pub relative_path: String,
}

impl LogicalPath {
    pub fn workspace(relative_path: impl Into<String>) -> Self {
        Self {
            root: LOGICAL_ROOT_WORKSPACE.to_string(),
            relative_path: relative_path.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectoryEntryKind {
    File,
    Directory,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectoryEntryView {
    pub logical_path: LogicalPath,
    pub kind: DirectoryEntryKind,
    pub depth: usize,
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceListDirView {
    pub root: LogicalPath,
    pub offset: usize,
    pub limit: usize,
    pub total_entries: usize,
    pub truncated: bool,
    pub entries: Vec<DirectoryEntryView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceFileReadView {
    pub logical_path: LogicalPath,
    pub start_line: usize,
    pub end_line: usize,
    pub total_lines: usize,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceGrepMatchView {
    pub logical_path: LogicalPath,
    pub line_number: usize,
    pub line_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceGrepView {
    pub root: LogicalPath,
    pub pattern: String,
    pub case_sensitive: bool,
    pub total_matches: usize,
    pub truncated: bool,
    pub matches: Vec<WorkspaceGrepMatchView>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PatchChangeKind {
    Add,
    Update,
    Delete,
    Move,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchChangeView {
    pub kind: PatchChangeKind,
    pub path: LogicalPath,
    pub move_path: Option<LogicalPath>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceApplyPatchView {
    pub patch: String,
    pub changed_paths: Vec<LogicalPath>,
    pub changes: Vec<PatchChangeView>,
}

pub fn build_tool_replacement_policy_id(
    backend_family: &str,
    inhouse_polar_id: Option<&str>,
    runtime_profile: Option<&str>,
    native_tool_name: &str,
) -> String {
    format!(
        "{}::{}::{}::{}",
        sanitize_scope_segment(backend_family),
        sanitize_optional_scope_segment(inhouse_polar_id),
        sanitize_optional_scope_segment(runtime_profile),
        sanitize_scope_segment(native_tool_name),
    )
}

fn sanitize_optional_scope_segment(raw: Option<&str>) -> String {
    raw.map(sanitize_scope_segment)
        .unwrap_or_else(|| "*".to_string())
}

fn sanitize_scope_segment(raw: &str) -> String {
    let cleaned = raw
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if cleaned.is_empty() {
        "_".to_string()
    } else {
        cleaned
    }
}

fn scope_matches(policy_scope: Option<&str>, requested_scope: Option<&str>) -> bool {
    match policy_scope {
        Some(policy_scope) => requested_scope == Some(policy_scope),
        None => true,
    }
}

mod unix_timestamp_serde {
    use polar_identity::UnixTimestamp;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(value: &UnixTimestamp, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u64(value.as_secs())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<UnixTimestamp, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(UnixTimestamp::from_secs(u64::deserialize(deserializer)?))
    }
}

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TmuxStatusField {
    Connection,
    Host,
    Project,
    SessionType,
    SessionName,
    StartupPath,
    StartupBranch,
    Worktree,
    Workspace,
    Purpose,
    Program,
    User,
    PermissionMode,
    CreatedAt,
    SessionId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct TmuxStatusBarSettings(pub BTreeSet<TmuxStatusField>);

impl<'de> Deserialize<'de> for TmuxStatusBarSettings {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut fields = BTreeSet::deserialize(deserializer)?;
        fields.remove(&TmuxStatusField::Connection);
        Ok(Self(fields))
    }
}

pub type TmuxStatusFacts = BTreeMap<TmuxStatusField, String>;

#[derive(Deserialize)]
pub struct TmuxStatusBarPreview {
    pub fields: TmuxStatusBarSettings,
    pub facts: Option<TmuxStatusFacts>,
    #[serde(default)]
    pub purpose: Option<String>,
}

impl TmuxStatusBarPreview {
    pub fn view(mut self) -> TmuxStatusBarView {
        if let (Some(facts), Some(purpose)) = (&mut self.facts, self.purpose) {
            facts.insert(TmuxStatusField::Purpose, purpose.trim().to_owned());
        }
        self.fields.view(self.facts.as_ref())
    }
}

impl Default for TmuxStatusBarSettings {
    fn default() -> Self {
        use TmuxStatusField::*;
        Self([Host, Project, SessionType, SessionName].into())
    }
}

#[derive(Serialize)]
pub struct TmuxStatusFieldView {
    pub id: TmuxStatusField,
    pub label: &'static str,
    pub selected: bool,
}

#[derive(Serialize)]
pub struct TmuxStatusBarView {
    pub fields: Vec<TmuxStatusFieldView>,
    pub preview: String,
}

use TmuxStatusField::*;
const FIELDS: [(TmuxStatusField, &str); 14] = [
    (Host, "Host"),
    (Project, "Project or startup folder"),
    (SessionType, "Session type"),
    (SessionName, "Session name"),
    (StartupPath, "Full startup path"),
    (StartupBranch, "Startup Git branch"),
    (Worktree, "Worktree name"),
    (Workspace, "Workspace name"),
    (Purpose, "Task or purpose"),
    (Program, "Startup program"),
    (User, "Startup user"),
    (PermissionMode, "Startup permission mode"),
    (CreatedAt, "Creation time"),
    (SessionId, "tmux Session ID"),
];

impl TmuxStatusBarSettings {
    pub fn all_fields() -> Self {
        Self(FIELDS.iter().map(|&(field, _)| field).collect())
    }

    pub fn view(&self, facts: Option<&TmuxStatusFacts>) -> TmuxStatusBarView {
        TmuxStatusBarView {
            fields: FIELDS
                .iter()
                .map(|&(id, label)| TmuxStatusFieldView {
                    id,
                    label,
                    selected: self.0.contains(&id),
                })
                .collect(),
            preview: facts
                .map(|facts| self.render_facts(facts))
                .unwrap_or_default(),
        }
    }

    pub fn render_facts(&self, facts: &TmuxStatusFacts) -> String {
        [
            &[Host][..],
            &[Project],
            &[SessionType, SessionName],
            &[StartupPath, StartupBranch, Worktree, Workspace, Purpose],
            &[Program, User, PermissionMode, CreatedAt, SessionId],
        ]
        .iter()
        .map(|fields| {
            fields
                .iter()
                .filter(|field| self.0.contains(field))
                .filter_map(|field| facts.get(field).map(|value| value.trim()))
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
                .join(" · ")
        })
        .filter(|group| !group.is_empty())
        .collect::<Vec<_>>()
        .join("  │  ")
    }
}



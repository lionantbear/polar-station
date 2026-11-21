//! Shared Settings destinations. Domain controllers retain their own drafts;
//! native adapters retain focus, scroll offsets and Workspace geometry.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "page", rename_all = "snake_case")]
pub enum SettingsRoute {
    #[default]
    Home,
    Setup {
        remote: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        section: Option<String>,
    },
    AppearanceGroup,
    Agents,
    Intelligence,
    Appearance,
    TmuxStatusBar,
    Workspace,
    #[serde(alias = "workspaces")]
    WorkspaceLayout,
    StationBackend,
    Voice { section: Option<String> },
    Connections { target: Option<String>, object: Option<String>, #[serde(default,skip_serializing_if="Option::is_none")] section: Option<super::connections::Page> },
    Notifications,
    Connectors,
    Resources { resource: Option<String> },
}
impl SettingsRoute {
    fn setup(remote: bool) -> Self {
        Self::Setup { remote, target: None, section: None }
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct SettingsEntry {
    pub title: &'static str,
    pub subtitle: &'static str,
    pub symbol: &'static str,
    pub route: SettingsRoute,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SettingsNavigationInput {
    Open { route: SettingsRoute },
    Navigate { route: SettingsRoute },
    Back,
    Show,
    ShowLayout,
}
#[derive(Debug, Clone, Serialize)]
pub struct SettingsNavigationView {
    pub is_directory: bool,
    pub visible: bool,
    pub route: SettingsRoute,
    pub title: &'static str,
    pub can_back: bool,
    pub back_label: &'static str,
    pub scopes: Vec<SettingsEntry>,
    pub entries: Vec<SettingsEntry>,
}
#[derive(Debug, Default)]
pub struct SettingsNavigation {
    visible: bool,
    history: Vec<SettingsRoute>,
    route: SettingsRoute,
}
impl SettingsNavigation {
    pub fn dispatch(&mut self, input: SettingsNavigationInput) -> SettingsNavigationView {
        match input {
            SettingsNavigationInput::Open { route } => {
                self.visible = true;
                if route != self.route {
                    // Navigation depth is semantic, not a log of every clicked object.
                    // A feature link always has the directory as its parent.
                    self.history = match (&self.route, &route) {
                        (source @ SettingsRoute::Voice { .. }, SettingsRoute::Setup { section: Some(_), .. }) => vec![SettingsRoute::Home, source.clone()],
                        (SettingsRoute::Setup { .. }, SettingsRoute::Setup { .. }) => self.history.clone(),
                        (_, SettingsRoute::Home) => vec![],
                        (_, SettingsRoute::Workspace | SettingsRoute::TmuxStatusBar) => vec![SettingsRoute::Home, SettingsRoute::AppearanceGroup],
                        _ => vec![SettingsRoute::Home],
                    };
                    self.route = route;
                }
            }
            SettingsNavigationInput::Navigate { route } => {
                if self.view().is_directory && self.view().entries.iter().any(|entry| entry.route == route) && route != self.route {
                    self.history.push(self.route.clone()); self.route = route; self.visible = true;
                }
            }
            SettingsNavigationInput::Back => {
                if let Some(route) = self.history.pop() { self.route = route; }
            }
            SettingsNavigationInput::Show => self.visible = true,
            SettingsNavigationInput::ShowLayout => self.visible = false,
        }
        self.view()
    }
    pub fn view(&self) -> SettingsNavigationView {
        SettingsNavigationView {
            back_label: "Back to Settings",
            is_directory: matches!(self.route, SettingsRoute::Home | SettingsRoute::AppearanceGroup | SettingsRoute::Agents),
            visible: self.visible, route: self.route.clone(), can_back: !self.history.is_empty(),
            title: match &self.route {
                SettingsRoute::Home => "Settings",
                SettingsRoute::Setup { .. } => "Setup & diagnostics",
                SettingsRoute::AppearanceGroup => "Appearance & Workspace",
                SettingsRoute::Agents => "Agents & connections",
                SettingsRoute::Intelligence => "Session intelligence",
                SettingsRoute::Appearance => "Appearance",
                SettingsRoute::TmuxStatusBar => "tmux Status Bar",
                SettingsRoute::Workspace => "Workspace Preferences",
                SettingsRoute::WorkspaceLayout => "Layout",
                SettingsRoute::StationBackend => "Station Backend",
                SettingsRoute::Voice { .. } => "Voice",
                SettingsRoute::Connections { .. } => "Agents & connections",
                SettingsRoute::Notifications => "Notifications",
                SettingsRoute::Connectors => "Codex monitoring and messages",
                SettingsRoute::Resources { .. } => "Resources",
            },
            scopes: vec![
                entry("This computer", "", "desktopcomputer", SettingsRoute::setup(false)),
                entry("Remote hosts", "", "network", SettingsRoute::setup(true)),
            ],
            entries: match &self.route {
                SettingsRoute::AppearanceGroup => vec![
                    entry("Appearance", "Theme and application text", "paintbrush", SettingsRoute::Appearance),
                    entry("tmux Status Bar", "Static Session fields", "terminal", SettingsRoute::TmuxStatusBar),
                    entry("Workspace Preferences", "Terminal font", "square.split.2x2", SettingsRoute::Workspace),
                ],
                SettingsRoute::Agents => vec![
                    entry("Machines & endpoints", "SSH, Polar and Codex", "network", SettingsRoute::Connections { target: None, object: None, section: None }),
                    entry("Session intelligence", "Summaries and monitoring", "sparkles", SettingsRoute::Intelligence),
                ],
                _ => vec![
                entry("Setup & diagnostics", "This computer and remote hosts", "checkmark.shield", SettingsRoute::setup(false)),
                entry("Appearance & Workspace", "Theme, text and layout", "paintbrush", SettingsRoute::AppearanceGroup),
                entry("Station Backend", "Local compact or full Polar; applies to every Workspace", "server.rack", SettingsRoute::StationBackend),
                entry("Voice", "Dictate, cleanup and terminology", "mic", SettingsRoute::Voice { section: None }),
                entry("Agents & connections", "Accounts, endpoints and machines", "network", SettingsRoute::Agents),
                entry("Notifications", "Delivery and attention", "bell", SettingsRoute::Notifications),
                entry("Resources", "Files, ports and devices", "externaldrive", SettingsRoute::Resources { resource: None }),
            ],
            },
        }
    }
}
fn entry(title: &'static str, subtitle: &'static str, symbol: &'static str, route: SettingsRoute) -> SettingsEntry {
    SettingsEntry { title, subtitle, symbol, route }
}


//! Toolkit-independent controls for an Idea Tree view, including local previews.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    OpenFiles,
    SelectBrowser,
    SelectTheme,
    InstallViewPackage,
    ExportViewPackage,
    UninstallViewPackage,
    LoadRelated,
    PauseLoading,
    Back,
    Forward,
    Search,
    Locate,
    Read,
    Fit,
    FocusBranch,
    Tilt,
    Relations,
    CanvasTwoD,
    CanvasLayered,
    NavigationAnimation,
    FrameEndpoints,
    PathFeedback,
    CameraInertia,
    NodeElasticity,
    ReduceMotion,
    OpenSource,
    RelationshipFocus,
    FocusThisNode,
    FramingReading,
    FramingBoth,
    FramingFixed,
    ConnectionFlow,
    ConnectionLabels,
    BrowserMindMap,
    BrowserDepthForest,
    ThemeDefault,
    ThemeChristmas,
}

pub type BrowserId = String;
pub type ThemeId = String;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ViewChoice {
    pub id: String,
    pub title: String,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CanvasMode {
    #[default]
    TwoD,
    Layered,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FramingPolicy {
    #[default]
    ReadingPriority,
    BothEndpoints,
    FixedZoom,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RelatedSource {
    pub path: String,
    pub label: String,
    pub status: String,
    pub reason: String,
}

/// Explicit local-file reference captured by the composer, not edit authority.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NodeReference {
    pub source_path: String,
    pub source_sha256: String,
    pub node_key: String,
    pub node_uuid: String,
    pub title: String,
    pub details_open: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstructionAnchor {
    #[serde(deserialize_with = "deserialize_anchor_coordinate")]
    pub x: f64,
    #[serde(deserialize_with = "deserialize_anchor_coordinate")]
    pub y: f64,
    #[serde(deserialize_with = "deserialize_anchor_coordinate")]
    pub width: f64,
    #[serde(deserialize_with = "deserialize_anchor_coordinate")]
    pub height: f64,
}

// Internally tagged native inputs buffer numbers through Serde Content. The
// full Host enables arbitrary_precision, whose buffered decimals use JSON's
// Number representation. Reuse that decoder rather than a plain f64 visitor.
fn deserialize_anchor_coordinate<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<f64, D::Error> {
    serde_json::Number::deserialize(deserializer)?
        .as_f64()
        .filter(|value| value.is_finite())
        .ok_or_else(|| serde::de::Error::custom("anchor coordinate must be a finite number"))
}

/// Presentation tokens exported by the current canvas theme, not draft data.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstructionStyle {
    pub card: String,
    pub text: String,
    pub muted: String,
    pub selection: String,
    pub border: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ControlsState {
    pub source_paths: Vec<String>,
    pub related_sources: Vec<RelatedSource>,
    pub loading_paused: bool,
    pub source_refresh_count: u64,
    pub selected_reference: Option<NodeReference>,
    pub instruction_request_id: u64,
    pub instruction_anchor: Option<InstructionAnchor>,
    pub instruction_editor_rect: Option<InstructionAnchor>,
    pub instruction_style: Option<InstructionStyle>,
    pub browser_id: BrowserId,
    pub theme_id: ThemeId,
    pub browsers: Vec<ViewChoice>,
    pub themes: Vec<ViewChoice>,
    pub view_packages: Vec<ViewChoice>,
    pub ready: bool,
    pub has_selection: bool,
    pub can_back: bool,
    pub can_forward: bool,
    pub reader_open: bool,
    pub tilted: bool,
    pub canvas_mode: CanvasMode,
    pub canvas_modes: Vec<CanvasMode>,
    pub navigation_animation: bool,
    pub frame_endpoints: bool,
    pub framing_policy: FramingPolicy,
    pub connection_flow: bool,
    pub connection_labels: bool,
    pub relationship_focus: bool,
    pub path_feedback: bool,
    pub camera_inertia: bool,
    pub node_elasticity: bool,
    pub reduce_motion: bool,
    pub relations: bool,
    pub selection: String,
    pub status: String,
}

impl Default for ControlsState {
    fn default() -> Self {
        Self {
            source_paths: Vec::new(),
            related_sources: Vec::new(),
            loading_paused: false,
            source_refresh_count: 0,
            selected_reference: None,
            instruction_request_id: 0,
            instruction_anchor: None,
            instruction_editor_rect: None,
            instruction_style: None,
            browser_id: "depth_forest".into(),
            theme_id: "default".into(),
            browsers: Vec::new(),
            themes: Vec::new(),
            view_packages: Vec::new(),
            ready: false,
            has_selection: false,
            can_back: false,
            can_forward: false,
            reader_open: false,
            tilted: false,
            relations: false,
            canvas_mode: CanvasMode::TwoD,
            canvas_modes: vec![CanvasMode::Layered],
            navigation_animation: true,
            frame_endpoints: true,
            framing_policy: FramingPolicy::ReadingPriority,
            connection_flow: true,
            connection_labels: true,
            relationship_focus: false,
            path_feedback: true,
            camera_inertia: true,
            node_elasticity: true,
            reduce_motion: false,
            selection: String::new(),
            status: String::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Control {
    pub action: Action,
    pub label: String,
    pub query: String,
    pub enabled: bool,
    pub active: bool,
    pub overflow: bool,
}
impl ControlsState {
    pub fn controls(&self) -> Vec<Control> {
        use Action::*;
        let mut controls: Vec<Control> = [
            (
                BrowserMindMap,
                "Mind Map",
                self.ready,
                self.browser_id == "mind_map",
                true,
            ),
            (
                BrowserDepthForest,
                "Depth Forest",
                self.ready,
                self.browser_id == "depth_forest",
                true,
            ),
            (
                ThemeDefault,
                "Default",
                self.ready,
                self.theme_id == "default",
                true,
            ),
            (
                ThemeChristmas,
                "Christmas",
                self.ready,
                self.theme_id == "christmas",
                true,
            ),
            (OpenFiles, "Open files", true, false, true),
            (Back, "←", self.can_back, false, false),
            (Forward, "→", self.can_forward, false, false),
            (Locate, "Locate", self.has_selection, false, true),
            (Read, "Read", self.has_selection, self.reader_open, true),
            (Fit, "Fit", self.ready, false, true),
            (
                RelationshipFocus,
                "Focus",
                self.has_selection,
                self.relationship_focus,
                false,
            ),
            (
                FocusThisNode,
                "Focus this node",
                self.relationship_focus && self.has_selection,
                false,
                true,
            ),
            (
                Tilt,
                "Switch Canvas mode",
                self.ready && self.canvas_modes.contains(&CanvasMode::TwoD),
                self.canvas_mode == CanvasMode::Layered,
                true,
            ),
            (
                CanvasTwoD,
                "2D",
                self.ready && self.canvas_modes.contains(&CanvasMode::TwoD),
                self.canvas_mode == CanvasMode::TwoD,
                false,
            ),
            (
                CanvasLayered,
                "2.5D",
                self.ready && self.canvas_modes.contains(&CanvasMode::Layered),
                self.canvas_mode == CanvasMode::Layered,
                false,
            ),
            (Relations, "Relations", self.ready, self.relations, true),
            (
                NavigationAnimation,
                "Navigation animation",
                self.ready,
                self.navigation_animation,
                true,
            ),
            (
                FramingReading,
                "Reading priority",
                self.ready,
                self.framing_policy == FramingPolicy::ReadingPriority,
                true,
            ),
            (
                FramingBoth,
                "Both endpoints",
                self.ready,
                self.framing_policy == FramingPolicy::BothEndpoints,
                true,
            ),
            (
                FramingFixed,
                "Fixed zoom",
                self.ready,
                self.framing_policy == FramingPolicy::FixedZoom,
                true,
            ),
            (ConnectionLabels, "Connection labels", self.ready, self.connection_labels, true),
            (
                ConnectionFlow,
                "Connection flow",
                self.ready,
                self.connection_flow,
                true,
            ),
            (
                PathFeedback,
                "Browse path feedback",
                self.ready,
                self.path_feedback,
                true,
            ),
            (
                CameraInertia,
                "Camera inertia",
                self.ready,
                self.camera_inertia,
                true,
            ),
            (
                NodeElasticity,
                "Node elasticity",
                self.ready,
                self.node_elasticity,
                true,
            ),
            (
                ReduceMotion,
                "Reduce motion",
                self.ready,
                self.reduce_motion,
                true,
            ),
            (
                OpenSource,
                "Open source file",
                self.has_selection,
                false,
                true,
            ),
        ]
        .into_iter()
        .map(|(action, label, enabled, active, overflow)| Control {
            action,
            label: label.into(),
            query: String::new(),
            enabled,
            active,
            overflow,
        })
        .collect();
        for (items, action, current) in [(&self.browsers, SelectBrowser, &self.browser_id), (&self.themes, SelectTheme, &self.theme_id)] {
            for item in items {
                controls.push(Control { action, label: item.title.clone(), query: item.id.clone(), enabled: self.ready, active: &item.id == current, overflow: true });
            }
        }
        controls.push(Control { action: InstallViewPackage, label: "Install trusted view package…".into(), query: String::new(), enabled: true, active: false, overflow: true });
        for package in &self.view_packages {
            for (action, verb) in [(ExportViewPackage,"Export"),(UninstallViewPackage,"Uninstall")] {
                controls.push(Control { action, label: format!("{verb} {}",package.title), query: package.id.clone(), enabled: true, active: false, overflow: true });
            }
        }
        if !self.related_sources.is_empty() {
            controls.push(Control { action: PauseLoading, label: if self.loading_paused { "Resume related loading" } else { "Pause related loading" }.into(), query: String::new(), enabled: self.ready, active: self.loading_paused, overflow: true });
        }
        controls.extend(self.related_sources.iter().map(|source| Control {
            action: LoadRelated, label: format!("Load {} · {}{}", source.label, source.status, if source.reason.is_empty() { String::new() } else { format!(" · {}",source.reason) }),
            query: source.path.clone(), enabled: self.ready && source.status != "loading", active: false, overflow: true,
        }));
        controls
    }
    pub fn permits(&self, action: Action) -> bool {
        if action == Action::FocusBranch {
            return self.has_selection;
        }
        if action == Action::FrameEndpoints {
            return self.ready;
        }
        if action == Action::Search {
            return self.ready;
        }
        self.controls()
            .iter()
            .any(|c| c.action == action && c.enabled)
    }
}



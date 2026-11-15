//! Curated product palettes. Terminal colors are loaded by Ghostty from its pinned theme bundle.
use crate::ValidationError;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeModeV1 {
    #[default]
    System,
    Preset,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemePresetV1 {
    #[default]
    CatppuccinMocha,
    CatppuccinLatte,
    Nord,
    Tokyonight,
    GruvboxDark,
    Dracula,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ThemeSettingsV1 {
    pub mode: ThemeModeV1,
    pub preset: ThemePresetV1,
    pub terminal_theme: Option<String>,
}
impl ThemeSettingsV1 {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if let Some(name) = &self.terminal_theme {
            if name.is_empty()
                || name.len() > 160
                || name.trim() != name
                || name.starts_with('.')
                || name
                    .chars()
                    .any(|c| c.is_control() || matches!(c, '/' | '\\'))
            {
                return Err(ValidationError::InvalidText("terminal theme name"));
            }
        }
        Ok(())
    }
    pub fn projection(&self) -> ThemeProjectionV1 {
        let choices: Vec<ThemePaletteV1> = serde_json::from_str(include_str!("themes.json"))
            .expect("bundled curated palettes are validated by tests");
        let selected = choices
            .iter()
            .find(|p| p.id == self.preset)
            .unwrap()
            .clone();
        let (light, dark) = if self.mode == ThemeModeV1::System {
            (choices[1].clone(), choices[0].clone())
        } else {
            (selected.clone(), selected)
        };
        ThemeProjectionV1 {
            choices,
            light,
            dark,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThemePaletteV1 {
    pub id: ThemePresetV1,
    pub name: String,
    pub dark: bool,
    pub background: String,
    pub foreground: String,
    pub surface: String,
    pub accent: String,
    pub selection_background: String,
    pub selection_foreground: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThemeProjectionV1 {
    pub choices: Vec<ThemePaletteV1>,
    pub light: ThemePaletteV1,
    pub dark: ThemePaletteV1,
}

//! Shared, data-only selection and last-valid content. Hosts own file I/O,
//! persistence and OS notifications; this module never changes content trees.
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ConfigurationInfo {
    pub title: String,
    pub path: String,
    pub requested_path: String,
    pub using_builtin: bool,
    pub revision: u64,
    pub error: String,
}
impl Default for ConfigurationInfo {
    fn default() -> Self {
        Self {
            title: "Built-in default".into(),
            path: String::new(),
            requested_path: String::new(),
            using_builtin: true,
            revision: 0,
            error: String::new(),
        }
    }
}

pub struct BrowsingConfiguration {
    builtin: String,
    content: String,
    info: ConfigurationInfo,
}
impl BrowsingConfiguration {
    pub fn new(builtin: &str) -> Result<Self, String> {
        let config = crate::perspectives::validate_configuration(builtin)?;
        let mut value = Self {
            builtin: builtin.into(),
            content: builtin.into(),
            info: ConfigurationInfo::default(),
        };
        value.info.title = title(&config);
        value.info.revision = 1;
        Ok(value)
    }
    pub fn info(&self) -> &ConfigurationInfo {
        &self.info
    }
    pub fn content(&self) -> &str {
        &self.content
    }
    pub fn select(&mut self, path: &str, text: &str) -> bool {
        if !std::path::Path::new(path).is_absolute() {
            self.info.error = "Choose an absolute browsing configuration path.".into();
            return false;
        }
        match crate::perspectives::validate_configuration(text) {
            Ok(config) => {
                if self.content != text {
                    self.info.revision += 1;
                }
                self.content = text.into();
                self.info.title = title(&config);
                self.info.path = path.into();
                self.info.requested_path = path.into();
                self.info.using_builtin = false;
                self.info.error.clear();
                true
            }
            Err(error) => {
                self.info.error = error;
                false
            }
        }
    }
    pub fn restore(&mut self, path: &str, text: &str) -> bool {
        let ok = self.select(path, text);
        if !ok {
            self.info.requested_path = path.into();
        }
        ok
    }
    pub fn reload(&mut self, text: &str) -> bool {
        let path = self.info.requested_path.clone();
        if path.is_empty() {
            return false;
        }
        self.select(&path, text)
    }
    pub fn use_default(&mut self) {
        if self.content != self.builtin {
            self.info.revision += 1;
        }
        self.content.clone_from(&self.builtin);
        self.info.title = title(&serde_json::from_str(&self.builtin).expect("validated builtin"));
        self.info.path.clear();
        self.info.requested_path.clear();
        self.info.using_builtin = true;
        self.info.error.clear();
    }
    pub fn watch_failed(&mut self, error: &str) {
        self.info.error = format!("Configuration notification failed: {error}");
    }
}
fn title(config: &Value) -> String {
    config["nodes"][config["root"].as_str().expect("validated root")]["properties"]["title"]
        .as_str()
        .unwrap_or("Browsing configuration")
        .into()
}



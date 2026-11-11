//! Process-wide presentation choice. Per-view navigation and drafts are separate.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewSelection {
    pub browser: String,
    pub theme: String,
}

impl ViewSelection {
    /// Callers validate package availability before committing this choice.
    pub fn replace(&mut self, browser: &str, theme: &str) -> bool {
        if browser.trim().is_empty() || theme.trim().is_empty() {
            return false;
        }
        self.browser = browser.into();
        self.theme = theme.into();
        true
    }
}



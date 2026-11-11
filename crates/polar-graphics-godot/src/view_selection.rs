use godot::classes::{IRefCounted, RefCounted};
use godot::prelude::*;
use polar_graphics_package::view_selection::ViewSelection;
use std::sync::{LazyLock, Mutex};

static SELECTION: LazyLock<Mutex<ViewSelection>> =
    LazyLock::new(|| Mutex::new(ViewSelection::default()));

#[derive(GodotClass)]
#[class(base=RefCounted)]
struct PolarViewSelection {
    base: Base<RefCounted>,
}

#[godot_api]
impl IRefCounted for PolarViewSelection {
    fn init(base: Base<RefCounted>) -> Self {
        Self { base }
    }
}

#[godot_api]
impl PolarViewSelection {
    #[func]
    fn current(&self) -> GString {
        serde_json::to_string(&*SELECTION.lock().unwrap_or_else(|e| e.into_inner()))
            .expect("selection is serializable")
            .as_str()
            .into()
    }
    #[func]
    fn select(&self, browser: GString, theme: GString) -> bool {
        SELECTION
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .replace(&browser.to_string(), &theme.to_string())
    }
}

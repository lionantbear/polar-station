//! Thin value bridge; business organization belongs to IDEA-CAVE presentation.
use godot::classes::{IRefCounted, RefCounted};
use godot::prelude::*;
use polar_ideacave_presentation::browsing_configuration::BrowsingConfiguration;
use polar_ideacave_presentation::perspectives::PerspectiveSession;
use serde_json::{json, Value};
use std::sync::{LazyLock, Mutex};

static CONFIGURATION: LazyLock<Mutex<Option<BrowsingConfiguration>>> =
    LazyLock::new(|| Mutex::new(None));

#[derive(GodotClass)]
#[class(base=RefCounted)]
struct PolarBrowsingConfiguration {
    base: Base<RefCounted>,
}
#[godot_api]
impl IRefCounted for PolarBrowsingConfiguration {
    fn init(base: Base<RefCounted>) -> Self {
        Self { base }
    }
}
#[godot_api]
impl PolarBrowsingConfiguration {
    #[func]
    fn initialize(&self, builtin: GString) -> GString {
        let mut state = CONFIGURATION.lock().unwrap_or_else(|e| e.into_inner());
        if state.is_none() {
            match BrowsingConfiguration::new(&builtin.to_string()) {
                Ok(value) => *state = Some(value),
                Err(error) => return encode(Err(error)),
            }
        }
        encode(Ok(json!({"ok":true})))
    }
    #[func]
    fn current(&self) -> GString {
        let state = CONFIGURATION.lock().unwrap_or_else(|e| e.into_inner());
        encode(
            state
                .as_ref()
                .map(|s| {
                    serde_json::to_value(s.info()).expect("configuration metadata is serializable")
                })
                .ok_or("Configuration is not initialized".into()),
        )
    }
    #[func]
    fn content(&self) -> GString {
        CONFIGURATION
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|s| s.content())
            .unwrap_or("")
            .into()
    }
    #[func]
    fn select_file(&self, path: GString, text: GString) -> bool {
        CONFIGURATION
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
            .is_some_and(|s| s.select(&path.to_string(), &text.to_string()))
    }
    #[func]
    fn restore_file(&self, path: GString, text: GString) -> bool {
        CONFIGURATION
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
            .is_some_and(|s| s.restore(&path.to_string(), &text.to_string()))
    }
    #[func]
    fn reload(&self, text: GString) -> bool {
        CONFIGURATION
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
            .is_some_and(|s| s.reload(&text.to_string()))
    }
    #[func]
    fn use_default(&self) {
        if let Some(s) = CONFIGURATION
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
        {
            s.use_default();
        }
    }
    #[func]
    fn watch_failed(&self, error: GString) {
        if let Some(s) = CONFIGURATION
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
        {
            s.watch_failed(&error.to_string());
        }
    }
}

#[derive(GodotClass)]
#[class(base=RefCounted)]
struct PolarIdeaPerspective {
    base: Base<RefCounted>,
    session: Option<PerspectiveSession>,
}
#[godot_api]
impl IRefCounted for PolarIdeaPerspective {
    fn init(base: Base<RefCounted>) -> Self {
        Self {
            base,
            session: None,
        }
    }
}
fn encode(result: Result<Value, String>) -> GString {
    let value = result.unwrap_or_else(|error| json!({"ok":false,"error":error}));
    value.to_string().as_str().into()
}
#[godot_api]
impl PolarIdeaPerspective {
    #[func]
    fn configure(&mut self, config: GString, graph: GString) -> GString {
        match PerspectiveSession::new(&config.to_string(), &graph.to_string()) {
            Ok(session) => {
                self.session = Some(session);
                encode(Ok(json!({"ok":true})))
            }
            Err(error) => encode(Err(error)),
        }
    }
    #[func]
    fn perspectives(&self) -> GString {
        encode(
            self.session
                .as_ref()
                .map(PerspectiveSession::perspectives)
                .ok_or("No input graph".into()),
        )
    }
    #[func]
    fn projection(&mut self, perspective: GString, level: i64) -> GString {
        encode(
            self.session
                .as_mut()
                .ok_or_else(|| "No input graph".to_owned())
                .and_then(|s| s.projection(&perspective.to_string(), level.max(0) as usize)),
        )
    }
    #[func]
    fn matches(&self, filter: GString) -> GString {
        encode(
            self.session
                .as_ref()
                .ok_or_else(|| "No input graph".to_owned())
                .and_then(|s| s.matches(&filter.to_string())),
        )
    }
    #[func]
    fn search(&self, query: GString) -> GString {
        encode(
            self.session
                .as_ref()
                .ok_or_else(|| "No input graph".to_owned())
                .and_then(|s| s.search(&query.to_string())),
        )
    }
    #[func]
    fn stats(&self) -> GString {
        encode(
            self.session
                .as_ref()
                .map(PerspectiveSession::stats)
                .ok_or("No input graph".into()),
        )
    }
}

use godot::classes::{IRefCounted, RefCounted};
use godot::prelude::*;
use std::path::Path;
#[derive(GodotClass)]
#[class(base=RefCounted)]
struct PolarViewPackages {
    base: Base<RefCounted>,
}
#[godot_api]
impl IRefCounted for PolarViewPackages {
    fn init(base: Base<RefCounted>) -> Self {
        Self { base }
    }
}
fn encode(value: Result<serde_json::Value, String>) -> GString {
    match value {
        Ok(value) => serde_json::json!({"ok":true,"value":value}),
        Err(error) => serde_json::json!({"ok":false,"error":error}),
    }
    .to_string()
    .as_str()
    .into()
}
#[godot_api]
impl PolarViewPackages {
    #[func]
    fn catalog(&self, root: GString) -> GString {
        encode(
            polar_graphics_package::list(Path::new(&root.to_string()))
                .map(|v| serde_json::to_value(v).expect("package serialization"))
                .map_err(|e| e.to_string()),
        )
    }
    #[func]
    fn install(&self, archive: GString, root: GString) -> GString {
        encode(
            polar_graphics_package::install(
                Path::new(&archive.to_string()),
                Path::new(&root.to_string()),
            )
            .map(|v| serde_json::to_value(v).expect("package serialization"))
            .map_err(|e| e.to_string()),
        )
    }
    #[func]
    fn export_package(
        &self,
        root: GString,
        id: GString,
        version: GString,
        destination: GString,
    ) -> GString {
        encode(
            polar_graphics_package::export(
                Path::new(&root.to_string()),
                &id.to_string(),
                &version.to_string(),
                Path::new(&destination.to_string()),
            )
            .map(|()| serde_json::Value::Null)
            .map_err(|e| e.to_string()),
        )
    }
    #[func]
    fn uninstall(&self, root: GString, id: GString, version: GString) -> GString {
        encode(
            polar_graphics_package::uninstall(
                Path::new(&root.to_string()),
                &id.to_string(),
                &version.to_string(),
            )
            .map(|()| serde_json::Value::Null)
            .map_err(|e| e.to_string()),
        )
    }
}

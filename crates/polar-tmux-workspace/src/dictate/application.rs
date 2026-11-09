//! Dictate application use cases shared by all native entrypoints.
//! The existing Engine owns state; ports supply existing Host and snapshot IO.
use super::{Engine, Input};
use serde_json::{Value, json};
use std::path::Path;

pub const MAX_INPUT: usize = 8 * 1024 * 1024;

pub trait Host {
    fn request(&self, payload: Value) -> Result<Value, String>;
}
pub trait Ports {
    fn connect_host(&mut self) -> Result<(), String>;
    fn host(&self) -> Option<&dyn Host>;
    fn exchange(
        &self,
        path: &Path,
        document: Option<&str>,
        maximum: usize,
    ) -> Result<Option<String>, String>;
    fn now_ms(&self) -> u64;
}
pub trait CapturePort: Host {
    fn begin(
        &self,
        id: &str,
        context: Option<String>,
        cleanup: bool,
        is_test: bool,
    ) -> Result<(), String>;
}

fn recording_missing(error: &str) -> bool {
    error.rsplit(": ").next() == Some("dictate.recording_missing")
}
fn reconcile_saved(
    engine: &mut Engine,
    mut get: impl FnMut(&str) -> Result<Value, String>,
) -> Result<(), String> {
    let ids: Vec<_> = engine
        .entries()
        .iter()
        .filter(|entry| entry.host_recording || entry.saved_samples > 0)
        .map(|entry| entry.operation_id.clone())
        .collect();
    for id in ids {
        match get(&id) {
            Ok(payload) if payload["recording"]["id"].as_str() == Some(&id) => {}
            Ok(_) => return Err("Invalid saved recording response; history was preserved".into()),
            Err(error) if recording_missing(&error) => {
                engine.discard_saved(&id);
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

pub struct Application<P> {
    ports: P,
    delivery_tokens: std::collections::HashMap<String, String>,
    engine: Engine,
    started: bool,
    path: Option<std::path::PathBuf>,
    storage_error: Option<String>,
    open_request: Option<Value>,
    open_error: Option<String>,
}
impl<P: Ports> Application<P> {
    pub fn new(ports: P) -> Self {
        Self {
            ports,
            delivery_tokens: Default::default(),
            engine: Engine::default(),
            started: false,
            path: None,
            storage_error: None,
            open_request: None,
            open_error: None,
        }
    }
    pub fn dispatch(&mut self, mut value: Value) -> Result<Value, String> {
        if value["type"] == "retry_open" {
            value = self.open_request.clone().ok_or("No failed Dictate open to retry")?;
        }
        let opening = value["type"] == "open" && !self.started;
        if opening {
            self.open_request = Some(value.clone());
        } else if let Some(error) = &self.open_error {
            if !matches!(value["type"].as_str(), Some("shortcut_binding" | "shortcut_status" | "shortcut_mode" | "snapshot")) {
                return Err(error.clone());
            }
        }
        match self.dispatch_inner(value) {
            Ok(mut result) => {
                if opening {
                    self.open_error = None;
                    self.open_request = None;
                    result = self.response(Vec::new());
                }
                Ok(result)
            }
            Err(error) => {
                if opening { self.open_error = Some(error.clone()); }
                Err(error)
            }
        }
    }
    /// Failed initialization retains its retry action across shortcut notifications.
    pub fn failure_response(&self, error: String) -> Value {
        let mut result = self.response(Vec::new());
        result["ok"] = json!(false);
        result["error"] = json!(error);
        result
    }
    fn dispatch_inner(&mut self, mut value: Value) -> Result<Value, String> {
        let kind = value.get("type").and_then(Value::as_str).map(str::to_owned);
        if kind.as_deref() == Some("open")
            && value.get("host_managed").and_then(Value::as_bool) == Some(true)
        {
            self.ports.connect_host()?;
        }
        if kind.as_deref() == Some("use_original") {
            if let (Some(host), Some(id)) = (
                self.ports.host(),
                value.get("operation_id").and_then(Value::as_str),
            ) {
                if self
                    .engine
                    .entries()
                    .iter()
                    .any(|e| e.operation_id == id && e.host_recording)
                {
                    host.request(json!({"op":"use_original","id":id}))?;
                }
            }
        }
        if kind.as_deref() == Some("delivery") {
            if let (Some(host), Some(id)) = (
                self.ports.host(),
                value.get("operation_id").and_then(Value::as_str),
            ) {
                if let Some(token) = self.delivery_tokens.remove(id) {
                    let applied = match value.get("outcome").and_then(Value::as_str) {
                        Some("applied") => Some(true),
                        Some("unchanged" | "no_target") => Some(false),
                        _ => None,
                    };
                    let result = host.request(
                        json!({"op":"complete_delivery","id":id,"token":token,"applied":applied,
                        "now_ms":self.ports.now_ms()}),
                    );
                    if result.is_err() {
                        value["outcome"] = json!("unknown");
                    }
                } else if self
                    .engine
                    .entries()
                    .iter()
                    .any(|e| e.operation_id == id && e.host_recording)
                {
                    value["outcome"] = json!("unknown");
                }
            }
        }
        let mut effects = match kind.as_deref() {
            Some("open") => {
                if self.started {
                    return Err("Open is only allowed before Dictate starts".into());
                }
                let path = std::path::PathBuf::from(
                    value
                        .get("path")
                        .and_then(Value::as_str)
                        .ok_or("Missing Dictate history path")?,
                );
                if !path.is_absolute() || path.file_name().is_none() {
                    return Err("Invalid Dictate history path".into());
                }
                // Never replace a corrupt, oversized or unreadable history with
                // an empty snapshot. Failed opens do not mutate the live handle.
                let document = self
                    .ports
                    .exchange(&path, None, MAX_INPUT)
                    .map_err(|_| "Cannot read Dictate history; existing history was preserved")?;
                let mut engine = match document {
                    Some(document) => {
                        Engine::restore(serde_json::from_str(&document).map_err(
                            |_| "Invalid Dictate history; existing history was preserved",
                        )?)?
                    }
                    None => Engine::default(),
                };
                if let Some(host) = self.ports.host() {
                    reconcile_saved(&mut engine, |id| host.request(json!({"op":"get", "id":id})))?;
                }
                engine.shortcut_status = self.engine.shortcut_status;
                engine.shortcut_trigger = self.engine.shortcut_trigger.clone();
                engine.shortcut_toggle = self.engine.shortcut_toggle;
                self.engine = engine;
                self.path = Some(path);
                self.started = true;
                vec![]
            }
            Some("restore") => {
                if self.started {
                    return Err("Restore is only allowed before Dictate starts".into());
                }
                self.engine = Engine::restore(
                    serde_json::from_value(
                        value
                            .get("snapshot")
                            .cloned()
                            .ok_or("Missing Dictate snapshot")?,
                    )
                    .map_err(|e| e.to_string())?,
                )?;
                self.started = true;
                vec![]
            }
            Some("snapshot") => vec![],
            Some("recording_deleted") => {
                let host = self
                    .ports
                    .host()
                    .ok_or("Recording deletion requires the local Host")?;
                let id = value
                    .get("operation_id")
                    .and_then(Value::as_str)
                    .ok_or("Missing recording ID")?;
                match host.request(json!({"op":"get", "id":id})) {
                    Err(error) if recording_missing(&error) => self.engine.discard_saved(id),
                    Err(error) => return Err(error),
                    Ok(_) => return Err("Recording still exists; draft was preserved".into()),
                }
            }
            Some("recover_recording") => {
                let host = self
                    .ports
                    .host()
                    .ok_or("Saved recording recovery requires the local Host")?;
                let id = value
                    .get("operation_id")
                    .and_then(Value::as_str)
                    .ok_or("Missing recording ID")?;
                let result = host.request(json!({"op":"get", "id":id}))?;
                let recording: polar_dictate::Recording =
                    serde_json::from_value(result["recording"].clone())
                        .map_err(|_| "Invalid saved recording")?;
                self.engine.stage_saved(&recording)?;
                self.started = true;
                vec![]
            }
            _ => {
                let input = serde_json::from_value::<Input>(value).map_err(|e| e.to_string())?;
                let effects = self.engine.dispatch(input)?;
                if !matches!(kind.as_deref(), Some("shortcut_binding" | "shortcut_status" | "shortcut_mode")) {
                    self.started = true;
                }
                effects
            }
        };
        if let Some(host) = self.ports.host() {
            let mut admitted = Vec::new();
            for effect in effects {
                if let super::Effect::Insert { operation_id, text, .. }
                    | super::Effect::InsertCurrent { operation_id, text } = &effect
                {
                    if self
                        .engine
                        .entries()
                        .iter()
                        .any(|e| e.operation_id == *operation_id && !e.host_recording)
                    {
                        admitted.push(effect);
                        continue;
                    }
                    let prepare = (|| -> Result<String, String> {
                        let payload = host.request(json!({"op":"get","id":operation_id}))?;
                        let record: polar_dictate::Recording =
                            serde_json::from_value(payload["recording"].clone())
                                .map_err(|_| "Invalid saved recording")?;
                        let edited = self.engine.entries().iter().any(|entry| entry.operation_id == *operation_id && entry.edited);
                        if !edited && record.text().as_deref() != Some(text) {
                            return Err("Saved text changed; review before inserting".into());
                        }
                        let token = polar_identity::generation::new_uuid_v4().to_string();
                        host.request(json!({"op":"prepare_delivery","id":operation_id,"token":token,"expected_revision":record.revision}))?;
                        Ok(token)
                    })();
                    match prepare {
                        Ok(token) => {
                            self.delivery_tokens.insert(operation_id.clone(), token);
                        }
                        Err(_) => {
                            self.engine.dispatch(Input::Delivery {
                                operation_id: operation_id.clone(),
                                outcome: super::DeliveryOutcome::Unchanged,
                            })?;
                            continue;
                        }
                    }
                }
                admitted.push(effect);
            }
            effects = admitted;
        }
        if !matches!(kind.as_deref(), Some("snapshot" | "audio_saved" | "transcript_preview")) {
            if let Some(path) = &self.path {
                let document = serde_json::to_string(&self.engine.persistent_snapshot())
                    .map_err(|e| e.to_string())?;
                match self.ports.exchange(path, Some(&document), MAX_INPUT) {
                    Ok(_) => self.storage_error = None,
                    Err(_) => {
                        self.storage_error = Some(
                            "Dictate could not save. Copy any text before closing the app.".into(),
                        );
                        // Cancellation must still reach capture and workers even
                        // when disk IO fails. No insertion/upload effect escapes.
                        effects.retain(|effect| matches!(effect, super::Effect::Cancel { .. }));
                        effects.extend(self.engine.dispatch(Input::StorageFailed)?);
                    }
                }
            }
        }
        Ok(self.response(effects))
    }
    fn response(&self, effects: Vec<super::Effect>) -> Value {
        let mut presentation = self.engine.presentation();
        if let Some(error) = self.storage_error.as_deref() {
            presentation.status = error.into();
        }
        if !self.started {
            presentation.enabled = false;
            presentation.action = "wait";
            presentation.status = "Starting Dictate…".into();
        }
        if let Some(error) = self.open_error.as_deref() {
            presentation.action = "retry_open";
            presentation.button_label = "Retry Dictate";
            presentation.enabled = true;
            presentation.status = error.into();
        }
        json!({"ok":true,"initialized":self.started,"entries":self.engine.entries(),"presentation":presentation,"history":self.engine.history(),
            "snapshot":self.engine.persistent_snapshot(),"panel":self.engine.panel(),"effects":effects})
    }
}

pub fn service_request(service: &impl CapturePort, value: Value) -> Result<Value, String> {
    match value.get("op").and_then(Value::as_str) {
        Some("capture_begin") => {
            service.begin(
                value
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or("Missing recording ID")?,
                value
                    .get("context")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                value
                    .get("cleanup")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
                value
                    .get("is_test")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            )?;
            Ok(json!({"ok":true}))
        }
        Some("refine_text") => {
            let id = value
                .get("id")
                .and_then(Value::as_str)
                .ok_or("Missing edit ID")?;
            let result = service.request(json!({"op":"refine_text","operation_id":id,
                    "text":value.get("raw").and_then(Value::as_str).ok_or("Missing draft")?,
                    "context":value.get("context").and_then(Value::as_str)}))?;
            if result["operation_id"].as_str() != Some(id) {
                return Err("Refinement identity mismatch".into());
            }
            let text = result["text"].as_str().ok_or("Missing refinement result")?;
            Ok(json!({"ok":true,"input":{"type":"rewritten","operation_id":id,"text":text}}))
        }
        Some("retry_result") => {
            let id = value.get("id").and_then(Value::as_str).ok_or("Missing recording ID")?;
            let retried = (|| -> Result<String, String> {
                service.request(json!({"op":"retry","id":id}))?;
                let record: polar_dictate::Recording = serde_json::from_value(service.request(json!({"op":"wait","id":id,"stage":"transcription"}))?).map_err(|_| "Invalid saved recording")?;
                record.original().ok_or_else(|| record.error.unwrap_or_else(|| "No usable transcription".into()))
            })();
            Ok(match retried {
                Ok(text) => json!({"ok":true,"input":{"type":"transcribed","operation_id":id,"text":text}}),
                Err(error) => json!({"ok":true,"input":{"type":"failed","operation_id":id,"error":error}}),
            })
        }
        Some("rewrite_result") => {
            let id = value
                .get("id")
                .and_then(Value::as_str)
                .ok_or("Missing recording ID")?;
            let result = service.request(json!({"op":"get","id":id}))?;
            let record: polar_dictate::Recording =
                serde_json::from_value(result["recording"].clone())
                    .map_err(|_| "Invalid saved recording")?;
            if record.cleanup_deferred {
                let context = value.get("context").and_then(Value::as_str).unwrap_or("");
                service.request(json!({"op":"bind_context","id":id,"context":context}))?;
            }
            let record: polar_dictate::Recording =
                serde_json::from_value(service.request(json!({"op":"wait","id":id}))?)
                    .map_err(|_| "Invalid saved recording")?;
            Ok(
                json!({"ok":true,"input":{"type":"rewritten","operation_id":id,"text":record.cleaned.clone().or_else(||record.original()),"error":record.cleanup_error}}),
            )
        }
        _ => Ok(json!({"ok":true,"payload":service.request(value)?})),
    }
}



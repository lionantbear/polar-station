//! Dictate application use cases shared by all native entrypoints.
//! The existing Engine owns state; ports supply existing Host and snapshot IO.
use super::{Effect, Engine, Input};
use serde_json::{json, Value};
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
    #[allow(clippy::too_many_arguments)] // Preserve the existing capture port while adding trusted project context.
    fn begin(
        &self,
        id: &str,
        context: Option<String>,
        reference: String,
        project: Option<String>,
        cleanup: bool,
        is_test: bool,
    ) -> Result<(), String>;
}

fn recording_missing(error: &str) -> bool {
    error.rsplit(": ").next() == Some("dictate.recording_missing")
}


pub struct Application<P> {
    history: super::history::History,
    ports: P,
    delivery_tokens: std::collections::HashMap<String, String>,
    engine: Engine,
    started: bool,
    path: Option<std::path::PathBuf>,
    storage_error: Option<String>,
    open_request: Option<Value>,
    open_error: Option<String>,
    copy_completed: Option<String>,
    output_mode: polar_dictate::DictateOutputMode,
    reference_policy: polar_dictate::ReferencePolicy,
    cleanup_preview: Option<Value>,
    legacy_pending: bool,
    migration_error: Option<String>,
    cleanup_plan: Option<polar_dictate::CleanupPlan>,
}
impl<P: Ports> Application<P> {
    pub fn new(ports: P) -> Self {
        Self {
            history: Default::default(),
            ports,
            delivery_tokens: Default::default(),
            engine: Engine::default(),
            started: false,
            path: None,
            storage_error: None,
            open_request: None,
            open_error: None,
            copy_completed: None,
            output_mode: Default::default(),
            reference_policy: Default::default(),
            cleanup_preview: None,
            legacy_pending: false,
            migration_error: None,
            cleanup_plan: None,
        }
    }
    fn refresh_reference_policy(&mut self) {
        self.reference_policy = self.ports.host()
            .and_then(|host| host.request(json!({"op":"reference_policy"})).ok())
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default();
    }
    pub fn dispatch(&mut self, mut value: Value) -> Result<Value, String> {
        if value["type"] == "retry_open" {
            value = self
                .open_request
                .clone()
                .ok_or("No failed Dictate open to retry")?;
        }
        let opening = value["type"] == "open" && !self.started;
        if opening {
            self.open_request = Some(value.clone());
        } else if let Some(error) = &self.open_error {
            if !matches!(
                value["type"].as_str(),
                Some("shortcut_binding" | "shortcut_status" | "shortcut_mode" | "snapshot")
            ) {
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
                if opening {
                    self.open_error = Some(error.clone());
                }
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
    fn observe_saved(
        &mut self,
        mut record: polar_dictate::Recording,
    ) -> Result<Vec<Effect>, String> {
        let active = self.engine.entries().iter().find(|e| {
            e.operation_id == record.id
                && e.host_recording
                && matches!(
                    e.phase,
                    super::Phase::Transcribing | super::Phase::Rewriting
                )
        });
        if record.cleanup_deferred && record.original().is_some() {
            if let (Some(entry), Some(host)) = (active, self.ports.host()) {
                let context = entry
                    .target
                    .as_ref()
                    .map(|t| t.context.as_str())
                    .unwrap_or("");
                // An older deferred event may race the already-bound one.
                let response = host
                    .request(json!({"op":"bind_context","id":record.id,"context":context}))
                    .or_else(|_| host.request(json!({"op":"get","id":record.id})))?;
                record = serde_json::from_value(response["recording"].clone())
                    .map_err(|_| "Invalid recording projection")?;
            }
        }
        let effects = self.engine.observe_recording(&record);
        self.history.update(record);
        Ok(effects)
    }
    fn current_event_item(
        &self,
        item: polar_dictate::HistoryItem,
    ) -> Result<polar_dictate::HistoryItem, String> {
        // Existing summaries carry the record's canonical revision. Only an
        // unknown/migrated reference needs a read; normal PCM events stay push-only.
        if self.engine.host_events
            && self
                .history
                .records
                .iter()
                .find(|r| r.id == item.id())
                .is_none_or(|r| r.revision == 0)
        {
            let current = self.saved_item(item.id())?;
            if current.revision() != item.revision() {
                return Ok(current);
            }
        }
        Ok(item)
    }
    fn saved_item(&self, id: &str) -> Result<polar_dictate::HistoryItem, String> {
        let response = self
            .ports
            .host()
            .ok_or("Voice Host unavailable")?
            .request(json!({"op":"item","id":id}))?;
        let item: polar_dictate::HistoryItem =
            serde_json::from_value(response["item"].clone()).map_err(|_| "Invalid saved item")?;
        if item.id() != id {
            return Err("Saved item identity mismatch".into());
        }
        Ok(item)
    }
    fn change_saved_item(
        &self,
        id: &str,
        change: polar_dictate::ItemChange,
    ) -> Result<polar_dictate::HistoryItem, String> {
        let response = self
            .ports
            .host()
            .ok_or("Voice Host unavailable")?
            .request(json!({"op":"item_change","id":id,"change":change}))?;
        let item: polar_dictate::HistoryItem =
            serde_json::from_value(response["item"].clone()).map_err(|_| "Invalid saved item")?;
        if item.id() != id {
            return Err("Saved item identity mismatch".into());
        }
        Ok(item)
    }
    fn dispatch_inner(&mut self, mut value: Value) -> Result<Value, String> {
        self.copy_completed = None;
        self.cleanup_preview = None;
        let mut processing_update = false;
        let kind = value.get("type").and_then(Value::as_str).map(str::to_owned);
        let operation = value
            .get("operation_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if kind.as_deref() == Some("open")
            && value.get("host_managed").and_then(Value::as_bool) == Some(true)
        {
            self.ports.connect_host()?;
        }
        // Capture startup is asynchronous; the first saved-audio event also
        // supplies a chance to read its immutable mode. Stop after confirmation.
        if matches!(
            kind.as_deref(),
            Some("capture_ready" | "audio_saved" | "transcript_preview")
        ) && self.engine.entries().iter().any(|entry| {
            Some(&entry.operation_id) == operation.as_ref() && entry.transcription_mode.is_none()
        }) {
            let mode = self
                .ports
                .host()
                .zip(operation.as_deref())
                .and_then(|(host, id)| {
                    let payload = host.request(json!({"op":"get","id":id})).ok()?;
                    if payload["recording"]["id"].as_str() != Some(id) {
                        return None;
                    }
                    let record: polar_dictate::Recording =
                        serde_json::from_value(payload["recording"].clone()).ok()?;
                    Some((id, record.transcription_mode, record.output_mode()))
                });
            if let Some((id, mode, output)) = mode {
                self.engine.set_recording_mode(id, mode);
                if let Some(e) = self
                    .engine
                    .entries
                    .iter_mut()
                    .find(|e| e.operation_id == id)
                {
                    e.output_mode = Some(output);
                }
            }
        }
        if kind.as_deref() == Some("use_available") {
            if let (Some(host), Some(id)) = (self.ports.host(), operation.as_deref()) {
                if self.engine.entries().iter().any(|e| e.operation_id == id
                    && matches!(e.phase, super::Phase::Transcribing | super::Phase::Rewriting)) {
                    let response = host.request(json!({"op":"use_available","id":id}))?;
                    let item: polar_dictate::HistoryItem = serde_json::from_value(response["item"].clone())
                        .map_err(|_| "Invalid available-text response")?;
                    if item.id() != id { return Err("Available-text identity mismatch".into()); }
                    if let Some(entry) = self.engine.entries.iter_mut().find(|e| e.operation_id == id) {
                        entry.text = item.text().unwrap_or_default();
                        entry.raw = item.original().unwrap_or_default();
                    }
                }
            }
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
        if kind.as_deref() == Some("clipboard") && value["copied"] == true {
            if let Some(id) = operation.as_deref() {
                let saved = self.engine.entries().iter().any(|e| {
                    e.operation_id == id
                        && (e.host_recording || self.engine.host_events)
                        && e.phase == super::Phase::Copying
                });
                if saved {
                    if self.engine.host_events {
                        if let Some(text) = self
                            .engine
                            .entries()
                            .iter()
                            .find(|e| e.operation_id == id && e.unsaved)
                            .map(|e| e.text.clone())
                        {
                            if self
                                .change_saved_item(id, polar_dictate::ItemChange::Edit { text })
                                .is_err()
                            {
                                value["copied"] = json!(false);
                                if let Some(entry) = self
                                    .engine
                                    .entries
                                    .iter_mut()
                                    .find(|e| e.operation_id == id)
                                {
                                    entry.copy_error=Some("Copied to clipboard; edited text is retained here but could not be saved.".into());
                                }
                            } else if let Some(entry) = self
                                .engine
                                .entries
                                .iter_mut()
                                .find(|e| e.operation_id == id)
                            {
                                entry.unsaved = false;
                            }
                        }
                    }
                    let result = if value["copied"] == false {
                        Err("Copy receipt save failed".into())
                    } else if self.engine.host_events {
                        self.change_saved_item(
                            id,
                            polar_dictate::ItemChange::Copied {
                                now_ms: self.ports.now_ms(),
                                text: self
                                    .engine
                                    .entries()
                                    .iter()
                                    .find(|e| e.operation_id == id)
                                    .map(|e| e.delivery_text()),
                            },
                        )
                        .map(|_| Value::Null)
                    } else {
                        self.ports.host().ok_or_else(||"Voice Host unavailable".to_owned()).and_then(|host|host.request(json!({"op":"mark_copied","id":id,"now_ms":self.ports.now_ms()})).map_err(|_|"Copy receipt save failed".into()))
                    };
                    if result.is_err() {
                        self.engine.dispatch(Input::Clipboard {
                            operation_id: id.into(),
                            copied: false,
                        })?;
                        if let Some(entry) = self
                            .engine
                            .entries
                            .iter_mut()
                            .find(|e| e.operation_id == id)
                        {
                            entry.copy_error=Some("Copied to clipboard; the recording receipt could not be saved. Retry Copy.".into());
                        }
                        value["copied"] = json!(false);
                    }
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
                    let result = if self.engine.host_events {
                        host.request(json!({"op":"item_change","id":id,"change":{"type":"complete","token":token,"applied":applied,"now_ms":self.ports.now_ms()}}))
                    } else {
                        host.request(json!({"op":"complete_delivery","id":id,"token":token,"applied":applied,"now_ms":self.ports.now_ms()}))
                    };
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
        if kind.as_deref() == Some("clipboard")
            && value["copied"] == true
            && self.engine.entries().iter().any(|e| {
                Some(&e.operation_id) == operation.as_ref() && e.phase == super::Phase::Copying
            })
        {
            self.copy_completed = operation.clone();
        }
        if self.engine.host_events
            && matches!(
                kind.as_deref(),
                Some("start" | "start_context" | "refine_text" | "refine_context")
            )
        {
            self.engine
                .entries
                .retain(|e| e.phase.active() || e.unsaved);
        }
        let mut effects = match kind.as_deref() {
            Some("open") => {
                if self.started {
                    return Err("Open is only allowed before Dictate starts".into());
                }
                let original_path = std::path::PathBuf::from(
                    value["path"]
                        .as_str()
                        .ok_or("Missing Dictate history path")?,
                );
                if !original_path.is_absolute() || original_path.file_name().is_none() {
                    return Err("Invalid Dictate history path".into());
                }
                let managed = value["host_managed"] == true;
                let path = if managed {
                    original_path.with_file_name("dictate-v2.json")
                } else {
                    original_path.clone()
                };
                let document = self
                    .ports
                    .exchange(&path, None, MAX_INPUT)
                    .map_err(|_| "Cannot read Dictate state; existing data retained")?;
                let saved: Option<super::Snapshot> = document
                    .as_deref()
                    .map(|s| {
                        serde_json::from_str(s)
                            .map_err(|_| "Invalid Dictate state; existing data retained")
                    })
                    .transpose()?;
                let current = saved.as_ref().and_then(|s| s.current_ref.clone());
                let import = managed && saved.as_ref().is_none_or(|s| s.legacy_pending);
                let mut engine = match saved {
                    Some(s) => Engine::restore(s)?,
                    None => Engine::default(),
                };
                if import {
                    let result = (|| -> Result<(), String> {
                        let Some(legacy) = self.ports.exchange(&original_path, None, MAX_INPUT)?
                        else {
                            return Ok(());
                        };
                        let snapshot: super::Snapshot =
                            serde_json::from_str(&legacy).map_err(|_| {
                                "Old Dictate drafts need recovery; original file retained"
                            })?;
                        let host = self.ports.host().ok_or("Voice Host unavailable")?;
                        for entry in snapshot.entries {
                            let entry: polar_dictate::LegacyInput = serde_json::from_value(
                                serde_json::to_value(entry).map_err(|e| e.to_string())?,
                            )
                            .map_err(|e| e.to_string())?;
                            let response =
                                host.request(json!({"op":"import_legacy","entries":[entry]}))?;
                            if response["report"]["conflicts"]
                                .as_array()
                                .is_some_and(|v| !v.is_empty())
                            {
                                return Err("Some old drafts conflict with saved edits; both versions retained for recovery".into());
                            }
                        }
                        Ok(())
                    })();
                    self.legacy_pending = result.is_err();
                    self.migration_error = result.err();
                }
                if managed {
                    engine.use_host_events();
                    for entry in &mut engine.entries {
                        if !entry.unsaved {
                            continue;
                        }
                        let imported = (|| -> Result<(), String> {
                            let legacy: polar_dictate::LegacyInput = serde_json::from_value(
                                serde_json::to_value(&*entry).map_err(|e| e.to_string())?,
                            )
                            .map_err(|e| e.to_string())?;
                            let response = self
                                .ports
                                .host()
                                .ok_or("Voice Host unavailable")?
                                .request(json!({"op":"import_legacy","entries":[legacy]}))?;
                            if response["report"]["conflicts"]
                                .as_array()
                                .is_some_and(|v| !v.is_empty())
                            {
                                return Err("Edited text conflicts with a newer saved draft; both versions retained".into());
                            }
                            Ok(())
                        })();
                        if imported.is_ok() {
                            entry.unsaved = false;
                        } else {
                            self.migration_error = imported.err();
                        }
                    }
                    if !self.legacy_pending && self.migration_error.is_none() {
                        if let Some(host) = self.ports.host() {
                            if let Err(error) = host.request(json!({"op":"finish_legacy_import"})) {
                                self.migration_error = Some(error);
                            }
                        }
                    }
                    if let Some(id) = current {
                        if let Ok(item) = self.saved_item(&id) {
                            if !engine
                                .entries
                                .iter()
                                .any(|e| e.operation_id == id && e.unsaved)
                            {
                                let _ = engine.stage_item(&item);
                            }
                            // Restore is a reader; it never arms deferred insertion.
                            for entry in &mut engine.entries {
                                if matches!(
                                    entry.phase,
                                    super::Phase::Pending | super::Phase::Rewriting
                                ) {
                                    entry.phase = super::Phase::Review;
                                }
                            }
                            self.history.update_item(item);
                        }
                    }
                    if let Some(host) = self.ports.host() {
                        if let Ok(v) = host.request(json!({"op":"voice_settings"})) {
                            if let Ok(settings) =
                                serde_json::from_value::<polar_dictate::VoiceSettings>(v)
                            {
                                self.output_mode = settings.mode();
                                self.refresh_reference_policy();
                            }
                        }
                    }
                }
                engine.shortcut_status = self.engine.shortcut_status;
                engine.shortcut_trigger = self.engine.shortcut_trigger.clone();
                if value["host_managed"] == true {
                    engine.use_host_events();
                }
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
            Some("edit_draft") if self.engine.host_events => {
                let id = operation.as_deref().ok_or("Missing draft operation")?;
                let text = value["text"]
                    .as_str()
                    .ok_or("Missing draft text")?
                    .to_owned();
                self.engine.dispatch(Input::EditDraft {
                    operation_id: id.into(),
                    text: text.clone(),
                })?;
                match self.change_saved_item(id, polar_dictate::ItemChange::Edit { text }) {
                    Ok(item) => {
                        self.history.update_item(item);
                        if let Some(e) = self
                            .engine
                            .entries
                            .iter_mut()
                            .find(|e| e.operation_id == id)
                        {
                            e.unsaved = false;
                        }
                        self.storage_error = None;
                    }
                    Err(_) => {
                        if let Some(e) = self
                            .engine
                            .entries
                            .iter_mut()
                            .find(|e| e.operation_id == id)
                        {
                            e.unsaved = true;
                        }
                        self.storage_error=Some("Edited text is retained here but could not be saved. Copy it or retry before closing.".into());
                    }
                }
                vec![]
            }
            Some("refresh_settings") => {
                let value = self
                    .ports
                    .host()
                    .ok_or("Voice Host unavailable")?
                    .request(json!({"op":"voice_settings"}))?;
                let settings: polar_dictate::VoiceSettings =
                    serde_json::from_value(value).map_err(|_| "Invalid Voice settings")?;
                self.output_mode = settings.mode();
                self.refresh_reference_policy();
                vec![]
            }
            Some("set_output_mode") => {
                let mode: polar_dictate::DictateOutputMode =
                    serde_json::from_value(value["mode"].clone())
                        .map_err(|_| "Invalid output mode")?;
                self.ports
                    .host()
                    .ok_or("Voice Host unavailable")?
                    .request(json!({"op":"voice_output_mode_update","mode":mode}))?;
                self.output_mode = mode;
                self.refresh_reference_policy();
                vec![]
            }
            Some("cleanup_preview" | "cleanup_apply") => {
                let apply = kind.as_deref() == Some("cleanup_apply");
                let response=self.ports.host().ok_or("Voice Host unavailable")?.request(if apply {json!({"op":"cleanup_apply","confirmation":value["confirmation"],"plan":self.cleanup_plan})}else{json!({"op":"cleanup_preview"})})?;
                if !apply {
                    self.cleanup_plan = Some(
                        serde_json::from_value(response["plan"].clone())
                            .map_err(|_| "Invalid cleanup plan")?,
                    );
                }
                self.cleanup_preview = Some(response);
                vec![]
            }
            Some("host_disconnected") => {
                self.history.error = Some(
                    "Voice Host disconnected; saved results retained. Start again to reconnect."
                        .into(),
                );
                let mut effects = Vec::new();
                for entry in &mut self.engine.entries {
                    if !matches!(
                        entry.phase,
                        super::Phase::Starting
                            | super::Phase::Recording
                            | super::Phase::Transcribing
                            | super::Phase::Rewriting
                    ) {
                        continue;
                    }
                    if matches!(
                        entry.phase,
                        super::Phase::Starting | super::Phase::Recording
                    ) {
                        effects.push(Effect::Cancel {
                            operation_id: entry.operation_id.clone(),
                        });
                    }
                    entry.phase = super::Phase::Failed;
                    entry.error = self.history.error.clone();
                    entry.target = None;
                }
                effects
            }
            Some("snapshot") => vec![],
            Some("history_open" | "refresh_history" | "history_more") => {
                let more = kind.as_deref() == Some("history_more");
                let response=self.ports.host().ok_or("Voice Host unavailable")?.request(json!({"op":"history","cursor":if more {self.history.next.clone()}else{None},"limit":100}));
                match response.and_then(|v| {
                    serde_json::from_value::<polar_dictate::HistoryPage>(v["page"].clone())
                        .map_err(|_| "Invalid history page".into())
                }) {
                    Ok(page) => {
                        if !more {
                            self.history.records.clear();
                            self.history.loaded = 0;
                        }
                        self.history.loaded += page.items.len() as u32;
                        self.history.next = page.next;
                        self.history.more = self.history.next.is_some();
                        for item in page.items {
                            self.history.records.retain(|r| r.id != item.id);
                            self.history.records.push(item);
                        }
                        self.history.error = None;
                        if kind.as_deref() == Some("history_open") {
                            self.history.selected = self
                                .history
                                .rows(&self.engine)
                                .first()
                                .and_then(|r| r["operation_id"].as_str())
                                .map(str::to_owned);
                        }
                        if let Some(id) = self.history.selected.clone() {
                            match self.saved_item(&id) {
                                Ok(item) => self.history.update_item(item),
                                Err(e) if recording_missing(&e) => {
                                    self.history.remove(&id);
                                    self.engine.entries.retain(|e| e.operation_id != id);
                                }
                                Err(e) => self.history.error = Some(e),
                            }
                        }
                    }
                    Err(error) => self.history.error = Some(error),
                }
                if let Some(removed) = value["removed"].as_array() {
                    for id in removed.iter().filter_map(Value::as_str) {
                        self.history.remove(id);
                        self.engine.entries.retain(|e| e.operation_id != id);
                    }
                }
                let mut effects = Vec::new();
                if self.engine.host_events && !more {
                    let ids: Vec<_> = self
                        .engine
                        .entries()
                        .iter()
                        .filter(|e| {
                            e.host_recording
                                && matches!(
                                    e.phase,
                                    super::Phase::Transcribing | super::Phase::Rewriting
                                )
                        })
                        .map(|e| e.operation_id.clone())
                        .collect();
                    for id in ids {
                        if let polar_dictate::HistoryItem::Recording(record) =
                            self.saved_item(&id)?
                        {
                            effects.extend(self.observe_saved(*record)?);
                            processing_update = true;
                        }
                    }
                }
                effects
            }
            Some("history_event" | "history_item_event") => {
                let item = if kind.as_deref() == Some("history_event") {
                    polar_dictate::HistoryItem::Recording(Box::new(
                        serde_json::from_value(value["recording"].clone())
                            .map_err(|_| "Invalid history event")?,
                    ))
                } else {
                    serde_json::from_value(value["item"].clone())
                        .map_err(|_| "Invalid saved item event")?
                };
                let id = item.id().to_owned();
                let before = self
                    .engine
                    .entries()
                    .iter()
                    .find(|e| e.operation_id == id)
                    .map(|e| e.phase);
                let effects = match self.current_event_item(item) {
                    Ok(item) if !self.history.accepts(&item) => vec![],
                    Ok(polar_dictate::HistoryItem::Recording(record)) => {
                        self.observe_saved(*record)?
                    }
                    Ok(item) => {
                        self.history.update_item(item);
                        vec![]
                    }
                    Err(error) if recording_missing(&error) => {
                        self.history.remove(&id);
                        self.engine.discard_saved(&id)
                    }
                    Err(error) => return Err(error),
                };
                processing_update = !effects.is_empty()
                    || before
                        != self
                            .engine
                            .entries()
                            .iter()
                            .find(|e| e.operation_id == id)
                            .map(|e| e.phase);
                effects
            }
            Some("history_copy") => {
                let id = operation.as_deref().ok_or("Missing history operation")?;
                let item = self.saved_item(id)?;
                if let polar_dictate::HistoryItem::Recording(record) = &item {
                    self.engine.copy_saved(record, value["original"] == true)?
                } else {
                    let body = if value["original"] == true {
                        item.original()
                    } else {
                        item.text()
                    }
                    .ok_or("No saved text")?;
                    self.engine.stage_item(&item)?;
                    self.engine.dispatch(Input::CopyDraft {
                        operation_id: id.into(),
                        text: body,
                    })?
                }
            }
            Some("history_dismiss") => {
                let id = operation.as_deref().ok_or("Missing history operation")?;
                let item = self.change_saved_item(id, polar_dictate::ItemChange::Dismiss)?;
                self.history.update_item(item);
                self.engine.entries.retain(|e| e.operation_id != id);
                vec![]
            }
            Some("history_delete") => {
                let id = operation.as_deref().ok_or("Missing history operation")?;
                if value["confirmation"] != true {
                    return Err("Confirm history deletion".into());
                }
                self.ports
                    .host()
                    .ok_or("Voice Host unavailable")?
                    .request(json!({"op":"delete","id":id,"confirmation":id}))?;
                self.history.remove(id);
                self.engine.entries.retain(|e| e.operation_id != id);
                vec![]
            }
            Some("history_pin") => {
                let id = operation.as_deref().ok_or("Missing history operation")?;
                let item = self.change_saved_item(
                    id,
                    polar_dictate::ItemChange::Pin {
                        pinned: value["pinned"].as_bool().ok_or("Missing pin state")?,
                    },
                )?;
                self.history.update_item(item);
                vec![]
            }
            Some("history_select" | "history_insert" | "history_retry") => {
                let id = operation.as_deref().ok_or("Missing history operation")?;
                self.history.selected = Some(id.into());
                self.history.detail = None;
                self.history.error = None;
                match self.saved_item(id) {
                    Ok(mut item) => {
                        let inserting = kind.as_deref() == Some("history_insert");
                        let retry = kind.as_deref() == Some("history_retry");
                        if (inserting || retry)
                            && item.delivery() == polar_dictate::DeliveryState::Copied
                        {
                            item = self.change_saved_item(id, polar_dictate::ItemChange::Reopen)?;
                        }
                        self.history.update_item(item.clone());
                        if inserting {
                            self.engine.stage_item(&item)?;
                            self.engine.dispatch(Input::InsertDraft {
                                operation_id: id.into(),
                            })?
                        } else if retry {
                            let polar_dictate::HistoryItem::Recording(record) = item else {
                                return Err("Text refinement has no audio to retranscribe".into());
                            };
                            self.engine.stage_retry(&record)?;
                            self.engine.dispatch(Input::RetrySaved {
                                operation_id: id.into(),
                            })?
                        } else {
                            vec![]
                        }
                    }
                    Err(error) => {
                        self.history.error = Some(error);
                        vec![]
                    }
                }
            }
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
                    Err(error) if recording_missing(&error) => {
                        self.history.records.retain(|r| r.id != id);
                        if self.history.selected.as_deref() == Some(id) {
                            self.history.selected = None;
                            self.history.detail = None;
                        }
                        self.engine.discard_saved(id)
                    }
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
                if !matches!(
                    kind.as_deref(),
                    Some("shortcut_binding" | "shortcut_status" | "shortcut_mode")
                ) {
                    self.started = true;
                }
                effects
            }
        };
        if matches!(
            kind.as_deref(),
            Some("start" | "start_context" | "refine_text" | "refine_context")
        ) {
            if let Some(entry) = self
                .engine
                .entries
                .iter_mut()
                .find(|e| Some(&e.operation_id) == operation.as_ref())
            {
                entry.created_at_ms = Some(self.ports.now_ms());
            }
        }
        if let Some(host) = self.ports.host() {
            let mut processing_effects = Vec::new();
            for effect in effects {
                if self.engine.host_events {
                    if let Effect::Rewrite {
                        saved_recording: false,
                        operation_id,
                        raw,
                        english,
                        ..
                    } = &effect
                    {
                        let mode = if *english {
                            polar_transcreation::RefineMode::English
                        } else {
                            polar_transcreation::RefineMode::Cleanup
                        };
                        if let Err(error) = host.request(
                            json!({"op":"text_begin","id":operation_id,"text":raw,"mode":mode}),
                        ) {
                            self.engine.dispatch(Input::Failed {
                                operation_id: operation_id.clone(),
                                error,
                            })?;
                            continue;
                        }
                    }
                }
                let saved_edit = if self.engine.host_events {
                    match &effect {
                        Effect::Rewrite {
                            saved_recording: true,
                            operation_id,
                            context,
                            ..
                        } => Some((operation_id, Some(context.as_str()))),
                        Effect::RetryTranscribe { operation_id } => Some((operation_id, None)),
                        _ => None,
                    }
                } else {
                    None
                };
                if let Some((id, context)) = saved_edit {
                    if context.is_none() {
                        host.request(json!({"op":"retry","id":id}))?;
                    }
                    let response = host.request(json!({"op":"get","id":id}))?;
                    let mut record: polar_dictate::Recording =
                        serde_json::from_value(response["recording"].clone())
                            .map_err(|_| "Invalid recording projection")?;
                    if record.cleanup_deferred && record.original().is_some() {
                        let response = host.request(
                            json!({"op":"bind_context","id":id,"context":context.unwrap_or("")}),
                        )?;
                        record = serde_json::from_value(response["recording"].clone())
                            .map_err(|_| "Invalid recording projection")?;
                    }
                    processing_effects.extend(self.engine.observe_recording(&record));
                } else {
                    processing_effects.push(effect);
                }
            }
            let mut admitted = Vec::new();
            for effect in processing_effects {
                if let super::Effect::Insert {
                    operation_id, text, ..
                }
                | super::Effect::InsertCurrent {
                    operation_id, text, ..
                } = &effect
                {
                    if self.engine.entries().iter().any(|e| {
                        e.operation_id == *operation_id
                            && !e.host_recording
                            && !self.engine.host_events
                    }) {
                        admitted.push(effect);
                        continue;
                    }
                    let prepare = (|| -> Result<String, String> {
                        let edited = self
                            .engine
                            .entries()
                            .iter()
                            .any(|entry| entry.operation_id == *operation_id && entry.edited);
                        let token = polar_identity::generation::new_uuid_v4().to_string();
                        if self.engine.host_events {
                            host.request(json!({"op":"item_change","id":operation_id,"change":{"type":"prepare","token":token,"text":text,"edited":edited}}))?;
                        } else {
                            host.request(json!({"op":"prepare_text_delivery","id":operation_id,"token":token,"text":text,"edited":edited}))?;
                        }
                        Ok(token)
                    })();
                    match prepare {
                        Ok(token) => {
                            if let Some(entry) = self
                                .engine
                                .entries
                                .iter_mut()
                                .find(|e| e.operation_id == *operation_id)
                            {
                                entry.unsaved = false;
                            }
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
        if processing_update
            || !matches!(
                kind.as_deref(),
                Some(
                    "snapshot"
                        | "audio_saved"
                        | "transcript_preview"
                        | "history_event"
                        | "history_open"
                        | "refresh_history"
                        | "history_more"
                        | "history_select"
                )
            )
        {
            if let Some(path) = &self.path {
                let mut snapshot = self.engine.persistent_snapshot();
                snapshot.legacy_pending = self.legacy_pending;
                let document = serde_json::to_string(&snapshot).map_err(|e| e.to_string())?;
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
    fn snapshot(&self) -> super::Snapshot {
        let mut snapshot = self.engine.persistent_snapshot();
        snapshot.legacy_pending = self.legacy_pending;
        snapshot
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
        let microphone_state = if presentation.action == "stop" {
            "recording"
        } else if presentation.action == "retry_open" || presentation.setup_required {
            "unavailable"
        } else if presentation.action == "start" {
            "ready"
        } else {
            "busy"
        };
        let history_error = self
            .open_error
            .clone()
            .or_else(|| self.storage_error.clone())
            .or_else(|| self.migration_error.clone())
            .or_else(|| self.history.error.clone())
            .or_else(|| {
                self.engine
                    .entries()
                    .last()
                    .filter(|e| {
                        e.phase == super::Phase::Failed
                            && e.saved_samples == 0
                            && e.text.trim().is_empty()
                            && e.raw.trim().is_empty()
                    })
                    .map(|e| e.status().into_owned())
            });
        json!({"ok":true,"initialized":self.started,"entries":self.engine.entries(),"presentation":presentation,"history":self.engine.history(),"microphone_state":microphone_state,
            "history_rows":self.history.rows(&self.engine),"history_detail":self.history.detail(&self.engine),
            "history_error":history_error,"history_retry_open":self.open_error.is_some(),"history_more":self.history.more,
            "delete_ui":{"confirmation":"Delete this history record?","summary":"This permanently deletes this record's saved text and audio.","detail":"Text already inserted into an input is unchanged.","can_apply":true,"confirm_label":"Delete record","cancel_label":"Cancel"},
            "reference_policy":self.reference_policy,"output_mode":self.output_mode,"output_mode_label":self.output_mode.label(),"output_modes":polar_dictate::DictateOutputMode::choices(),"cleanup_preview":self.cleanup_preview,
            "copy_completed":self.copy_completed,
            "refine_modes":self.engine.refine_modes,
            "refine_ui":{"cleanup":"Refine","english":"Refine + English","hint":"Hold to choose Refine mode"},
            "history_ui":{"retry_open":"Retry Dictate initialization","title":"Dictate history","detail":"Dictate history detail","empty":"No saved dictation yet","original":"Original","refined":"Result","copy":"Copy","insert":"Insert in current input","retry":"Retry transcription","dismiss":"Dismiss draft","settings":"Voice Settings…","more":"Load older","earlier":"Earlier","text":"Text refinement","cleanup":"Clean up by rules…","delete":"Delete record…","pin":"Keep this record","unpin":"Allow automatic cleanup"},
            "snapshot":self.snapshot(),"panel":self.engine.panel(),"effects":effects})
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
                    .get("reference")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                value
                    .get("project")
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
                    "project":value.get("project").and_then(Value::as_str),"context":value.get("context").and_then(Value::as_str),"scope":value.get("scope").and_then(Value::as_str),"mode":if value["english"].as_bool()==Some(true){"english"}else{"cleanup"}}))?;
            if result["operation_id"].as_str() != Some(id) {
                return Err("Refinement identity mismatch".into());
            }
            let text = result["text"].as_str().ok_or("Missing refinement result")?;
            Ok(json!({"ok":true,"input":{"type":"rewritten","operation_id":id,"text":text}}))
        }
        Some("retry_result") => {
            let id = value
                .get("id")
                .and_then(Value::as_str)
                .ok_or("Missing recording ID")?;
            let retried = (|| -> Result<String, String> {
                service.request(json!({"op":"retry","id":id}))?;
                let record: polar_dictate::Recording = serde_json::from_value(
                    service.request(json!({"op":"wait","id":id,"stage":"transcription"}))?,
                )
                .map_err(|_| "Invalid saved recording")?;
                record.original().ok_or_else(|| {
                    record
                        .error
                        .unwrap_or_else(|| "No usable transcription".into())
                })
            })();
            Ok(match retried {
                Ok(text) => {
                    json!({"ok":true,"input":{"type":"transcribed","operation_id":id,"text":text}})
                }
                Err(error) => {
                    json!({"ok":true,"input":{"type":"failed","operation_id":id,"error":error}})
                }
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



//! Host summaries plus one selected detail. History never controls admission.
use super::Engine;
use polar_dictate::{DeliveryState, HistoryItem, HistorySummary, Recording, SAMPLE_RATE};
use serde_json::{json, Value};
pub(super) fn labels() -> Value {
    json!({"secondary":"Record actions","inputs":"Inputs","retry_open":"Retry Dictate initialization","title":"Dictate history","detail":"Dictate history detail","empty":"No saved dictation yet","original":"Original","refined":"Result","copy":"Copy","insert":"Insert in current input","retry":"Retry transcription","dismiss":"Dismiss draft","settings":"Voice Settings…","more":"Load older","earlier":"Earlier","text":"Text refinement","cleanup":"Clean up by rules…","delete":"Delete record…","pin":"Keep this record","unpin":"Allow automatic cleanup"})
}

#[derive(Default)]
pub(super) struct History {
    pub records: Vec<HistorySummary>,
    pub selected: Option<String>,
    pub detail: Option<HistoryItem>,
    pub error: Option<String>,
    pub loaded: u32,
    pub more: bool,
    pub next: Option<polar_dictate::HistoryCursor>,
}
impl History {
    /// Resolve against the same filtered order rendered by both native readers.
    /// Navigation changes selection only; the application owns loading the item.
    pub fn adjacent(&self, engine: &Engine, down: bool) -> Option<String> {
        let rows = self.rows(engine);
        let current = rows.iter().position(|row| row["operation_id"].as_str() == self.selected.as_deref());
        let index = match current {
            Some(index) if down => (index + 1).min(rows.len().saturating_sub(1)),
            Some(index) => index.saturating_sub(1),
            None => 0,
        };
        rows.get(index)?["operation_id"].as_str().map(str::to_owned)
    }
    pub fn update(&mut self, r: Recording) {
        self.update_item(HistoryItem::Recording(Box::new(r)));
    }
    pub fn accepts(&self, item: &HistoryItem) -> bool {
        self.records
            .iter()
            .find(|r| r.id == item.id())
            .is_none_or(|r| r.revision <= item.revision())
    }
    pub fn update_item(&mut self, item: HistoryItem) {
        if !self.accepts(&item) {
            return;
        }
        let summary = item.summary();
        self.records.retain(|r| r.id != summary.id);
        self.records.push(summary);
        self.records.sort_by(|a, b| {
            b.created_at_ms
                .cmp(&a.created_at_ms)
                .then_with(|| b.id.cmp(&a.id))
        });
        self.records.truncate(self.loaded.max(100) as usize);
        if self.selected.as_deref() == Some(item.id()) {
            self.detail = Some(item);
        }
    }
    pub fn remove(&mut self, id: &str) {
        self.records.retain(|r| r.id != id);
        if self.selected.as_deref() == Some(id) {
            self.selected = None;
            self.detail = None;
        }
    }
    pub fn rows(&self, engine: &Engine) -> Vec<Value> {
        let mut rows = Vec::new();
        for r in &self.records {
            if !r.visible() {
                continue;
            }
            let status: String = if r.active {
                "Processing…".into()
            } else if r.delivery == DeliveryState::Copied {
                "Copied".into()
            } else if r.delivery == DeliveryState::Delivered {
                "Inserted".into()
            } else if r.delivery == DeliveryState::Unknown {
                "Check the input before inserting again".into()
            } else if r.error.is_some() {
                if r.has_text {
                    "Original retained"
                } else {
                    "Transcription failed"
                }
                .into()
            } else if !r.has_text {
                "Recoverable recording".into()
            } else {
                "Saved".into()
            };
            rows.push(json!({"operation_id":r.id,"preview":if r.preview.trim().is_empty(){"Recoverable recording"}else{&r.preview},
                "created_at_ms":r.created_at_ms,"duration_ms":r.saved_samples.saturating_mul(1000)/SAMPLE_RATE,"status":status,"kind":r.kind,"pinned":r.pinned,"output_mode":r.output_mode,"mode_label":r.output_mode.map(|m|m.label()).unwrap_or("Earlier mode")}));
        }
        // Only an unimported legacy snapshot may supply local-only rows.
        {
            for e in engine.entries().iter().rev() {
                if engine.host_events && !e.unsaved {
                    continue;
                }
                if (!e.unsaved && self.records.iter().any(|r| r.id == e.operation_id))
                    || !polar_dictate::history::has_history_content(
                        !e.text.trim().is_empty() || !e.raw.trim().is_empty(),
                        e.saved_samples > 0,
                        e.saved_samples,
                    )
                {
                    continue;
                }
                if e.unsaved {
                    rows.retain(|r| r["operation_id"] != e.operation_id);
                }
                let text = if e.text.trim().is_empty() {
                    &e.raw
                } else {
                    &e.text
                };
                rows.push(json!({"operation_id":e.operation_id,"preview":text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(160).collect::<String>(),
                "created_at_ms":e.created_at_ms,"duration_ms":e.saved_samples.saturating_mul(1000)/SAMPLE_RATE,"status":e.status(),"kind":if e.host_recording{"recording"}else{"text"},"pinned":false}));
            }
        }
        rows.sort_by(|a, b| {
            b["created_at_ms"]
                .as_u64()
                .cmp(&a["created_at_ms"].as_u64())
        });
        rows
    }
    pub fn detail(&self, engine: &Engine) -> Value {
        let Some(id) = self.selected.as_deref() else {
            return Value::Null;
        };
        let local = engine.entries().iter().find(|e| e.operation_id == id);
        let saved = self.detail.as_ref().filter(|r| r.id() == id);
        if local.is_none() && saved.is_none() {
            return Value::Null;
        }
        let original = saved
            .and_then(HistoryItem::original)
            .unwrap_or_else(|| local.map(|e| e.raw.clone()).unwrap_or_default());
        let text = if local.is_some_and(|e| e.unsaved || (!engine.host_events && e.edited)) {
            local.map(|e| e.text.clone()).unwrap_or_default()
        } else {
            saved
                .and_then(HistoryItem::text)
                .or_else(|| local.map(|e| e.text.clone()))
                .unwrap_or_else(|| original.clone())
        };
        let active = saved.is_some_and(HistoryItem::active);
        let busy = engine.entries().iter().any(|e| e.phase.active());
        let delivery = saved.map(HistoryItem::delivery);
        let record = saved.and_then(|r| match r {
            HistoryItem::Recording(r) => Some(r.as_ref()),
            _ => None,
        });
        let reference = record
            .map(|r| r.reference.as_str())
            .or_else(|| local.map(|e| e.reference.as_str()))
            .unwrap_or_default();
        let quote = |body: String| {
            if body.is_empty() {
                body
            } else {
                polar_dictate::quoted_transcript(reference, &body)
            }
        };
        let reusable = !active
            && !busy
            && !matches!(
                delivery,
                Some(DeliveryState::Delivered | DeliveryState::Unknown | DeliveryState::Prepared)
            );
        let can_recover =
            reusable && record.is_some_and(|r| r.saved_samples > 0 && !r.audio_expired);
        let has_result = saved.is_some_and(|item| match item {
            HistoryItem::Recording(r) => r.cleaned.is_some() || r.draft_text.is_some(),
            HistoryItem::Text(r) => r.result.is_some() || r.draft_text.is_some(),
        }) || local.is_some_and(|e| e.refined || e.edited);
        let summary = saved.map(HistoryItem::summary);
        let has_transcript = super::valid_text(&text) || super::valid_text(&original);
        let error = summary
            .as_ref()
            .and_then(|s| s.error.as_deref())
            .unwrap_or("");
        let (notice_title, notice) =
            if record.is_some_and(|r| r.saved_samples > 0) && !has_transcript && !active {
                (
                    if error.is_empty() {
                        "Saved recording"
                    } else {
                        "Transcription failed"
                    },
                    if record.is_some_and(|r| r.audio_expired) {
                        "The recording audio is no longer available."
                    } else if can_recover {
                        "Your recording is saved. You can retry transcription."
                    } else {
                        "Your recording is saved. Finish the active operation before retrying."
                    },
                )
            } else if !error.is_empty() {
                ("Original retained", "Your available text is preserved.")
            } else {
                ("", "")
            };

        json!({"operation_id":id,"has_transcript":has_transcript,"notice_title":notice_title,"notice":notice,"error":error,
            "duration_ms":summary.as_ref().map_or(0,|s|s.saved_samples.saturating_mul(1000)/SAMPLE_RATE),"text":quote(text.clone()),"raw":quote(original.clone()),"can_copy":!text.trim().is_empty(),"can_copy_original":!original.trim().is_empty(),
            "can_original":!original.trim().is_empty(),"can_refined":has_result,
            "can_recover":can_recover&&super::valid_text(&text)&&record.is_some_and(|r|r.validate_saved_input().is_ok()),
            "can_retry":can_recover&&record.is_some_and(|r|r.original().is_none()||r.cleanup_error.is_some()),
            "retry_label":if record.is_some_and(|r|r.original().is_some()) {"Retry refinement"}else{"Retry transcription"},
            "can_insert":reusable&&super::valid_text(&text)&&(record.is_none()||record.is_some_and(|r|r.validate_saved_input().is_ok())),
            "can_pin":saved.is_some(),"pin_label":if summary.as_ref().is_some_and(|s|s.pinned){"Allow automatic cleanup"}else{"Keep this record"},
            "can_dismiss":!active,"can_delete":!active&&delivery!=Some(DeliveryState::Prepared),"pinned":summary.as_ref().is_some_and(|s|s.pinned),
            "status":self.rows(engine).iter().find(|r|r["operation_id"]==id).map(|r|r["status"].clone()),
            "created_at_ms":summary.and_then(|s|s.created_at_ms).or_else(||local.and_then(|e|e.created_at_ms))})
    }
}



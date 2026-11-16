//! Read-only projection of frozen input history. Never reads current settings.
use super::application::Host;
use polar_dictate::InputDetails;
use serde_json::{json, Value};
#[derive(Default)]
pub(super) struct Inspection {
    pub id: Option<String>,
    pub stage: String,
    pub key: Option<String>,
    pub sources: bool,
    pub details: InputDetails,
    pub source_text: String,
    pub open_requested: bool,
}
impl Inspection {
    pub fn refresh(&mut self, host: Option<&dyn Host>) {
        let Some(id) = self.id.as_deref() else { return };
        let result = (|| {
            let host = host.ok_or("Voice Host unavailable")?;
            let response = host.request(json!({"op":"inputs","id":id,"key":self.key}))?;
            let mut details: InputDetails = serde_json::from_value(response["details"].clone())
                .map_err(|_| "Invalid input history")?;
            if self.stage.is_empty() {
                self.stage = details
                    .entries
                    .iter()
                    .find(|e| e.stage != "sources")
                    .map(|e| e.stage.clone())
                    .unwrap_or_else(|| "transcribe".into());
            }
            let chosen = self
                .key
                .as_deref()
                .and_then(|k| {
                    details
                        .entries
                        .iter()
                        .find(|e| e.key == k && e.stage == self.stage)
                })
                .or_else(|| details.entries.iter().find(|e| e.stage == self.stage));
            let chosen_key = chosen.map(|e| e.key.clone());
            if chosen_key != self.key
                || details
                    .selected
                    .as_ref()
                    .is_some_and(|s| s.stage != self.stage)
            {
                details.selected = if let Some(k) = &chosen_key {
                    let v = host.request(json!({"op":"inputs","id":id,"key":k}))?;
                    serde_json::from_value::<InputDetails>(v["details"].clone())
                        .map_err(|_| "Invalid input detail")?
                        .selected
                } else {
                    None
                };
            }
            if self.sources {
                let mut sources = Vec::new();
                for e in details.entries.iter().filter(|e| {
                    e.stage == "sources"
                        || (e.stage == self.stage
                            && details
                                .selected
                                .as_ref()
                                .is_some_and(|s| s.request_id == e.request_id)
                            && (e.channel == "sources" || e.channel == "learning"))
                }) {
                    let v = host.request(json!({"op":"inputs","id":id,"key":e.key}))?;
                    let d: InputDetails = serde_json::from_value(v["details"].clone())
                        .map_err(|_| "Invalid captured sources")?;
                    if let Some(s) = d.selected {
                        sources.push(json!({"request_id":s.request_id,"source":s.channel,"status":s.status,"content":s.content}));
                    }
                }
                self.source_text = if sources.is_empty() {
                    String::new()
                } else {
                    serde_json::to_string_pretty(&sources).unwrap_or_default()
                };
            }
            // None means follow the latest request; explicit selection stays stable.
            Ok::<_, String>(details)
        })();
        match result {
            Ok(details) => self.details = details,
            Err(error) => self.details.error = Some(error),
        }
    }
    pub fn view(&self) -> Value {
        let requests:Vec<_>=self.details.entries.iter().filter(|e|e.stage==self.stage).map(|e|json!({"key":e.key,"label":format!("{} · {} · {}",match e.channel.as_str(){"session.instructions"=>"Instructions","reference"=>"Reference","provider"|"transcribe"=>"Request",other=>other},e.status,e.request_id.chars().take(8).collect::<String>())})).collect();
        let selected = self.details.selected.as_ref();
        let text = if self.sources {
            self.source_text.clone()
        } else {
            selected
                .map(|s| serde_json::to_string_pretty(&s.content).unwrap_or_default())
                .unwrap_or_default()
        };
        let status = if let Some(error) = &self.details.error {
            error.clone()
        } else if self.sources {
            if self.source_text.is_empty() {
                "Source details were not recorded".into()
            } else {
                "Captured sources. Request text shows the submitted content.".into()
            }
        } else if let Some(selected) = selected {
            format!(
                "{} · {}. Submitted means sent locally, not proof of provider consumption.",
                selected.channel, selected.status
            )
        } else {
            "Input details were not recorded for this stage".into()
        };
        json!({"visible":self.id.is_some(),"operation_id":self.id,"open_requested":self.open_requested,"stage":self.stage,"sources":self.sources,"requests":requests,
            "selected_key":selected.map(|s|s.key()),"text":text,"status":status,"can_copy":!text.is_empty(),
            "stages":[{"id":"transcribe","label":"Transcribe"},{"id":"refine","label":"Refine"}],
            "title":"Inputs","sources_label":"Sources","request_text_label":"Request text","request_label":"Stage / request","copy_label":"Copy inputs"})
    }
}



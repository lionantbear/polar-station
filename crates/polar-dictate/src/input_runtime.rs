use crate::{DictateEvent, DictateStore, InputDetails, InputSnapshot};
use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};

#[derive(Clone)]
pub(crate) struct InputRecorder {
    id: String,
    store: Arc<dyn DictateStore>,
    errors: Arc<Mutex<HashSet<String>>>,
    events: tokio::sync::broadcast::Sender<DictateEvent>,
}
impl InputRecorder {
    pub(crate) async fn save(&self, snapshot: InputSnapshot) {
        if self.store.save_input(&self.id, snapshot).await.is_err() {
            if let Ok(mut errors) = self.errors.lock() {
                errors.insert(self.id.clone());
            }
        }
        let _ = self
            .events
            .send(DictateEvent::InputsChanged(self.id.clone()));
    }
    pub(crate) fn voice(self) -> polar_voice::inspection::InputObserver {
        Arc::new(move |request_id, channel, status, content| {
            let recorder = self.clone();
            Box::pin(async move {
                recorder
                    .save(InputSnapshot {
                        request_id,
                        stage: "transcribe".into(),
                        channel,
                        status,
                        content,
                    })
                    .await;
            })
        })
    }
}
impl polar_transcreation::InhouseTurnEventHandler for InputRecorder {
    fn on_agent_text_delta(&mut self, _delta: &str) -> anyhow::Result<()> {
        Ok(())
    }
    fn on_input_snapshot(
        &mut self,
        s: polar_transcreation::InhouseInputSnapshot,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        let recorder = self.clone();
        Box::pin(async move {
            recorder
                .save(InputSnapshot {
                    request_id: s.request_id,
                    stage: "refine".into(),
                    channel: s.channel,
                    status: s.status,
                    content: s.content,
                })
                .await;
        })
    }
}
impl crate::runtime::DictateRuntime {
    pub(crate) fn input_recorder(&self, id: &str) -> InputRecorder {
        InputRecorder {
            id: id.into(),
            store: self.store.clone(),
            errors: self.input_errors.clone(),
            events: self.events.clone(),
        }
    }
    pub async fn input_details(&self, id: &str, key: Option<&str>) -> anyhow::Result<InputDetails> {
        let mut details = self.store.inputs(id, key).await?;
        if self
            .input_errors
            .lock()
            .map_err(|_| anyhow::anyhow!("dictate.worker_unavailable"))?
            .contains(id)
        {
            details.error = Some("Input details could not be saved".into());
        }
        Ok(details)
    }
    pub async fn record_input_sources(&self, id: &str, content: serde_json::Value) {
        self.input_recorder(id)
            .save(InputSnapshot {
                request_id: id.into(),
                stage: "sources".into(),
                channel: "capture".into(),
                status: "prepared".into(),
                content,
            })
            .await;
    }
}

//! Optional, request-scoped observation. Never serialized into a provider request.
use serde_json::Value;
use std::{future::Future, pin::Pin, sync::Arc};
pub type InputObserver = Arc<
    dyn Fn(String, String, String, Value) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync,
>;
#[derive(Clone)]
pub struct InputObservation {
    pub request_id: String,
    pub observer: InputObserver,
}
impl InputObservation {
    pub async fn emit(&self, channel: &str, status: &str, content: Value) {
        (self.observer)(
            self.request_id.clone(),
            channel.into(),
            status.into(),
            content,
        )
        .await;
    }
}

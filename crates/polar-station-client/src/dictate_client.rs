//! The GUI only talks to the authenticated local Host. No credentials/provider.
use anyhow::{Context, Result};
use polar_local_access_contract::{LocalExtensionRequestV1, LocalExtensionResultV1};
use polar_station_protocol::{LocalClientFrame, LocalHostFrame};
use serde_json::Value;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
pub type EndpointResult = Result<std::path::PathBuf>;
pub type EndpointReader = std::sync::Arc<dyn Fn() -> Result<std::path::PathBuf> + Send + Sync>;
#[derive(Clone, Default)]
pub struct DictateClient {
    path: Option<std::path::PathBuf>,
    reader: Option<EndpointReader>,
}
impl DictateClient {
    pub fn with_socket_path(path: std::path::PathBuf) -> Self { Self { path:Some(path), reader:None } }
    pub fn with_endpoint_reader(reader: EndpointReader) -> Self { Self { path:None, reader:Some(reader) } }
    pub(crate) fn endpoint(&self) -> Result<std::path::PathBuf> {
        if let Some(reader)=&self.reader {return reader();}
        self.path.clone().map(Ok).unwrap_or_else(crate::local_ipc::default_socket_path)
    }
    /// An admitted multi-request operation retains its original backend.
    pub(crate) fn snapshot(&self) -> Result<Self> { Ok(Self::with_socket_path(self.endpoint()?)) }
    async fn connect(&self) -> Result<tokio::net::UnixStream> {
        let path = self.endpoint()?;
        let owner = crate::local_ipc::effective_uid();
        let metadata =
            std::fs::symlink_metadata(&path).context("Start Polar Station Host to use Voice")?;
        anyhow::ensure!(
            metadata.file_type().is_socket() && metadata.uid() == owner,
            "Voice Host ownership mismatch"
        );
        let stream = tokio::net::UnixStream::connect(path)
            .await
            .context("Voice Host is unavailable; saved recordings remain on this computer")?;
        anyhow::ensure!(
            stream.peer_cred()?.uid() == owner,
            "Voice Host peer mismatch"
        );
        Ok(stream)
    }
    pub async fn call(&self, payload: Value) -> Result<Value> {
        let mut stream = self.connect().await?;
        let request_id = polar_identity::generation::new_uuid_v4().to_string();
        let wait = payload.get("op").and_then(Value::as_str) == Some("wait");
        let frame = LocalClientFrame::ExtensionRequest {
            request: LocalExtensionRequestV1 {
                extension: polar_station_protocol::DICTATE_EXTENSION.into(),
                version: 1,
                request_id: request_id.clone(),
                timeout_ms: 30_000,
                payload,
            },
        };
        frame.validate()?;
        crate::local_ipc::write_json_frame(&mut stream, &frame).await?;
        // `wait` is a server event subscription: no periodic request, no retry of
        // mutations. It ends only on a terminal result or socket close.
        let read = crate::local_ipc::read_json_frame::<_, LocalHostFrame>(&mut stream);
        let response = if wait {
            read.await?
        } else {
            tokio::time::timeout(std::time::Duration::from_secs(30), read).await??
        }
        .context("Voice Host disconnected; reload the saved recording before retrying")?;
        let LocalHostFrame::ExtensionResponse { response } = response else {
            anyhow::bail!("Unexpected Voice Host response");
        };
        response.validate()?;
        anyhow::ensure!(
            response.extension == polar_station_protocol::DICTATE_EXTENSION
                && response.version == 1
                && response.request_id == request_id,
            "Voice response identity mismatch"
        );
        match response.result {
            LocalExtensionResultV1::Success { payload } => Ok(payload),
            LocalExtensionResultV1::Rejected { error } => anyhow::bail!("{}", error.message),
            _ => anyhow::bail!("Update Polar Station Host to enable Voice settings"),
        }
    }
    /// One authenticated event connection; no timer, polling or mutation replay.
    pub async fn observe(&self, id: &str, callback: impl FnMut(Value) -> Result<()> + Send) -> Result<()> {
        self.observe_request(serde_json::json!({"op":"subscribe","id":id}), callback).await
    }
    pub async fn observe_history(&self, callback: impl FnMut(Value) -> Result<()> + Send) -> Result<()> {
        self.observe_request(serde_json::json!({"op":"subscribe_history"}), callback).await
    }
    async fn observe_request(&self, payload: Value, mut callback: impl FnMut(Value) -> Result<()> + Send) -> Result<()> {
        let mut stream = self.connect().await?;
        let request_id = polar_identity::generation::new_uuid_v4().to_string();
        let frame = LocalClientFrame::ExtensionRequest { request: LocalExtensionRequestV1 {
            extension: polar_station_protocol::DICTATE_EXTENSION.into(), version:1, request_id:request_id.clone(),
            timeout_ms:30_000, payload,
        }};
        frame.validate()?;
        crate::local_ipc::write_json_frame(&mut stream,&frame).await?;
        let mut sequence = 0;
        loop {
            let frame = crate::local_ipc::read_json_frame::<_,LocalHostFrame>(&mut stream).await?
                .context("Voice preview disconnected; saved audio retained")?;
            frame.validate()?;
            match frame {
                LocalHostFrame::ExtensionResponse { response } => {
                    anyhow::ensure!(response.extension == polar_station_protocol::DICTATE_EXTENSION && response.version == 1 && response.request_id == request_id,"Voice subscription identity mismatch");
                    anyhow::ensure!(matches!(response.result,LocalExtensionResultV1::Success{..}),"Voice preview subscription rejected");
                }
                LocalHostFrame::ExtensionEvent { event } => {
                    anyhow::ensure!(event.extension == polar_station_protocol::DICTATE_EXTENSION && event.version == 1
                        && event.subscription_id == request_id && event.sequence > sequence,"Voice preview identity mismatch");
                    sequence=event.sequence;
                    let done=event.payload["done"].as_bool()==Some(true);
                    callback(event.payload)?;
                    if done { return Ok(()); }
                }
                _ => anyhow::bail!("Unexpected Voice preview frame"),
            }
        }
    }

}



//! One-shot, OS-authenticated access to the existing user Host. This adapter
//! owns no session catalog, connection pool or input receipts.
use anyhow::{Context, Result};
use polar_local_access_contract::{LocalExtensionRequestV1, LocalExtensionResultV1};
use polar_station_session::*;
use polar_tmux_protocol::{LocalClientFrame, LocalHostFrame};

use std::{
    os::unix::fs::{FileTypeExt, MetadataExt},
    path::PathBuf,
    time::Duration,
};

pub const EXTENSION: &str = "polar.station.sessions";
pub(crate) use polar_station_session::LocalRequest as Request;
pub struct SessionClient {
    path: PathBuf,
    owner_uid: u32,
    target_id: Option<String>,
}
impl SessionClient {
    pub fn new(path: PathBuf, owner_uid: u32, target_id: Option<String>) -> Self {
        Self {
            path,
            owner_uid,
            target_id,
        }
    }
    fn target(&self, id: &str) -> Result<()> {
        anyhow::ensure!(
            self.target_id.as_ref().is_none_or(|allowed| allowed == id),
            "execution target is outside this backend binding"
        );
        Ok(())
    }
    async fn call<T: serde::de::DeserializeOwned>(&self, request: Request) -> Result<T> {
        tokio::time::timeout(Duration::from_secs(30), async {
            let metadata = std::fs::symlink_metadata(&self.path)
                .context("inspect the configured Station Host socket")?;
            anyhow::ensure!(
                metadata.file_type().is_socket() && metadata.uid() == self.owner_uid,
                "Station Host socket ownership mismatch"
            );
            let mut stream = tokio::net::UnixStream::connect(&self.path)
                .await
                .context("connect the configured Station Host")?;
            anyhow::ensure!(
                stream.peer_cred()?.uid() == self.owner_uid,
                "Station Host peer UID mismatch"
            );
            let request_id = "station-session-control";
            let frame = LocalClientFrame::ExtensionRequest {
                request: LocalExtensionRequestV1 {
                    extension: EXTENSION.into(),
                    version: 1,
                    request_id: request_id.into(),
                    timeout_ms: 30_000,
                    payload: serde_json::to_value(request)?,
                },
            };
            crate::local_ipc::write_json_frame(&mut stream, &frame).await?;
            let frame = crate::local_ipc::read_json_frame::<_, LocalHostFrame>(&mut stream)
                .await?
                .context("Station Host closed the session control profile")?;
            let LocalHostFrame::ExtensionResponse { response } = frame else {
                anyhow::bail!("unexpected Station Host session response")
            };
            response.validate()?;
            anyhow::ensure!(
                response.extension == EXTENSION
                    && response.version == 1
                    && response.request_id == request_id,
                "session response identity mismatch"
            );
            match response.result {
                LocalExtensionResultV1::Success { payload } => Ok(serde_json::from_value(payload)?),
                LocalExtensionResultV1::Rejected { error } => {
                    anyhow::bail!("Station session request unconfirmed: {}", error.code)
                }
                _ => anyhow::bail!("Station session control is unavailable"),
            }
        })
        .await
        .context("Station session outcome unknown after transport timeout")?
    }
}
#[async_trait::async_trait]
impl SessionBackend for SessionClient {
    async fn list(&self, mut input: ListRequest) -> Result<Catalog> {
        if let Some(id) = &input.target_id {
            self.target(id)?;
        }
        if input.target_id.is_none() {
            input.target_id = self.target_id.clone();
        }
        self.call(Request::List { input }).await
    }
    async fn send(&self, principal: &str, input: SendRequest) -> Result<Receipt> {
        self.target(&input.reference.target_id)?;
        self.call(Request::Send {
            principal: principal.into(),
            input,
        })
        .await
    }
    async fn receipt(&self, principal: &str, input: ReceiptRequest) -> Result<Receipt> {
        self.target(&input.reference.target_id)?;
        self.call(Request::Receipt {
            principal: principal.into(),
            input,
        })
        .await
    }
}

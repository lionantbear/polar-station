//! Execute terminal input effects through the existing authenticated Session client.
//! Scheduling belongs to DesktopClient; this adapter owns no queue or UI state.
use crate::session_client::{Request, SessionClient};
use polar_station_session::SessionBackend;
use std::path::PathBuf;

pub(crate) use polar_station_input::terminal::SessionJob as DesktopSessionRequest;

pub(crate) async fn execute(
    job: DesktopSessionRequest,
    socket_path: PathBuf,
) -> crate::desktop_client::DesktopClientEvent {
    let client = SessionClient::new(socket_path, crate::local_ipc::effective_uid(), None);
    let result: anyhow::Result<serde_json::Value> = async {
        match job.request {
            Request::List { input } => Ok(serde_json::to_value(client.list(input).await?)?),
            Request::Send { principal, input } => {
                Ok(serde_json::to_value(client.send(&principal, input).await?)?)
            }
            Request::Receipt { principal, input } => Ok(serde_json::to_value(
                client.receipt(&principal, input).await?,
            )?),
        }
    }
    .await;
    match result {
        Ok(value) => crate::desktop_client::DesktopClientEvent::SessionCompleted {
            request_id: job.request_id,
            value: Some(value),
            error: None,
        },
        Err(error) => crate::desktop_client::DesktopClientEvent::SessionCompleted {
            request_id: job.request_id,
            value: None,
            error: Some(format!("{error:#}")),
        },
    }
}



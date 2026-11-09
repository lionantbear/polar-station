//! IO forwarding for the existing authenticated Voice extension profile.
use anyhow::Result;
use polar_local_access_contract::{LocalExtensionEventV1, LocalExtensionRequestV1, LocalExtensionResponseV1, LocalExtensionResultV1};
use polar_tmux_protocol::LocalHostFrame;
use serde_json::json;
use tokio::io::AsyncReadExt;

impl super::DictateHost {
    pub(crate) async fn serve_preview(&self, mut stream: tokio::net::UnixStream, request: LocalExtensionRequestV1) -> Result<()> {
        anyhow::ensure!(request.version==1,"Update Polar Station Host to use Voice");
        let polar_dictate::Request::Subscribe { id } = serde_json::from_value(request.payload.clone())? else {
            anyhow::bail!("Invalid Voice subscription");
        };
        anyhow::ensure!(polar_dictate::identifier(&id),"Invalid recording ID");
        let mut updates=self.runtime.subscribe();
        let mut record=self.runtime.preview_snapshot(&id).await?;
        let response=LocalExtensionResponseV1::for_request(&request,LocalExtensionResultV1::Success {payload:json!({"subscribed":id})});
        response.validate()?;
        crate::local_ipc::write_json_frame(&mut stream,&LocalHostFrame::ExtensionResponse {response}).await?;
        let mut sequence=0;
        let mut last=None;
        loop {
            let receipt=record.receipt();
            let done=record.preview_complete();
            let state=(receipt.transcript_preview.clone(),receipt.saved_through_sample,done);
            if last.as_ref()!=Some(&state) {
                sequence+=1;
                let event=LocalExtensionEventV1 {extension:super::EXTENSION.into(),version:1,
                    subscription_id:request.request_id.clone(),sequence,payload:json!({"receipt":receipt,"done":done})};
                event.validate()?;
                crate::local_ipc::write_json_frame(&mut stream,&LocalHostFrame::ExtensionEvent {event}).await?;
                last=Some(state);
            }
            if done { return Ok(()); }
            let mut closed=[0u8;1];
            tokio::select! {
                result=stream.read(&mut closed) => {
                    anyhow::ensure!(result?==0,"Unexpected input on Voice preview subscription");
                    return Ok(());
                }
                result=updates.recv() => match result {
                    Ok(next) if next.id==id => record=next,
                    Ok(_) => {},
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => record=self.runtime.preview_snapshot(&id).await?,
                    Err(_) => anyhow::bail!("Voice observation stopped"),
                }
            }
        }
    }
}

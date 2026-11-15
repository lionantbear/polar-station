//! Resolve a canonical remote choice through existing local execution routes.
//! The remote result contains resource facts, never a command to execute.
use super::*;
use anyhow::Context;
use sha2::{Digest, Sha256};
fn digest(value: &str) -> String { format!("{:x}",Sha256::digest(value.as_bytes())) }
fn execution_target<'a>(route:Option<&'a PolarTargetConfig>, provider:&'a str) -> AccessResult<&'a str> {
    let Some(route)=route else {return Ok(provider);};
    route.terminal_targets.get(provider).map(String::as_str)
        .or_else(|| if provider=="local" {Some(match &route.route {
            PolarRoute::LocalUds{..}=>"local",PolarRoute::SshUds{execution_target_id,..}=>execution_target_id,
        })}else{None})
        .ok_or_else(||failure("view_route_unbound","Connect and bind this Host in the Polar connection before sending. No task was sent."))
}
impl PolarGateway {
    #[tracing::instrument(target="polar_station_operation", name="session_view_prepare", skip_all, fields(component="Session", failure=tracing::field::Empty))]
    pub(super) async fn prepare_terminal_view(&self, route: Option<&PolarTargetConfig>, mut value: Value, activate: bool) -> AccessResult<Value> {
        if route.is_some_and(|route|!self.targets.polar_targets().iter().any(|current|current==route)) { return Err(failure("view_route_changed","The Polar connection changed. Task not sent.")); }
        let reference: polar_station_session::SessionReference = serde_json::from_value(value["reference"].clone())
            .map_err(|_|failure("view_invalid_reference","The selected target has no valid terminal reference."))?;
        reference.validate().map_err(|_|failure("view_invalid_reference","The terminal reference is invalid."))?;
        let target_id = execution_target(route,&reference.target_id)?;
        let target = self.targets.get(target_id).ok_or_else(||failure("view_route_unavailable","The configured target Host is unavailable. No task was sent."))?;
        let gate = target.operation_gate(); let _guard = gate.write().await;
        let checked = async {
            let client = target.managed_client(&self.ssh).await.map_err(|e|anyhow::anyhow!(e.to_string()))?;
            let observed = client.observe_sessions().await?;
            anyhow::ensure!(observed.generation.as_deref().is_some_and(|s|digest(s)==reference.server_generation),"server changed");
            let session = observed.sessions.iter().find(|(s,_)|s.id==reference.session_id && s.created_unix_seconds==Some(reference.session_created)).context("session changed")?;
            let panes = client.list_panes(&session.0.id).await?;
            let pane = panes.iter().find(|p|p.id==reference.pane_id && !p.dead && p.pid.is_some_and(|p|p>0)).context("pane changed")?;
            let generations = client.pane_input_generations().await?;
            let generation = generations.get(&pane.id).map(String::as_str).unwrap_or("");
            anyhow::ensure!(digest(&format!("{}:{}:{}",pane.id,pane.pid.unwrap_or(0),generation))==reference.pane_incarnation,"pane input changed");
            anyhow::ensure!(self.targets.get(target_id).is_some_and(|current|current.configuration_revision()==target.configuration_revision()),"target configuration changed");
            if activate {
                let binding = polar_tmux_core::TmuxSessionBinding {session_id:reference.session_id.clone(),
                    server_generation:observed.generation.clone().context("server missing")?,created_unix_seconds:reference.session_created,managed_marker:session.1.clone()};
                anyhow::ensure!(client.show_bound_pane(&binding,pane).await?,"target changed during display activation");
            }
            Ok::<_,anyhow::Error>(json!({"execution_target_id":target_id,"tmux_session_id":reference.session_id,"tmux_pane_id":reference.pane_id,
                "configuration_revision":target.configuration_revision(),"server_generation":reference.server_generation,
                "session_created":reference.session_created,"pane_pid":pane.pid}))
        }.await.map_err(|error| {tracing::Span::current().record("failure",error.to_string());failure("view_target_changed","The configured connection does not match the selected terminal, or the terminal changed. No task was sent.")})?;
        if route.is_some_and(|route|!self.targets.polar_targets().iter().any(|current|current==route)) { return Err(failure("view_route_changed","The Polar connection changed. Task not sent.")); }
        value["client_view"] = checked;
        Ok(value)
    }
}



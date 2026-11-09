//! One tmux command queue for a complete Session identity observation.
use crate::{
    parser, TmuxClient, TmuxError, TmuxErrorKind, TmuxResult, MANAGED_SESSION_MARKER,
    SESSION_FORMAT,
};
use polar_tmux_types::{TmuxClientInfo, TmuxSession};

#[derive(Debug, Clone)]
pub struct TmuxSessionObservation {
    pub generation: Option<String>,
    pub sessions: Vec<(TmuxSession, Option<String>)>,
    pub clients: Vec<TmuxClientInfo>,
    pub panes: Vec<(String, String, bool)>,
}

impl TmuxClient {
    /// One child/channel; -N forbids starting a server while observing it.
    /// The leading/trailing identity delimit the same native command queue.
    pub async fn observe_sessions(&self) -> TmuxResult<TmuxSessionObservation> {
        let output = self.run_checked_batch(vec![vec![
            "-N".into(), "display-message".into(), "-p".into(), "G\t#{pid}\t#{start_time}\t#{socket_path}".into(),
            ], vec![ "list-sessions".into(), "-F".into(), format!("S\t{SESSION_FORMAT}\t#{{{MANAGED_SESSION_MARKER}}}"),
            ], vec![ "list-panes".into(), "-a".into(), "-F".into(), "P\t#{session_id}\t#{pane_id}\t#{pane_dead}".into(),
            ], vec![ "list-clients".into(), "-F".into(), "C\t#{client_name}\t#{client_tty}\t#{session_id}\t#{client_pid}\t#{client_created}\t#{client_last_session}\t#{client_readonly}\t#{client_control_mode}".into(),
            ], vec![ "display-message".into(), "-p".into(), "E\t#{pid}\t#{start_time}\t#{socket_path}".into(),
        ]]).await;
        match output {
            Err(error) if error.kind() == TmuxErrorKind::NoServer => Ok(TmuxSessionObservation {
                generation: None,
                sessions: vec![],
                clients: vec![],
                panes: vec![],
            }),
            Err(error) => Err(error),
            Ok(output) => parse_observation(&output),
        }
    }
}

#[tracing::instrument(target = "polar_station_operation", name = "session_observation_parse", skip_all,
    fields(component="Tmux", bytes=output.len()))]
fn parse_observation(output: &str) -> TmuxResult<TmuxSessionObservation> {
    let mut lines = output.lines();
    let generation = lines
        .next()
        .and_then(|line| line.strip_prefix("G\t"))
        .ok_or_else(|| TmuxError::parse_failed("missing observation start"))?;
    let fields: Vec<_> = generation.split('\t').collect();
    if fields.len() != 3
        || fields[0].parse::<u64>().is_err()
        || fields[1].parse::<u64>().is_err()
        || fields[2].is_empty()
    {
        return Err(TmuxError::parse_failed("invalid observation generation"));
    }
    let mut observation = TmuxSessionObservation {
        generation: Some(generation.to_owned()),
        sessions: vec![],
        clients: vec![],
                panes: vec![],
    };
    let end = lines.next_back().and_then(|line| line.strip_prefix("E\t"));
    if end != Some(generation) {
        return Err(TmuxError::parse_failed(
            "tmux server changed during observation",
        ));
    }
    for line in lines {
        if let Some(session) = line.strip_prefix("S\t") {
            let (row, marker) = session
                .rsplit_once('\t')
                .ok_or_else(|| TmuxError::parse_failed("missing Session marker"))?;
            let session = parser::parse_sessions(row)?
                .pop()
                .ok_or_else(|| TmuxError::parse_failed("missing Session"))?;
            observation
                .sessions
                .push((session, Some(marker.to_owned()).filter(|v| !v.is_empty())));
        } else if let Some(pane) = line.strip_prefix("P\t") {
            let fields: Vec<_> = pane.split('\t').collect();
            if fields.len() != 3 || !matches!(fields[2], "0" | "1") {
                return Err(TmuxError::parse_failed("invalid observed pane"));
            }
            observation.panes.push((fields[0].into(), fields[1].into(), fields[2] == "1"));
        } else if let Some(client) = line.strip_prefix("C\t") {
            observation.clients.extend(parser::parse_clients(client)?);
        } else {
            return Err(TmuxError::parse_failed("unknown observation record"));
        }
    }
    Ok(observation)
}



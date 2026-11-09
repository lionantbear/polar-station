use crate::TmuxWindowActivity;
use crate::{TmuxError, TmuxResult};
use polar_tmux_types::{TmuxClientInfo, TmuxPane, TmuxSession, TmuxTopologyPane, TmuxWindow};

pub(crate) fn parse_sessions(input: &str) -> TmuxResult<Vec<TmuxSession>> {
    input
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let fields = fields(line, 5, "session")?;
            Ok(TmuxSession {
                id: fields[0].to_string(),
                name: fields[1].to_string(),
                window_count: parse_u32(fields[2], "session_windows")?,
                // tmux exposes `session_attached` as the attached-client
                // count, not a strict 0/1 boolean.
                attached: parse_count_bool(fields[3], "session_attached")?,
                created_unix_seconds: parse_optional_i64(fields[4], "session_created")?,
            })
        })
        .collect()
}

pub(crate) fn parse_windows(input: &str) -> TmuxResult<Vec<TmuxWindow>> {
    input
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let fields = fields(line, 7, "window")?;
            Ok(TmuxWindow {
                id: fields[0].to_string(),
                session_id: fields[1].to_string(),
                index: parse_u32(fields[2], "window_index")?,
                name: fields[3].to_string(),
                active: parse_bool(fields[4], "window_active")?,
                pane_count: parse_u32(fields[5], "window_panes")?,
                layout: fields[6].to_string(),
            })
        })
        .collect()
}

pub(crate) fn parse_window_activity(input: &str) -> TmuxResult<Vec<TmuxWindowActivity>> {
    input
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let fields = fields(line, 3, "window activity")?;
            Ok(TmuxWindowActivity {
                window_id: fields[0].to_string(),
                session_id: fields[1].to_string(),
                activity_epoch: fields[2].parse().map_err(|_| {
                    TmuxError::parse_failed(format!(
                        "invalid window_activity value {:?}",
                        fields[1]
                    ))
                })?,
            })
        })
        .collect()
}

pub(crate) fn parse_panes(input: &str) -> TmuxResult<Vec<TmuxPane>> {
    input
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let fields = fields(line, 10, "pane")?;
            Ok(TmuxPane {
                id: fields[0].to_string(),
                window_id: fields[1].to_string(),
                index: parse_u32(fields[2], "pane_index")?,
                title: fields[3].to_string(),
                current_command: fields[4].to_string(),
                current_path: fields[5].to_string(),
                active: parse_bool(fields[6], "pane_active")?,
                dead: parse_bool(fields[7], "pane_dead")?,
                pid: parse_optional_u32(fields[8], "pane_pid")?,
                tty: fields[9].to_string(),
            })
        })
        .collect()
}

pub(crate) fn parse_topology_panes(input: &str) -> TmuxResult<Vec<TmuxTopologyPane>> {
    input
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let fields = fields(line, 6, "topology pane")?;
            Ok(TmuxTopologyPane {
                id: fields[0].to_string(),
                window_id: fields[1].to_string(),
                index: parse_u32(fields[2], "pane_index")?,
                title: fields[3].to_string(),
                active: parse_bool(fields[4], "pane_active")?,
                dead: parse_bool(fields[5], "pane_dead")?,
            })
        })
        .collect()
}

pub(crate) fn parse_clients(input: &str) -> TmuxResult<Vec<TmuxClientInfo>> {
    input
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let fields = fields(line, 8, "client")?;
            let (last_session_unix_seconds, last_session) = if fields[5].is_empty() {
                (None, String::new())
            } else if let Ok(timestamp) = fields[5].parse::<i64>() {
                (Some(timestamp), String::new())
            } else {
                (None, fields[5].to_string())
            };
            Ok(TmuxClientInfo {
                name: fields[0].to_string(),
                tty: fields[1].to_string(),
                session_id: fields[2].to_string(),
                pid: parse_optional_u32(fields[3], "client_pid")?,
                created_unix_seconds: parse_optional_i64(fields[4], "client_created")?,
                last_session_unix_seconds,
                last_session,
                readonly: parse_bool(fields[6], "client_readonly")?,
                control_mode: parse_bool(fields[7], "client_control_mode")?,
            })
        })
        .collect()
}

fn fields<'a>(line: &'a str, expected: usize, kind: &str) -> TmuxResult<Vec<&'a str>> {
    let fields: Vec<_> = line.split('\t').collect();
    if fields.len() != expected {
        return Err(TmuxError::parse_failed(format!(
            "invalid {kind} format: expected {expected} fields, got {} in {line:?}",
            fields.len()
        )));
    }
    Ok(fields)
}

fn parse_u32(value: &str, field: &str) -> TmuxResult<u32> {
    value
        .parse()
        .map_err(|_| TmuxError::parse_failed(format!("invalid {field} value {value:?}")))
}

fn parse_optional_u32(value: &str, field: &str) -> TmuxResult<Option<u32>> {
    if value.is_empty() {
        Ok(None)
    } else {
        parse_u32(value, field).map(Some)
    }
}

fn parse_optional_i64(value: &str, field: &str) -> TmuxResult<Option<i64>> {
    if value.is_empty() {
        Ok(None)
    } else {
        value
            .parse()
            .map(Some)
            .map_err(|_| TmuxError::parse_failed(format!("invalid {field} value {value:?}")))
    }
}

fn parse_bool(value: &str, field: &str) -> TmuxResult<bool> {
    match value {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(TmuxError::parse_failed(format!(
            "invalid {field} boolean {value:?}; expected \"0\" or \"1\""
        ))),
    }
}

fn parse_count_bool(value: &str, field: &str) -> TmuxResult<bool> {
    parse_u32(value, field).map(|count| count > 0)
}



//! Typed tmux Control Mode planning and decoding.
//!
//! This module owns tmux syntax only. Agent, Inbox, AI, and native presentation
//! semantics belong to the Host and presentation crates.

use std::{error::Error, fmt};

pub const CONTROL_MODE_AGENT_EVENT_OPTION: &str = "@polar_station_agent_events_v1";
const MAX_CONTROL_LINE_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmuxControlPlan {
    pub program: String,
    pub args: Vec<String>,
    pub environment: Vec<(String, String)>,
    pub initial_commands: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TmuxControlEvent {
    WindowGeometry {
        window_id: String,
        columns: u16,
        rows: u16,
    },
    SubscriptionChanged {
        name: String,
        session_id: String,
        window_id: String,
        window_index: u64,
        pane_id: String,
        value: String,
    },
    Lifecycle(TmuxControlLifecycleEvent),
    CommandFailed,
    Exit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TmuxControlLifecycleEvent {
    SessionsChanged,
    SessionRenamed { name: String },
    WindowAdded { window_id: String },
    WindowClosed { window_id: String },
    WindowRenamed { window_id: String },
    WindowChanged { window_id: String },
    PaneModeChanged { pane_id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlModeDecodeError {
    InvalidUtf8,
    LineTooLong,
    MalformedSubscription,
    UnexpectedPaneOutput,
}

impl fmt::Display for ControlModeDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidUtf8 => "tmux Control Mode emitted invalid UTF-8",
            Self::LineTooLong => "tmux Control Mode line exceeded the bounded decoder limit",
            Self::MalformedSubscription => "tmux Control Mode subscription was malformed",
            Self::UnexpectedPaneOutput => {
                "tmux Control Mode emitted pane output despite the no-output client flag"
            }
        })
    }
}

impl Error for ControlModeDecodeError {}

#[derive(Debug, Default)]
pub struct ControlModeDecoder {
    pending: Vec<u8>,
}

impl ControlModeDecoder {
    pub fn feed(&mut self, bytes: &[u8]) -> Result<Vec<TmuxControlEvent>, ControlModeDecodeError> {
        if self.pending.len().saturating_add(bytes.len()) > MAX_CONTROL_LINE_BYTES
            && !bytes.contains(&b'\n')
        {
            self.pending.clear();
            return Err(ControlModeDecodeError::LineTooLong);
        }
        self.pending.extend_from_slice(bytes);
        let mut events = Vec::new();
        while let Some(newline) = self.pending.iter().position(|byte| *byte == b'\n') {
            if newline > MAX_CONTROL_LINE_BYTES {
                self.pending.drain(..=newline);
                return Err(ControlModeDecodeError::LineTooLong);
            }
            let mut line = self.pending.drain(..=newline).collect::<Vec<_>>();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let line =
                std::str::from_utf8(&line).map_err(|_| ControlModeDecodeError::InvalidUtf8)?;
            if let Some(event) = decode_line(line)? {
                events.push(event);
            }
        }
        if self.pending.len() > MAX_CONTROL_LINE_BYTES {
            self.pending.clear();
            return Err(ControlModeDecodeError::LineTooLong);
        }
        Ok(events)
    }
}

pub(crate) fn decode_line(line: &str) -> Result<Option<TmuxControlEvent>, ControlModeDecodeError> {
    // tmux -CC opens its private terminal protocol with this exact DCS frame.
    // Strip framing before all event and no-output checks, never payload text.
    let line = line.strip_prefix("\x1bP1000p").unwrap_or(line);
    if line.starts_with("%output ") || line.starts_with("%extended-output ") {
        return Err(ControlModeDecodeError::UnexpectedPaneOutput);
    }
    if line.starts_with("%error ") {
        return Ok(Some(TmuxControlEvent::CommandFailed));
    }
    if let Some(rest) = line.strip_prefix("%subscription-changed ") {
        return decode_subscription(rest).map(Some);
    }
    let lifecycle = if line == "%sessions-changed" {
        Some(TmuxControlLifecycleEvent::SessionsChanged)
    } else if let Some(name) = line.strip_prefix("%session-renamed ") {
        Some(TmuxControlLifecycleEvent::SessionRenamed {
            name: decode_tmux_value(name),
        })
    } else if let Some(window_id) = one_token(line, "%window-add ") {
        Some(TmuxControlLifecycleEvent::WindowAdded { window_id })
    } else if let Some(window_id) = one_token(line, "%window-close ") {
        Some(TmuxControlLifecycleEvent::WindowClosed { window_id })
    } else if let Some(window_id) = first_token(line, "%window-renamed ") {
        Some(TmuxControlLifecycleEvent::WindowRenamed { window_id })
    } else if let Some(rest) = line.strip_prefix("%layout-change ") {
        let mut fields = rest.split_whitespace();
        let window_id = fields
            .next()
            .ok_or(ControlModeDecodeError::MalformedSubscription)?;
        let layout = fields
            .next()
            .ok_or(ControlModeDecodeError::MalformedSubscription)?;
        let (_, dimensions) = layout
            .split_once(',')
            .ok_or(ControlModeDecodeError::MalformedSubscription)?;
        let (columns, remaining) = dimensions
            .split_once('x')
            .ok_or(ControlModeDecodeError::MalformedSubscription)?;
        let rows = remaining
            .split(',')
            .next()
            .ok_or(ControlModeDecodeError::MalformedSubscription)?;
        return Ok(Some(TmuxControlEvent::WindowGeometry {
            window_id: window_id.into(),
            columns: columns
                .parse()
                .map_err(|_| ControlModeDecodeError::MalformedSubscription)?,
            rows: rows
                .parse()
                .map_err(|_| ControlModeDecodeError::MalformedSubscription)?,
        }));
    } else {
        first_token(line, "%pane-mode-changed ")
            .map(|pane_id| TmuxControlLifecycleEvent::PaneModeChanged { pane_id })
    };
    if let Some(lifecycle) = lifecycle {
        return Ok(Some(TmuxControlEvent::Lifecycle(lifecycle)));
    }
    if line.starts_with("%exit") {
        return Ok(Some(TmuxControlEvent::Exit));
    }
    // Command boundaries and future notification kinds do not carry Agent
    // semantics. Ignoring them keeps the decoder forward compatible.
    Ok(None)
}

fn decode_subscription(rest: &str) -> Result<TmuxControlEvent, ControlModeDecodeError> {
    let (identity, value) = rest
        .split_once(" : ")
        .ok_or(ControlModeDecodeError::MalformedSubscription)?;
    let mut fields = identity.split_whitespace();
    let name = fields
        .next()
        .ok_or(ControlModeDecodeError::MalformedSubscription)?;
    let session_id = fields
        .next()
        .ok_or(ControlModeDecodeError::MalformedSubscription)?;
    let window_id = fields
        .next()
        .ok_or(ControlModeDecodeError::MalformedSubscription)?;
    let index = fields
        .next()
        .ok_or(ControlModeDecodeError::MalformedSubscription)?;
    let window_index = if window_id == "-" && index == "-" {
        0
    } else {
        index
            .parse()
            .map_err(|_| ControlModeDecodeError::MalformedSubscription)?
    };
    let pane_id = fields
        .next()
        .ok_or(ControlModeDecodeError::MalformedSubscription)?;
    if !valid_prefixed_id(session_id, '$')
        || !(window_id == "-" || valid_prefixed_id(window_id, '@'))
        || !(pane_id == "-" || valid_prefixed_id(pane_id, '%'))
    {
        return Err(ControlModeDecodeError::MalformedSubscription);
    }
    Ok(TmuxControlEvent::SubscriptionChanged {
        name: name.to_owned(),
        session_id: session_id.to_owned(),
        window_id: window_id.to_owned(),
        window_index,
        pane_id: pane_id.to_owned(),
        value: if name.starts_with("target-") {
            value.to_owned()
        } else {
            decode_tmux_value(value)
        },
    })
}

fn valid_prefixed_id(value: &str, prefix: char) -> bool {
    value.strip_prefix(prefix).is_some_and(|digits| {
        !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
    })
}

fn one_token(line: &str, prefix: &str) -> Option<String> {
    let rest = line.strip_prefix(prefix)?;
    (!rest.is_empty() && !rest.contains(char::is_whitespace)).then(|| rest.to_owned())
}

fn first_token(line: &str, prefix: &str) -> Option<String> {
    line.strip_prefix(prefix)?
        .split_whitespace()
        .next()
        .map(str::to_owned)
}

fn decode_tmux_value(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut cursor = 0;
    while cursor < bytes.len() {
        if bytes[cursor] == b'\\' && cursor + 3 < bytes.len() {
            let octal = &bytes[cursor + 1..cursor + 4];
            if octal.iter().all(|byte| (b'0'..=b'7').contains(byte)) {
                decoded.push((octal[0] - b'0') * 64 + (octal[1] - b'0') * 8 + octal[2] - b'0');
                cursor += 4;
                continue;
            }
        }
        decoded.push(bytes[cursor]);
        cursor += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

pub(crate) fn initial_subscription_commands(session_id: &str) -> Vec<String> {
    let mut commands: Vec<String> = [
        ("pane-title", "%*", "#{pane_title}"),
        ("pane-command", "%*", "#{pane_current_command}"),
        ("pane-active", "%*", "#{pane_active}"),
        ("pane-dead", "%*", "#{pane_dead}"),
        ("window-active", "@*", "#{window_active}"),
        ("window-activity", "@*", "#{window_activity}"),
        ("window-bell", "@*", "#{window_bell_flag}"),
        ("agent-events", "%*", "#{@polar_station_agent_events_v1}"),
    ]
    .into_iter()
    // The Control Mode stdin is parsed as tmux command language. The format
    // marker begins with '#', which is a comment introducer unless the entire
    // -B argument is quoted.
    .map(|(name, scope, format)| format!("refresh-client -B '{name}:{scope}:{format}'"))
    .collect();
    // Native session-scoped subscription; Control Mode clients never count as terminals.
    let count = "#{n:#{L:#{?#{&&:#{==:#{client_session},#{session_name}},#{==:#{client_control_mode},0}},1,}}}";
    let condition = format!("#{{==:#{{session_id}},{session_id}}}");
    commands.push(format!(
        "refresh-client -B 'attached-clients::#{{?{condition},{count},0}}'"
    ));
    commands
}

/// The same native formats across all tmux Sessions, using one Control Mode client.
/// Values use tmux's q modifier so field/record separators cannot change identity.
pub fn target_subscription_commands() -> Vec<String> {
    let mut commands = Vec::new();
    for (name, field, pane) in [
        ("pane-title", "pane_title", true),
        ("pane-command", "pane_current_command", true),
        ("pane-active", "pane_active", true),
        ("pane-dead", "pane_dead", true),
        ("window-active", "window_active", false),
        ("window-activity", "window_activity", false),
        ("window-bell", "window_bell_flag", false),
        ("agent-events", "@polar_station_agent_events_v1", true),
    ] {
        let format = if name == "window-active" {
            "#{S:#{W:#{session_id}|#{window_id}|#{window_index}|-|#{window_active}:#{window_width}:#{window_height};}}".to_owned()
        } else if pane {
            [
                "#{S:#{W:#{P:#{session_id}|#{window_id}|#{window_index}|#{pane_id}|#{q:",
                field,
                "};}}}",
            ]
            .concat()
        } else {
            [
                "#{S:#{W:#{session_id}|#{window_id}|#{window_index}|-|#{q:",
                field,
                "};}}",
            ]
            .concat()
        };
        commands.push(format!("refresh-client -B 'target-{name}::{format}'"));
    }
    // L changes the client context but inherits the outer Session. Read the
    // client's actual Session name and join with IDs from this same expansion.
    // Two linear loops avoid a nested Session-by-client scan.
    commands.push("refresh-client -B 'target-attached-clients::#{S:s|#{session_id}|#{q:session_name};}#{L:#{?client_session,c|#{q:client_session}|#{client_control_mode};,}}'".into());
    commands
}

pub fn expand_target_subscription(
    event: TmuxControlEvent,
) -> Result<Vec<TmuxControlEvent>, ControlModeDecodeError> {
    let TmuxControlEvent::SubscriptionChanged {
        ref name,
        ref value,
        ..
    } = event
    else {
        return Ok(vec![event]);
    };
    let Some(name) = name.strip_prefix("target-") else {
        return Ok(vec![event]);
    };
    let rows = split_format_records(value)?;
    let mut expanded = Vec::new();
    if name == "attached-clients" {
        let mut sessions = std::collections::BTreeMap::new();
        let mut counts = std::collections::BTreeMap::new();
        for fields in &rows {
            match fields.as_slice() {
                [kind, id, name]
                    if kind == "s" && valid_prefixed_id(id, '$') && !name.is_empty() =>
                {
                    if sessions.insert(name.as_str(), id.as_str()).is_some()
                        || counts.insert(id.as_str(), 0_u32).is_some()
                    {
                        return Err(ControlModeDecodeError::MalformedSubscription);
                    }
                }
                [kind, name, mode]
                    if kind == "c" && !name.is_empty() && matches!(mode.as_str(), "0" | "1") => {}
                _ => return Err(ControlModeDecodeError::MalformedSubscription),
            }
        }
        for fields in &rows {
            if fields[0] == "c" {
                let id = sessions
                    .get(fields[1].as_str())
                    .ok_or(ControlModeDecodeError::MalformedSubscription)?;
                *counts.get_mut(id).expect("Session row initialized count") +=
                    u32::from(fields[2] == "0");
            }
        }
        for (session_id, count) in counts {
            expanded.push(TmuxControlEvent::SubscriptionChanged {
                name: name.into(),
                session_id: session_id.into(),
                window_id: "-".into(),
                window_index: 0,
                pane_id: "-".into(),
                value: count.to_string(),
            });
        }
        return Ok(expanded);
    }
    for mut fields in rows {
        if fields.len() != 5
            || !valid_prefixed_id(&fields[0], '$')
            || !valid_prefixed_id(&fields[1], '@')
            || !(fields[3] == "-" || valid_prefixed_id(&fields[3], '%'))
        {
            return Err(ControlModeDecodeError::MalformedSubscription);
        }
        if name == "window-active" && fields[4].contains(':') {
            let geometry: Vec<_> = fields[4].split(':').collect();
            if geometry.len() != 3 {
                return Err(ControlModeDecodeError::MalformedSubscription);
            }
            expanded.push(TmuxControlEvent::WindowGeometry {
                window_id: fields[1].clone(),
                columns: geometry[1]
                    .parse()
                    .map_err(|_| ControlModeDecodeError::MalformedSubscription)?,
                rows: geometry[2]
                    .parse()
                    .map_err(|_| ControlModeDecodeError::MalformedSubscription)?,
            });
            fields[4] = geometry[0].to_owned();
        }
        expanded.push(TmuxControlEvent::SubscriptionChanged {
            name: name.into(),
            session_id: fields[0].clone(),
            window_id: fields[1].clone(),
            window_index: fields[2]
                .parse()
                .map_err(|_| ControlModeDecodeError::MalformedSubscription)?,
            pane_id: fields[3].clone(),
            value: fields[4].clone(),
        });
    }
    Ok(expanded)
}

fn split_format_records(value: &str) -> Result<Vec<Vec<String>>, ControlModeDecodeError> {
    let mut rows = vec![];
    let mut fields = vec![];
    let mut field = String::new();
    let mut escaped = false;
    for ch in value.chars() {
        if escaped {
            field.push(ch);
            escaped = false;
        } else {
            match ch {
                '\\' => escaped = true,
                '|' => fields.push(std::mem::take(&mut field)),
                ';' => {
                    fields.push(std::mem::take(&mut field));
                    rows.push(std::mem::take(&mut fields));
                }
                _ => field.push(ch),
            }
        }
    }
    if escaped || !fields.is_empty() || !field.is_empty() {
        return Err(ControlModeDecodeError::MalformedSubscription);
    }
    Ok(rows)
}





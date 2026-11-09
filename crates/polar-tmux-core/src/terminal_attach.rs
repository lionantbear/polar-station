//! Native tmux primitives for a Host-owned, initially passive terminal.

use polar_tmux_types::{TmuxAttachPlan, REMOTE_TMUX_PATH_ASSIGNMENT};

use crate::{
    target::{quote_posix, remote_tmux_command},
    TmuxClient, TmuxError, TmuxResult, TmuxTarget,
};

/// Private post-attach receipt, never a frontend-provided client selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmuxTerminalClient {
    pub pid: u32,
    pub name: String,
}

impl TmuxTerminalClient {
    pub fn validate(&self) -> TmuxResult<()> {
        if self.pid == 0
            || self.name.len() > 256
            || !self.name.starts_with("/dev/")
            || !self
                .name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"/_.-".contains(&c))
        {
            return Err(TmuxError::invalid_input("invalid terminal client receipt"));
        }
        Ok(())
    }
}

/// Private child-process plan, never sent to a frontend. Native tmux reports
/// exact client PID after tmux initializes the tty; Host strips that line.
/// This identifies the exact client even when OpenSSH allocates a remote PTY.
pub struct TmuxTerminalAttachPlan {
    pub program: String,
    pub args: Vec<String>,
    pub identity_prefix: String,
    pub client: TmuxClient,
    pub session_id: String,
}

/// Verified Session identity from the shared target observation. The private
/// attach command checks it inside tmux before attaching, with no preflight I/O.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmuxSessionBinding {
    pub session_id: String,
    pub server_generation: String,
    pub created_unix_seconds: i64,
    pub managed_marker: Option<String>,
}

impl TmuxSessionBinding {
    pub(crate) fn condition(&self) -> TmuxResult<String> {
        crate::validate_stable_session_id(&self.session_id)?;
        let parts: Vec<_> = self.server_generation.split('\t').collect();
        if parts.len() != 3
            || parts[0].parse::<u64>().is_err()
            || parts[1].parse::<u64>().is_err()
            || parts[2].is_empty()
            || self.created_unix_seconds < 0
            || self.managed_marker.as_ref().is_some_and(|marker| {
                marker.is_empty()
                    || !marker
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || c == b'-')
            })
        {
            return Err(TmuxError::invalid_input(
                "invalid observed Session identity",
            ));
        }
        let clauses = [
            format!("#{{==:#{{pid}},{}}}", parts[0]),
            format!("#{{==:#{{start_time}},{}}}", parts[1]),
            format!("#{{==:#{{session_id}},{}}}", self.session_id),
            format!("#{{==:#{{session_created}},{}}}", self.created_unix_seconds),
            format!(
                "#{{==:#{{@polar_station_managed_session}},{}}}",
                self.managed_marker.as_deref().unwrap_or("")
            ),
        ];
        Ok(clauses
            .into_iter()
            .reduce(|left, right| format!("#{{&&:{left},{right}}}"))
            .unwrap())
    }
}

impl TmuxTerminalAttachPlan {
    /// Open the existing SSH/PTY channel before the shared observation finishes.
    /// The shell executes no tmux work until Host supplies the generated bound
    /// command. Readiness is private metadata, never terminal content.
    pub fn defer_until_binding(&mut self) -> TmuxResult<Vec<u8>> {
        if self.program != "ssh" || self.args.len() != 8 {
            return Err(TmuxError::invalid_input("deferred attachment requires SSH"));
        }
        let ready = format!("{}READY\n", self.identity_prefix);
        self.args[7] = format!(
            "{REMOTE_TMUX_PATH_ASSIGNMENT}; stty raw -echo || exit 1; printf '{}READY\\n'; IFS= read -r station_attach || exit 1; exec /bin/sh -c \"$station_attach\"",
            self.identity_prefix
        );
        Ok(ready.into_bytes())
    }

    /// One trusted generated command line, not frontend or terminal input.
    pub fn deferred_binding_command(&self) -> TmuxResult<Vec<u8>> {
        if self.program != "ssh" || self.args.len() != 8 || self.args[7].contains(['\n', '\r']) {
            return Err(TmuxError::invalid_input("invalid deferred binding command"));
        }
        Ok(format!("{}\n", self.args[7]).into_bytes())
    }

    pub fn new(plan: &TmuxAttachPlan, nonce: u64) -> TmuxResult<Self> {
        Self::build(plan, nonce, None)
    }

    pub fn new_bound(
        plan: &TmuxAttachPlan,
        nonce: u64,
        binding: &TmuxSessionBinding,
    ) -> TmuxResult<Self> {
        Self::build(plan, nonce, Some(binding))
    }

    fn build(
        plan: &TmuxAttachPlan,
        nonce: u64,
        binding: Option<&TmuxSessionBinding>,
    ) -> TmuxResult<Self> {
        plan.validate()
            .map_err(|_| TmuxError::invalid_input("invalid terminal attach plan"))?;
        if nonce == 0 {
            return Err(TmuxError::invalid_input("terminal identity nonce is zero"));
        }
        let (target, mut tmux_args) = if plan.program == "tmux" {
            (TmuxTarget::Local, plan.args.clone())
        } else {
            let words = plan.args[7].split(' ').collect::<Vec<_>>();
            let words = if words[0] == REMOTE_TMUX_PATH_ASSIGNMENT {
                &words[1..]
            } else {
                &words[..]
            };
            let args = words[1..]
                .iter()
                .map(|word| word.trim_matches('\'').to_owned())
                .collect();
            (
                TmuxTarget::multiplexed_ssh(&plan.args[6], &plan.args[2])?,
                args,
            )
        };
        let mut client = TmuxClient::new(target);
        if tmux_args.first().is_some_and(|arg| arg == "-L") {
            client = client.with_socket_name(&tmux_args[1]);
        }
        let session_id = plan.attached_session_id().unwrap().to_owned();
        tmux_args.extend(["-f".to_owned(), "ignore-size".to_owned()]);
        // tty_start_tty flushes pending terminal output. Emit identity only after
        // attach returns, using the command queue's now-attached exact client.
        // This is one tmux/SSH invocation, with no additional network round trip.
        let identity_prefix = format!("POLAR_STATION_CLIENT_{nonce:016x}:");
        tmux_args.extend([
            ";".into(),
            "run-shell".into(),
            "-b".into(),
            // Pin the opened tty before checking its owner. A delayed job may
            // otherwise reopen a recycled pathname and write into a new Pane.
            // The nested query runs on this server, not through another SSH hop.
            // A missing/stale owner emits no receipt. Always finish quietly:
            // tmux otherwise displays a failed run-shell job in Pane view-mode.
            // -c alone does not select the format's Session context.
            // Redirect the compound command, rather than exec: POSIX shells
            // may exit immediately when exec cannot open a vanished tty.
            format!("{REMOTE_TMUX_PATH_ASSIGNMENT}; station_tty=#{{q:client_tty}}; test -n \"$station_tty\" || exit 0; {{ test \"$(tmux -S #{{q:socket_path}} display-message -p -c \"$station_tty\" -t '{session_id}' '##{{client_pid}}' 2>/dev/null)\" = '#{{client_pid}}' && printf '{identity_prefix}%s\\t%s\\n' '#{{client_pid}}' '#{{client_tty}}' >&3; }} 2>/dev/null 3>\"$station_tty\"; exit 0"),
        ]);
        if let Some(binding) = binding {
            if binding.session_id != session_id {
                return Err(TmuxError::invalid_input(
                    "attach Session differs from observation",
                ));
            }
            let prefix = if tmux_args.first().is_some_and(|arg| arg == "-L") {
                2
            } else {
                0
            };
            let attach = tmux_args.split_off(prefix);
            // argv command separators become grammar in if-shell's command
            // string, not quoted literal arguments to attach-session.
            let quoted = attach
                .split(|arg| arg == ";")
                .map(|command| {
                    remote_tmux_command("tmux", command)
                        .strip_prefix(&format!("{REMOTE_TMUX_PATH_ASSIGNMENT} tmux "))
                        .unwrap()
                        .to_owned()
                })
                .collect::<Vec<_>>()
                .join(" ; ");
            let attach_command = quoted;
            tmux_args.extend([
                "-N".into(),
                "if-shell".into(),
                "-F".into(),
                "-t".into(),
                session_id.clone(),
                binding.condition()?,
                attach_command,
            ]);
        }
        let command = format!(
            "{REMOTE_TMUX_PATH_ASSIGNMENT}; exec {}",
            remote_tmux_command("tmux", &tmux_args)
                .strip_prefix(&format!("{REMOTE_TMUX_PATH_ASSIGNMENT} "))
                .unwrap()
        );
        let (program, args) = if plan.program == "tmux" {
            ("/bin/sh".to_owned(), vec!["-c".to_owned(), command])
        } else {
            let mut args = plan.args[..7].to_vec();
            args.push(command);
            ("ssh".to_owned(), args)
        };
        Ok(Self {
            program,
            args,
            identity_prefix,
            client,
            session_id,
        })
    }
}

impl TmuxClient {
    /// One native command checks the exact live client and activates its size.
    /// The receipt already supplies its name, so no name-discovery RTT is needed.
    pub async fn activate_terminal_size(
        &self,
        client: &TmuxTerminalClient,
        session: &str,
    ) -> TmuxResult<()> {
        self.claim_terminal_size(client, session, true)
            .await
            .map(|_| ())
    }

    /// One shared-channel batch; the returned snapshot is an observation, not
    /// an assertion that asynchronous SIGWINCH processing has completed.
    pub async fn claim_terminal_size(
        &self,
        client: &TmuxTerminalClient,
        session: &str,
        enable: bool,
    ) -> TmuxResult<polar_tmux_types::TerminalGeometryV1> {
        self.claim_terminal_size_guarded(client, session, enable, None, None)
            .await
    }

    pub async fn claim_terminal_size_guarded(
        &self,
        client: &TmuxTerminalClient,
        session: &str,
        enable: bool,
        epoch: Option<crate::TmuxControlEpoch>,
        binding: Option<&TmuxSessionBinding>,
    ) -> TmuxResult<polar_tmux_types::TerminalGeometryV1> {
        crate::validate_stable_session_id(session)?;
        client.validate()?;
        let matches = format!("#{{&&:#{{==:#{{client_pid}},{}}},#{{&&:#{{==:#{{client_name}},{}}},#{{==:#{{client_session}},#{{session_name}}}}}}}}", client.pid, client.name);
        let condition = format!("#{{==:#{{L:#{{?{matches},1,}}}},1}}");
        let condition = if let Some(binding) = binding {
            if binding.session_id != session {
                return Err(TmuxError::invalid_input(
                    "terminal claim Session differs from binding",
                ));
            }
            format!("#{{&&:{condition},{}}}", binding.condition()?)
        } else {
            condition
        };
        let eligibility = if enable {
            tracing::info!(target: "polar_station_operation", component="Tmux", event="terminal_size_eligibility_planned");
            format!("refresh-client -t '{}' -f '!ignore-size' ; ", client.name)
        } else {
            String::new()
        };
        let geometry = format!("display-message -p -c '{}' -t '{}' '#{{window_id}}\t#{{window_width}}\t#{{window_height}}\t#{{client_width}}\t#{{client_height}}\t#{{status}}\t#{{window-size}}'", client.name, session);
        // if-shell consumes the job's exit status without displaying a failed
        // run-shell command in a Pane. Geometry follows a successful signal.
        let notify = format!(
            "{eligibility}if-shell {} {} {}",
            quote_posix(&format!("kill -s WINCH {} 2>/dev/null", client.pid)),
            quote_posix(&geometry),
            quote_posix("display-message -p POLAR_TERMINAL_SIZE_SIGNAL_FAILED"),
        );
        tracing::info!(target: "polar_station_operation", component="Tmux", event="terminal_size_refresh_execute");
        tracing::info!(target: "polar_station_operation", component="Tmux", event="terminal_size_winch_planned", child_pid=client.pid);
        let output = self
            .run_checked_owned_guarded(
                vec![
                    "if-shell".into(),
                    "-F".into(),
                    "-t".into(),
                    session.into(),
                    condition,
                    notify,
                    "display-message -p POLAR_TERMINAL_CLIENT_ABSENT".into(),
                ],
                epoch,
            )
            .await?;
        if output.trim() == "POLAR_TERMINAL_CLIENT_ABSENT" {
            return Err(TmuxError::target_not_found(
                "terminal client is no longer attached",
            ));
        }
        if output.trim() == "POLAR_TERMINAL_SIZE_SIGNAL_FAILED" {
            return Err(TmuxError::command_failed(
                "terminal size notification failed",
            ));
        }
        tracing::info!(target: "polar_station_operation", component="Tmux", event="terminal_size_winch_notified", child_pid=client.pid);
        tracing::info!(target: "polar_station_operation", component="Tmux", event="terminal_size_snapshot_received");
        parse_terminal_geometry(output.trim())
    }

    /// Remove only this exact client's participation before resizing a passive
    /// viewport. The existing control writer checks the attachment-local epoch.
    pub async fn release_terminal_size_guarded(
        &self,
        client: &TmuxTerminalClient,
        session: &str,
        epoch: crate::TmuxControlEpoch,
        binding: Option<&TmuxSessionBinding>,
    ) -> TmuxResult<()> {
        crate::validate_stable_session_id(session)?;
        client.validate()?;
        let matches = format!("#{{&&:#{{==:#{{client_pid}},{}}},#{{&&:#{{==:#{{client_name}},{}}},#{{==:#{{client_session}},#{{session_name}}}}}}}}", client.pid, client.name);
        let mut condition = format!("#{{==:#{{L:#{{?{matches},1,}}}},1}}");
        if let Some(binding) = binding {
            if binding.session_id != session {
                return Err(TmuxError::invalid_input(
                    "terminal release Session differs from binding",
                ));
            }
            condition = format!("#{{&&:{condition},{}}}", binding.condition()?);
        }
        let output = self
            .run_checked_owned_guarded(
                vec![
                    "if-shell".into(),
                    "-F".into(),
                    "-t".into(),
                    session.into(),
                    condition,
                    format!("refresh-client -t '{}' -f ignore-size", client.name),
                    "display-message -p POLAR_TERMINAL_CLIENT_ABSENT".into(),
                ],
                Some(epoch),
            )
            .await?;
        if output.trim() == "POLAR_TERMINAL_CLIENT_ABSENT" {
            return Err(TmuxError::target_not_found(
                "terminal client is no longer attached",
            ));
        }
        Ok(())
    }

    /// Subscribe before mutation so a fast native layout notification is kept.
    pub fn terminal_control_events(
        &self,
    ) -> Option<tokio::sync::broadcast::Receiver<crate::TmuxControlEvent>> {
        self.control_connection
            .as_ref()
            .map(|connection| connection.subscribe())
    }
}

fn parse_terminal_geometry(value: &str) -> TmuxResult<polar_tmux_types::TerminalGeometryV1> {
    let fields: Vec<_> = value.split('\t').collect();
    if fields.len() != 7 || !fields[0].starts_with('@') {
        return Err(TmuxError::parse_failed("invalid terminal geometry receipt"));
    }
    let number = |index: usize| {
        fields[index]
            .parse::<u16>()
            .map_err(|_| TmuxError::parse_failed("invalid terminal geometry number"))
    };
    let status_lines = match fields[5] {
        "on" => 1,
        "off" => 0,
        _ => number(5)?,
    };
    Ok(polar_tmux_types::TerminalGeometryV1 {
        window_id: fields[0].into(),
        window_columns: number(1)?,
        window_rows: number(2)?,
        client_columns: number(3)?,
        client_rows: number(4)?,
        status_lines,
        window_policy: fields[6].into(),
    })
}



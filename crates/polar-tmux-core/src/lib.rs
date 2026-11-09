//! tmux control library used by Polar Tmux native hosts and future Polar tools.
//!
//! The public API follows tmux's own object model. `TmuxTarget` only describes
//! where the tmux command is executed: locally, over SSH, or inside WSL.

mod content_channel;
mod terminal_feedback;
pub use terminal_feedback::terminal_feedback;
mod control_connection;
mod control_mode;
pub use control_connection::{TmuxControlConnection, TmuxControlEpoch};
mod error;
mod parser;
mod session_observation;
mod target;
mod terminal_attach;
pub use session_observation::TmuxSessionObservation;

use std::{
    collections::HashMap,
    io::ErrorKind,
    sync::{Arc, Mutex as StdMutex, OnceLock, Weak},
    time::Duration,
};

use polar_tmux_types::{
    ResizeDirection, TmuxAttachPlan, TmuxClientInfo, TmuxPane, TmuxSession, TmuxSnapshot,
    TmuxTopologyPane, TmuxTopologySnapshot, TmuxWindow,
};
use tokio::{process::Command, sync::Mutex as AsyncMutex};
use tracing::{debug, Instrument};

pub use content_channel::{
    ContentChannel, ContentChannelAction, ContentChannelError, ContentChannelPhase,
    PendingContentTransition,
};
pub use control_mode::{
    expand_target_subscription, target_subscription_commands, ControlModeDecodeError,
    ControlModeDecoder, TmuxControlEvent, TmuxControlLifecycleEvent, TmuxControlPlan,
    CONTROL_MODE_AGENT_EVENT_OPTION,
};
pub use error::{TmuxError, TmuxErrorKind, TmuxResult};
pub use target::TmuxTarget;
pub use terminal_attach::{TmuxSessionBinding, TmuxTerminalAttachPlan, TmuxTerminalClient};

const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const SESSION_FORMAT: &str =
    "#{session_id}\t#{session_name}\t#{session_windows}\t#{session_attached}\t#{session_created}";
const WINDOW_FORMAT: &str = "#{window_id}\t#{session_id}\t#{window_index}\t#{window_name}\t#{window_active}\t#{window_panes}\t#{window_layout}";
const WINDOW_ACTIVITY_FORMAT: &str = "#{window_id}\t#{session_id}\t#{window_activity}";
const PANE_FORMAT: &str = "#{pane_id}\t#{window_id}\t#{pane_index}\t#{pane_title}\t#{pane_current_command}\t#{pane_current_path}\t#{pane_active}\t#{pane_dead}\t#{pane_pid}\t#{pane_tty}";
const TOPOLOGY_PANE_FORMAT: &str =
    "#{pane_id}\t#{window_id}\t#{pane_index}\t#{pane_title}\t#{pane_active}\t#{pane_dead}";
const MANAGED_SESSION_MARKER: &str = "@polar_station_managed_session";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TmuxServerKey {
    target: TmuxTarget,
    socket_name: Option<String>,
}

type MutationGate = AsyncMutex<()>;
type MutationGateRegistry = HashMap<TmuxServerKey, Weak<MutationGate>>;

static MUTATION_GATES: OnceLock<StdMutex<MutationGateRegistry>> = OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TmuxAvailability {
    Available { version: String },
    ExecutableMissing { executable: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmuxWindowActivity {
    pub window_id: String,
    pub session_id: String,
    pub activity_epoch: u64,
}

#[derive(Debug, Clone)]
pub struct TmuxClient {
    target: TmuxTarget,
    socket_name: Option<String>,
    tmux_executable: String,
    command_timeout: Duration,
    control_connection: Option<TmuxControlConnection>,
}

impl TmuxClient {
    /// Identity of this server incarnation, not of the client or SSH transport.
    /// Querying an absent server never starts one.
    pub async fn server_generation(&self) -> TmuxResult<Option<String>> {
        match self
            .run_checked_owned(vec![
                "display-message".into(),
                "-p".into(),
                "#{pid}\t#{start_time}\t#{socket_path}".into(),
            ])
            .await
        {
            Ok(value) => {
                let value = value.trim();
                let parts: Vec<_> = value.split('\t').collect();
                if parts.len() != 3
                    || parts[0].parse::<u64>().is_err()
                    || parts[1].parse::<u64>().is_err()
                    || parts[2].is_empty()
                {
                    return Err(TmuxError::parse_failed("invalid tmux server identity"));
                }
                Ok(Some(value.to_owned()))
            }
            Err(error) if error.kind() == TmuxErrorKind::NoServer => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub fn local() -> Self {
        Self {
            target: TmuxTarget::Local,
            socket_name: None,
            tmux_executable: "tmux".to_string(),
            command_timeout: DEFAULT_COMMAND_TIMEOUT,
            control_connection: None,
        }
    }

    pub fn new(target: TmuxTarget) -> Self {
        Self {
            target,
            socket_name: None,
            tmux_executable: "tmux".to_string(),
            command_timeout: DEFAULT_COMMAND_TIMEOUT,
            control_connection: None,
        }
    }

    pub fn with_socket_name(mut self, socket_name: impl Into<String>) -> Self {
        self.control_connection = None;
        self.socket_name = Some(socket_name.into());
        self
    }

    pub fn target(&self) -> &TmuxTarget {
        &self.target
    }

    pub fn with_control_connection(mut self, connection: TmuxControlConnection) -> Self {
        self.control_connection = Some(connection);
        self
    }

    pub fn with_tmux_executable(mut self, executable: impl Into<String>) -> Self {
        self.control_connection = None;
        self.tmux_executable = executable.into();
        self
    }

    pub fn with_command_timeout(mut self, timeout: Duration) -> Self {
        self.command_timeout = timeout;
        self
    }

    /// Build a read-only, size-independent, no-output Control Mode attachment
    /// for one stable tmux Session. The returned plan contains only tmux
    /// subscriptions; process lifetime and Agent semantics remain Host-owned.
    pub fn control_mode_plan(&self, session_id: &str) -> TmuxResult<TmuxControlPlan> {
        self.control_mode_plan_inner(session_id, None, false)
    }

    pub fn control_mode_plan_bound(
        &self,
        binding: &TmuxSessionBinding,
    ) -> TmuxResult<TmuxControlPlan> {
        self.control_mode_plan_inner(&binding.session_id, Some(binding), false)
    }

    /// Target-wide subscriptions are executed in the initial guarded command.
    pub fn target_control_mode_plan(
        &self,
        session_id: &str,
        binding: Option<&TmuxSessionBinding>,
    ) -> TmuxResult<TmuxControlPlan> {
        if binding.is_some_and(|binding| binding.session_id != session_id) {
            return Err(TmuxError::invalid_input(
                "control Session differs from binding",
            ));
        }
        self.control_mode_plan_inner(session_id, binding, true)
    }

    fn control_mode_plan_inner(
        &self,
        session_id: &str,
        binding: Option<&TmuxSessionBinding>,
        bootstrap_target: bool,
    ) -> TmuxResult<TmuxControlPlan> {
        validate_stable_session_id(session_id)?;
        let mut tmux_args = Vec::new();
        if let Some(socket_name) = &self.socket_name {
            tmux_args.extend(["-L".to_owned(), socket_name.clone()]);
        }
        // A remote monitor needs SSH's terminal hangup boundary. Double -C
        // puts that private terminal in raw, no-echo Control Mode.
        let control_flag = if matches!(
            &self.target,
            TmuxTarget::Ssh { .. } | TmuxTarget::MultiplexedSsh { .. }
        ) {
            "-CC"
        } else {
            "-C"
        };
        tmux_args.extend([
            control_flag.to_owned(),
            "attach-session".to_owned(),
            "-E".to_owned(),
            "-f".to_owned(),
            "read-only,ignore-size,no-output,active-pane,no-detach-on-destroy".to_owned(),
            "-t".to_owned(),
            session_id.to_owned(),
        ]);
        if bootstrap_target {
            let command = format!(
                "{} ; display-message -p {}",
                target_subscription_commands().join(" ; "),
                control_connection::BOOTSTRAP_MARKER
            );
            tmux_args.extend([
                ";".into(),
                "if-shell".into(),
                "-F".into(),
                "1".into(),
                command,
            ]);
        }
        if let Some(binding) = binding {
            let prefix = if self.socket_name.is_some() { 3 } else { 1 };
            let attach = tmux_args.split_off(prefix);
            let command = attach
                .split(|arg| arg == ";")
                .map(|args| {
                    let command = target::remote_tmux_command("tmux", args);
                    command
                        .strip_prefix(&format!(
                            "{} tmux ",
                            polar_tmux_types::REMOTE_TMUX_PATH_ASSIGNMENT
                        ))
                        .unwrap()
                        .to_owned()
                })
                .collect::<Vec<_>>()
                .join(" ; ");
            tmux_args.extend([
                "-N".into(),
                "if-shell".into(),
                "-F".into(),
                "-t".into(),
                session_id.into(),
                binding.condition()?,
                command,
            ]);
        }
        self.target.control_mode_plan(
            &self.tmux_executable,
            &tmux_args,
            if bootstrap_target {
                Vec::new()
            } else {
                control_mode::initial_subscription_commands(session_id)
            },
        )
    }

    /// Preserve the tmux socket, executable, and timeout policy while routing
    /// the next operation through a newly verified execution target.
    pub fn with_target(mut self, target: TmuxTarget) -> Self {
        self.control_connection = None;
        self.target = target;
        self
    }

    pub async fn availability(&self) -> TmuxResult<TmuxAvailability> {
        match self.version().await {
            Ok(version) => Ok(TmuxAvailability::Available { version }),
            Err(error) if error.kind() == TmuxErrorKind::ExecutableMissing => {
                Ok(TmuxAvailability::ExecutableMissing {
                    executable: self.tmux_executable.clone(),
                })
            }
            Err(error) => Err(error),
        }
    }

    pub async fn version(&self) -> TmuxResult<String> {
        if self.control_connection.is_some() {
            return self
                .run_checked(["display-message", "-p", "#{version}"])
                .await
                .map(|version| format!("tmux {}", version.trim()));
        }
        self.run_checked(["-V"])
            .await
            .map(|version| version.trim().to_string())
    }

    pub async fn list_sessions(&self) -> TmuxResult<Vec<TmuxSession>> {
        let output = self
            .run_listing(["list-sessions", "-F", SESSION_FORMAT])
            .await?;
        parser::parse_sessions(&output)
    }

    /// Read Session identity and its recovery marker in the same tmux command.
    pub async fn list_sessions_with_managed_markers(
        &self,
    ) -> TmuxResult<Vec<(TmuxSession, Option<String>)>> {
        let format = format!("{SESSION_FORMAT}\t#{{{MANAGED_SESSION_MARKER}}}");
        let output = self.run_listing(["list-sessions", "-F", &format]).await?;
        output
            .lines()
            .filter(|line| !line.is_empty())
            .map(|line| {
                let (session_row, marker) = line
                    .rsplit_once('\t')
                    .ok_or_else(|| TmuxError::parse_failed("Session marker field is missing"))?;
                let session = parser::parse_sessions(session_row)?
                    .pop()
                    .ok_or_else(|| TmuxError::parse_failed("Session row is missing"))?;
                let marker = Some(marker.trim().to_owned()).filter(|marker| !marker.is_empty());
                Ok((session, marker))
            })
            .collect()
    }

    pub async fn list_windows(&self, session: &str) -> TmuxResult<Vec<TmuxWindow>> {
        let output = self
            .run_listing(["list-windows", "-t", session, "-F", WINDOW_FORMAT])
            .await?;
        parser::parse_windows(&output)
    }

    /// Return tmux's activity epoch for every window in one server query.
    /// The Host uses this cheap signal to decide when a quiet Pane may be
    /// captured; no terminal text crosses this API.
    pub async fn list_window_activity(&self) -> TmuxResult<Vec<TmuxWindowActivity>> {
        let output = self
            .run_listing(["list-windows", "-a", "-F", WINDOW_ACTIVITY_FORMAT])
            .await?;
        parser::parse_window_activity(&output)
    }

    pub async fn list_panes(&self, target: &str) -> TmuxResult<Vec<TmuxPane>> {
        let output = self
            .run_listing(["list-panes", "-t", target, "-F", PANE_FORMAT])
            .await?;
        parser::parse_panes(&output)
    }

    /// List pane topology without asking tmux for process, path, PID, or TTY
    /// fields.
    pub async fn list_topology_panes(&self, target: &str) -> TmuxResult<Vec<TmuxTopologyPane>> {
        let output = self
            .run_listing(["list-panes", "-t", target, "-F", TOPOLOGY_PANE_FORMAT])
            .await?;
        parser::parse_topology_panes(&output)
    }

    pub async fn list_clients(&self) -> TmuxResult<Vec<TmuxClientInfo>> {
        let format = "#{client_name}\t#{client_tty}\t#{session_id}\t#{client_pid}\t#{client_created}\t#{client_last_session}\t#{client_readonly}\t#{client_control_mode}";
        let output = self.run_listing(["list-clients", "-F", format]).await?;
        parser::parse_clients(&output)
    }

    /// Collect topology without querying tmux clients.
    ///
    /// Privacy-preserving desktop and Companion paths use this method so
    /// client/TTY/process facts are never collected merely to be discarded.
    pub async fn topology_snapshot(&self) -> TmuxResult<TmuxTopologySnapshot> {
        let sessions = self.list_sessions().await?;
        let mut windows = Vec::new();
        let mut panes = Vec::new();

        for session in &sessions {
            let session_windows = self.list_windows(&session.id).await?;
            for window in &session_windows {
                panes.extend(self.list_topology_panes(&window.id).await?);
            }
            windows.extend(session_windows);
        }

        Ok(TmuxTopologySnapshot {
            sessions,
            windows,
            panes,
        })
    }

    pub async fn snapshot(&self) -> TmuxResult<TmuxSnapshot> {
        let sessions = self.list_sessions().await?;
        let mut windows = Vec::new();
        let mut panes = Vec::new();
        for session in &sessions {
            let session_windows = self.list_windows(&session.id).await?;
            for window in &session_windows {
                panes.extend(self.list_panes(&window.id).await?);
            }
            windows.extend(session_windows);
        }
        let clients = self.list_clients().await?;

        Ok(TmuxSnapshot {
            sessions,
            windows,
            panes,
            clients,
        })
    }

    pub async fn create_session(
        &self,
        name: &str,
        working_directory: Option<&str>,
        command: Option<&str>,
    ) -> TmuxResult<TmuxSession> {
        self.create_session_with_marker(name, working_directory, command, None)
            .await
    }

    pub async fn create_managed_session(
        &self,
        name: &str,
        working_directory: Option<&str>,
        command: Option<&str>,
        marker: &str,
    ) -> TmuxResult<TmuxSession> {
        validate_target(marker, "Session marker")?;
        self.create_session_with_marker(name, working_directory, command, Some(marker))
            .await
    }

    /// Resolve a public Agent session name and allocate its terminal in
    /// one native tmux command queue. This serializes independent controllers
    /// on the execution host, including clients reached through different SSH aliases.
    pub async fn create_unique_managed_session(
        &self, name: &str, working_directory: Option<&str>, command: Option<&str>,
        marker: &str,
    ) -> TmuxResult<TmuxSession> {
        self.ensure_mutation_target()?;
        validate_session_name(name)?;
        validate_target(marker, "Session marker")?;
        if !name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') {
            return Err(TmuxError::invalid_input("invalid public Agent terminal name"));
        }
        if command.is_some_and(|v|v.trim().is_empty() || v.contains('\0'))
            || working_directory.is_some_and(|v|v.contains('\0')) {
            return Err(TmuxError::invalid_input("invalid launch arguments"));
        }
        // tmux parses this branch itself: quote its grammar, never shell-expand
        // a title, path or command. No filesystem lock or observer-local index.
        let quote = |value: &str| format!("'{}'", value.replace('\\', "\\\\").replace('\'', "'\\''"));
        let ids = format!("#{{S:#{{?#{{==:#{{session_name}},{name}}},#{{session_id}},}}}}");
        let filter = format!("#{{==:#{{session_name}},{name}}}");
        let destination = quote(&format!("={name}:"));
        let mut create = format!("new-session -d -P -F {} -s {}", quote(SESSION_FORMAT), quote(name));
        if let Some(path) = working_directory { create.push_str(&format!(" -c {}", quote(path))); }
        if let Some(command) = command { create.push_str(&format!(" {}", quote(command))); }
        create.push_str(&format!(" ; set-option -t {destination} {MANAGED_SESSION_MARKER} {} ; set-option -w -t {destination} remain-on-exit on", quote(marker)));
        let output = self.run_checked_batch(vec![vec!["start-server".into()], vec!["if-shell".into(), "-F".into(),
            format!("#{{!=:{ids},}}"), format!("list-sessions -F {} -f {}", quote(SESSION_FORMAT), quote(&filter)), create]]).await?;
        take_exactly_one(parser::parse_sessions(output.trim())?, "original terminal; select which duplicate to retain")
    }

    async fn create_session_with_marker(
        &self,
        name: &str,
        working_directory: Option<&str>,
        command: Option<&str>,
        marker: Option<&str>,
    ) -> TmuxResult<TmuxSession> {
        self.ensure_mutation_target()?;
        validate_session_name(name)?;
        if command.is_some_and(|value| value.trim().is_empty() || value.contains('\0')) {
            return Err(TmuxError::invalid_input("invalid Session command"));
        }
        let gate = self.mutation_gate();
        let _guard = gate.lock().await;
        let mut args = vec![
            "new-session".to_string(),
            "-d".to_string(),
            "-P".to_string(),
            "-F".to_string(),
            SESSION_FORMAT.to_string(),
            "-s".to_string(),
            name.to_string(),
        ];
        if let Some(path) = working_directory {
            args.extend(["-c".to_string(), path.to_string()]);
        }
        let output = self.run_checked_owned(args).await?;
        let session = take_exactly_one(parser::parse_sessions(&output)?, "created session")?;
        if let Some(marker) = marker {
            self.run_checked_owned(vec![
                "set-option".into(),
                "-t".into(),
                session.id.clone(),
                MANAGED_SESSION_MARKER.into(),
                marker.into(),
            ])
            .await?;
        }
        self.run_checked_owned(vec![
            "set-option".to_owned(),
            "-w".to_owned(),
            "-t".to_owned(),
            format!("{}:", session.id),
            "remain-on-exit".to_owned(),
            "on".to_owned(),
        ])
        .await?;
        if let Some(command) = command {
            let mut args = vec!["respawn-pane".to_owned(), "-k".to_owned(), "-t".to_owned(), format!("{}:0.0", session.id)];
            if let Some(path) = working_directory { args.extend(["-c".to_owned(), path.to_owned()]); }
            args.push(command.to_owned());
            self.run_checked_owned(args).await?;
        }
        Ok(session)
    }

    pub async fn respawn_pane(&self, target: &str, command: &str) -> TmuxResult<()> {
        self.ensure_mutation_target()?;
        validate_target(target, "pane target")?;
        if command.trim().is_empty() || command.chars().any(char::is_control) {
            return Err(TmuxError::invalid_input("pane respawn command is invalid"));
        }
        let gate = self.mutation_gate();
        let _guard = gate.lock().await;
        self.run_checked_owned(vec![
            "respawn-pane".to_string(),
            "-k".to_string(),
            "-t".to_string(),
            target.to_string(),
            command.to_string(),
        ])
        .await?;
        Ok(())
    }

    pub async fn set_managed_session_marker(&self, session: &str, marker: &str) -> TmuxResult<()> {
        self.ensure_mutation_target()?;
        validate_target(session, "session target")?;
        if marker.trim().is_empty() || marker.chars().any(char::is_control) {
            return Err(TmuxError::invalid_input(
                "managed session marker is invalid",
            ));
        }
        let gate = self.mutation_gate();
        let _guard = gate.lock().await;
        self.run_checked_owned(vec![
            "set-option".to_string(),
            "-t".to_string(),
            session.to_string(),
            MANAGED_SESSION_MARKER.to_string(),
            marker.to_string(),
        ])
        .await?;
        Ok(())
    }

    pub async fn managed_session_marker(&self, session: &str) -> TmuxResult<Option<String>> {
        validate_target(session, "session target")?;
        match self
            .run_checked_owned(vec![
                "show-options".to_string(),
                "-v".to_string(),
                "-t".to_string(),
                session.to_string(),
                MANAGED_SESSION_MARKER.to_string(),
            ])
            .await
        {
            Ok(value) => Ok(Some(value.trim().to_string()).filter(|value| !value.is_empty())),
            Err(error) if error.kind() == TmuxErrorKind::CommandFailed => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// End only the original observed Session inside tmux's command queue.
    pub async fn kill_bound_session(&self, binding: &TmuxSessionBinding) -> TmuxResult<()> {
        self.ensure_mutation_target()?;
        let gate = self.mutation_gate();
        let _guard = gate.lock().await;
        let output = self.run_checked_owned(vec!["if-shell".into(), "-F".into(), "-t".into(),
            binding.session_id.clone(), binding.condition()?,
            format!("kill-session -t '{}'", binding.session_id),
            "display-message -p POLAR_SESSION_IDENTITY_CHANGED".into()]).await?;
        if output.trim() == "POLAR_SESSION_IDENTITY_CHANGED" {
            return Err(TmuxError::target_not_found("The original Session identity changed; no Session was killed"));
        }
        Ok(())
    }

    /// Retire an exited pane only while both its Session identity and death
    /// still hold in tmux's native command queue. A respawn is never killed.
    pub async fn kill_bound_dead_pane(&self, binding: &TmuxSessionBinding, pane: &str) -> TmuxResult<()> {
        self.ensure_mutation_target()?;
        if pane.strip_prefix('%').and_then(|id| id.parse::<u64>().ok()).is_none() {
            return Err(TmuxError::invalid_input("invalid pane identity"));
        }
        let condition = format!("#{{&&:{},#{{pane_dead}}}}", binding.condition()?);
        let gate = self.mutation_gate();
        let _guard = gate.lock().await;
        self.run_checked_owned(vec!["if-shell".into(), "-F".into(), "-t".into(), pane.into(),
            condition, format!("kill-pane -t '{pane}'"), "".into()]).await?;
        Ok(())
    }

    pub async fn kill_session(&self, target: &str) -> TmuxResult<()> {
        self.ensure_mutation_target()?;
        validate_target(target, "session target")?;
        let gate = self.mutation_gate();
        let _guard = gate.lock().await;
        self.run_idempotent_kill(["kill-session", "-t", target])
            .await
    }

    pub async fn rename_session(&self, target: &str, name: &str) -> TmuxResult<TmuxSession> {
        self.ensure_mutation_target()?;
        validate_target(target, "session target")?;
        validate_session_name(name)?;
        let gate = self.mutation_gate();
        let _guard = gate.lock().await;
        self.run_checked(["rename-session", "-t", target, name])
            .await?;
        let output = self
            .run_checked(["display-message", "-p", "-t", target, "-F", SESSION_FORMAT])
            .await?;
        take_exactly_one(parser::parse_sessions(&output)?, "renamed session")
    }

    pub async fn create_window(
        &self,
        session: &str,
        name: Option<&str>,
        working_directory: Option<&str>,
        command: Option<&str>,
    ) -> TmuxResult<TmuxWindow> {
        self.ensure_mutation_target()?;
        validate_target(session, "session target")?;
        if let Some(name) = name {
            validate_object_name(name, "window name")?;
        }
        let gate = self.mutation_gate();
        let _guard = gate.lock().await;
        let mut args = vec![
            "new-window".to_string(),
            "-P".to_string(),
            "-F".to_string(),
            WINDOW_FORMAT.to_string(),
            "-t".to_string(),
            session.to_string(),
        ];
        if let Some(name) = name {
            args.extend(["-n".to_string(), name.to_string()]);
        }
        if let Some(path) = working_directory {
            args.extend(["-c".to_string(), path.to_string()]);
        }
        if let Some(command) = command {
            args.push(command.to_string());
        }
        let output = self.run_checked_owned(args).await?;
        take_exactly_one(parser::parse_windows(&output)?, "created window")
    }

    pub async fn kill_window(&self, target: &str) -> TmuxResult<()> {
        self.ensure_mutation_target()?;
        validate_target(target, "window target")?;
        let gate = self.mutation_gate();
        let _guard = gate.lock().await;
        self.run_idempotent_kill(["kill-window", "-t", target])
            .await
    }

    pub async fn rename_window(&self, target: &str, name: &str) -> TmuxResult<TmuxWindow> {
        self.ensure_mutation_target()?;
        validate_target(target, "window target")?;
        validate_object_name(name, "window name")?;
        let gate = self.mutation_gate();
        let _guard = gate.lock().await;
        self.run_checked(["rename-window", "-t", target, name])
            .await?;
        let output = self
            .run_checked(["display-message", "-p", "-t", target, "-F", WINDOW_FORMAT])
            .await?;
        take_exactly_one(parser::parse_windows(&output)?, "renamed window")
    }

    pub async fn split_pane(
        &self,
        target: &str,
        horizontal: bool,
        working_directory: Option<&str>,
        command: Option<&str>,
    ) -> TmuxResult<TmuxPane> {
        self.ensure_mutation_target()?;
        validate_target(target, "pane target")?;
        let gate = self.mutation_gate();
        let _guard = gate.lock().await;
        let mut args = vec![
            "split-window".to_string(),
            if horizontal { "-h" } else { "-v" }.to_string(),
            "-P".to_string(),
            "-F".to_string(),
            PANE_FORMAT.to_string(),
            "-t".to_string(),
            target.to_string(),
        ];
        if let Some(path) = working_directory {
            args.extend(["-c".to_string(), path.to_string()]);
        }
        if let Some(command) = command {
            args.push(command.to_string());
        }
        let output = self.run_checked_owned(args).await?;
        take_exactly_one(parser::parse_panes(&output)?, "created pane")
    }

    pub async fn kill_pane(&self, target: &str) -> TmuxResult<()> {
        self.ensure_mutation_target()?;
        validate_target(target, "pane target")?;
        let gate = self.mutation_gate();
        let _guard = gate.lock().await;
        self.run_idempotent_kill(["kill-pane", "-t", target]).await
    }

    /// Show an existing pane only while the original server/session/process
    /// binding is still true; display activation never writes terminal input.
    pub async fn show_bound_pane(&self, binding: &TmuxSessionBinding, pane: &TmuxPane) -> TmuxResult<bool> {
        self.ensure_mutation_target()?;
        validate_target(&pane.id,"pane target")?;
        validate_target(&pane.window_id,"window target")?;
        let condition = format!("#{{&&:{},#{{&&:#{{==:#{{pane_pid}},{}}},#{{==:#{{pane_dead}},0}}}}}}",binding.condition()?,pane.pid.unwrap_or(0));
        let commands = format!("select-window -t {} ; select-pane -t {}",crate::target::quote_posix(&pane.window_id),crate::target::quote_posix(&pane.id));
        let gate=self.mutation_gate();let _guard=gate.lock().await;
        // The shared observer uses tmux's per-client active-pane flag and may
        // belong to another Session. Cross-session selection through that
        // client can terminate tmux. Keep its context untouched and reuse the
        // same verified target transport for this guarded one-shot command.
        let mut activation=self.clone();
        activation.control_connection=None;
        let output=activation.run_checked_owned(vec!["if-shell".into(),"-F".into(),"-t".into(),pane.id.clone(),condition,commands,"display-message -p POLAR_VIEW_TARGET_CHANGED".into()]).await?;
        Ok(!output.contains("POLAR_VIEW_TARGET_CHANGED"))
    }

    pub async fn select_pane(&self, target: &str) -> TmuxResult<()> {
        self.ensure_mutation_target()?;
        validate_target(target, "pane target")?;
        let gate = self.mutation_gate();
        let _guard = gate.lock().await;
        self.run_checked(["select-pane", "-t", target]).await?;
        Ok(())
    }

    pub async fn resize_pane(
        &self,
        target: &str,
        direction: ResizeDirection,
        amount: u16,
    ) -> TmuxResult<()> {
        self.ensure_mutation_target()?;
        validate_target(target, "pane target")?;
        if amount == 0 {
            return Err(TmuxError::invalid_input(
                "pane resize amount must be greater than zero",
            ));
        }
        let gate = self.mutation_gate();
        let _guard = gate.lock().await;
        let flag = match direction {
            ResizeDirection::Left => "-L",
            ResizeDirection::Right => "-R",
            ResizeDirection::Up => "-U",
            ResizeDirection::Down => "-D",
        };
        let amount = amount.to_string();
        self.run_checked(["resize-pane", "-t", target, flag, &amount])
            .await?;
        Ok(())
    }

    pub async fn send_keys(&self, target: &str, keys: &[String]) -> TmuxResult<()> {
        self.ensure_mutation_target()?;
        validate_target(target, "pane target")?;
        if keys.is_empty() {
            return Err(TmuxError::invalid_input(
                "at least one named tmux key is required",
            ));
        }
        for key in keys {
            validate_object_name(key, "tmux key name")?;
        }
        let gate = self.mutation_gate();
        let _guard = gate.lock().await;
        let mut args = vec![
            "send-keys".to_string(),
            "-t".to_string(),
            target.to_string(),
        ];
        args.extend(keys.iter().cloned());
        self.run_checked_owned(args).await?;
        Ok(())
    }

    /// Read the pane-local input generations in one existing tmux listing.
    pub async fn pane_input_generations(&self) -> TmuxResult<HashMap<String, String>> {
        let rows = self.run_listing(["list-panes", "-a", "-F",
            "#{pane_id}\t#{@polar_station_input_generation}\t."]).await?;
        rows.lines().filter(|row| !row.is_empty()).map(|row| {
            let (pane, generation) = row.split_once('\t')
                .ok_or_else(|| TmuxError::parse_failed("invalid pane input generation"))?;
            let generation = generation.strip_suffix("\t.")
                .ok_or_else(|| TmuxError::parse_failed("invalid pane input generation"))?;
            Ok((pane.to_owned(), generation.to_owned()))
        }).collect()
    }

    /// Input only while the observed Session and live Pane incarnation still
    /// match inside tmux's command queue. No active-pane/name lookup is used.
    /// `false` confirms the identity condition rejected input. Errors can occur
    /// after a partial write and must never be classified as a safe retry.
    pub async fn send_bound_text(
        &self,
        binding: &TmuxSessionBinding,
        pane: &TmuxPane,
        expected_input_generation: &str,
        text: &str,
        enter: bool,
    ) -> TmuxResult<bool> {
        self.ensure_mutation_target()?;
        let pid = pane.pid.unwrap_or(0);
        let pane = pane.id.as_str();
        let next_generation = if expected_input_generation.is_empty() { 0 } else {
            expected_input_generation.parse::<u64>()
                .map_err(|_| TmuxError::invalid_input("invalid pane input generation"))?
        }.checked_add(1).ok_or_else(|| TmuxError::invalid_input("pane input generation exhausted"))?;
        if pane
            .strip_prefix('%')
            .and_then(|v| v.parse::<u64>().ok())
            .is_none()
            || pid == 0
            || text.contains('\0')
        {
            return Err(TmuxError::invalid_input("invalid original Pane or input"));
        }
        let condition = format!(
            "#{{&&:{},#{{&&:#{{==:#{{pane_pid}},{pid}}},#{{!:#{{pane_dead}}}}}}}}",
            binding.condition()?
        );
        let condition = format!("#{{&&:{condition},#{{==:#{{@polar_station_input_generation}},{expected_input_generation}}}}}");
        // tmux wraps the paste only when the application requests it. Keep
        // original LF bytes, avoid the clipboard, and remove the named buffer.
        let buffer = format!("polar-station-input-{pid}-{pane}");
        let mut command = format!(
            "set-option -p -t '{pane}' @polar_station_input_generation {next_generation} ; set-buffer -b '{buffer}' -- {} ; paste-buffer -d -p -r -b '{buffer}' -t '{pane}'",
            crate::target::quote_tmux_argument(text)
        );
        if enter {
            command.push_str(&format!(" ; send-keys -t '{}' Enter", pane));
        }
        let gate = self.mutation_gate();
        let _guard = gate.lock().await;
        let output = self
            .run_checked_owned(vec![
                "if-shell".into(),
                "-F".into(),
                "-t".into(),
                pane.into(),
                condition,
                command,
                "display-message -p POLAR_INPUT_IDENTITY_CHANGED".into(),
            ])
            .await?;
        if output.trim() == "POLAR_INPUT_IDENTITY_CHANGED" {
            return Ok(false);
        }
        Ok(true)
    }

    pub async fn send_text(&self, target: &str, text: &str, press_enter: bool) -> TmuxResult<()> {
        self.ensure_mutation_target()?;
        validate_target(target, "pane target")?;
        let gate = self.mutation_gate();
        let _guard = gate.lock().await;
        self.run_checked_owned(vec![
            "send-keys".to_string(),
            "-l".to_string(),
            "-t".to_string(),
            target.to_string(),
            text.to_string(),
        ])
        .await?;
        if press_enter {
            self.run_checked(["send-keys", "-t", target, "Enter"])
                .await?;
        }
        Ok(())
    }

    pub async fn switch_client(&self, client: &str, session: &str) -> TmuxResult<()> {
        self.ensure_mutation_target()?;
        validate_target(client, "client target")?;
        validate_target(session, "session target")?;
        let gate = self.mutation_gate();
        let _guard = gate.lock().await;
        self.run_checked(["switch-client", "-c", client, "-t", session])
            .await?;
        Ok(())
    }

    pub async fn capture_pane(&self, target: &str, history: bool) -> TmuxResult<String> {
        self.ensure_local_capture()?;
        validate_target(target, "pane target")?;
        let mut args = vec![
            "capture-pane".to_string(),
            "-p".to_string(),
            "-t".to_string(),
            target.to_string(),
        ];
        if history {
            args.extend(["-S".to_string(), "-".to_string()]);
        }
        self.run_checked_owned(args).await
    }

    /// Capture a bounded recent screen for the Host's ephemeral summary
    /// provider. Unlike the public local operator capture, this path can use
    /// the managed SSH target; callers must not persist or wire the text.
    pub async fn capture_pane_for_attention(
        &self,
        target: &str,
        history_lines: usize,
    ) -> TmuxResult<String> {
        validate_target(target, "pane target")?;
        let start = format!("-{}", history_lines.min(100));
        self.run_checked_owned(vec![
            "capture-pane".to_string(),
            "-p".to_string(),
            "-t".to_string(),
            target.to_string(),
            "-S".to_string(),
            start,
        ])
        .await
    }

    /// Validate a stable session ID against the selected tmux server and
    /// return a platform-ready argv plan. The returned plan is data, never a
    /// shell fragment supplied by a client.
    pub async fn prepare_attach(&self, session: &str) -> TmuxResult<TmuxAttachPlan> {
        validate_stable_session_id(session)?;
        let exists = self
            .list_sessions()
            .await?
            .into_iter()
            .any(|candidate| candidate.id == session);
        if !exists {
            return Err(TmuxError::target_not_found(
                "tmux attach session was not found",
            ));
        }

        let mut args = Vec::new();
        if let Some(socket_name) = &self.socket_name {
            args.extend(["-L".to_string(), socket_name.clone()]);
        }
        args.extend([
            "attach-session".to_string(),
            "-t".to_string(),
            session.to_string(),
        ]);
        self.target.attach_plan(&self.tmux_executable, &args)
    }

    /// Build an attach plan for a stable session identity already selected
    /// from a Host-owned topology snapshot. This performs no session listing
    /// or transport health probe; the interactive child reports any race with
    /// subsequent session or transport loss.
    pub fn prepare_attach_known_session(&self, session: &str) -> TmuxResult<TmuxAttachPlan> {
        validate_stable_session_id(session)?;
        let mut args = Vec::new();
        if let Some(socket_name) = &self.socket_name {
            args.extend(["-L".to_string(), socket_name.clone()]);
        }
        args.extend([
            "attach-session".to_string(),
            "-t".to_string(),
            session.to_string(),
        ]);
        self.target.attach_plan(&self.tmux_executable, &args)
    }

    fn ensure_mutation_target(&self) -> TmuxResult<()> {
        match self.target {
            TmuxTarget::Local | TmuxTarget::Ssh { .. } | TmuxTarget::MultiplexedSsh { .. } => {
                Ok(())
            }
            TmuxTarget::Wsl { .. } => Err(TmuxError::unsupported_target(
                "WSL tmux mutations remain deferred until the Windows frontend phase",
            )),
        }
    }

    fn ensure_local_capture(&self) -> TmuxResult<()> {
        if !matches!(self.target, TmuxTarget::Local) {
            return Err(TmuxError::unsupported_target(
                "terminal capture is available only for a local operator target",
            ));
        }
        Ok(())
    }

    fn mutation_gate(&self) -> Arc<MutationGate> {
        let key = TmuxServerKey {
            target: self.target.clone(),
            socket_name: self.socket_name.clone(),
        };
        let registry = MUTATION_GATES.get_or_init(|| StdMutex::new(HashMap::new()));
        let mut registry = registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        registry.retain(|_, gate| gate.strong_count() > 0);
        if let Some(gate) = registry.get(&key).and_then(Weak::upgrade) {
            return gate;
        }
        let gate = Arc::new(AsyncMutex::new(()));
        registry.insert(key, Arc::downgrade(&gate));
        gate
    }

    async fn run_checked<const N: usize>(&self, args: [&str; N]) -> TmuxResult<String> {
        self.run_checked_owned(args.into_iter().map(str::to_string).collect())
            .await
    }

    async fn run_listing<const N: usize>(&self, args: [&str; N]) -> TmuxResult<String> {
        match self
            .run_checked_owned(args.into_iter().map(str::to_string).collect())
            .await
        {
            Ok(output) => Ok(output),
            Err(error) if error.kind() == TmuxErrorKind::NoServer => Ok(String::new()),
            Err(error) => Err(error),
        }
    }

    async fn run_idempotent_kill<const N: usize>(&self, args: [&str; N]) -> TmuxResult<()> {
        match self.run_checked(args).await {
            Ok(_) => Ok(()),
            Err(error)
                if matches!(
                    error.kind(),
                    TmuxErrorKind::NoServer | TmuxErrorKind::TargetNotFound
                ) =>
            {
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    #[tracing::instrument(target = "polar_station_operation", name = "tmux_command", skip_all,
        fields(component = "Tmux", command = args.first().map(String::as_str).unwrap_or("none"), outcome = "cancelled"))]
    async fn run_checked_owned(&self, args: Vec<String>) -> TmuxResult<String> {
        self.run_checked_owned_guarded(args, None).await
    }

    async fn run_checked_owned_guarded(&self, args: Vec<String>, epoch: Option<TmuxControlEpoch>) -> TmuxResult<String> {
        let result = async {
            if let Some(epoch) = &epoch { if !epoch.is_current() { return Err(TmuxError::target_unavailable("terminal selection superseded")); } }
            if let Some(connection) = &self.control_connection {
                let reply = connection.command_batch_guarded(&[args], self.command_timeout, epoch).await?;
                return if reply.failed {
                    Err(self.classify_failed_message(reply.output.trim(), Some(1), "1"))
                } else {
                    Ok(reply.output)
                };
            }
            let output = self.run(args).await?;
            if !output.status.success() {
                return Err(self.classify_failed_output(&output));
            }
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        }
        .await;
        tracing::Span::current().record("outcome", if result.is_ok() { "ok" } else { "failed" });
        result
    }

    #[tracing::instrument(target = "polar_station_operation", name = "tmux_batch",
        skip_all, fields(component="Tmux", count=commands.len(),
        reason=if self.control_connection.is_some() { "control_mode" } else { "process" }))]
    async fn run_checked_batch(&self, commands: Vec<Vec<String>>) -> TmuxResult<String> {
        if let Some(connection) = &self.control_connection {
            let span = tracing::info_span!(target: "polar_station_operation", "tmux_command",
                component="Tmux", command="batch", outcome="cancelled");
            let reply = connection
                .command_batch(&commands, self.command_timeout)
                .instrument(span.clone())
                .await;
            span.record(
                "outcome",
                if matches!(&reply, Ok(reply) if !reply.failed) {
                    "ok"
                } else {
                    "failed"
                },
            );
            let reply = reply?;
            if reply.failed {
                return Err(self.classify_failed_message(reply.output.trim(), Some(1), "1"));
            }
            Ok(reply.output)
        } else {
            let mut args = vec![];
            for command in commands {
                if !args.is_empty() {
                    args.push(";".into());
                }
                args.extend(command);
            }
            self.run_checked_owned(args).await
        }
    }

    async fn run(&self, args: Vec<String>) -> TmuxResult<std::process::Output> {
        let operation = args.first().map(String::as_str).unwrap_or("<none>");
        debug!(
            target = self.target.kind(),
            operation,
            argument_count = args.len(),
            "executing tmux operation"
        );

        let mut tmux_args = Vec::new();
        if let Some(socket_name) = &self.socket_name {
            tmux_args.extend(["-L".to_string(), socket_name.clone()]);
        }
        tmux_args.extend(args);

        let mut command: Command = self.target.command(&self.tmux_executable, &tmux_args)?;
        command.kill_on_drop(true);
        tracing::info!(target: "polar_station_operation", component="Tmux", event="tmux_process_execute");
        let executing = async {
            let child = {
                let span = tracing::info_span!(target: "polar_station_operation", "tmux_process_spawn", component="Tmux");
                let _entered = span.enter();
                command
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()?
            };
            child
                .wait_with_output()
                .instrument(tracing::info_span!(
                    target: "polar_station_operation", "tmux_process_output_wait", component="Tmux"
                ))
                .await
        };
        match tokio::time::timeout(self.command_timeout, executing).await {
            Ok(Ok(output)) => Ok(output),
            Ok(Err(error)) if error.kind() == ErrorKind::NotFound => {
                if matches!(self.target, TmuxTarget::Local) {
                    Err(TmuxError::executable_missing(&self.tmux_executable))
                } else {
                    Err(TmuxError::target_unavailable(format!(
                        "{} command runner is unavailable",
                        self.target.kind()
                    )))
                }
            }
            Ok(Err(error)) => Err(TmuxError::target_unavailable(format!(
                "failed to execute {} tmux target: {error}",
                self.target.kind()
            ))),
            Err(_) => Err(TmuxError::timeout(self.command_timeout.as_millis())),
        }
    }

    fn classify_failed_output(&self, output: &std::process::Output) -> TmuxError {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let message = stderr.trim();
        self.classify_failed_message(message, output.status.code(), &output.status.to_string())
    }

    fn classify_failed_message(&self, message: &str, code: Option<i32>, status: &str) -> TmuxError {
        if matches!(self.target, TmuxTarget::MultiplexedSsh { .. }) && code == Some(255) {
            return TmuxError::ssh_master_unavailable("the Host-owned SSH master is unavailable");
        }
        if is_no_server_message(message) {
            return TmuxError::no_server(if message.is_empty() {
                "tmux server is not running".to_string()
            } else {
                message.to_string()
            });
        }
        if code == Some(1) && message.starts_with("duplicate session: ") {
            return TmuxError::session_already_exists();
        }
        if is_target_not_found_message(message) {
            return TmuxError::target_not_found(if message.is_empty() {
                "tmux target was not found".to_string()
            } else {
                message.to_string()
            });
        }
        if message.contains("tmux: not found") || message.contains("tmux: command not found") {
            return TmuxError::executable_missing(&self.tmux_executable);
        }
        if !matches!(self.target, TmuxTarget::Local) && code == Some(255) {
            return TmuxError::target_unavailable(if message.is_empty() {
                format!("{} tmux target is unavailable", self.target.kind())
            } else {
                message.to_string()
            });
        }
        TmuxError::command_failed(format!(
            "tmux operation failed with status {}{}",
            status,
            if message.is_empty() {
                String::new()
            } else {
                format!(": {message}")
            }
        ))
    }
}

fn is_no_server_message(message: &str) -> bool {
    message.contains("no server running")
        || message.contains("failed to connect to server")
        || message.contains("error connecting to ")
        || message.contains("server exited unexpectedly")
}

fn is_target_not_found_message(message: &str) -> bool {
    message.contains("can't find session")
        || message.contains("can't find window")
        || message.contains("can't find pane")
        || message.contains("can't find client")
        || message.contains("session not found")
        || message.contains("window not found")
        || message.contains("pane not found")
        || message.contains("client not found")
}

fn validate_session_name(name: &str) -> TmuxResult<()> {
    validate_object_name(name, "tmux session name")?;
    if name.trim() != name {
        return Err(TmuxError::invalid_input(
            "tmux session names cannot start or end with whitespace",
        ));
    }
    if name.contains(['.', ':']) {
        return Err(TmuxError::invalid_input(
            "tmux session names cannot contain . or :",
        ));
    }
    Ok(())
}

fn validate_object_name(value: &str, label: &str) -> TmuxResult<()> {
    if value.is_empty() {
        return Err(TmuxError::invalid_input(format!("{label} is required")));
    }
    if value.chars().any(char::is_control) {
        return Err(TmuxError::invalid_input(format!(
            "{label} cannot contain control characters"
        )));
    }
    Ok(())
}

fn validate_target(value: &str, label: &str) -> TmuxResult<()> {
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err(TmuxError::invalid_input(format!(
            "{label} must be non-empty and contain no control characters"
        )));
    }
    Ok(())
}

fn validate_stable_session_id(value: &str) -> TmuxResult<()> {
    let Some(digits) = value.strip_prefix('$') else {
        return Err(TmuxError::invalid_input(
            "attach requires a stable tmux session ID",
        ));
    };
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(TmuxError::invalid_input(
            "attach requires a stable tmux session ID",
        ));
    }
    Ok(())
}

fn take_exactly_one<T>(mut items: Vec<T>, label: &str) -> TmuxResult<T> {
    if items.len() != 1 {
        return Err(TmuxError::parse_failed(format!(
            "expected one {label}, received {} records",
            items.len()
        )));
    }
    Ok(items.remove(0))
}



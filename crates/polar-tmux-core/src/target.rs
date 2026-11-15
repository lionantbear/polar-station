use crate::{TmuxControlPlan, TmuxError, TmuxResult};
use polar_tmux_types::{TmuxAttachPlan, REMOTE_TMUX_PATH_ASSIGNMENT, TMUX_EXECUTION_PATH};
use tokio::process::Command;

fn local_execution_path() -> String {
    match std::env::var("HOME") {
        Ok(home) => format!("{TMUX_EXECUTION_PATH}:{home}/.local/bin"),
        Err(_) => TMUX_EXECUTION_PATH.to_owned(),
    }
}
pub(crate) fn local_executable(executable: &str) -> String {
    if executable == "tmux"
        && !std::env::split_paths(TMUX_EXECUTION_PATH).any(|dir| dir.join("tmux").is_file())
    {
        if let Ok(home) = std::env::var("HOME") {
            let path = std::path::Path::new(&home).join(".local/bin/tmux");
            if path.is_file() {
                return path.to_string_lossy().into_owned();
            }
        }
    }
    executable.to_owned()
}

const TMUX_UTF8_LOCALE: &str = "C.UTF-8";
const REMOTE_TMUX_UTF8_LOCALE_ASSIGNMENT: &str = "LC_ALL=C.UTF-8";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TmuxTarget {
    Local,
    Ssh {
        destination: String,
    },
    MultiplexedSsh {
        destination: String,
        control_socket: String,
    },
    Wsl {
        distribution: Option<String>,
    },
}

impl From<polar_ssh::SshEndpoint> for TmuxTarget {
    fn from(endpoint: polar_ssh::SshEndpoint) -> Self {
        Self::MultiplexedSsh {
            destination: endpoint.destination().into(),
            control_socket: endpoint.control_socket().into(),
        }
    }
}

impl TmuxTarget {
    pub fn ssh(destination: impl Into<String>) -> Self {
        Self::Ssh {
            destination: destination.into(),
        }
    }

    pub fn multiplexed_ssh(
        destination: impl Into<String>,
        control_socket: impl Into<String>,
    ) -> TmuxResult<Self> {
        let destination = destination.into();
        let control_socket = control_socket.into();
        if !polar_tmux_types::is_canonical_ssh_destination(&destination) {
            return Err(TmuxError::invalid_input(
                "managed SSH destination is not canonical",
            ));
        }
        if !polar_tmux_types::is_canonical_ssh_control_socket(&control_socket) {
            return Err(TmuxError::invalid_input(
                "managed SSH control socket is not a canonical absolute path",
            ));
        }
        Ok(Self::MultiplexedSsh {
            destination,
            control_socket,
        })
    }

    pub fn wsl(distribution: Option<impl Into<String>>) -> Self {
        Self::Wsl {
            distribution: distribution.map(Into::into),
        }
    }

    pub(crate) fn command(
        &self,
        tmux_executable: &str,
        tmux_args: &[String],
    ) -> TmuxResult<Command> {
        match self {
            Self::Local => {
                let mut command = Command::new(local_executable(tmux_executable));
                command.env("LC_ALL", TMUX_UTF8_LOCALE);
                command.env("PATH", local_execution_path());
                command.args(tmux_args);
                Ok(command)
            }
            Self::Ssh { .. } | Self::MultiplexedSsh { .. } => {
                let spec = self.ssh_command(
                    remote_tmux_command_with_locale(tmux_executable, tmux_args),
                    polar_ssh::RemotePty::None,
                )?;
                let mut command = Command::new(spec.program);
                command.args(spec.args);
                Ok(command)
            }
            Self::Wsl { distribution } => {
                let mut command = Command::new("wsl.exe");
                if let Some(distribution) = distribution {
                    command.args(["-d", distribution]);
                }
                command.arg("--");
                command.arg(tmux_executable);
                command.args(tmux_args);
                Ok(command)
            }
        }
    }

    pub(crate) fn control_mode_plan(
        &self,
        tmux_executable: &str,
        tmux_args: &[String],
        initial_commands: Vec<String>,
    ) -> TmuxResult<TmuxControlPlan> {
        let (program, args, environment) = match self {
            Self::Local => (
                local_executable(tmux_executable),
                tmux_args.to_vec(),
                vec![
                    ("LC_ALL".to_owned(), TMUX_UTF8_LOCALE.to_owned()),
                    ("PATH".to_owned(), local_execution_path()),
                ],
            ),
            Self::Ssh { .. } | Self::MultiplexedSsh { .. } => {
                let spec = self.ssh_command(
                    remote_tmux_command_with_locale(tmux_executable, tmux_args),
                    polar_ssh::RemotePty::Required,
                )?;
                (spec.program, spec.args, Vec::new())
            }
            Self::Wsl { distribution } => {
                let mut args = Vec::new();
                if let Some(distribution) = distribution {
                    args.extend(["-d".to_owned(), distribution.clone()]);
                }
                args.push("--".to_owned());
                args.push(tmux_executable.to_owned());
                args.extend_from_slice(tmux_args);
                ("wsl.exe".to_owned(), args, Vec::new())
            }
        };
        Ok(TmuxControlPlan {
            program,
            args,
            environment,
            initial_commands,
        })
    }

    fn ssh_command(
        &self,
        command: String,
        pty: polar_ssh::RemotePty,
    ) -> TmuxResult<polar_ssh::SshCommand> {
        let result = match self {
            Self::MultiplexedSsh {
                destination,
                control_socket,
            } => polar_ssh::SshEndpoint::managed(destination, control_socket)
                .map(|endpoint| endpoint.command(command, pty)),
            Self::Ssh { destination } => polar_ssh::direct_command(destination, command, pty),
            _ => return Err(TmuxError::invalid_input("not an SSH target")),
        };
        result.map_err(|_| TmuxError::invalid_input("invalid SSH endpoint"))
    }

    pub(crate) fn terminal_command(&self, command: String) -> TmuxResult<(String, Vec<String>)> {
        match self {
            Self::Local => Ok(("/bin/sh".into(), vec!["-c".into(), command])),
            Self::MultiplexedSsh { .. } => {
                let spec = self.ssh_command(command, polar_ssh::RemotePty::Required)?;
                Ok((spec.program, spec.args))
            }
            Self::Ssh { .. } => Err(TmuxError::invalid_input(
                "terminal requires verified SSH master",
            )),
            Self::Wsl { .. } => Err(TmuxError::unsupported_target(
                "WSL terminal remains deferred",
            )),
        }
    }

    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Ssh { .. } | Self::MultiplexedSsh { .. } => "ssh",
            Self::Wsl { .. } => "wsl",
        }
    }

    pub(crate) fn attach_plan(
        &self,
        tmux_executable: &str,
        tmux_args: &[String],
    ) -> TmuxResult<TmuxAttachPlan> {
        let plan = match self {
            Self::Local => TmuxAttachPlan {
                program: local_executable(tmux_executable),
                args: tmux_args.to_vec(),
            },
            Self::Ssh { .. } => {
                return Err(TmuxError::invalid_input(
                    "typed SSH attach requires a verified Host control socket",
                ));
            }
            Self::MultiplexedSsh {
                destination,
                control_socket,
            } => {
                let endpoint = polar_ssh::SshEndpoint::managed(destination, control_socket)
                    .map_err(|_| TmuxError::invalid_input("invalid managed SSH endpoint"))?;
                let spec = endpoint
                    .legacy_attach_metadata(remote_tmux_command(tmux_executable, tmux_args));
                TmuxAttachPlan {
                    program: spec.program,
                    args: spec.args,
                }
            }
            Self::Wsl { .. } => {
                return Err(TmuxError::unsupported_target(
                    "typed WSL attach remains deferred until the Windows frontend phase",
                ));
            }
        };
        plan.validate().map_err(|_| {
            TmuxError::invalid_input(
                "typed attach requires canonical tmux executable, socket, target, and session arguments",
            )
        })?;
        Ok(plan)
    }
}

pub(crate) fn remote_tmux_command(executable: &str, args: &[String]) -> String {
    std::iter::once(REMOTE_TMUX_PATH_ASSIGNMENT.to_string())
        .chain(std::iter::once(quote_posix(executable)))
        .chain(args.iter().map(|arg| quote_posix(arg)))
        .collect::<Vec<_>>()
        .join(" ")
}

fn remote_tmux_command_with_locale(executable: &str, args: &[String]) -> String {
    format!(
        "{REMOTE_TMUX_UTF8_LOCALE_ASSIGNMENT} {}",
        remote_tmux_command(executable, args)
    )
}

/// Quote an argument parsed by tmux, keeping its line-framed Control Mode wire intact.
/// tmux expands escaped LF/CR inside double quotes, including nested if-shell commands.
pub(crate) fn quote_tmux_argument(value: &str) -> String {
    if !value.contains(['\n', '\r']) {
        return quote_posix(value);
    }
    let mut quoted = String::from("\"");
    for character in value.chars() {
        match character {
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\\' | '"' | '$' | '~' => {
                quoted.push('\\');
                quoted.push(character);
            }
            _ => quoted.push(character),
        }
    }
    quoted.push('"');
    quoted
}

pub(crate) use polar_ssh::quote_posix;

/// tmux command grammar is distinct from the surrounding POSIX shell.
pub(crate) fn tmux_command_sequence(args: &[String]) -> String {
    args.split(|arg| arg == ";")
        .map(|command| {
            command
                .iter()
                .map(|arg| quote_tmux_argument(arg))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join(" ; ")
}



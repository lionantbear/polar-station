use crate::{TmuxControlPlan, TmuxError, TmuxResult};
use polar_tmux_types::{TmuxAttachPlan, REMOTE_TMUX_PATH_ASSIGNMENT, TMUX_EXECUTION_PATH};
use tokio::process::Command;

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
                let mut command = Command::new(tmux_executable);
                command.env("LC_ALL", TMUX_UTF8_LOCALE);
                command.env("PATH", TMUX_EXECUTION_PATH);
                command.args(tmux_args);
                Ok(command)
            }
            Self::Ssh { destination } => {
                if !polar_tmux_types::is_canonical_ssh_destination(destination) {
                    return Err(TmuxError::invalid_input("SSH destination is not canonical"));
                }
                let mut command = Command::new("ssh");
                command.arg(destination);
                command.arg(remote_tmux_command_with_locale(tmux_executable, tmux_args));
                Ok(command)
            }
            Self::MultiplexedSsh {
                destination,
                control_socket,
            } => {
                if !polar_tmux_types::is_canonical_ssh_destination(destination)
                    || !polar_tmux_types::is_canonical_ssh_control_socket(control_socket)
                {
                    return Err(TmuxError::invalid_input(
                        "managed SSH target identity is invalid",
                    ));
                }
                let mut command = Command::new("ssh");
                command.args([
                    "-q",
                    "-S",
                    control_socket,
                    "-o",
                    "ProxyCommand=/usr/bin/false",
                ]);
                command.arg(destination);
                command.arg(remote_tmux_command_with_locale(tmux_executable, tmux_args));
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
                tmux_executable.to_owned(),
                tmux_args.to_vec(),
                vec![
                    ("LC_ALL".to_owned(), TMUX_UTF8_LOCALE.to_owned()),
                    (
                        "PATH".to_owned(),
                        polar_tmux_types::TMUX_EXECUTION_PATH.to_owned(),
                    ),
                ],
            ),
            Self::Ssh { destination } => {
                if !polar_tmux_types::is_canonical_ssh_destination(destination) {
                    return Err(TmuxError::invalid_input("SSH destination is not canonical"));
                }
                (
                    "ssh".to_owned(),
                    vec![
                        "-tt".to_owned(),
                        destination.clone(),
                        remote_tmux_command_with_locale(tmux_executable, tmux_args),
                    ],
                    Vec::new(),
                )
            }
            Self::MultiplexedSsh {
                destination,
                control_socket,
            } => {
                if !polar_tmux_types::is_canonical_ssh_destination(destination)
                    || !polar_tmux_types::is_canonical_ssh_control_socket(control_socket)
                {
                    return Err(TmuxError::invalid_input(
                        "managed SSH target identity is invalid",
                    ));
                }
                (
                    "ssh".to_owned(),
                    vec![
                        "-tt".to_owned(),
                        "-q".to_owned(),
                        "-S".to_owned(),
                        control_socket.clone(),
                        "-o".to_owned(),
                        "ProxyCommand=/usr/bin/false".to_owned(),
                        destination.clone(),
                        remote_tmux_command_with_locale(tmux_executable, tmux_args),
                    ],
                    Vec::new(),
                )
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
                program: tmux_executable.to_string(),
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
                if !polar_tmux_types::is_canonical_ssh_destination(destination)
                    || !polar_tmux_types::is_canonical_ssh_control_socket(control_socket)
                {
                    return Err(TmuxError::invalid_input(
                        "managed SSH target identity is invalid",
                    ));
                }
                // An interactive tmux attach requires a remote pseudo-terminal.
                // The exact control socket and fail-closed ProxyCommand force
                // this channel through the Host-owned master.
                let mut args = vec![
                    "-q".to_string(),
                    "-S".to_string(),
                    control_socket.clone(),
                    "-o".to_string(),
                    "ProxyCommand=/usr/bin/false".to_string(),
                    "-tt".to_string(),
                ];
                args.push(destination.clone());
                args.push(remote_tmux_command(tmux_executable, tmux_args));
                TmuxAttachPlan {
                    program: "ssh".to_string(),
                    args,
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

pub(crate) fn quote_posix(value: &str) -> String {
    if !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&byte))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}



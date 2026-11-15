use std::{net::SocketAddr, path::PathBuf, time::Duration};

use anyhow::{bail, Result};
use clap::{Parser, Subcommand, ValueEnum};
use polar_tmux_core::{TmuxAvailability, TmuxClient, TmuxTarget};
use polar_tmux_protocol::is_canonical_tmux_socket_name;
use serde::Serialize;
use zeroize::{Zeroize, Zeroizing};

#[cfg(unix)]
mod channel_cli;

use polar_tmux::{host_identity, host_runtime, ssh_connection_manager};
#[cfg(unix)]
use polar_tmux::{local_ipc, pane_host};

#[derive(Debug, Parser)]
#[command(
    name = "polar-tmux",
    version,
    about = "Cross-platform tmux host and control CLI"
)]
struct Cli {
    #[arg(long, global = true)]
    socket_name: Option<String>,

    #[arg(long, global = true)]
    ssh: Option<String>,

    #[arg(long, global = true)]
    wsl_distribution: Option<String>,

    #[arg(long, global = true, default_value = "tmux")]
    tmux_executable: String,

    #[arg(long, global = true, default_value_t = 10_000)]
    timeout_ms: u64,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    #[cfg(unix)]
    Connector {
        #[command(subcommand)]
        command: channel_cli::ConnectorCommand,
    },
    #[cfg(unix)]
    Imessage {
        #[command(subcommand)]
        command: channel_cli::ImessageCommand,
    },
    #[cfg(unix)]
    Telegram {
        #[command(subcommand)]
        command: channel_cli::TelegramCommand,
    },
    #[cfg(unix)]
    Codex {
        #[command(subcommand)]
        command: channel_cli::CodexCommand,
    },
    Host {
        #[arg(long, default_value = "127.0.0.1:47820")]
        listen: SocketAddr,
        #[arg(long)]
        target_config: Option<PathBuf>,
        #[arg(long)]
        ipc_socket: Option<PathBuf>,
        #[arg(long)]
        runtime_data_root: Option<PathBuf>,
        #[arg(long)]
        advertise: bool,
        #[arg(long)]
        parent_lease_stdin: bool,
    },
    PaneHost {
        #[arg(long)]
        channel_id: String,
        #[arg(long)]
        channel_generation: u64,
        #[arg(long)]
        control_socket: PathBuf,
        #[arg(long)]
        host_socket: Option<PathBuf>,
        #[arg(long, requires = "initial_session")]
        initial_execution_target: Option<String>,
        #[arg(long, requires = "initial_execution_target")]
        initial_session: Option<String>,
    },
    PairingPayload,
    Availability,
    Version,
    Sessions,
    Windows {
        session: String,
    },
    Panes {
        target: String,
    },
    Clients,
    Snapshot,
    NewSession {
        name: String,
        #[arg(long)]
        working_directory: Option<String>,
        #[arg(long)]
        command: Option<String>,
    },
    KillSession {
        target: String,
    },
    CreateWindow {
        session: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        working_directory: Option<String>,
        #[arg(long)]
        command: Option<String>,
    },
    KillWindow {
        target: String,
    },
    RenameWindow {
        target: String,
        name: String,
    },
    SplitPane {
        target: String,
        #[arg(long)]
        horizontal: bool,
        #[arg(long)]
        working_directory: Option<String>,
        #[arg(long)]
        command: Option<String>,
    },
    KillPane {
        target: String,
    },
    SelectPane {
        target: String,
    },
    ResizePane {
        target: String,
        direction: ResizeDirectionArg,
        amount: u16,
    },
    SendKeys {
        target: String,
        #[arg(required = true)]
        keys: Vec<String>,
    },
    SendText {
        target: String,
        text: String,
        #[arg(long)]
        enter: bool,
    },
    SwitchClient {
        client: String,
        session: String,
    },
    CapturePane {
        target: String,
        #[arg(long)]
        history: bool,
    },
    #[command(hide = true)]
    CodexNotify {
        payload: String,
    },
    #[command(hide = true, trailing_var_arg = true)]
    SftpChannel {
        #[arg(long)]
        target_id: String,
        #[arg(long)]
        generation: u64,
        #[arg(allow_hyphen_values = true)]
        ssh_args: Vec<String>,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ResizeDirectionArg {
    Left,
    Right,
    Up,
    Down,
}

impl From<ResizeDirectionArg> for polar_tmux_protocol::ResizeDirection {
    fn from(value: ResizeDirectionArg) -> Self {
        match value {
            ResizeDirectionArg::Left => Self::Left,
            ResizeDirectionArg::Right => Self::Right,
            ResizeDirectionArg::Up => Self::Up,
            ResizeDirectionArg::Down => Self::Down,
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn main() -> Result<()> {
    polar_tmux::run_standalone_runtime(run_cli())
}

#[cfg(target_os = "macos")]
fn main() -> Result<()> {
    polar_tmux::run_with_messages_main_thread(run_cli())
}

async fn run_cli() -> Result<()> {
    polar_tmux::operation_trace::init();
    tracing::info!(target: "polar_station_operation", component="Host", event="process_entry");

    let cli = Cli::parse();
    let client = make_client(&cli);
    match cli.command {
        #[cfg(unix)]
        Commands::Imessage { command } => channel_cli::imessage(command).await,
        #[cfg(unix)]
        Commands::Connector { command } => channel_cli::connector(command).await,
        #[cfg(unix)]
        Commands::Telegram { command } => channel_cli::telegram(command).await,
        #[cfg(unix)]
        Commands::Codex { command } => channel_cli::codex(command).await,
        Commands::Host {
            listen,
            target_config,
            ipc_socket,
            runtime_data_root,
            advertise,
            parent_lease_stdin,
        } => {
            if cli.ssh.is_some() || cli.wsl_distribution.is_some() {
                bail!(
                    "Host execution targets must come from --target-config; global --ssh/--wsl-distribution are one-shot CLI options"
                );
            }
            if cli.tmux_executable != "tmux" {
                bail!("Host mode requires the canonical tmux executable name \"tmux\"");
            }
            if cli
                .socket_name
                .as_deref()
                .is_some_and(|name| !is_canonical_tmux_socket_name(name))
            {
                bail!("Host mode tmux socket names must match [A-Za-z0-9][A-Za-z0-9._-]{{0,95}}");
            }
            host_runtime::run(
                client,
                listen,
                target_config.as_deref(),
                ipc_socket,
                runtime_data_root,
                advertise,
                parent_lease_stdin,
            )
            .await
        }
        Commands::PaneHost {
            channel_id,
            channel_generation,
            control_socket,
            host_socket,
            initial_execution_target,
            initial_session,
        } => {
            #[cfg(unix)]
            {
                let host_socket = match host_socket {
                    Some(path) => path,
                    None => local_ipc::default_socket_path()?,
                };
                pane_host::run(
                    channel_id,
                    channel_generation,
                    &control_socket,
                    &host_socket,
                    initial_execution_target.as_deref(),
                    initial_session.as_deref(),
                )
                .await
            }
            #[cfg(not(unix))]
            {
                let _ = (
                    channel_id,
                    channel_generation,
                    control_socket,
                    host_socket,
                    initial_execution_target,
                    initial_session,
                );
                bail!("pane-host mode is unavailable on this platform")
            }
        }
        Commands::PairingPayload => {
            let identity = host_identity::HostIdentity::load_platform_isolated()?;
            print_sensitive_json(&pairing_payload(&identity))
        }
        Commands::Availability => match client.availability().await? {
            TmuxAvailability::Available { version } => {
                print_json(&serde_json::json!({"available": true, "version": version}))
            }
            TmuxAvailability::ExecutableMissing { executable } => {
                print_json(&serde_json::json!({"available": false, "executable": executable}))
            }
        },
        Commands::Version => print_json(&client.version().await?),
        Commands::Sessions => print_json(&client.list_sessions().await?),
        Commands::Windows { session } => print_json(&client.list_windows(&session).await?),
        Commands::Panes { target } => print_json(&client.list_panes(&target).await?),
        Commands::Clients => print_json(&client.list_clients().await?),
        Commands::Snapshot => print_json(&client.snapshot().await?),
        Commands::NewSession {
            name,
            working_directory,
            command,
        } => print_json(
            &client
                .create_session(&name, working_directory.as_deref(), command.as_deref())
                .await?,
        ),
        Commands::KillSession { target } => {
            client.kill_session(&target).await?;
            print_json(&serde_json::json!({"accepted": true}))
        }
        Commands::CreateWindow {
            session,
            name,
            working_directory,
            command,
        } => print_json(
            &client
                .create_window(
                    &session,
                    name.as_deref(),
                    working_directory.as_deref(),
                    command.as_deref(),
                )
                .await?,
        ),
        Commands::KillWindow { target } => {
            client.kill_window(&target).await?;
            print_json(&serde_json::json!({"accepted": true}))
        }
        Commands::RenameWindow { target, name } => {
            print_json(&client.rename_window(&target, &name).await?)
        }
        Commands::SplitPane {
            target,
            horizontal,
            working_directory,
            command,
        } => print_json(
            &client
                .split_pane(
                    &target,
                    horizontal,
                    working_directory.as_deref(),
                    command.as_deref(),
                )
                .await?,
        ),
        Commands::KillPane { target } => {
            client.kill_pane(&target).await?;
            print_json(&serde_json::json!({"accepted": true}))
        }
        Commands::SelectPane { target } => {
            client.select_pane(&target).await?;
            print_json(&serde_json::json!({"accepted": true}))
        }
        Commands::ResizePane {
            target,
            direction,
            amount,
        } => {
            client
                .resize_pane(&target, direction.into(), amount)
                .await?;
            print_json(&serde_json::json!({"accepted": true}))
        }
        Commands::SendKeys { target, keys } => {
            client.send_keys(&target, &keys).await?;
            print_json(&serde_json::json!({"accepted": true}))
        }
        Commands::SendText {
            target,
            text,
            enter,
        } => {
            client.send_text(&target, &text, enter).await?;
            print_json(&serde_json::json!({"accepted": true}))
        }
        Commands::SwitchClient {
            client: name,
            session,
        } => {
            client.switch_client(&name, &session).await?;
            print_json(&serde_json::json!({"accepted": true}))
        }
        Commands::CapturePane { target, history } => print_json(&serde_json::json!({
            "text": client.capture_pane(&target, history).await?
        })),
        Commands::CodexNotify { payload } => {
            let _ = polar_tmux::record_codex_notify(&payload);
            Ok(())
        }
        Commands::SftpChannel {
            target_id,
            generation,
            ssh_args,
        } => {
            #[cfg(unix)]
            {
                ssh_connection_manager::run_sftp_channel(&target_id, generation, &ssh_args)?;
                Ok(())
            }
            #[cfg(not(unix))]
            {
                let _ = (target_id, generation, ssh_args);
                bail!("SFTP channel mode is unavailable on this platform")
            }
        }
    }
}

#[derive(Serialize)]
struct PairingPayload {
    schema: &'static str,
    host_id: String,
    pairing_code: String,
}

impl Drop for PairingPayload {
    fn drop(&mut self) {
        self.pairing_code.zeroize();
    }
}

fn pairing_payload(identity: &host_identity::HostIdentity) -> PairingPayload {
    PairingPayload {
        schema: "polar-tmux-pairing-payload-v1",
        host_id: identity.host_id().to_string(),
        pairing_code: identity.pairing_code().to_string(),
    }
}

fn make_client(cli: &Cli) -> TmuxClient {
    let target = if let Some(destination) = &cli.ssh {
        TmuxTarget::ssh(destination)
    } else if cli.wsl_distribution.is_some() {
        TmuxTarget::wsl(cli.wsl_distribution.clone())
    } else {
        TmuxTarget::Local
    };
    let client = TmuxClient::new(target)
        .with_tmux_executable(cli.tmux_executable.clone())
        .with_command_timeout(Duration::from_millis(cli.timeout_ms));
    match &cli.socket_name {
        Some(socket_name) => client.with_socket_name(socket_name),
        None => client,
    }
}

fn print_json(value: &impl serde::Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn print_sensitive_json(value: &impl serde::Serialize) -> Result<()> {
    let encoded = Zeroizing::new(serde_json::to_string_pretty(value)?);
    println!("{}", encoded.as_str());
    Ok(())
}



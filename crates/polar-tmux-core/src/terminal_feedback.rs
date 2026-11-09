//! Shared terminal presentation. Native adapters render this projection only.
use polar_tmux_types::{TerminalRecoveryPhaseV1 as Phase, TerminalRecoveryViewV1};

pub fn terminal_feedback(phase: Phase, retry_seconds: Option<u64>) -> TerminalRecoveryViewV1 {
    let message = match phase {
        Phase::Hidden => String::new(),
        Phase::Probing => "Connecting…".into(),
        Phase::Attaching => "Reconnecting…".into(),
        Phase::Waiting => format!(
            "Connection lost. Retrying in {}s…",
            retry_seconds.unwrap_or(1)
        ),
        Phase::Paused => "Unable to connect. Automatic retries paused.".into(),
        Phase::Ended => "Session ended".into(),
        Phase::Recovered => "Connection restored".into(),
        Phase::HostUnavailable => "Reconnecting to the local service…".into(),
        Phase::StatusUnavailable => "Connection status unavailable.".into(),
    };
    TerminalRecoveryViewV1 {
        phase,
        message,
        visible: phase != Phase::Hidden,
        retry_enabled: matches!(
            phase,
            Phase::Waiting | Phase::Paused | Phase::HostUnavailable | Phase::StatusUnavailable
        ),
        ..Default::default()
    }
}

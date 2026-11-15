//! Original single setup attempt, driven by native PTY lifecycle events.
//! Focus and mounting never establish success; actual readiness is rechecked.
use polar_tmux_protocol::{SessionEditorResultV1, ToolSetupLaunchV1};

/// Copy for the native task surface; it does not create or resume a workflow.
#[derive(Debug, serde::Serialize)]
pub struct ToolSetupCopy {
    pub restored_title: &'static str,
    pub restored_detail: &'static str,
    pub close_title: &'static str,
    pub close_detail: &'static str,
    pub keep: &'static str,
    pub stop: &'static str,
}
pub fn tool_setup_copy() -> ToolSetupCopy {
    ToolSetupCopy {
        restored_title: "Previous setup",
        restored_detail: "This setup Pane was restored without its process. Recheck the machine in Setup & diagnostics. Installation is never restarted automatically.",
        close_title: "Stop setup and close this Pane?",
        close_detail: "Setup is still running. Closing interrupts it; check the machine before starting another installation.",
        keep: "Keep running", stop: "Stop and close",
    }
}

#[derive(Debug)]
enum Phase {
    Preparing,
    Launching(String),
    Running(String),
}
#[derive(Debug)]
struct Pending {
    token: u64,
    source_target: String,
    execution_target: String,
    phase: Phase,
}
#[derive(Debug, Default)]
pub(super) struct ToolPreparation {
    pending: Option<Pending>,
}
pub(super) enum PreparationOutcome {
    Launch {
        token: u64,
        setup: ToolSetupLaunchV1,
    },
    Running,
    Recheck {
        message: Option<String>,
    },
    Ignored,
    Stale,
    Failed(String),
}
impl ToolPreparation {
    pub fn start(&mut self, token: u64, source: String, target: String) {
        self.pending = Some(Pending {
            token,
            source_target: source,
            execution_target: target,
            phase: Phase::Preparing,
        });
    }
    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }
    pub fn matches(&self, token: u64) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|p| p.token == token && matches!(p.phase, Phase::Preparing))
    }
    pub fn focus(&self) -> Option<(u64, &str)> {
        let pending = self.pending.as_ref()?;
        if let Phase::Running(id) = &pending.phase {
            Some((pending.token, id))
        } else {
            None
        }
    }
    pub fn complete(
        &mut self,
        source: Option<&str>,
        result: Option<SessionEditorResultV1>,
        error: Option<String>,
    ) -> PreparationOutcome {
        let Some(pending) = &mut self.pending else {
            return PreparationOutcome::Ignored;
        };
        if !matches!(pending.phase, Phase::Preparing) {
            return PreparationOutcome::Ignored;
        }
        if source != Some(pending.source_target.as_str()) {
            self.pending = None;
            return PreparationOutcome::Stale;
        }
        match (result, error) {
            (Some(SessionEditorResultV1::ToolsPrepared { setup }), None)
                if setup.execution_target_id == pending.execution_target
                    && setup.validate().is_ok() =>
            {
                pending.phase = Phase::Launching(setup.id.clone());
                PreparationOutcome::Launch {
                    token: pending.token,
                    setup,
                }
            }
            (_, error) => {
                self.pending = None;
                PreparationOutcome::Failed(error.unwrap_or_else(||"The Host did not prepare a setup Pane for the requested machine. Update the Host and retry.".into()))
            }
        }
    }
    pub fn started(
        &mut self,
        source: Option<&str>,
        token: u64,
        error: Option<String>,
    ) -> PreparationOutcome {
        let Some(pending) = &mut self.pending else {
            return PreparationOutcome::Ignored;
        };
        if pending.token != token || !matches!(pending.phase, Phase::Launching(_)) {
            return PreparationOutcome::Ignored;
        }
        if source != Some(pending.source_target.as_str()) {
            self.pending = None;
            return PreparationOutcome::Stale;
        }
        if let Some(error) = error {
            self.pending = None;
            return PreparationOutcome::Failed(error);
        }
        let Phase::Launching(id) = &pending.phase else {
            unreachable!()
        };
        pending.phase = Phase::Running(id.clone());
        PreparationOutcome::Running
    }
    pub fn exited(
        &mut self,
        source: Option<&str>,
        token: u64,
        exit_code: Option<u32>,
    ) -> PreparationOutcome {
        let Some(pending) = &self.pending else {
            return PreparationOutcome::Ignored;
        };
        if pending.token != token || matches!(pending.phase, Phase::Preparing) {
            return PreparationOutcome::Ignored;
        }
        let current = source == Some(pending.source_target.as_str());
        self.pending = None;
        if !current {
            return PreparationOutcome::Stale;
        }
        PreparationOutcome::Recheck {
            message: match exit_code {
                Some(0) => None,
                Some(code) => Some(format!("Setup exited with status {code}. Check its output; rechecking the machine.")),
                None => Some("Setup was interrupted or its result is unknown. Rechecking does not restart it.".into()),
            },
        }
    }
}



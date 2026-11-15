//! OpenSSH process creation, check and exact-child reaping.
use super::*;
use tokio::io::AsyncReadExt;

const STDERR_TAIL_BYTES: usize = 4096;

#[derive(Debug, Clone, Serialize)]
pub(super) struct MasterExitReport {
    pub(super) pid: u32,
    pub(super) code: Option<u32>,
    pub(super) signal: Option<i32>,
    pub(super) reason: &'static str,
    pub(super) lifetime_ms: u64,
    pub(super) stderr_bytes: u64,
    pub(super) stderr_complete: bool,
    pub(super) owned_stop: bool,
}

fn stderr_reason(tail: &[u8]) -> &'static str {
    let text = String::from_utf8_lossy(tail);
    if text.contains("Timeout, server ") && text.contains(" not responding.") {
        "keepalive_timeout"
    } else if text.contains("Connection timed out") {
        "connect_timeout"
    } else if text.contains("Connection reset by") {
        "connection_reset"
    } else if text.contains("Connection closed by") {
        "connection_closed"
    } else if text.contains("Permission denied") {
        "authentication_failed"
    } else if text.contains("Too many") || text.contains("Resource temporarily unavailable") {
        "resource_unavailable"
    } else if tail.is_empty() {
        "no_stderr"
    } else {
        "unclassified_stderr"
    }
}

#[derive(Debug)]
pub(super) struct OpenSshBackend {
    pub(super) executable: OsString,
}

impl OpenSshBackend {
    pub(super) fn system() -> Self {
        Self {
            executable: "ssh".into(),
        }
    }
}

#[cfg(target_os = "linux")]
pub(super) fn configure_master_parent_lease(command: &mut Command) -> io::Result<()> {
    // Capture the expected Host before fork. The child compares this after
    // arming PR_SET_PDEATHSIG so a Host death in the fork-to-prctl window
    // cannot leave an unleased SSH master behind.
    let expected_parent = unsafe { libc::getpid() };
    // SAFETY: the pre-exec closure invokes only Linux syscalls and constructs
    // errno-backed errors. It does not access shared locks or heap-owned state.
    unsafe {
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::getppid() != expected_parent {
                return Err(io::Error::from_raw_os_error(libc::ECHILD));
            }
            Ok(())
        });
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub(super) fn configure_master_parent_lease(_command: &mut Command) -> io::Result<()> {
    Ok(())
}

pub(super) async fn wait_for_master_exit(exited: &mut watch::Receiver<bool>) {
    while !*exited.borrow_and_update() {
        if exited.changed().await.is_err() {
            break;
        }
    }
}

struct OpenSshMaster {
    pid: u32,
    exited: watch::Receiver<bool>,
    stop: Option<oneshot::Sender<()>>,
    report: Arc<StdMutex<Option<MasterExitReport>>>,
}

impl Drop for OpenSshMaster {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

impl OwnedMaster for OpenSshMaster {
    fn pid(&self) -> u32 {
        self.pid
    }

    fn is_alive(&mut self) -> io::Result<bool> {
        Ok(!*self.exited.borrow())
    }

    fn exit_notification(&self) -> watch::Receiver<bool> {
        self.exited.clone()
    }

    fn exit_report(&self) -> Option<MasterExitReport> {
        self.report.lock().expect("SSH exit report").clone()
    }
}

impl SshMasterBackend for OpenSshBackend {
    fn spawn_master(&self, spec: &MasterSpec) -> io::Result<Box<dyn OwnedMaster>> {
        let mut command = Command::new(&self.executable);
        command
            .args(spec.master_args())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        configure_master_parent_lease(&mut command)?;
        let mut child = command.spawn()?;
        let pid = child
            .id()
            .ok_or_else(|| io::Error::other("SSH master process has no PID"))?;
        let (exited_tx, exited) = watch::channel(false);
        let (stop, stopped) = oneshot::channel();
        let report = Arc::new(StdMutex::new(None));
        let report_out = report.clone();
        let control_socket = spec.control_socket.clone();
        let target_id = spec.target_id.clone();
        let generation = spec.generation;
        let started = StdInstant::now();
        let mut stderr = child.stderr.take().expect("piped SSH stderr");
        tokio::spawn(async move {
            // Drain concurrently so a full stderr pipe cannot deadlock wait().
            // Only allowlisted classifications leave this task, never raw text.
            let mut drain = Box::pin(async move {
                let mut tail = VecDeque::with_capacity(STDERR_TAIL_BYTES);
                let mut count = 0_u64;
                let mut buffer = [0_u8; 1024];
                loop {
                    let n = match stderr.read(&mut buffer).await {
                        Ok(0) => return (tail.into_iter().collect::<Vec<_>>(), count, true),
                        Ok(n) => n,
                        Err(_) => return (tail.into_iter().collect::<Vec<_>>(), count, false),
                    };
                    count = count.saturating_add(n as u64);
                    for byte in &buffer[..n] {
                        if tail.len() == STDERR_TAIL_BYTES {
                            tail.pop_front();
                        }
                        tail.push_back(*byte);
                    }
                }
            });
            let mut stderr_result = None;
            let mut owned_stop = false;
            let mut stopped = stopped;
            let status = loop {
                tokio::select! {
                    status = child.wait() => break status,
                    _ = &mut stopped => {
                        owned_stop = true;
                        let _ = child.start_kill();
                        break child.wait().await;
                    },
                    result = &mut drain, if stderr_result.is_none() => stderr_result = Some(result),
                }
            };
            // An inherited stderr descriptor must not keep the owner alive.
            let (tail, stderr_bytes, stderr_complete) = match stderr_result {
                Some(result) => result,
                None => tokio::time::timeout(Duration::from_millis(100), drain)
                    .await
                    .unwrap_or_default(),
            };
            #[cfg(unix)]
            let signal = {
                use std::os::unix::process::ExitStatusExt;
                status.as_ref().ok().and_then(|status| status.signal())
            };
            #[cfg(not(unix))]
            let signal = None;
            let result = MasterExitReport {
                pid,
                code: status
                    .as_ref()
                    .ok()
                    .and_then(|status| status.code())
                    .and_then(|code| code.try_into().ok()),
                signal,
                reason: if owned_stop {
                    "owned_stop"
                } else if status.is_err() {
                    "wait_failed"
                } else if !stderr_complete {
                    "stderr_incomplete"
                } else {
                    stderr_reason(&tail)
                },
                lifetime_ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
                stderr_bytes,
                stderr_complete,
                owned_stop,
            };
            tracing::info!(target: "polar_station_operation", component="SSH", event="ssh_master_reaped",
                target_id, generation, child_pid=pid, exit_code=?result.code, signal=?result.signal,
                reason=result.reason, lifetime_ms=result.lifetime_ms, stderr_bytes, stderr_complete, owned_stop);
            if !owned_stop {
                tracing::warn!(target_id, generation, child_pid=pid, exit_code=?result.code,
                    signal=?result.signal, reason=result.reason, "SSH master exited");
            }
            *report_out.lock().expect("SSH exit report") = Some(result);
            cleanup_control_socket(&control_socket);
            exited_tx.send_replace(true);
        });
        Ok(Box::new(OpenSshMaster {
            pid,
            exited,
            stop: Some(stop),
            report,
        }))
    }

    fn check_master<'a>(&'a self, spec: &'a MasterSpec) -> CheckFuture<'a> {
        Box::pin(async move {
            let output = tokio::time::timeout(
                CHECK_TIMEOUT,
                Command::new(&self.executable)
                    .args(["-S".as_ref(), spec.control_socket.as_os_str()])
                    .args(["-O", "check"])
                    .arg(&spec.destination)
                    .stdin(Stdio::null())
                    .kill_on_drop(true)
                    .output(),
            )
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "SSH master check timed out"))??;
            if output.status.success() {
                Ok(())
            } else {
                Err(io::Error::new(
                    io::ErrorKind::ConnectionAborted,
                    "SSH master check failed",
                ))
            }
        })
    }
}



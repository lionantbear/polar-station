//! Host-scoped accounting and the single owner of a terminal child's teardown.
//! A ticket is acquired before blocking spawn dispatch, so cancellation cannot
//! hide a late child from Host shutdown. No polling or output-queue dependency.
use super::BackendPtyEvent;
use anyhow::{Context, Result};
use portable_pty::{ChildKiller, MasterPty};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Counts {
    pub live: usize,
    pub started: usize,
    pub completed: usize,
    pub failed: usize,
    closing: bool,
}

#[derive(Clone)]
pub(crate) struct TerminalLifetimes(watch::Sender<Counts>);
impl Default for TerminalLifetimes {
    fn default() -> Self {
        Self(watch::channel(Counts::default()).0)
    }
}
impl TerminalLifetimes {
    pub(super) fn acquire(&self) -> Result<Ticket> {
        let mut admitted = false;
        self.0.send_modify(|counts| {
            if !counts.closing {
                counts.live += 1;
                counts.started += 1;
                admitted = true;
            }
        });
        anyhow::ensure!(admitted, "Host terminal scope is stopping");
        tracing::info!(target: "polar_station_operation", component="PTY", event="terminal_owner_registered",
            live=self.0.borrow().live, count=1);
        Ok(Ticket {
            scope: self.clone(),
            finished: false,
            child_started: false,
        })
    }

    pub(crate) fn close(&self) {
        self.0.send_modify(|counts| counts.closing = true);
    }

    pub(crate) async fn drain(&self, deadline: Duration) -> Result<()> {
        self.close();
        let start = Instant::now();
        let mut receiver = self.0.subscribe();
        let drained = tokio::time::timeout(deadline, receiver.wait_for(|counts| counts.live == 0))
            .await
            .is_ok();
        let counts = *self.0.borrow();
        let success = drained && counts.live == 0 && counts.failed == 0;
        tracing::info!(target: "polar_station_operation", component="Host",
            event=if success { "terminal_owners_drained" } else { "terminal_owners_drain_failed" },
            wait_us=start.elapsed().as_micros() as u64, live=counts.live, started=counts.started,
            completed=counts.completed, failed=counts.failed);
        anyhow::ensure!(
            success,
            "terminal cleanup incomplete: live={}, completed={}, failed={}, started={}",
            counts.live,
            counts.completed,
            counts.failed,
            counts.started
        );
        Ok(())
    }
}

pub(super) struct Ticket {
    scope: TerminalLifetimes,
    finished: bool,
    child_started: bool,
}
impl Ticket {
    pub(super) fn child_started(&mut self) {
        self.child_started = true;
    }
    pub(super) fn finish(mut self, success: bool) {
        self.release(success);
    }
    fn release(&mut self, success: bool) {
        self.scope.0.send_modify(|counts| {
            counts.live -= 1;
            if success {
                counts.completed += 1;
            } else {
                counts.failed += 1;
            }
        });
        self.finished = true;
    }
}
impl Drop for Ticket {
    fn drop(&mut self) {
        if !self.finished {
            self.release(!self.child_started);
        }
    }
}

pub(super) type SharedMaster = Arc<Mutex<Option<Box<dyn MasterPty + Send>>>>;
pub(super) type Stop = oneshot::Sender<()>;

#[derive(Debug)]
pub(crate) struct Cleanup {
    pub child_pid: u32,
    pub signals: usize,
}

pub(super) struct Owner {
    pub master: SharedMaster,
    pub killer: Box<dyn ChildKiller + Send + Sync>,
    pub waiter: JoinHandle<BackendPtyEvent>,
    pub input: JoinHandle<()>,
    pub reader: JoinHandle<()>,
    pub drain: Option<Arc<crate::terminal_io::AsyncTerminalFd>>,
    pub ticket: Ticket,
    pub child_pid: u32,
    pub consumer_id: String,
    pub operation_id: String,
}

impl Owner {
    pub(super) fn start(
        self,
    ) -> (
        Stop,
        watch::Receiver<Option<BackendPtyEvent>>,
        oneshot::Receiver<Result<Cleanup>>,
    ) {
        let (stop, stopped) = oneshot::channel();
        let (exit, exited) = watch::channel(None);
        let (done, completion) = oneshot::channel();
        tokio::spawn(async move {
            let result = self.run(stopped, exit).await;
            let _ = done.send(result);
        });
        (stop, exited, completion)
    }

    async fn run(
        mut self,
        mut stopped: oneshot::Receiver<()>,
        exit: watch::Sender<Option<BackendPtyEvent>>,
    ) -> Result<Cleanup> {
        let lifetime = Instant::now();
        let mut signals = 0;
        let mut stop_time = None;
        let mut stop_received = false;
        let mut signal_failed = false;
        let mut io_failed = None;
        let reaped = tokio::select! {
            biased;
            result = &mut self.waiter => result,
            _ = &mut stopped => {
                stop_received = true;
                stop_time = Some(Instant::now());
                self.input.abort();
                self.reader.abort();
                tracing::info!(target: "polar_station_operation", component="PTY", event="terminal_stop_requested",
                    child_pid=self.child_pid, consumer_id=self.consumer_id.as_str(), operation_id=self.operation_id.as_str());
                if !self.waiter.is_finished() {
                    signals = 1;
                    signal_failed = self.killer.kill().is_err();
                    tracing::info!(target: "polar_station_operation", component="PTY", event="terminal_stop_signal",
                        child_pid=self.child_pid, operation_id=self.operation_id.as_str(), count=1, failed=signal_failed);
                }
                io_failed = Some(self.join_io().await);
                self.reap_while_draining().await
            }
        };
        let wait_failed = !matches!(&reaped, Ok(BackendPtyEvent::Exited { error: None, .. }));
        let event = reaped.unwrap_or_else(|_| BackendPtyEvent::Exited {
            code: None,
            signal: None,
            error: Some("terminal wait task failed".into()),
        });
        tracing::info!(target: "polar_station_operation", component="PTY", event="terminal_child_reaped",
            child_pid=self.child_pid, consumer_id=self.consumer_id.as_str(), operation_id=self.operation_id.as_str(),
            failed=wait_failed, lifetime_us=lifetime.elapsed().as_micros() as u64,
            stop_wait_us=stop_time.map(|t| t.elapsed().as_micros() as u64), count=1);
        // Never enqueue lifecycle completion behind terminal output. Keep I/O
        // available until the consumer observes exit or requests retirement.
        exit.send_replace(Some(event));
        if !stop_received {
            let _ = stopped.await;
        }
        let io_failed = match io_failed {
            Some(failed) => failed,
            None => self.join_io().await,
        };
        let close_start = Instant::now();
        tracing::info!(target: "polar_station_operation", component="PTY", event="terminal_master_close_dispatch",
            child_pid=self.child_pid, operation_id=self.operation_id.as_str());
        let operation_id = self.operation_id.clone();
        // Keep the master until child reap and I/O joins. Attribute dispatch
        // wait separately from the actual close syscall on the blocking pool.
        let close = tokio::task::spawn_blocking(move || {
            let work_start = Instant::now();
            tracing::info!(target: "polar_station_operation", component="PTY", event="terminal_master_close_begin",
                child_pid=self.child_pid, operation_id=operation_id.as_str(), wait_us=close_start.elapsed().as_micros() as u64);
            drop(self.drain);
            drop(self.master.lock().unwrap().take());
            tracing::info!(target: "polar_station_operation", component="PTY", event="terminal_master_close_end",
                child_pid=self.child_pid, operation_id=operation_id.as_str(), work_us=work_start.elapsed().as_micros() as u64, count=1);
        }).await;
        tracing::info!(target: "polar_station_operation", component="PTY", event="terminal_master_close_joined",
            child_pid=self.child_pid, operation_id=self.operation_id.as_str(), wait_us=close_start.elapsed().as_micros() as u64, failed=close.is_err());
        // A racing natural exit can make SIGHUP report ESRCH. Successful wait
        // is authoritative; the signal failure remains visible in diagnostics.
        let success = !wait_failed && !io_failed && close.is_ok();
        let cleanup = Cleanup {
            child_pid: self.child_pid,
            signals,
        };
        tracing::info!(target: "polar_station_operation", component="PTY", event="terminal_owner_completed",
            child_pid=self.child_pid, consumer_id=self.consumer_id.as_str(), operation_id=self.operation_id.as_str(),
            signals=cleanup.signals, reaped_pid=cleanup.child_pid, signal_failed, lifetime_us=lifetime.elapsed().as_micros() as u64, failed=!success);
        self.ticket.finish(success);
        close.context("terminal master close task failed")?;
        anyhow::ensure!(success, "terminal child or I/O cleanup failed");
        Ok(cleanup)
    }

    async fn join_io(&mut self) -> bool {
        let join_start = Instant::now();
        self.input.abort();
        self.reader.abort();
        let input = (&mut self.input).await;
        tracing::info!(target: "polar_station_operation", component="PTY", event="terminal_input_joined",
            child_pid=self.child_pid, operation_id=self.operation_id.as_str(), wait_us=join_start.elapsed().as_micros() as u64, count=1);
        let reader_start = Instant::now();
        let reader = (&mut self.reader).await;
        tracing::info!(target: "polar_station_operation", component="PTY", event="terminal_reader_joined",
            child_pid=self.child_pid, operation_id=self.operation_id.as_str(), wait_us=reader_start.elapsed().as_micros() as u64, count=1);
        let io_failed = [input, reader]
            .into_iter()
            .any(|r| r.is_err_and(|e| !e.is_cancelled()));
        tracing::info!(target: "polar_station_operation", component="PTY", event="terminal_io_joined",
            child_pid=self.child_pid, operation_id=self.operation_id.as_str(),
            wait_us=join_start.elapsed().as_micros() as u64, failed=io_failed, count=2);
        io_failed
    }

    async fn reap_while_draining(
        &mut self,
    ) -> std::result::Result<BackendPtyEvent, tokio::task::JoinError> {
        let start = Instant::now();
        let mut reads = 0_u64;
        let mut bytes = 0_u64;
        tracing::info!(target: "polar_station_operation", component="PTY", event="terminal_retirement_drain_begin",
            child_pid=self.child_pid, operation_id=self.operation_id.as_str());
        let result = if let Some(fd) = self.drain.as_ref() {
            let mut buffer = [0_u8; super::TERMINAL_CHUNK_BYTES];
            loop {
                tokio::select! {
                    biased;
                    result = &mut self.waiter => break result,
                    read = fd.read(&mut buffer) => {
                        reads += 1;
                        match read {
                            Ok(n) if n > 0 => {
                                bytes += n as u64;
                                tracing::info!(target: "polar_station_operation", component="PTY", event="terminal_retirement_drain_read",
                                    child_pid=self.child_pid, operation_id=self.operation_id.as_str(), bytes=n, count=1);
                            }
                            // EOF/error is not readiness to read again. Wait for
                            // reap without an EOF spin or periodic retry timer.
                            _ => break (&mut self.waiter).await,
                        }
                    }
                }
            }
        } else {
            (&mut self.waiter).await
        };
        tracing::info!(target: "polar_station_operation", component="PTY", event="terminal_retirement_drain_end",
            child_pid=self.child_pid, operation_id=self.operation_id.as_str(), bytes, count=reads,
            wait_us=start.elapsed().as_micros() as u64);
        result
    }
}



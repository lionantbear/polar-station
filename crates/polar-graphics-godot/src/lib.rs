//! Thin native adapter. Godot values are converted only on the calling thread;
//! workers own plain Rust strings, numbers, and channels, never Godot objects.
mod file_events;
mod plaque_raster;
mod view_packages;

use godot::classes::{IRefCounted, RefCounted};
use godot::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

struct PolarGraphExtension;

#[gdextension]
unsafe impl ExtensionLibrary for PolarGraphExtension {}

#[derive(GodotClass)]
#[class(base=RefCounted)]
struct PolarForceLayout {
    base: Base<RefCounted>,
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}

#[derive(Clone, Copy)]
enum JobKind {
    Layout,
    BeginInteraction,
    InteractionTick,
}

struct Job {
    kind: JobKind,
    input: String,
    steps: usize,
    generation: i64,
    cancelled: Arc<AtomicBool>,
}

#[derive(Default)]
struct WorkerState {
    pending: Option<Job>,
    result: Option<String>,
    cancellation: Option<Arc<AtomicBool>>,
    running: bool,
    stopping: bool,
}

#[derive(Default)]
struct Shared {
    state: Mutex<WorkerState>,
    wake: Condvar,
}

#[godot_api]
impl IRefCounted for PolarForceLayout {
    fn init(base: Base<RefCounted>) -> Self {
        Self {
            base,
            shared: Arc::new(Shared::default()),
            worker: None,
        }
    }
}

fn checked_steps(steps: i64) -> Result<usize, String> {
    if !(0..=polar_force_layout_core::MAX_STEPS as i64).contains(&steps) {
        Err(format!(
            "steps must be between 0 and {}",
            polar_force_layout_core::MAX_STEPS
        ))
    } else {
        Ok(steps as usize)
    }
}

fn error_json(message: &str) -> String {
    serde_json::json!({"ok":false,"error":message}).to_string()
}

#[godot_api]
impl PolarForceLayout {
    /// Deterministic synchronous API for bounded jobs and integration tests.
    #[func]
    fn layout(&self, graph_json: GString, steps: i64) -> GString {
        let output = match checked_steps(steps) {
            Ok(steps) => polar_force_layout_core::layout_json(&graph_json.to_string(), steps, None),
            Err(message) => error_json(&message),
        };
        GString::from(output.as_str())
    }

    /// Start/replaces an asynchronous native job. No child process or JS runtime.
    /// Poll exactly once per completed job; generation is echoed for stale rejection.
    #[func]
    fn submit(&mut self, graph_json: GString, steps: i64, generation: i64) -> bool {
        self.enqueue(graph_json.to_string(), steps, generation, JobKind::Layout)
    }

    /// Retain the existing core simulation in this instance's single worker.
    #[func]
    fn begin_interaction(&mut self, graph_json: GString, generation: i64) -> bool {
        self.enqueue(
            graph_json.to_string(),
            0,
            generation,
            JobKind::BeginInteraction,
        )
    }

    /// Bounded incremental ticks. The command contains a dragged id/position;
    /// untouched fixed nodes and the core's decay remain authoritative.
    #[func]
    fn interaction_tick(&mut self, command: GString, generation: i64) -> bool {
        self.enqueue(command.to_string(), 2, generation, JobKind::InteractionTick)
    }

    /// Empty string means no result yet. Result snapshots are delivered once.
    #[func]
    fn poll(&mut self) -> GString {
        let mut state = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.running && self.worker.as_ref().is_some_and(JoinHandle::is_finished) {
            state.running = false;
            state.result = Some(error_json("layout worker stopped unexpectedly"));
        }
        state
            .result
            .take()
            .map_or_else(GString::new, |message| GString::from(message.as_str()))
    }

    #[func]
    fn is_running(&self) -> bool {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .running
    }

    #[func]
    fn cancel(&mut self) {
        let mut state = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(flag) = state.cancellation.take() {
            flag.store(true, Ordering::Relaxed);
        }
        state.pending = None;
        state.result = None;
        state.running = false;
    }

    #[func]
    fn upstream_revision(&self) -> GString {
        "d3-force-3d@263b3bfc37578e541a7d7e59dbb32d026d66fa38".into()
    }
}

impl PolarForceLayout {
    fn enqueue(&mut self, input: String, steps: i64, generation: i64, kind: JobKind) -> bool {
        self.cancel();
        let steps = match checked_steps(steps) {
            Ok(value) => value,
            Err(error) => {
                self.shared
                    .state
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .result = Some(
                    serde_json::json!({"ok":false,"error":error,"generation":generation})
                        .to_string(),
                );
                return false;
            }
        };
        if self.worker.is_none() {
            let shared = Arc::clone(&self.shared);
            match std::thread::Builder::new()
                .name("polar-force-layout".into())
                .spawn(move || worker_loop(shared))
            {
                Ok(handle) => self.worker = Some(handle),
                Err(_) => return false,
            }
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut state = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
        state.cancellation = Some(Arc::clone(&cancelled));
        state.pending = Some(Job {
            input,
            steps,
            generation,
            cancelled,
            kind,
        });
        state.running = true;
        self.shared.wake.notify_one();
        true
    }
}

/// Worker-private retained state. No Godot values cross this boundary.
fn interactive_job(
    simulation: &mut Option<(i64, polar_force_layout_core::Simulation, usize)>,
    job: &Job,
) -> Result<serde_json::Value, String> {
    if matches!(job.kind, JobKind::BeginInteraction) {
        let input = serde_json::from_str(&job.input).map_err(|e| format!("invalid graph: {e}"))?;
        *simulation = Some((
            job.generation,
            polar_force_layout_core::Simulation::new(input)?,
            0,
        ));
    }
    let (generation, sim, reheated_at) = simulation
        .as_mut()
        .ok_or("interaction is not initialized")?;
    if *generation != job.generation {
        return Err("stale interaction generation".into());
    }
    if matches!(job.kind, JobKind::InteractionTick) {
        let command: serde_json::Value =
            serde_json::from_str(&job.input).map_err(|e| format!("invalid command: {e}"))?;
        if let Some(id) = command["id"].as_str() {
            let position = command["position"]
                .as_array()
                .filter(|v| v.len() == 3)
                .ok_or("invalid drag position")?;
            let mut values = [0.0; 3];
            for (index, value) in position.iter().enumerate() {
                values[index] = value
                    .as_f64()
                    .filter(|v| v.is_finite() && v.abs() <= 1.0e12)
                    .ok_or("invalid drag coordinate")?;
            }
            let node = sim
                .nodes
                .iter_mut()
                .find(|node| node.id == id)
                .ok_or("unknown dragged node")?;
            node.fx = Some(values[0]);
            node.fy = Some(values[1]);
            node.fz = Some(values[2]);
            sim.alpha = sim.alpha.max(0.12);
            *reheated_at = sim.iterations;
        }
    }
    let cancelled = sim.run(job.steps, Some(&job.cancelled))?;
    let output = sim.output(cancelled);
    let settled = sim.iterations.saturating_sub(*reheated_at) >= 180
        || (sim.alpha < 0.02
            && sim
                .nodes
                .iter()
                .all(|node| node.vx.abs() + node.vy.abs() + node.vz.abs() < 0.005));
    let mut value = serde_json::to_value(output).map_err(|e| e.to_string())?;
    value["interactive"] = true.into();
    value["settled"] = settled.into();
    Ok(value)
}

impl Drop for PolarForceLayout {
    fn drop(&mut self) {
        {
            let mut state = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(flag) = &state.cancellation {
                flag.store(true, Ordering::Relaxed);
            }
            state.stopping = true;
            state.pending = None;
            self.shared.wake.notify_one();
        }
        // Workers must finish before the extension library may be unloaded.
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

fn worker_loop(shared: Arc<Shared>) {
    let mut simulation = None;
    loop {
        let job = {
            let mut state = shared.state.lock().unwrap_or_else(|p| p.into_inner());
            while state.pending.is_none() && !state.stopping {
                state = shared.wake.wait(state).unwrap_or_else(|p| p.into_inner());
            }
            if state.stopping {
                return;
            }
            state
                .pending
                .take()
                .expect("pending job checked while locked")
        };
        let mut value = if matches!(job.kind, JobKind::Layout) {
            simulation = None;
            let output =
                polar_force_layout_core::layout_json(&job.input, job.steps, Some(&job.cancelled));
            serde_json::from_str(&output).unwrap_or_else(
                |_| serde_json::json!({"ok":false,"error":"invalid worker response"}),
            )
        } else {
            interactive_job(&mut simulation, &job).unwrap_or_else(
                |error| serde_json::json!({"ok":false,"error":error,"interactive":true}),
            )
        };
        value["generation"] = job.generation.into();
        let mut state = shared.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.stopping {
            return;
        }
        if !job.cancelled.load(Ordering::Relaxed) {
            state.result = Some(value.to_string());
            state.running = false;
            state.cancellation = None;
        }
    }
}



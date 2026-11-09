//! Data-only 3D force layout. No Godot, renderer, JavaScript runtime or timer.
//!
//! Source-conformant adaptation of d3-force-3d, commit
//! 263b3bfc37578e541a7d7e59dbb32d026d66fa38 (MIT, Vasco Asturiano).
//! Spatial partitioning follows d3-octree, commit
//! 28beff983b94757c90b003c51c2c8a053ddd63be (MIT, Vasco Asturiano).
//! See upstream/NOTICE.md for the exact adaptation boundary.

mod octree;

use octree::Octree;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

pub const MAX_NODES: usize = 100_000;
pub const MAX_LINKS: usize = 500_000;
pub const MAX_STEPS: usize = 2_000;
const COORD_LIMIT: f64 = 1.0e12;

pub(crate) fn is_cancelled(cancel: Option<&AtomicBool>) -> bool {
    cancel.is_some_and(|flag| flag.load(Ordering::Relaxed))
}

#[derive(Clone, Debug, Deserialize)]
pub struct NodeInput {
    pub id: String,
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub z: Option<f64>,
    pub vx: Option<f64>,
    pub vy: Option<f64>,
    pub vz: Option<f64>,
    pub fx: Option<f64>,
    pub fy: Option<f64>,
    pub fz: Option<f64>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct LinkInput {
    pub source: String,
    pub target: String,
    pub distance: Option<f64>,
    pub strength: Option<f64>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub seed: u32,
    pub alpha: f64,
    pub alpha_min: f64,
    pub alpha_decay: f64,
    pub alpha_target: f64,
    /// Public d3 convention: fraction removed, not fraction retained.
    pub velocity_decay: f64,
    pub charge_strength: f64,
    pub theta: f64,
    pub distance_min: f64,
    pub distance_max: Option<f64>,
    pub link_distance: f64,
    pub link_strength: Option<f64>,
    pub link_iterations: usize,
    pub center: [f64; 3],
    pub center_strength: f64,
    /// Explicit stable force order. Defaults match three-forcegraph.
    pub forces: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            seed: 1,
            alpha: 1.0,
            alpha_min: 0.0,
            alpha_decay: 0.0228,
            alpha_target: 0.0,
            velocity_decay: 0.4,
            charge_strength: -30.0,
            theta: 0.9,
            distance_min: 1.0,
            distance_max: None,
            link_distance: 30.0,
            link_strength: None,
            link_iterations: 1,
            center: [0.0; 3],
            center_strength: 1.0,
            forces: vec!["link".into(), "charge".into(), "center".into()],
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct GraphInput {
    pub nodes: Vec<NodeInput>,
    #[serde(default)]
    pub links: Vec<LinkInput>,
    #[serde(default)]
    pub config: Config,
}

#[derive(Clone, Debug, Serialize)]
pub struct NodeState {
    pub id: String,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub vx: f64,
    pub vy: f64,
    pub vz: f64,
    pub fx: Option<f64>,
    pub fy: Option<f64>,
    pub fz: Option<f64>,
}

impl NodeState {
    pub(crate) fn position(&self) -> [f64; 3] {
        [self.x, self.y, self.z]
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Position {
    pub id: String,
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct LayoutOutput {
    pub ok: bool,
    pub positions: Vec<Position>,
    pub states: Vec<NodeState>,
    pub iterations: usize,
    pub alpha: f64,
    pub cancelled: bool,
}

#[derive(Clone, Debug)]
struct Link {
    source: usize,
    target: usize,
    distance: f64,
    strength: f64,
    bias: f64,
}

#[derive(Clone, Debug)]
pub struct Lcg(u32);

impl Lcg {
    pub fn new(seed: u32) -> Self {
        Self(seed)
    }

    pub fn next_f64(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        f64::from(self.0) / 4_294_967_296.0
    }

    fn jiggle(&mut self) -> f64 {
        (self.next_f64() - 0.5) * 1.0e-6
    }
}

/// Single owner simulation. Send it to a worker; never share mutable nodes.
pub struct Simulation {
    pub nodes: Vec<NodeState>,
    links: Vec<Link>,
    config: Config,
    rng: Lcg,
    pub alpha: f64,
    pub iterations: usize,
}

fn finite_bounded(value: f64) -> bool {
    value.is_finite() && value.abs() <= COORD_LIMIT
}

impl Simulation {
    pub fn new(input: GraphInput) -> Result<Self, String> {
        if input.nodes.len() > MAX_NODES || input.links.len() > MAX_LINKS {
            return Err("graph exceeds node/link safety budget".into());
        }
        let cfg = &input.config;
        let finite = [cfg.alpha, cfg.alpha_min, cfg.alpha_decay, cfg.alpha_target,
            cfg.velocity_decay, cfg.charge_strength, cfg.theta, cfg.distance_min,
            cfg.link_distance, cfg.center_strength, cfg.center[0], cfg.center[1], cfg.center[2]];
        if finite.iter().any(|v| !finite_bounded(*v))
            || !(0.0..=1.0).contains(&cfg.alpha_decay)
            || !(0.0..=1.0).contains(&cfg.velocity_decay)
            || cfg.alpha < 0.0 || cfg.alpha_min < 0.0 || cfg.alpha_target < 0.0
            || cfg.theta < 0.0 || cfg.distance_min <= 0.0 || cfg.link_distance < 0.0
            || cfg.link_iterations == 0 || cfg.link_iterations > 16
            || cfg.distance_max.is_some_and(|v| !finite_bounded(v) || v <= 0.0)
            || cfg.link_strength.is_some_and(|v| !finite_bounded(v)) {
            return Err("invalid or non-finite simulation configuration".into());
        }
        let mut force_names = HashSet::new();
        for name in &cfg.forces {
            if !matches!(name.as_str(), "link" | "charge" | "center") || !force_names.insert(name) {
                return Err("forces must be unique names: link, charge, center".into());
            }
        }
        let mut ids = HashMap::with_capacity(input.nodes.len());
        let mut nodes = Vec::with_capacity(input.nodes.len());
        let roll = std::f64::consts::PI * (3.0 - 5.0_f64.sqrt());
        let yaw = std::f64::consts::PI * 20.0 / (9.0 + 221.0_f64.sqrt());
        for (i, n) in input.nodes.into_iter().enumerate() {
            if n.id.is_empty() || n.id.len() > 4096 || ids.insert(n.id.clone(), i).is_some() {
                return Err("node IDs must be unique, nonempty strings of at most 4096 bytes".into());
            }
            for value in [n.x, n.y, n.z, n.vx, n.vy, n.vz, n.fx, n.fy, n.fz].into_iter().flatten() {
                if !finite_bounded(value) { return Err(format!("invalid coordinate at node {}", n.id)); }
            }
            // Upstream first applies fixed axes; a missing remaining axis then reinitializes all axes.
            let px = n.fx.or(n.x);
            let py = n.fy.or(n.y);
            let pz = n.fz.or(n.z);
            let p = match (px, py, pz) {
                (Some(x), Some(y), Some(z)) => [x, y, z],
                _ => {
                    let radius = 10.0 * (0.5 + i as f64).cbrt();
                    let a = i as f64 * roll;
                    let b = i as f64 * yaw;
                    [radius * a.sin() * b.cos(), radius * a.cos(), radius * a.sin() * b.sin()]
                }
            };
            let v = match (n.vx, n.vy, n.vz) {
                (Some(x), Some(y), Some(z)) => [x, y, z],
                _ => [0.0; 3],
            };
            nodes.push(NodeState { id: n.id, x: p[0], y: p[1], z: p[2],
                vx: v[0], vy: v[1], vz: v[2], fx: n.fx, fy: n.fy, fz: n.fz });
        }
        let mut counts = vec![0usize; nodes.len()];
        let mut indexed = Vec::with_capacity(input.links.len());
        for l in input.links {
            let s = *ids.get(&l.source).ok_or_else(|| format!("missing source node: {}", l.source))?;
            let t = *ids.get(&l.target).ok_or_else(|| format!("missing target node: {}", l.target))?;
            if l.distance.is_some_and(|d| !finite_bounded(d) || d < 0.0)
                || l.strength.is_some_and(|d| !finite_bounded(d)) {
                return Err("invalid link distance/strength".into());
            }
            counts[s] += 1;
            counts[t] += 1;
            indexed.push((s, t, l));
        }
        let links = indexed.into_iter().map(|(s, t, l)| Link {
            source: s, target: t,
            distance: l.distance.unwrap_or(cfg.link_distance),
            strength: l.strength.or(cfg.link_strength).unwrap_or(1.0 / counts[s].min(counts[t]) as f64),
            bias: counts[s] as f64 / (counts[s] + counts[t]) as f64,
        }).collect();
        Ok(Self { nodes, links, alpha: cfg.alpha, iterations: 0,
            rng: Lcg::new(cfg.seed), config: input.config })
    }

    /// A fixed number of ticks, independent of wall clock and display framerate.
    pub fn tick(&mut self) -> Result<(), String> {
        self.tick_interruptible(None).map(|_| ())
    }

    /// Cancellation can leave a partially computed tick; callers must discard
    /// cancelled snapshots. Checks do not reorder any uncancelled arithmetic.
    fn tick_interruptible(&mut self, cancel: Option<&AtomicBool>) -> Result<bool, String> {
        if is_cancelled(cancel) { return Ok(true); }
        self.alpha += (self.config.alpha_target - self.alpha) * self.config.alpha_decay;
        for name in self.config.forces.clone() {
            let cancelled = match name.as_str() {
                "link" => self.apply_links(cancel),
                "charge" => self.apply_charge(cancel)?,
                "center" => self.apply_center(cancel),
                _ => unreachable!("configuration was validated"),
            };
            if cancelled { return Ok(true); }
        }
        let retain = 1.0 - self.config.velocity_decay;
        for (index, n) in self.nodes.iter_mut().enumerate() {
            if index % 128 == 0 && is_cancelled(cancel) { return Ok(true); }
            if let Some(f) = n.fx { n.x = f; n.vx = 0.0; } else { n.vx *= retain; n.x += n.vx; }
            if let Some(f) = n.fy { n.y = f; n.vy = 0.0; } else { n.vy *= retain; n.y += n.vy; }
            if let Some(f) = n.fz { n.z = f; n.vz = 0.0; } else { n.vz *= retain; n.z += n.vz; }
            if [n.x, n.y, n.z, n.vx, n.vy, n.vz].iter().any(|v| !finite_bounded(*v)) {
                return Err("simulation diverged beyond its finite coordinate budget".into());
            }
        }
        self.iterations += 1;
        Ok(false)
    }

    pub fn run(&mut self, steps: usize, cancel: Option<&AtomicBool>) -> Result<bool, String> {
        if steps > MAX_STEPS { return Err(format!("steps exceeds {MAX_STEPS}")); }
        for _ in 0..steps {
            if is_cancelled(cancel) { return Ok(true); }
            if self.config.alpha_min > 0.0 && self.alpha < self.config.alpha_min { break; }
            if self.tick_interruptible(cancel)? { return Ok(true); }
        }
        Ok(false)
    }

    pub fn output(&self, cancelled: bool) -> LayoutOutput {
        LayoutOutput { ok: true, positions: self.nodes.iter().map(|n| Position {
            id: n.id.clone(), x: n.x, y: n.y, z: n.z }).collect(),
            states: self.nodes.clone(), iterations: self.iterations, alpha: self.alpha, cancelled }
    }

    fn apply_links(&mut self, cancel: Option<&AtomicBool>) -> bool {
        for _ in 0..self.config.link_iterations {
            for (index, link) in self.links.iter().enumerate() {
                if index % 128 == 0 && is_cancelled(cancel) { return true; }
                let s = &self.nodes[link.source];
                let t = &self.nodes[link.target];
                let mut x = t.x + t.vx - s.x - s.vx;
                let mut y = t.y + t.vy - s.y - s.vy;
                let mut z = t.z + t.vz - s.z - s.vz;
                if x == 0.0 { x = self.rng.jiggle(); }
                if y == 0.0 { y = self.rng.jiggle(); }
                if z == 0.0 { z = self.rng.jiggle(); }
                let length = (x * x + y * y + z * z).sqrt();
                let scale = (length - link.distance) / length * self.alpha * link.strength;
                x *= scale; y *= scale; z *= scale;
                // Sequential updates deliberately preserve self-link and duplicate-link behavior.
                self.nodes[link.target].vx -= x * link.bias;
                self.nodes[link.target].vy -= y * link.bias;
                self.nodes[link.target].vz -= z * link.bias;
                self.nodes[link.source].vx += x * (1.0 - link.bias);
                self.nodes[link.source].vy += y * (1.0 - link.bias);
                self.nodes[link.source].vz += z * (1.0 - link.bias);
            }
        }
        false
    }

    fn apply_charge(&mut self, cancel: Option<&AtomicBool>) -> Result<bool, String> {
        if self.nodes.is_empty() { return Ok(false); }
        let Some(tree) = Octree::build(&self.nodes, self.config.charge_strength, cancel)? else {
            return Ok(true);
        };
        let min2 = self.config.distance_min * self.config.distance_min;
        let max2 = self.config.distance_max.map_or(f64::INFINITY, |v| v * v);
        let theta2 = self.config.theta * self.config.theta;
        for i in 0..self.nodes.len() {
            if is_cancelled(cancel) { return Ok(true); }
            let pos = self.nodes[i].position();
            let mut stack = vec![tree.root];
            let mut visited = 0usize;
            while let Some(cell_index) = stack.pop() {
                if visited.is_multiple_of(128) && is_cancelled(cancel) { return Ok(true); }
                visited += 1;
                let cell = &tree.cells[cell_index];
                if cell.value == 0.0 { continue; }
                let mut x = cell.center[0] - pos[0];
                let mut y = cell.center[1] - pos[1];
                let mut z = cell.center[2] - pos[2];
                let mut l = x * x + y * y + z * z;
                let w = cell.upper[0] - cell.lower[0];
                if w * w / theta2 < l {
                    if l < max2 {
                        if x == 0.0 { x = self.rng.jiggle(); l += x * x; }
                        if y == 0.0 { y = self.rng.jiggle(); l += y * y; }
                        if z == 0.0 { z = self.rng.jiggle(); l += z * z; }
                        if l < min2 { l = (min2 * l).sqrt(); }
                        self.nodes[i].vx += x * cell.value * self.alpha / l;
                        self.nodes[i].vy += y * cell.value * self.alpha / l;
                        self.nodes[i].vz += z * cell.value * self.alpha / l;
                    }
                    continue;
                }
                if cell.internal {
                    // d3-octree visit: callbacks see children in 0..7 order.
                    for child in cell.children.iter().rev().flatten() { stack.push(*child); }
                    continue;
                }
                if l >= max2 { continue; }
                if cell.leaves[0] != i || cell.leaves.len() > 1 {
                    if x == 0.0 { x = self.rng.jiggle(); l += x * x; }
                    if y == 0.0 { y = self.rng.jiggle(); l += y * y; }
                    if z == 0.0 { z = self.rng.jiggle(); l += z * z; }
                    if l < min2 { l = (min2 * l).sqrt(); }
                }
                for (leaf_index, &j) in cell.leaves.iter().enumerate() {
                    if leaf_index % 128 == 0 && is_cancelled(cancel) { return Ok(true); }
                    if j != i {
                        let weight = self.config.charge_strength * self.alpha / l;
                        self.nodes[i].vx += x * weight;
                        self.nodes[i].vy += y * weight;
                        self.nodes[i].vz += z * weight;
                    }
                }
            }
        }
        Ok(false)
    }

    fn apply_center(&mut self, cancel: Option<&AtomicBool>) -> bool {
        if self.nodes.is_empty() { return false; }
        let mut sum = [0.0; 3];
        for (index, n) in self.nodes.iter().enumerate() {
            if index % 128 == 0 && is_cancelled(cancel) { return true; }
            sum[0] += n.x; sum[1] += n.y; sum[2] += n.z;
        }
        let count = self.nodes.len() as f64;
        for (axis, value) in sum.iter_mut().enumerate() {
            *value = (*value / count - self.config.center[axis]) * self.config.center_strength;
        }
        for (index, n) in self.nodes.iter_mut().enumerate() {
            if index % 128 == 0 && is_cancelled(cancel) { return true; }
            n.x -= sum[0]; n.y -= sum[1]; n.z -= sum[2];
        }
        false
    }
}

pub fn layout_json(input: &str, steps: usize, cancel: Option<&AtomicBool>) -> String {
    let result = (|| {
        if input.len() > 64 * 1024 * 1024 { return Err("input exceeds 64 MiB".into()); }
        let graph: GraphInput = serde_json::from_str(input).map_err(|e| format!("invalid graph JSON: {e}"))?;
        let mut simulation = Simulation::new(graph)?;
        let cancelled = simulation.run(steps, cancel)?;
        if cancelled {
            // Never copy/serialize a large, partially updated cancelled graph.
            return Ok(serde_json::json!({"ok":true,"positions":[],"states":[],
                "iterations":simulation.iterations,"alpha":simulation.alpha,"cancelled":true}).to_string());
        }
        serde_json::to_string(&simulation.output(cancelled)).map_err(|e| e.to_string())
    })();
    match result {
        Ok(json) => json,
        Err(error) => serde_json::json!({"ok":false,"error":error}).to_string(),
    }
}




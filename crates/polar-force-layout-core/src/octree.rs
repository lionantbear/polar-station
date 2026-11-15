//! 3D Barnes-Hut index. Bounds, insertion order, coincident chains and traversal
//! match pinned d3-octree's add.js, cover.js, visit.js and visitAfter.js.
use crate::{is_cancelled, NodeState};
use std::sync::atomic::AtomicBool;

#[derive(Clone, Debug)]
pub(crate) struct Cell {
    pub lower: [f64; 3],
    pub upper: [f64; 3],
    pub children: [Option<usize>; 8],
    pub leaves: Vec<usize>,
    pub internal: bool,
    pub value: f64,
    pub center: [f64; 3],
}

impl Cell {
    fn leaf(lower: [f64; 3], upper: [f64; 3], node: usize) -> Self {
        Self { lower, upper, children: [None; 8], leaves: vec![node],
            internal: false, value: 0.0, center: [0.0; 3] }
    }
}

pub(crate) struct Octree {
    pub cells: Vec<Cell>,
    pub root: usize,
}

fn octant(point: [f64; 3], middle: [f64; 3]) -> usize {
    usize::from(point[0] >= middle[0]) | (usize::from(point[1] >= middle[1]) << 1)
        | (usize::from(point[2] >= middle[2]) << 2)
}

fn child_bounds(lower: [f64; 3], upper: [f64; 3], slot: usize) -> ([f64; 3], [f64; 3]) {
    let mut lo = lower; let mut hi = upper;
    for axis in 0..3 {
        let mid = (lo[axis] + hi[axis]) / 2.0;
        if slot & (1 << axis) != 0 { lo[axis] = mid; } else { hi[axis] = mid; }
    }
    (lo, hi)
}

impl Octree {
    pub fn build(nodes: &[NodeState], strength: f64, cancel: Option<&AtomicBool>) -> Result<Option<Self>, String> {
        if nodes.is_empty() { return Err("empty tree".into()); }
        let mut minimum = [f64::INFINITY; 3]; let mut maximum = [f64::NEG_INFINITY; 3];
        for (index, node) in nodes.iter().enumerate() {
            if index % 128 == 0 && is_cancelled(cancel) { return Ok(None); }
            for (axis, value) in node.position().into_iter().enumerate() {
                minimum[axis] = minimum[axis].min(value); maximum[axis] = maximum[axis].max(value);
            }
        }
        // addAll first covers the global minimum, then maximum. The lower bound
        // therefore stays fixed while all three extents double together.
        let lower = minimum.map(f64::floor);
        let mut side = 1.0;
        while (0..3).any(|axis| maximum[axis] >= lower[axis] + side) {
            side *= 2.0;
        }
        let upper = lower.map(|v| v + side);
        let mut result = Self { cells: vec![Cell::leaf(lower, upper, 0)], root: 0 };
        for i in 1..nodes.len() {
            if i % 128 == 0 && is_cancelled(cancel) { return Ok(None); }
            result.insert(i, nodes)?;
        }
        // Store coincident insertions in O(1), then restore upstream leaf.next
        // order once. Equal-position members have identical octant/center, so
        // this changes no insertion decision or floating-point force order.
        for cell in &mut result.cells { cell.leaves.reverse(); }
        // Postorder preserving child aggregation order 0..7 (floating point matters).
        let mut pending = vec![0usize]; let mut order = Vec::new();
        while let Some(index) = pending.pop() {
            if order.len() % 128 == 0 && is_cancelled(cancel) { return Ok(None); }
            order.push(index);
            for child in result.cells[index].children.iter().flatten() { pending.push(*child); }
        }
        for (ordinal, index) in order.into_iter().rev().enumerate() {
            if ordinal % 128 == 0 && is_cancelled(cancel) { return Ok(None); }
            if result.cells[index].internal {
                let mut total = 0.0; let mut weight = 0.0; let mut center = [0.0; 3];
                for child in result.cells[index].children.iter().flatten() {
                    let cell = &result.cells[*child]; let w = cell.value.abs();
                    if w == 0.0 { continue; }
                    total += cell.value; weight += w;
                    for (axis, value) in center.iter_mut().enumerate() { *value += w * cell.center[axis]; }
                }
                // Upstream's dimensional scaling: sqrt(4 / 8) in three dimensions.
                result.cells[index].value = total * (4.0_f64 / 8.0).sqrt();
                result.cells[index].center = if weight == 0.0 { [0.0; 3] } else { center.map(|v| v / weight) };
            } else {
                let mut total = 0.0;
                for leaf_index in 0..result.cells[index].leaves.len() {
                    if leaf_index % 128 == 0 && is_cancelled(cancel) { return Ok(None); }
                    total += strength;
                }
                result.cells[index].value = total;
                result.cells[index].center = nodes[result.cells[index].leaves[0]].position();
            }
        }
        Ok(Some(result))
    }

    fn insert(&mut self, new_index: usize, nodes: &[NodeState]) -> Result<(), String> {
        let pos = nodes[new_index].position();
        let mut index = self.root;
        for _ in 0..256 {
            let cell = &self.cells[index];
            let lower = cell.lower; let upper = cell.upper;
            let middle = [(lower[0]+upper[0])/2.0,(lower[1]+upper[1])/2.0,(lower[2]+upper[2])/2.0];
            if !cell.internal {
                let old_pos = nodes[cell.leaves[0]].position();
                if old_pos == pos {
                    self.cells[index].leaves.push(new_index); // reversed once after all insertions
                    return Ok(());
                }
                let old_slot = octant(old_pos,middle);
                let old_leaves = std::mem::take(&mut self.cells[index].leaves);
                self.cells[index].internal = true;
                let (lo, hi) = child_bounds(lower,upper,old_slot);
                let mut old_cell = Cell::leaf(lo,hi,old_leaves[0]); old_cell.leaves=old_leaves;
                let old_index=self.cells.len(); self.cells.push(old_cell);
                self.cells[index].children[old_slot]=Some(old_index);
            }
            let slot = octant(pos,middle);
            if let Some(child)=self.cells[index].children[slot] { index=child; }
            else {
                let (lo,hi)=child_bounds(lower,upper,slot);
                let next=self.cells.len(); self.cells.push(Cell::leaf(lo,hi,new_index));
                self.cells[index].children[slot]=Some(next);
                return Ok(());
            }
        }
        Err("octree subdivision exceeded safe depth; coordinates too close to represent".into())
    }
}




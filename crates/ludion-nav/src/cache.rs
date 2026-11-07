//! One shared flow field per target node.

use crate::field::FlowField;
use crate::grid::{NavGrid, NodeId};

#[derive(Debug)]
struct Entry {
    target: NodeId,
    field: FlowField,
    used: bool,
}

/// Flow fields keyed by target node, so every agent chasing the same cell shares one search. Keying by
/// node means a moving target recomputes only when it crosses into another cell. Call
/// [`end_frame`](FlowCache::end_frame) once per tick to evict targets nobody asked for. A grid
/// rebuild (its generation changing) drops every field.
#[derive(Debug)]
pub struct FlowCache {
    radius: f32,
    grid_generation: u64,
    entries: Vec<Entry>,
    spare: Vec<FlowField>,
    computes: u64,
}

impl FlowCache {
    /// An empty cache whose fields are capped at `radius` from their target.
    pub fn new(radius: f32) -> FlowCache {
        FlowCache {
            radius,
            grid_generation: 0,
            entries: Vec::new(),
            spare: Vec::new(),
            computes: 0,
        }
    }

    /// The field toward `target`, computing it on a miss, and marks it used this frame.
    pub fn field(&mut self, grid: &NavGrid, target: NodeId) -> &FlowField {
        if grid.generation() != self.grid_generation {
            self.grid_generation = grid.generation();
            for e in self.entries.drain(..) {
                self.spare.push(e.field);
            }
        }
        let pos = match self.entries.iter().position(|e| e.target == target) {
            Some(pos) => pos,
            None => {
                let mut field = self.spare.pop().unwrap_or_default();
                field.compute(grid, target, self.radius);
                self.computes += 1;
                self.entries.push(Entry {
                    target,
                    field,
                    used: false,
                });
                self.entries.len() - 1
            }
        };
        let entry = &mut self.entries[pos];
        entry.used = true;
        &entry.field
    }

    /// The field toward the node under `p`, or `None` when `p` has no node.
    pub fn field_at(&mut self, grid: &NavGrid, p: [f32; 3]) -> Option<&FlowField> {
        let target = grid.node_at(p)?;
        Some(self.field(grid, target))
    }

    /// Evicts the fields not asked for since the last call, keeping the rest in order.
    pub fn end_frame(&mut self) {
        for mut e in std::mem::take(&mut self.entries) {
            if e.used {
                e.used = false;
                self.entries.push(e);
            } else {
                self.spare.push(e.field);
            }
        }
    }

    /// How many fields have been computed over the cache's life.
    pub fn computes(&self) -> u64 {
        self.computes
    }

    /// The targets currently cached, in insertion order.
    pub fn targets(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.entries.iter().map(|e| e.target)
    }
}

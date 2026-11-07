//! Bounded Dijkstra flow fields over a [`NavGrid`].

use core::cmp::{Ordering, Reverse};
use core::f32::consts::SQRT_2;
use std::collections::BinaryHeap;

use ludion_core::math::sqrt;

use crate::grid::{NEIGHBORS, NavGrid, NodeId, OPPOSITE};

/// A Dijkstra frontier entry, ordered by `(distance, node)` so pop order is fully pinned.
#[derive(Clone, Copy, Debug, PartialEq)]
struct HeapNode {
    dist: f32,
    node: u32,
}
impl Eq for HeapNode {}
impl Ord for HeapNode {
    fn cmp(&self, other: &Self) -> Ordering {
        self.dist
            .total_cmp(&other.dist)
            .then(self.node.cmp(&other.node))
    }
}
impl PartialOrd for HeapNode {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Per-node distance from the node to a target along the links an agent walks, and the unit flow
/// direction toward it, computed within a radius.
/// Buffers are reused across [`compute`](FlowField::compute) calls.
#[derive(Clone, Debug, Default)]
pub struct FlowField {
    distance: Vec<f32>,
    flow: Vec<[f32; 2]>,
    reached: Vec<u32>,
    heap: BinaryHeap<Reverse<HeapNode>>,
    target: Option<NodeId>,
}

impl FlowField {
    /// Runs a Dijkstra backward from `target` over the links agents walk, so each node gets its
    /// distance to the target, expanding no node farther than `radius` from it, then derives the
    /// flow (the normalised down-gradient) over the reached set. Nodes beyond keep an infinite
    /// distance.
    pub fn compute(&mut self, grid: &NavGrid, target: NodeId, radius: f32) {
        let n = grid.node_count();
        if self.distance.len() != n || self.flow.len() != n {
            self.distance.clear();
            self.distance.resize(n, f32::INFINITY);
            self.flow.clear();
            self.flow.resize(n, [0.0; 2]);
            self.reached.clear();
        } else {
            for &node in &self.reached {
                self.distance[node as usize] = f32::INFINITY;
                self.flow[node as usize] = [0.0; 2];
            }
            self.reached.clear();
        }
        self.heap.clear();
        self.target = Some(target);
        if target.0 as usize >= n {
            return;
        }

        self.distance[target.0 as usize] = 0.0;
        self.reached.push(target.0);
        self.heap.push(Reverse(HeapNode {
            dist: 0.0,
            node: target.0,
        }));
        while let Some(Reverse(node)) = self.heap.pop() {
            if node.dist > self.distance[node.node as usize] {
                continue;
            }
            if node.dist > radius {
                continue;
            }
            // Relax over in-links: the distance is from `x` to the target along the links `x`
            // walks, and the grid's corner-cut rule makes links asymmetric.
            let (ui, uj) = grid.node_column(node.node);
            let (nx, nz) = (grid.params().nx as i32, grid.params().nz as i32);
            for (dir, ((di, dj), _cost)) in NEIGHBORS.into_iter().enumerate() {
                let (xi, xj) = (ui as i32 + di, uj as i32 + dj);
                if xi < 0 || xj < 0 || xi >= nx || xj >= nz {
                    continue;
                }
                for x in grid.column_nodes(xi as u32, xj as u32) {
                    if grid.raw_links(x)[OPPOSITE[dir]] != node.node {
                        continue;
                    }
                    let nd = node.dist + grid.raw_costs(x)[OPPOSITE[dir]];
                    if nd < self.distance[x as usize] {
                        if !self.distance[x as usize].is_finite() {
                            self.reached.push(x);
                        }
                        self.distance[x as usize] = nd;
                        self.heap.push(Reverse(HeapNode { dist: nd, node: x }));
                    }
                }
            }
        }

        for &here in &self.reached {
            let d0 = self.distance[here as usize];
            if !d0.is_finite() {
                continue;
            }
            let links = grid.raw_links(here);
            let mut acc = [0.0f32, 0.0];
            for (dir, ((di, dj), _cost)) in NEIGHBORS.into_iter().enumerate() {
                let l = links[dir];
                if l == crate::grid::NO_LINK {
                    continue;
                }
                let dn = self.distance[l as usize];
                if !dn.is_finite() || dn >= d0 {
                    continue;
                }
                let w = d0 - dn;
                let len = if di != 0 && dj != 0 { SQRT_2 } else { 1.0 };
                acc[0] += w * di as f32 / len;
                acc[1] += w * dj as f32 / len;
            }
            let mag = sqrt(acc[0] * acc[0] + acc[1] * acc[1]);
            if mag > 1e-6 {
                self.flow[here as usize] = [acc[0] / mag, acc[1] / mag];
            }
        }
    }

    /// The target of the last [`compute`](FlowField::compute), if any.
    pub fn target(&self) -> Option<NodeId> {
        self.target
    }

    /// Distance from `n` to the target along the links an agent walks; infinite when unreached or
    /// out of range.
    pub fn distance(&self, n: NodeId) -> f32 {
        self.distance
            .get(n.0 as usize)
            .copied()
            .unwrap_or(f32::INFINITY)
    }

    /// The unit flow direction `(dx, dz)` at `n`; `[0, 0]` when degenerate or unreached.
    pub fn flow(&self, n: NodeId) -> [f32; 2] {
        self.flow.get(n.0 as usize).copied().unwrap_or([0.0; 2])
    }

    /// The nodes the last search reached, in first-relaxation order.
    pub fn reached(&self) -> &[u32] {
        &self.reached
    }

    /// The flow direction at world point `p`, or `None` off the grid, on an unreached node, or where
    /// the flow is degenerate (the caller then falls back to straight-line seek).
    ///
    /// A point in a column with no node under it (a wall skirt) steps toward its lowest-distance
    /// reached neighbour instead, with the search's corner-cut rule.
    pub fn flow_at(&self, grid: &NavGrid, p: [f32; 3]) -> Option<[f32; 2]> {
        let (i, j) = grid.cell_of(p)?;
        match grid.node_at(p) {
            Some(n) => {
                if !self.distance(n).is_finite() {
                    return None;
                }
                let d = self.flow(n);
                (d != [0.0, 0.0]).then_some(d)
            }
            None => self.blocked_cell_flow(grid, i as i32, j as i32, p[1]),
        }
    }

    fn blocked_cell_flow(&self, grid: &NavGrid, i: i32, j: i32, y: f32) -> Option<[f32; 2]> {
        let g = grid.params();
        let near = |i: i32, j: i32| -> Option<u32> {
            if i < 0 || j < 0 || i >= g.nx as i32 || j >= g.nz as i32 {
                return None;
            }
            let mut best: Option<(f32, u32)> = None;
            for n in grid.column_nodes(i as u32, j as u32) {
                let dy = (grid.node_y(n) - y).abs();
                if dy <= g.max_step && best.is_none_or(|(b, _)| dy < b) {
                    best = Some((dy, n));
                }
            }
            best.map(|(_, n)| n)
        };
        let mut best: Option<(f32, [f32; 2])> = None;
        for ((di, dj), _cost) in NEIGHBORS {
            let Some(nb) = near(i + di, j + dj) else {
                continue;
            };
            if di != 0 && dj != 0 && (near(i + di, j).is_none() || near(i, j + dj).is_none()) {
                continue;
            }
            let dn = self.distance(NodeId(nb));
            if !dn.is_finite() || best.is_some_and(|(b, _)| dn >= b) {
                continue;
            }
            let len = if di != 0 && dj != 0 { SQRT_2 } else { 1.0 };
            best = Some((dn, [di as f32 / len, dj as f32 / len]));
        }
        best.map(|(_, dir)| dir)
    }
}

//! Point-to-point A* over a [`NavGrid`], plus the line-of-walk test and string pulling that turn a node
//! path into a few waypoints.

use core::cmp::{Ordering, Reverse};
use core::f32::consts::SQRT_2;
use std::collections::BinaryHeap;

use ludion_core::math::sqrt;

use crate::grid::{NEIGHBORS, NO_LINK, NavGrid, NodeId};
use crate::links::NavLinks;

/// How a [`find_path`] search ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathStatus {
    /// The path reaches the goal node.
    Complete,
    /// The goal was not reached; the path ends at the expanded node nearest to it.
    Partial,
    /// No path, and none was asked for (or the search saw no node at all).
    Unreachable,
}

/// Limits for one [`find_path`] search.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PathOptions {
    /// No node whose path cost exceeds this is expanded.
    pub max_cost: f32,
    /// Stop after this many node expansions.
    pub max_expanded: u32,
    /// Return the closest reached node's path instead of [`PathStatus::Unreachable`].
    pub partial: bool,
}

impl Default for PathOptions {
    fn default() -> Self {
        PathOptions {
            max_cost: f32::INFINITY,
            max_expanded: 20_000,
            partial: true,
        }
    }
}

/// The outcome of [`find_path`].
#[derive(Clone, Debug, PartialEq)]
pub struct PathResult {
    /// How the search ended.
    pub status: PathStatus,
    /// Path cost along the links; infinite when [`PathStatus::Unreachable`].
    pub cost: f32,
    /// The node path from start to end, both included; empty when unreachable.
    pub nodes: Vec<NodeId>,
    /// Nodes expanded by the search.
    pub expanded: u32,
    /// `(i, tag)` for each off-mesh link the path crosses: `nodes[i]` to `nodes[i + 1]` is link `tag`;
    /// ascending `i`. Empty for [`find_path`].
    pub links: Vec<(u32, u32)>,
}

/// An open-set entry ordered by `(f, node, g)` so pop order is fully pinned.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Open {
    f: f32,
    g: f32,
    node: u32,
}
impl Eq for Open {}
impl Ord for Open {
    fn cmp(&self, other: &Self) -> Ordering {
        self.f
            .total_cmp(&other.f)
            .then(self.node.cmp(&other.node))
            .then(self.g.total_cmp(&other.g))
    }
}
impl PartialOrd for Open {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Octile distance between two nodes' columns; never exceeds the cost of any link path between them.
fn octile(grid: &NavGrid, a: u32, b: u32) -> f32 {
    let ((ai, aj), (bi, bj)) = (grid.node_column(a), grid.node_column(b));
    let dx = ai.abs_diff(bi) as f32;
    let dz = aj.abs_diff(bj) as f32;
    let (hi, lo) = if dx > dz { (dx, dz) } else { (dz, dx) };
    (hi + (SQRT_2 - 1.0) * lo) * grid.params().cell_size
}

/// Horizontal distance between two nodes' column centres; admissible when off-mesh links cost at least the
/// distance between their ends.
fn column_dist(grid: &NavGrid, a: u32, b: u32) -> f32 {
    let ((ai, aj), (bi, bj)) = (grid.node_column(a), grid.node_column(b));
    let dx = ai.abs_diff(bi) as f32;
    let dz = aj.abs_diff(bj) as f32;
    sqrt(dx * dx + dz * dz) * grid.params().cell_size
}

/// A* from `from` to `to` over the links an agent walks. Out-of-range ids are [`PathStatus::Unreachable`].
///
/// ```
/// use ludion_nav::{ColumnSource, FloorSample, GridParams, NavGrid, PathOptions, PathStatus, find_path};
///
/// struct Flat;
/// impl ColumnSource for Flat {
///     fn column(&self, _i: u32, _j: u32, _c: [f32; 2], out: &mut Vec<FloorSample>) {
///         out.push(FloorSample { y: 0.0, normal: [0.0, 1.0, 0.0], headroom: f32::INFINITY });
///     }
/// }
/// let params = GridParams {
///     origin: [0.0, 0.0],
///     cell_size: 1.0,
///     nx: 4,
///     nz: 1,
///     agent_height: 1.8,
///     max_step: 0.3,
///     max_slope_deg: 45.0,
/// };
/// let grid = NavGrid::build(params, &Flat);
/// let a = grid.node_at([0.5, 0.0, 0.5]).unwrap();
/// let b = grid.node_at([3.5, 0.0, 0.5]).unwrap();
/// let path = find_path(&grid, a, b, &PathOptions::default());
/// assert_eq!(path.status, PathStatus::Complete);
/// assert_eq!(path.cost, 3.0);
/// ```
pub fn find_path(grid: &NavGrid, from: NodeId, to: NodeId, opts: &PathOptions) -> PathResult {
    find_path_linked(grid, &NavLinks::default(), from, to, opts)
}

/// [`find_path`] that also crosses the off-mesh edges of `links` (grid steps are relaxed first, then the
/// node's link edges in table order). With an empty table the result equals [`find_path`]'s.
///
/// ```
/// use ludion_nav::{
///     ColumnSource, FloorSample, GridParams, NavGrid, NavLinks, OffMeshLink, PathOptions, PathStatus,
///     find_path_linked,
/// };
///
/// struct Flat;
/// impl ColumnSource for Flat {
///     fn column(&self, i: u32, _j: u32, _c: [f32; 2], out: &mut Vec<FloorSample>) {
///         // A wall of no floor at column 2 splits the strip.
///         if i != 2 {
///             out.push(FloorSample { y: 0.0, normal: [0.0, 1.0, 0.0], headroom: f32::INFINITY });
///         }
///     }
/// }
/// let params = GridParams {
///     origin: [0.0, 0.0],
///     cell_size: 1.0,
///     nx: 5,
///     nz: 1,
///     agent_height: 1.8,
///     max_step: 0.3,
///     max_slope_deg: 45.0,
/// };
/// let grid = NavGrid::build(params, &Flat);
/// let a = grid.node_at([1.5, 0.0, 0.5]).unwrap();
/// let b = grid.node_at([3.5, 0.0, 0.5]).unwrap();
/// let opts = PathOptions { partial: false, ..PathOptions::default() };
/// assert_eq!(find_path_linked(&grid, &NavLinks::default(), a, b, &opts).status, PathStatus::Unreachable);
/// let links = NavLinks::new(vec![OffMeshLink { from: a, to: b, cost: 4.0, bidirectional: false, tag: 0 }]);
/// let path = find_path_linked(&grid, &links, a, b, &opts);
/// assert_eq!((path.status, path.cost, path.links), (PathStatus::Complete, 4.0, vec![(0, 0)]));
/// ```
pub fn find_path_linked(
    grid: &NavGrid,
    links: &NavLinks,
    from: NodeId,
    to: NodeId,
    opts: &PathOptions,
) -> PathResult {
    let h_of = |a: u32, b: u32| {
        if links.is_empty() {
            octile(grid, a, b)
        } else {
            column_dist(grid, a, b)
        }
    };
    let n = grid.node_count();
    let unreachable = |expanded| PathResult {
        status: PathStatus::Unreachable,
        cost: f32::INFINITY,
        nodes: Vec::new(),
        expanded,
        links: Vec::new(),
    };
    if from.0 as usize >= n || to.0 as usize >= n {
        return unreachable(0);
    }
    if from == to {
        return PathResult {
            status: PathStatus::Complete,
            cost: 0.0,
            nodes: vec![from],
            expanded: 0,
            links: Vec::new(),
        };
    }

    let mut g = vec![f32::INFINITY; n];
    let mut parent = vec![NO_LINK; n];
    let mut via = vec![NO_LINK; n];
    let mut heap = BinaryHeap::new();
    g[from.0 as usize] = 0.0;
    heap.push(Reverse(Open {
        f: h_of(from.0, to.0),
        g: 0.0,
        node: from.0,
    }));
    let mut expanded = 0u32;
    // (heuristic to goal, g, node) of the best expanded node so far.
    let mut best = (f32::INFINITY, f32::INFINITY, from.0);
    let mut reached = false;
    while let Some(Reverse(open)) = heap.pop() {
        if open.g > g[open.node as usize] {
            continue;
        }
        if open.node == to.0 {
            reached = true;
            break;
        }
        if expanded >= opts.max_expanded {
            break;
        }
        expanded += 1;
        let h = h_of(open.node, to.0);
        if (h, open.g, open.node)
            .partial_cmp(&best)
            .is_some_and(|o| o == Ordering::Less)
        {
            best = (h, open.g, open.node);
        }
        let (steps, costs) = (grid.raw_links(open.node), grid.raw_costs(open.node));
        for dir in 0..NEIGHBORS.len() {
            let l = steps[dir];
            if l == NO_LINK {
                continue;
            }
            let ng = open.g + costs[dir];
            if ng > opts.max_cost || ng >= g[l as usize] {
                continue;
            }
            g[l as usize] = ng;
            parent[l as usize] = open.node;
            via[l as usize] = NO_LINK;
            heap.push(Reverse(Open {
                f: ng + h_of(l, to.0),
                g: ng,
                node: l,
            }));
        }
        for edge in links.out_edges(NodeId(open.node)) {
            let l = edge.to.0;
            if l as usize >= n {
                continue;
            }
            let ng = open.g + edge.cost;
            if ng > opts.max_cost || ng >= g[l as usize] {
                continue;
            }
            g[l as usize] = ng;
            parent[l as usize] = open.node;
            via[l as usize] = edge.tag;
            heap.push(Reverse(Open {
                f: ng + h_of(l, to.0),
                g: ng,
                node: l,
            }));
        }
    }

    let end = if reached {
        to.0
    } else if opts.partial {
        best.2
    } else {
        return unreachable(expanded);
    };
    let mut nodes = vec![NodeId(end)];
    let mut tags = vec![via[end as usize]];
    let mut at = end;
    while parent[at as usize] != NO_LINK {
        at = parent[at as usize];
        nodes.push(NodeId(at));
        tags.push(via[at as usize]);
    }
    nodes.reverse();
    tags.reverse();
    // `tags[k]` is how `nodes[k]` was reached; step `k - 1 -> k` is a link when it is not a grid step.
    let crossed: Vec<(u32, u32)> = tags
        .iter()
        .enumerate()
        .skip(1)
        .filter(|(_, t)| **t != NO_LINK)
        .map(|(k, t)| (k as u32 - 1, *t))
        .collect();
    PathResult {
        status: if reached {
            PathStatus::Complete
        } else {
            PathStatus::Partial
        },
        cost: g[end as usize],
        nodes,
        expanded,
        links: crossed,
    }
}

/// Walks the xz segment from `start` toward `to` cell by cell (Amanatides–Woo), following the current
/// node's link for each step; returns the node reached in `to`'s column.
fn walk(grid: &NavGrid, start: NodeId, from: [f32; 3], to: [f32; 3]) -> Option<NodeId> {
    let (ei, ej) = grid.cell_of(to)?;
    let (mut ci, mut cj) = grid.cell_of(from)?;
    let p = grid.params();
    let cs = p.cell_size;
    let (ux, uz) = ((from[0] - p.origin[0]) / cs, (from[2] - p.origin[1]) / cs);
    let (dx, dz) = ((to[0] - from[0]) / cs, (to[2] - from[2]) / cs);
    let step = |d: f32| if d > 0.0 { 1i32 } else { -1 };
    let (sx, sz) = (step(dx), step(dz));
    let t_max = |u: f32, c: u32, s: i32, d: f32| {
        if d == 0.0 {
            f32::INFINITY
        } else {
            let edge = if s > 0 { c as f32 + 1.0 } else { c as f32 };
            (edge - u) / d
        }
    };
    let (mut tx, mut tz) = (t_max(ux, ci, sx, dx), t_max(uz, cj, sz, dz));
    let (ddx, ddz) = (
        if dx == 0.0 {
            f32::INFINITY
        } else {
            1.0 / dx.abs()
        },
        if dz == 0.0 {
            f32::INFINITY
        } else {
            1.0 / dz.abs()
        },
    );
    let mut node = start;
    let limit = ci.abs_diff(ei) + cj.abs_diff(ej) + 1;
    for _ in 0..limit {
        if (ci, cj) == (ei, ej) {
            return Some(node);
        }
        let (mx, mz) = if tx == tz {
            (true, true)
        } else {
            (tx < tz, tz < tx)
        };
        let (di, dj) = (if mx { sx } else { 0 }, if mz { sz } else { 0 });
        if mx {
            tx += ddx;
        }
        if mz {
            tz += ddz;
        }
        let dir = NEIGHBORS.iter().position(|(o, _)| *o == (di, dj))?;
        node = grid.link(node, dir)?;
        ci = (ci as i32 + di) as u32;
        cj = (cj as i32 + dj) as u32;
    }
    ((ci, cj) == (ei, ej)).then_some(node)
}

/// Whether an agent can walk the straight xz line from `from` to `to` along the grid's links: every
/// cell the segment crosses must be reachable from the previous one (a segment through a cell corner
/// takes the diagonal link). False when `from` has no node or `to` is off the grid.
pub fn walkable_line(grid: &NavGrid, from: [f32; 3], to: [f32; 3]) -> bool {
    grid.node_at(from)
        .is_some_and(|start| walk(grid, start, from, to).is_some())
}

/// Reduces a node path to the waypoints a straight-line walker needs: greedily extends each leg while
/// [`walkable_line`] holds, and always keeps the first and last node. Points are node positions.
pub fn string_pull(grid: &NavGrid, nodes: &[NodeId]) -> Vec<[f32; 3]> {
    let Some(&first) = nodes.first() else {
        return Vec::new();
    };
    let last = nodes.len() - 1;
    let mut out = vec![grid.node_pos(first)];
    let (mut anchor, mut j) = (0usize, 1usize);
    while j <= last {
        let (a, b) = (grid.node_pos(nodes[anchor]), grid.node_pos(nodes[j]));
        if walk(grid, nodes[anchor], a, b) == Some(nodes[j]) {
            j += 1;
            continue;
        }
        let k = (j - 1).max(anchor + 1);
        out.push(grid.node_pos(nodes[k]));
        anchor = k;
        j = k + 1;
    }
    if anchor != last {
        out.push(grid.node_pos(nodes[last]));
    }
    out
}

/// [`string_pull`] across off-mesh links: the node path is split at each link step, every grid run is pulled
/// on its own and the runs are concatenated. Returns the waypoints and, per link, `(waypoint index of the
/// link start, tag)`; the link end is the next waypoint. With no links it equals `string_pull(grid, &path.nodes)`.
pub fn string_pull_linked(grid: &NavGrid, path: &PathResult) -> (Vec<[f32; 3]>, Vec<(u32, u32)>) {
    let mut points = Vec::new();
    let mut starts = Vec::new();
    let mut from = 0usize;
    for &(i, tag) in &path.links {
        let i = i as usize;
        points.extend(string_pull(grid, &path.nodes[from..=i]));
        starts.push((points.len() as u32 - 1, tag));
        from = i + 1;
    }
    points.extend(string_pull(grid, &path.nodes[from..]));
    (points, starts)
}

/// The node nearest `p` within `max_dist` (3D distance to the node position): the node under `p` when
/// there is one, else the closest in the surrounding columns, ties by node id.
pub fn nearest_node(grid: &NavGrid, p: [f32; 3], max_dist: f32) -> Option<NodeId> {
    if let Some(n) = grid.node_at(p) {
        return Some(n);
    }
    let g = grid.params();
    let r = (max_dist / g.cell_size).ceil();
    let ci = ((p[0] - g.origin[0]) / g.cell_size).floor();
    let cj = ((p[2] - g.origin[1]) / g.cell_size).floor();
    if !(ci.is_finite() && cj.is_finite() && r.is_finite()) {
        return None;
    }
    let range =
        |c: f32, n: u32| (c - r).max(0.0) as u32..=((c + r).min(n as f32 - 1.0)).max(0.0) as u32;
    let limit = max_dist * max_dist;
    let mut best: Option<(f32, u32)> = None;
    for j in range(cj, g.nz) {
        for i in range(ci, g.nx) {
            for n in grid.column_nodes(i, j) {
                let q = grid.node_pos(NodeId(n));
                let (dx, dy, dz) = (q[0] - p[0], q[1] - p[1], q[2] - p[2]);
                let d = dx * dx + dy * dy + dz * dz;
                if d <= limit && best.is_none_or(|(b, _)| d < b) {
                    best = Some((d, n));
                }
            }
        }
    }
    best.map(|(_, n)| NodeId(n))
}

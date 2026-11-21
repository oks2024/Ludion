//! Point-to-point A* over a [`NavGrid`], plus the line-of-walk test and string pulling that turn a node
//! path into a few waypoints.

use core::cmp::{Ordering, Reverse};
use core::f32::consts::SQRT_2;
use std::collections::BinaryHeap;

use ludion_core::math::sqrt;

use crate::field::FlowField;
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
            if l as usize >= n || grid.is_blocked(NodeId(l)) {
                continue;
            }
            let ng = open.g + edge.cost + grid.entry_cost(open.node, l);
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

/// How far a cell walk got: the last node reached, the segment parameter where the walk stopped (where
/// the failing step starts, or the end of the segment when it completed), and whether it reached `to`'s cell.
struct WalkTrace {
    node: NodeId,
    t: f32,
    complete: bool,
}

/// Walks the xz segment from `start` toward `to` cell by cell (Amanatides–Woo), following the current
/// node's link for each step. `None` when `from` is off the grid or a point is not finite; a `to` off the
/// grid is a walk that fails at the grid's edge.
fn walk_trace(grid: &NavGrid, start: NodeId, from: [f32; 3], to: [f32; 3]) -> Option<WalkTrace> {
    if !from.iter().chain(to.iter()).all(|c| c.is_finite()) {
        return None;
    }
    let p = grid.params();
    let cs = p.cell_size;
    let (ux, uz) = ((from[0] - p.origin[0]) / cs, (from[2] - p.origin[1]) / cs);
    let (dx, dz) = ((to[0] - from[0]) / cs, (to[2] - from[2]) / cs);
    let (ei, ej) = (
        ((to[0] - p.origin[0]) / cs).floor() as i64,
        ((to[2] - p.origin[1]) / cs).floor() as i64,
    );
    let (ci, cj) = grid.cell_of(from)?;
    let (mut ci, mut cj) = (i64::from(ci), i64::from(cj));
    let step = |d: f32| if d > 0.0 { 1i64 } else { -1 };
    let (sx, sz) = (step(dx), step(dz));
    let t_max = |u: f32, c: i64, s: i64, d: f32| {
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
            return Some(WalkTrace {
                node,
                t: 1.0,
                complete: true,
            });
        }
        let t = tx.min(tz);
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
        let next = NEIGHBORS
            .iter()
            .position(|(o, _)| *o == (di as i32, dj as i32))
            .and_then(|dir| grid.link(node, dir));
        let Some(next) = next else {
            return Some(WalkTrace {
                node,
                t: t.clamp(0.0, 1.0),
                complete: false,
            });
        };
        // A line never walks into an `interact` region from outside: the crossing is its own leg.
        if grid.stop_tag(next).is_some() && grid.stop_tag(node).is_none() {
            return Some(WalkTrace {
                node,
                t: t.clamp(0.0, 1.0),
                complete: false,
            });
        }
        node = next;
        ci += di;
        cj += dj;
    }
    Some(WalkTrace {
        node,
        t: 1.0,
        complete: (ci, cj) == (ei, ej),
    })
}

/// Walks the xz segment from `start` toward `to`; returns the node reached in `to`'s column.
fn walk(grid: &NavGrid, start: NodeId, from: [f32; 3], to: [f32; 3]) -> Option<NodeId> {
    walk_trace(grid, start, from, to)
        .filter(|w| w.complete)
        .map(|w| w.node)
}

/// Where a straight walk from `from` toward `to` leaves the grid's walkable links: the point on the segment
/// where the first step that has no link begins, at the y of the last node reached. `Some(from)` when
/// `from` has no node; `None` when the whole line is walkable (exactly when [`walkable_line`] holds).
pub fn line_exit(grid: &NavGrid, from: [f32; 3], to: [f32; 3]) -> Option<[f32; 3]> {
    let Some(start) = grid.node_at(from) else {
        return Some(from);
    };
    let Some(trace) = walk_trace(grid, start, from, to) else {
        return Some(from);
    };
    if trace.complete {
        return None;
    }
    let y = grid.node_pos(trace.node)[1];
    Some([
        from[0] + trace.t * (to[0] - from[0]),
        y,
        from[2] + trace.t * (to[2] - from[2]),
    ])
}

/// The nodes within `radius` metres of path distance of `start` along the grid links (off-mesh links are not
/// followed), `start` included, ascending by node id. Distances count the links an agent walks to reach the
/// node from `start`.
pub fn reachable_within(grid: &NavGrid, start: NodeId, radius: f32) -> Vec<NodeId> {
    let mut field = FlowField::default();
    field.compute(grid, start, radius);
    let mut nodes: Vec<u32> = field
        .reached()
        .iter()
        .copied()
        .filter(|&n| field.distance(NodeId(n)) <= radius)
        .collect();
    nodes.sort_unstable();
    nodes.into_iter().map(NodeId).collect()
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
    let pulled = string_pull_marked(grid, path);
    (pulled.points, pulled.links)
}

/// A node path pulled to waypoints, with where it must stop.
#[derive(Clone, Debug, PartialEq)]
pub struct PulledPath {
    /// The waypoints, start first.
    pub points: Vec<[f32; 3]>,
    /// `(waypoint index of the link start, tag)` per off-mesh link; the link end is the next waypoint.
    pub links: Vec<(u32, u32)>,
    /// `(waypoint index of the node before the region, tag)` per entry into a stop region (an `interact`
    /// blocker); the next waypoint is the first node inside it.
    pub stops: Vec<(u32, u32)>,
}

/// [`string_pull_linked`] that also splits the path where it steps from a non-stop node into a
/// [`NodeMark::Stop`](crate::NodeMark::Stop) node, so a pulled leg never crosses a stop region. A link step is
/// never also a stop. With no stop marks `links` and `points` equal [`string_pull_linked`]'s.
pub fn string_pull_marked(grid: &NavGrid, path: &PathResult) -> PulledPath {
    let mut splits: Vec<(usize, Option<u32>, Option<u32>)> = path
        .links
        .iter()
        .map(|&(i, t)| (i as usize, Some(t), None))
        .collect();
    if grid.has_overlay() {
        for (k, pair) in path.nodes.windows(2).enumerate() {
            let is_link = path.links.iter().any(|&(i, _)| i as usize == k);
            if !is_link
                && grid.stop_tag(pair[0]).is_none()
                && let Some(tag) = grid.stop_tag(pair[1])
            {
                splits.push((k, None, Some(tag)));
            }
        }
        splits.sort_by_key(|s| s.0);
    }
    let mut out = PulledPath {
        points: Vec::new(),
        links: Vec::new(),
        stops: Vec::new(),
    };
    let mut from = 0usize;
    for (i, link, stop) in splits {
        out.points.extend(string_pull(grid, &path.nodes[from..=i]));
        let at = out.points.len() as u32 - 1;
        if let Some(tag) = link {
            out.links.push((at, tag));
        }
        if let Some(tag) = stop {
            out.stops.push((at, tag));
        }
        from = i + 1;
    }
    out.points.extend(string_pull(grid, &path.nodes[from..]));
    out
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

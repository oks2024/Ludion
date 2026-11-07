//! The layered walkability grid: floor samples per column, nodes, and the links between them.

use core::f32::consts::SQRT_2;
use ludion_core::math::{cos, sqrt};

/// Marks an absent link in [`NavGrid`]'s link table.
pub const NO_LINK: u32 = u32::MAX;

/// The eight grid neighbours: `((di, dj), step cost in cells)`, cardinals first, in a fixed order so the
/// search and the flow accumulation are order-pinned.
pub const NEIGHBORS: [((i32, i32), f32); 8] = [
    ((1, 0), 1.0),
    ((-1, 0), 1.0),
    ((0, 1), 1.0),
    ((0, -1), 1.0),
    ((1, 1), SQRT_2),
    ((1, -1), SQRT_2),
    ((-1, 1), SQRT_2),
    ((-1, -1), SQRT_2),
];

/// The direction index that undoes each [`NEIGHBORS`] step.
pub(crate) const OPPOSITE: [usize; 8] = [1, 0, 3, 2, 7, 6, 5, 4];

/// An upward-facing surface at a column centre.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FloorSample {
    /// Surface height.
    pub y: f32,
    /// Unit surface normal.
    pub normal: [f32; 3],
    /// Free height above the surface (`f32::INFINITY` when open).
    pub headroom: f32,
}

/// Where the floors are. The source reports a floor only where an agent of the intended radius fits;
/// the grid handles height, headroom, step and slope.
pub trait ColumnSource {
    /// Appends the floors of column `(i, j)`, whose centre is `center` (x, z), in any order.
    fn column(&self, i: u32, j: u32, center: [f32; 2], out: &mut Vec<FloorSample>);
}

/// Grid geometry and the agent's movement limits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridParams {
    /// World (x, z) of the minimum corner of cell `(0, 0)`.
    pub origin: [f32; 2],
    /// Cell edge length.
    pub cell_size: f32,
    /// Columns along x.
    pub nx: u32,
    /// Columns along z.
    pub nz: u32,
    /// Minimum headroom for a floor to be walkable.
    pub agent_height: f32,
    /// Largest step up or down on flat ground.
    pub max_step: f32,
    /// Steepest walkable surface, degrees from horizontal.
    pub max_slope_deg: f32,
}

/// A walkable floor in the grid. Ids run row-major by column, then by ascending height.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct NodeId(pub u32);

/// The layered walkability grid. Derived state: rebuild it, never serialise it.
#[derive(Clone, Debug)]
pub struct NavGrid {
    params: GridParams,
    col_sample_start: Vec<u32>,
    samples: Vec<FloorSample>,
    col_node_start: Vec<u32>,
    node_y: Vec<f32>,
    node_normal_y: Vec<f32>,
    node_cell: Vec<u32>,
    links: Vec<[u32; 8]>,
    link_cost: Vec<[f32; 8]>,
    generation: u64,
}

fn tan_of(normal_y: f32) -> f32 {
    let ny = normal_y.clamp(1e-6, 1.0);
    sqrt((1.0 - ny * ny).max(0.0)) / ny
}

impl NavGrid {
    /// Samples every column of `src` and derives nodes and links.
    pub fn build(params: GridParams, src: &impl ColumnSource) -> NavGrid {
        let mut grid = NavGrid {
            params,
            col_sample_start: Vec::new(),
            samples: Vec::new(),
            col_node_start: Vec::new(),
            node_y: Vec::new(),
            node_normal_y: Vec::new(),
            node_cell: Vec::new(),
            links: Vec::new(),
            link_cost: Vec::new(),
            generation: 0,
        };
        let ncol = grid.column_count();
        grid.col_sample_start.reserve(ncol + 1);
        grid.col_sample_start.push(0);
        for c in 0..ncol {
            grid.sample_column(src, c);
            grid.col_sample_start.push(grid.samples.len() as u32);
        }
        grid.derive();
        grid.generation = 1;
        grid
    }

    /// Re-samples the columns in the inclusive cell box `min..=max` (clamped to the grid), then
    /// re-derives. Sampling is the expensive part; deriving is cheap and keeps one code path.
    pub fn rebuild_region(&mut self, src: &impl ColumnSource, min: [u32; 2], max: [u32; 2]) {
        let (nx, nz) = (self.params.nx, self.params.nz);
        if nx > 0 && nz > 0 {
            let max = [max[0].min(nx - 1), max[1].min(nz - 1)];
            let old_start = std::mem::take(&mut self.col_sample_start);
            let old_samples = std::mem::take(&mut self.samples);
            let ncol = self.column_count();
            self.col_sample_start.reserve(ncol + 1);
            self.col_sample_start.push(0);
            for c in 0..ncol {
                let (i, j) = (c as u32 % nx, c as u32 / nx);
                if i >= min[0] && i <= max[0] && j >= min[1] && j <= max[1] {
                    self.sample_column(src, c);
                } else {
                    let run = old_start[c] as usize..old_start[c + 1] as usize;
                    self.samples.extend_from_slice(&old_samples[run]);
                }
                self.col_sample_start.push(self.samples.len() as u32);
            }
        }
        self.derive();
        self.generation += 1;
    }

    fn column_count(&self) -> usize {
        self.params.nx as usize * self.params.nz as usize
    }

    fn center(&self, i: u32, j: u32) -> [f32; 2] {
        let p = &self.params;
        [
            p.origin[0] + (i as f32 + 0.5) * p.cell_size,
            p.origin[1] + (j as f32 + 0.5) * p.cell_size,
        ]
    }

    fn sample_column(&mut self, src: &impl ColumnSource, c: usize) {
        let nx = self.params.nx;
        let (i, j) = (c as u32 % nx, c as u32 / nx);
        let center = self.center(i, j);
        src.column(i, j, center, &mut self.samples);
    }

    fn derive(&mut self) {
        let p = self.params;
        let cos_max = cos(p.max_slope_deg.to_radians());
        let ncol = self.column_count();
        self.col_node_start.clear();
        self.col_node_start.push(0);
        self.node_y.clear();
        self.node_normal_y.clear();
        self.node_cell.clear();
        let mut order: Vec<u32> = Vec::new();
        for c in 0..ncol {
            let run = self.col_sample_start[c] as usize..self.col_sample_start[c + 1] as usize;
            let s = &self.samples[run];
            order.clear();
            order.extend(0..s.len() as u32);
            order.sort_by(|&a, &b| {
                let (a, b) = (&s[a as usize], &s[b as usize]);
                a.y.total_cmp(&b.y)
                    .then(a.normal[1].total_cmp(&b.normal[1]))
                    .then(a.headroom.total_cmp(&b.headroom))
            });
            for &k in &order {
                let f = s[k as usize];
                if f.normal[1] >= cos_max && f.headroom >= p.agent_height {
                    self.node_y.push(f.y);
                    self.node_normal_y.push(f.normal[1]);
                    self.node_cell.push(c as u32);
                }
            }
            self.col_node_start.push(self.node_y.len() as u32);
        }

        let n = self.node_y.len();
        let mut links = vec![[NO_LINK; 8]; n];
        let mut link_cost = vec![[0.0f32; 8]; n];
        for a in 0..n {
            let c = self.node_cell[a] as usize;
            let (ci, cj) = ((c as u32 % p.nx) as i32, (c as u32 / p.nx) as i32);
            for (dir, ((di, dj), cost)) in NEIGHBORS.into_iter().enumerate() {
                let (ni, nj) = (ci + di, cj + dj);
                if ni < 0 || nj < 0 || ni >= p.nx as i32 || nj >= p.nz as i32 {
                    continue;
                }
                if di != 0 && dj != 0 {
                    let card = |d: (i32, i32)| NEIGHBORS.iter().position(|(o, _)| *o == d);
                    let (Some(h), Some(v)) = (card((di, 0)), card((0, dj))) else {
                        continue;
                    };
                    if links[a][h] == NO_LINK || links[a][v] == NO_LINK {
                        continue;
                    }
                }
                let h = p.cell_size * cost;
                let (a_y, a_tan) = (self.node_y[a], tan_of(self.node_normal_y[a]));
                let col = nj as usize * p.nx as usize + ni as usize;
                let mut best: Option<(f32, u32)> = None;
                for b in self.col_node_start[col]..self.col_node_start[col + 1] {
                    let b_i = b as usize;
                    let dy = (self.node_y[b_i] - a_y).abs();
                    let b_tan = tan_of(self.node_normal_y[b_i]);
                    let allowed = p.max_step + h * a_tan.max(b_tan);
                    if dy <= allowed && best.is_none_or(|(d, _)| dy < d) {
                        best = Some((dy, b));
                    }
                }
                if let Some((dy, b)) = best {
                    links[a][dir] = b;
                    link_cost[a][dir] = if self.node_y[b as usize] == a_y {
                        h
                    } else {
                        sqrt(h * h + dy * dy)
                    };
                }
            }
        }
        self.links = links;
        self.link_cost = link_cost;
    }

    /// Bumps on every build and rebuild; a cache keyed by it knows when to drop its fields.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The grid parameters.
    pub fn params(&self) -> &GridParams {
        &self.params
    }

    /// Number of walkable nodes.
    pub fn node_count(&self) -> usize {
        self.node_y.len()
    }

    /// The column `(i, j)` containing the (x, y, z) point's x and z, or `None` off the grid.
    pub fn cell_of(&self, p: [f32; 3]) -> Option<(u32, u32)> {
        let g = &self.params;
        let i = ((p[0] - g.origin[0]) / g.cell_size).floor();
        let j = ((p[2] - g.origin[1]) / g.cell_size).floor();
        (i >= 0.0 && j >= 0.0 && i < g.nx as f32 && j < g.nz as f32).then_some((i as u32, j as u32))
    }

    /// The nodes of column `(i, j)` as an id range, ascending in height.
    pub(crate) fn column_nodes(&self, i: u32, j: u32) -> core::ops::Range<u32> {
        let c = j as usize * self.params.nx as usize + i as usize;
        self.col_node_start[c]..self.col_node_start[c + 1]
    }

    pub(crate) fn node_y(&self, n: u32) -> f32 {
        self.node_y[n as usize]
    }

    /// The node under `p`: in its column, the highest one at most `max_step` above `p.y`.
    pub fn node_at(&self, p: [f32; 3]) -> Option<NodeId> {
        let (i, j) = self.cell_of(p)?;
        let limit = p[1] + self.params.max_step;
        self.column_nodes(i, j)
            .rev()
            .find(|&n| self.node_y[n as usize] <= limit)
            .map(NodeId)
    }

    /// World position of a node: its cell centre and floor height.
    pub fn node_pos(&self, n: NodeId) -> [f32; 3] {
        let c = self.node_cell[n.0 as usize];
        let c = self.center(c % self.params.nx, c / self.params.nx);
        [c[0], self.node_y[n.0 as usize], c[1]]
    }

    /// The node `n` links to in direction `dir` (an index into [`NEIGHBORS`]).
    pub fn link(&self, n: NodeId, dir: usize) -> Option<NodeId> {
        let l = self.links[n.0 as usize][dir];
        (l != NO_LINK).then_some(NodeId(l))
    }

    /// The traversal cost of the link `n` → `dir`, or `None` when unlinked.
    pub fn link_cost(&self, n: NodeId, dir: usize) -> Option<f32> {
        self.link(n, dir).map(|_| self.link_cost[n.0 as usize][dir])
    }

    /// The `(i, j)` column of node `n`.
    pub(crate) fn node_column(&self, n: u32) -> (u32, u32) {
        let c = self.node_cell[n as usize];
        (c % self.params.nx, c / self.params.nx)
    }

    pub(crate) fn raw_links(&self, n: u32) -> &[u32; 8] {
        &self.links[n as usize]
    }

    pub(crate) fn raw_costs(&self, n: u32) -> &[f32; 8] {
        &self.link_cost[n as usize]
    }
}

//! The engine's host for navigation: [`Nav`], [`NavConfig`] and the [`nav_update`] system, which
//! samples a [`ludion_nav::NavGrid`] from the static physics colliders.
//!
//! A game opts in with `Engine::builder().nav(config)` and schedules
//! `.system("nav_update", nav_update)` before the systems that read the grid. The engine installs a
//! [`Nav`] resource at every world build, empty until the first `nav_update`. The grid and its
//! [`FlowCache`] are derived state: not hashed, not snapshotted. A restored world gets a fresh
//! [`Nav`], whose first update re-samples the restored physics.
//!
//! Each column is sampled by physics queries against static colliders only: a down-walk of ray
//! casts finds the floors, an upward ray measures headroom, and a capsule overlap of the agent's
//! radius drops a floor the agent cannot stand on (the grid assumes the source already inflated
//! obstacles by the agent radius). The columns are sampled over [`ludion_core::par`], each a pure
//! function of the physics world and its coordinates, so the grid is identical at any thread count.
//!
//! Point-to-point routing sits beside the shared flow fields: [`nav_path`] and its siblings answer a
//! [`PathRequest`] (A* plus string pulling), and an agent that also carries a `NavGoal` walks a planned
//! `NavRoute` instead of reading a field. The route is hashed sim state; the grid stays derived.

use ludion_components::nav_route::{NavGoal, NavReplan, NavRoute, NavRouteStatus};
use ludion_components::{NavAgent, NavSteering};
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::math::sqrt;
use ludion_core::par;
use ludion_ecs::{SystemCtx, World};
use ludion_nav::{
    ColumnSource, FloorSample, FlowCache, GridParams, NavGrid, NodeId, PathOptions, PathStatus,
    find_path, nearest_node, string_pull, walkable_line,
};
use ludion_physics::{PhysicsError, PhysicsWorld, QueryFilter, StaticBody};
use ludion_spatial::Shape;
use ludion_spatial::spatial3d::{Filter, Handle, Index3d, Neighbor};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::diagnostics::report_full;
use crate::hierarchy::world_transform;

/// Agents per parallel steering chunk.
const STEER_CHUNK: usize = 64;
/// Floors a column keeps at most (a down-walk stops here).
const MAX_COLUMN_LAYERS: usize = 32;
/// Columns per parallel chunk.
const SAMPLE_CHUNK: usize = 64;
/// Most columns a grid may have.
const MAX_COLUMNS: u64 = 1 << 22;

/// The grid's extent and the agent it is built for.
///
/// # Example
/// ```
/// use ludion_engine::NavConfig;
///
/// let config = NavConfig { nx: 32, nz: 32, ..NavConfig::default() };
/// assert!(config.validate().is_ok());
/// assert!(NavConfig { nx: 0, ..config }.validate().is_err());
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NavConfig {
    /// World (x, z) of the minimum corner of cell `(0, 0)`.
    pub origin: [f32; 2],
    /// Cell edge length in metres.
    pub cell_size: f32,
    /// Columns along x.
    pub nx: u32,
    /// Columns along z.
    pub nz: u32,
    /// Lowest world height a floor is looked for at.
    pub y_min: f32,
    /// Highest world height sampled (floors and headroom are measured below it).
    pub y_max: f32,
    /// The agent's radius: a floor is dropped where a capsule of this radius collides.
    pub agent_radius: f32,
    /// The agent's height: a floor needs this much headroom.
    pub agent_height: f32,
    /// Largest step up or down on flat ground.
    pub max_step: f32,
    /// Steepest walkable surface, degrees from horizontal.
    pub max_slope_deg: f32,
    /// How far from its target a flow field reaches, in metres.
    pub flow_radius: f32,
    /// Static-collider layers that are not walkable ground or obstacles (optional; empty by
    /// default). Names come from the scene's `physics.layers`.
    pub ignore_layers: Vec<String>,
    /// Most routes `nav_steer` plans per tick; agents over the cap wait in `planning` until the next
    /// tick (default `8`, at least `1`).
    #[serde(default = "default_max_path_requests")]
    pub max_path_requests_per_tick: u32,
}

fn default_max_path_requests() -> u32 {
    8
}

impl Default for NavConfig {
    fn default() -> Self {
        Self {
            origin: [0.0, 0.0],
            cell_size: 1.0,
            nx: 64,
            nz: 64,
            y_min: -8.0,
            y_max: 32.0,
            agent_radius: 0.4,
            agent_height: 1.8,
            max_step: 0.4,
            max_slope_deg: 45.0,
            flow_radius: 48.0,
            ignore_layers: Vec::new(),
            max_path_requests_per_tick: default_max_path_requests(),
        }
    }
}

impl NavConfig {
    /// Checks the config.
    ///
    /// # Errors
    /// `L4032` if a value is not finite, a size or radius is not positive, the grid has no columns
    /// or more than 4194304, `y_min >= y_max`, the slope is outside `(0, 90)`, a layer name is
    /// empty, or `max_path_requests_per_tick` is 0.
    pub fn validate(&self) -> Result<(), Diagnostic> {
        let invalid = |path: &str, what: String, help: &str| {
            Diagnostic::new(code::NAV_CONFIG_INVALID, what)
                .with_path(path)
                .with_help(help)
        };
        for (i, v) in self.origin.iter().enumerate() {
            if !v.is_finite() {
                return Err(invalid(
                    &format!("/origin/{i}"),
                    format!("nav `origin` component {i} is {v}, not finite"),
                    "use a finite world coordinate",
                ));
            }
        }
        let positive = [
            ("/cell_size", "cell_size", self.cell_size),
            ("/agent_radius", "agent_radius", self.agent_radius),
            ("/agent_height", "agent_height", self.agent_height),
            ("/flow_radius", "flow_radius", self.flow_radius),
        ];
        for (path, name, v) in positive {
            if !v.is_finite() || v <= 0.0 {
                return Err(invalid(
                    path,
                    format!("nav `{name}` is {v}, not a positive finite number"),
                    "use a finite value above zero, in metres",
                ));
            }
        }
        if self.nx == 0 || self.nz == 0 || u64::from(self.nx) * u64::from(self.nz) > MAX_COLUMNS {
            let path = if self.nx == 0 || u64::from(self.nx) > MAX_COLUMNS {
                "/nx"
            } else {
                "/nz"
            };
            return Err(invalid(
                path,
                format!(
                    "nav grid is {} x {} columns, outside 1..={MAX_COLUMNS} columns",
                    self.nx, self.nz
                ),
                "raise `cell_size` or shrink `nx`/`nz`",
            ));
        }
        if !self.y_min.is_finite() || !self.y_max.is_finite() || self.y_min >= self.y_max {
            return Err(invalid(
                "/y_min",
                format!(
                    "nav height range {}..{} is empty or not finite",
                    self.y_min, self.y_max
                ),
                "set `y_min` below `y_max`",
            ));
        }
        if !self.max_step.is_finite() || self.max_step < 0.0 {
            return Err(invalid(
                "/max_step",
                format!(
                    "nav `max_step` is {}, not a finite number >= 0",
                    self.max_step
                ),
                "use the tallest step the agent climbs, in metres",
            ));
        }
        if !(self.max_slope_deg > 0.0 && self.max_slope_deg < 90.0) {
            return Err(invalid(
                "/max_slope_deg",
                format!(
                    "nav `max_slope_deg` is {}, outside (0, 90)",
                    self.max_slope_deg
                ),
                "use the steepest walkable angle in degrees, e.g. 45.0",
            ));
        }
        if let Some(i) = self.ignore_layers.iter().position(String::is_empty) {
            return Err(invalid(
                &format!("/ignore_layers/{i}"),
                "a nav `ignore_layers` name is empty".to_string(),
                "name a layer from the scene's `physics.layers`, or remove the entry",
            ));
        }
        if self.max_path_requests_per_tick == 0 {
            return Err(invalid(
                "/max_path_requests_per_tick",
                "nav `max_path_requests_per_tick` is 0, so no route could ever be planned"
                    .to_string(),
                "use at least 1; the default is 8",
            ));
        }
        Ok(())
    }

    fn grid_params(&self) -> GridParams {
        GridParams {
            origin: self.origin,
            cell_size: self.cell_size,
            nx: self.nx,
            nz: self.nz,
            agent_height: self.agent_height,
            max_step: self.max_step,
            max_slope_deg: self.max_slope_deg,
        }
    }
}

/// The nav resource: config, the grid once built, and the flow-field cache that shares one field per
/// target cell.
///
/// Installed by the engine when the builder is given `.nav(..)`.
pub struct Nav {
    config: NavConfig,
    grid: Option<NavGrid>,
    cache: FlowCache,
    last_statics: Vec<StaticBody>,
    full_rebuild: bool,
    rebuilds: u64,
    cells: Vec<[u32; 2]>,
}

impl Nav {
    /// A resource with no grid yet; the first [`nav_update`] builds it.
    ///
    /// # Errors
    /// `L4032` if the config is invalid.
    pub fn new(config: NavConfig) -> Result<Self, Diagnostic> {
        config.validate()?;
        Ok(Self {
            cache: FlowCache::new(config.flow_radius),
            config,
            grid: None,
            last_statics: Vec::new(),
            full_rebuild: true,
            rebuilds: 0,
            cells: Vec::new(),
        })
    }

    /// The config the grid is built from.
    pub fn config(&self) -> &NavConfig {
        &self.config
    }

    /// The grid, `None` until the first [`nav_update`] with a physics world.
    pub fn grid(&self) -> Option<&NavGrid> {
        self.grid.as_ref()
    }

    /// The flow-field cache. Borrow the [`grid`](Nav::grid) separately; they are distinct fields
    /// when read through [`parts`](Nav::parts).
    pub fn cache_mut(&mut self) -> &mut FlowCache {
        &mut self.cache
    }

    /// The grid and the cache together, so a field can be asked for from the cache against the grid.
    pub fn parts(&mut self) -> (Option<&NavGrid>, &mut FlowCache) {
        (self.grid.as_ref(), &mut self.cache)
    }

    /// Replaces the config and schedules a full rebuild at the next [`nav_update`].
    ///
    /// # Errors
    /// `L4032` if the config is invalid; the old config stays.
    pub fn set_config(&mut self, config: NavConfig) -> Result<(), Diagnostic> {
        config.validate()?;
        self.cache = FlowCache::new(config.flow_radius);
        self.config = config;
        self.full_rebuild = true;
        Ok(())
    }

    /// Asks the next [`nav_update`] to rebuild the whole grid, e.g. after a change the static diff
    /// cannot see (a layer table edit).
    pub fn request_full_rebuild(&mut self) {
        self.full_rebuild = true;
    }

    /// How many times the grid was built or rebuilt.
    pub fn rebuilds(&self) -> u64 {
        self.rebuilds
    }
}

/// A column source over a pre-sampled rectangle of columns.
struct Presampled {
    min: [u32; 2],
    width: u32,
    columns: Vec<Vec<FloorSample>>,
}

impl ColumnSource for Presampled {
    fn column(&self, i: u32, j: u32, _center: [f32; 2], out: &mut Vec<FloorSample>) {
        let (di, dj) = (i.wrapping_sub(self.min[0]), j.wrapping_sub(self.min[1]));
        if di < self.width
            && let Some(samples) = self.columns.get((dj * self.width + di) as usize)
        {
            out.extend_from_slice(samples);
        }
    }
}

fn eps(y: f32) -> f32 {
    1e-3 * y.abs().max(1.0)
}

/// Samples the floors of the column centred on `(x, z)`.
fn sample_column(
    physics: &PhysicsWorld,
    filter: &QueryFilter,
    config: &NavConfig,
    x: f32,
    z: f32,
) -> Result<Vec<FloorSample>, PhysicsError> {
    let mut out = Vec::new();
    let mut origin = config.y_max;
    for _ in 0..MAX_COLUMN_LAYERS {
        let reach = origin - config.y_min;
        if reach < 0.0 {
            break;
        }
        let hits = physics.ray_cast([x, origin, z], [0.0, -1.0, 0.0], reach, filter)?;
        // A zero distance is a solid containing the origin: not a surface to stand on.
        let Some(hit) = hits.iter().find(|h| h.distance > 0.0) else {
            break;
        };
        let y = hit.point[1];
        origin = y - eps(y);
        let mut normal = hit.normal;
        if normal == [0.0; 3] {
            continue;
        }
        if normal[1] < 0.0 {
            normal = normal.map(|c| -c);
        }
        let up = physics.ray_cast(
            [x, y + eps(y), z],
            [0.0, 1.0, 0.0],
            (config.y_max - y).max(0.0),
            filter,
        )?;
        let headroom = up.first().map_or(f32::INFINITY, |h| h.distance);
        let bottom = y + config.max_step;
        let top = (bottom + 2.0 * config.agent_radius).max(y + config.agent_height);
        let shape = Shape::Capsule {
            half_height: ((top - bottom) * 0.5 - config.agent_radius).max(1e-4),
            radius: config.agent_radius,
        };
        let centre = [x, (top + bottom) * 0.5, z];
        if !physics
            .overlap(&shape, centre, [0.0, 0.0, 0.0, 1.0], filter)?
            .is_empty()
        {
            continue;
        }
        out.push(FloorSample {
            y,
            normal,
            headroom,
        });
    }
    Ok(out)
}

/// Samples every cell in `cells` in parallel, in cell order.
fn presample(
    physics: &PhysicsWorld,
    filter: &QueryFilter,
    config: &NavConfig,
    cells: &[[u32; 2]],
    min: [u32; 2],
    width: u32,
) -> Result<Presampled, PhysicsError> {
    let chunks = par::par_map_chunks(cells, SAMPLE_CHUNK, |_, chunk| {
        chunk
            .iter()
            .map(|&[i, j]| {
                let x = config.origin[0] + (i as f32 + 0.5) * config.cell_size;
                let z = config.origin[1] + (j as f32 + 0.5) * config.cell_size;
                sample_column(physics, filter, config, x, z)
            })
            .collect::<Result<Vec<_>, _>>()
    });
    let mut columns = Vec::with_capacity(cells.len());
    for chunk in chunks {
        columns.extend(chunk?);
    }
    Ok(Presampled {
        min,
        width,
        columns,
    })
}

/// The inclusive cell rectangle the statics that differ between `old` and `new` can affect, or
/// `None` when nothing differs or the change lies off the grid. Both lists are sorted by entity.
fn dirty_rect(
    old: &[StaticBody],
    new: &[StaticBody],
    config: &NavConfig,
) -> Option<([u32; 2], [u32; 2])> {
    let mut lo = [f32::INFINITY; 2];
    let mut hi = [f32::NEG_INFINITY; 2];
    let mut dirty = false;
    let mut add = |b: &StaticBody| {
        dirty = true;
        lo = [lo[0].min(b.min[0]), lo[1].min(b.min[2])];
        hi = [hi[0].max(b.max[0]), hi[1].max(b.max[2])];
    };
    let (mut a, mut b) = (0, 0);
    while a < old.len() || b < new.len() {
        match (old.get(a), new.get(b)) {
            (Some(o), Some(n)) if o.entity == n.entity => {
                if o.min != n.min || o.max != n.max || o.revision != n.revision {
                    add(o);
                    add(n);
                }
                a += 1;
                b += 1;
            }
            (Some(o), Some(n)) if o.entity < n.entity => {
                add(o);
                a += 1;
            }
            (Some(_), Some(n)) => {
                add(n);
                b += 1;
            }
            (Some(o), None) => {
                add(o);
                a += 1;
            }
            (None, Some(n)) => {
                add(n);
                b += 1;
            }
            (None, None) => break,
        }
    }
    if !dirty {
        return None;
    }
    let pad = config.agent_radius + config.cell_size;
    let cell = |v: f32, axis: usize, n: u32| {
        ((v - config.origin[axis]) / config.cell_size)
            .floor()
            .clamp(-1.0, n as f32)
    };
    let (i0, i1) = (
        cell(lo[0] - pad, 0, config.nx),
        cell(hi[0] + pad, 0, config.nx),
    );
    let (j0, j1) = (
        cell(lo[1] - pad, 1, config.nz),
        cell(hi[1] + pad, 1, config.nz),
    );
    if i1 < 0.0 || j1 < 0.0 || i0 >= config.nx as f32 || j0 >= config.nz as f32 {
        return None;
    }
    Some((
        [i0.max(0.0) as u32, j0.max(0.0) as u32],
        [
            (i1 as u32).min(config.nx - 1),
            (j1 as u32).min(config.nz - 1),
        ],
    ))
}

/// Keeps the [`Nav`] grid current: a full build the first time (and after a config change or
/// [`Nav::request_full_rebuild`]), then one region rebuild per tick over the columns a static
/// collider that was added, removed, moved or edited can reach. A tick with no static change does no
/// work. A no-op in a game that configured no nav.
///
/// **Schedule it before the systems that read the grid.** It reads the physics state of the last
/// completed step (the physics systems run after the game's), so a static spawned at tick N shows in
/// the grid from tick N+1. Only static colliders are sampled and diffed, so kinematic and rigid
/// bodies, character motors included, never shape the grid. A bad layer name reports `L4032` once and
/// keeps the old grid.
///
/// # Example
/// ```
/// use ludion_engine::{Engine, NavConfig, nav_update};
///
/// let engine = Engine::builder()
///     .nav(NavConfig::default())
///     .system("nav_update", nav_update)
///     .build()
///     .unwrap();
/// # let _ = engine;
/// ```
pub fn nav_update(world: &mut World, _ctx: &SystemCtx) {
    let Some(mut nav) = world.remove_resource::<Nav>() else {
        return;
    };
    let outcome = match world.resource::<PhysicsWorld>() {
        Some(physics) => update(&mut nav, physics),
        None => Ok(()),
    };
    world.insert_resource(nav);
    if let Err(error) = outcome {
        report_full(
            world,
            "ignore_layers",
            Diagnostic::new(
                code::NAV_CONFIG_INVALID,
                format!("nav sampling failed: {error}"),
            )
            .with_path("/ignore_layers")
            .with_help("name layers declared in the scene's `physics.layers`"),
        );
    }
}

fn update(nav: &mut Nav, physics: &PhysicsWorld) -> Result<(), PhysicsError> {
    let mut statics: Vec<StaticBody> = physics.static_bodies().collect();
    statics.sort_by_key(|s| s.entity);
    let config = nav.config.clone();
    let filter = config
        .ignore_layers
        .iter()
        .fold(QueryFilter::new().only_fixed(), |q, name| {
            q.ignore_layer(name.clone())
        });
    let full = nav.full_rebuild || nav.grid.is_none();
    let (min, max) = if full {
        ([0, 0], [config.nx - 1, config.nz - 1])
    } else if let Some(rect) = dirty_rect(&nav.last_statics, &statics, &config) {
        rect
    } else {
        nav.last_statics = statics;
        return Ok(());
    };
    let width = max[0] - min[0] + 1;
    nav.cells.clear();
    for j in min[1]..=max[1] {
        for i in min[0]..=max[0] {
            nav.cells.push([i, j]);
        }
    }
    let source = presample(physics, &filter, &config, &nav.cells, min, width)?;
    match &mut nav.grid {
        Some(grid) if !full => grid.rebuild_region(&source, min, max),
        slot => {
            *slot = Some(NavGrid::build(config.grid_params(), &source));
            // A fresh grid restarts its generation, which would pass off stale fields as current.
            nav.cache = FlowCache::new(config.flow_radius);
        }
    }
    nav.last_statics = statics;
    nav.full_rebuild = false;
    nav.rebuilds += 1;
    Ok(())
}

/// A point-to-point path request for [`nav_path`]. Build it with [`PathRequest::new`] and override
/// fields with struct-update syntax, so later fields stay additive.
///
/// # Example
/// ```
/// use ludion_engine::PathRequest;
///
/// let req = PathRequest { max_cost: 40.0, ..PathRequest::new([0.0, 0.0, 0.0], [9.0, 0.0, 3.0]) };
/// assert!(req.partial && req.snap_radius == 1.0);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PathRequest {
    /// World start; snapped to the nearest walkable node within `snap_radius`.
    pub from: [f32; 3],
    /// World goal; snapped the same way.
    pub to: [f32; 3],
    /// No node past this path cost is expanded (default infinite).
    pub max_cost: f32,
    /// Stop after this many node expansions, at least 1 (default 20000).
    pub max_expanded: u32,
    /// On a goal that cannot be reached, return the path to the reachable node nearest to it instead
    /// of [`PathStatus::Unreachable`] (default `true`).
    pub partial: bool,
    /// How far, in metres, an end off the walkable grid may snap to a node (default `1`).
    pub snap_radius: f32,
}

impl PathRequest {
    /// A request from `from` to `to` with every limit at its default.
    pub fn new(from: [f32; 3], to: [f32; 3]) -> Self {
        let opts = PathOptions::default();
        Self {
            from,
            to,
            max_cost: opts.max_cost,
            max_expanded: opts.max_expanded,
            partial: opts.partial,
            snap_radius: 1.0,
        }
    }
}

/// The answer to a [`PathRequest`].
#[derive(Clone, Debug, PartialEq)]
pub struct NavPath {
    /// Whether the goal was reached, only approached, or not at all.
    pub status: PathStatus,
    /// Path length in metres along the grid links; infinite when unreachable.
    pub cost: f32,
    /// The string-pulled route, start first; empty when unreachable. A complete route ends at the goal's
    /// own x and z when the goal lies on a walkable cell, else at the node it snapped to.
    pub waypoints: Vec<[f32; 3]>,
    /// Nodes the search expanded.
    pub expanded: u32,
}

impl NavPath {
    fn unreachable(expanded: u32) -> Self {
        Self {
            status: PathStatus::Unreachable,
            cost: f32::INFINITY,
            waypoints: Vec::new(),
            expanded,
        }
    }
}

fn nav_grid(world: &World) -> Result<&NavGrid, Diagnostic> {
    world
        .resource::<Nav>()
        .and_then(Nav::grid)
        .ok_or_else(|| {
            Diagnostic::new(
                code::NAV_NOT_READY,
                "a navigation query ran before the nav grid exists",
            )
            .with_help("opt in with `Engine::builder().nav(NavConfig::default())` and run `nav_update` before querying")
        })
}

fn bad_request(path: &str, what: String, help: &str) -> Diagnostic {
    Diagnostic::new(code::NAV_PATH_REQUEST_INVALID, what)
        .with_path(path)
        .with_help(help)
}

fn check_point(name: &str, p: [f32; 3]) -> Result<(), Diagnostic> {
    match p.iter().position(|c| !c.is_finite()) {
        Some(i) => Err(bad_request(
            &format!("/{name}/{i}"),
            format!("navigation `{name}` component {i} is {}, not finite", p[i]),
            "pass finite world coordinates",
        )),
        None => Ok(()),
    }
}

fn check_radius(name: &str, v: f32) -> Result<(), Diagnostic> {
    if v.is_finite() && v >= 0.0 {
        Ok(())
    } else {
        Err(bad_request(
            &format!("/{name}"),
            format!("navigation `{name}` is {v}, not a finite number >= 0"),
            "use a distance in metres, 0 or more",
        ))
    }
}

fn check_request(req: &PathRequest) -> Result<(), Diagnostic> {
    check_point("from", req.from)?;
    check_point("to", req.to)?;
    check_radius("snap_radius", req.snap_radius)?;
    if req.max_cost.is_nan() || req.max_cost < 0.0 {
        return Err(bad_request(
            "/max_cost",
            format!(
                "navigation `max_cost` is {}, not a number >= 0",
                req.max_cost
            ),
            "use a cost in metres, or leave it infinite",
        ));
    }
    if req.max_expanded == 0 {
        return Err(bad_request(
            "/max_expanded",
            "navigation `max_expanded` is 0, so the search could not start".to_string(),
            "use at least 1; the default is 20000",
        ));
    }
    Ok(())
}

/// The walkable point for `p` on `node`: `p`'s own x and z when `p` lies in the node's column, else the
/// node's position (a point that snapped to a neighbouring cell).
fn project(grid: &NavGrid, p: [f32; 3], node: NodeId) -> [f32; 3] {
    let at = grid.node_pos(node);
    if grid.cell_of(p) == grid.cell_of(at) {
        [p[0], at[1], p[2]]
    } else {
        at
    }
}

/// Plans `req` over `grid`: the one planner `nav_path` and routed steering share.
fn plan(grid: &NavGrid, req: &PathRequest, project_start: bool) -> NavPath {
    let (Some(start), Some(goal)) = (
        nearest_node(grid, req.from, req.snap_radius),
        nearest_node(grid, req.to, req.snap_radius),
    ) else {
        return NavPath::unreachable(0);
    };
    let found = find_path(
        grid,
        start,
        goal,
        &PathOptions {
            max_cost: req.max_cost,
            max_expanded: req.max_expanded,
            partial: req.partial,
        },
    );
    if found.status == PathStatus::Unreachable {
        return NavPath::unreachable(found.expanded);
    }
    let mut waypoints = string_pull(grid, &found.nodes);
    if let Some(first) = waypoints.first_mut() {
        *first = if project_start {
            project(grid, req.from, start)
        } else {
            grid.node_pos(start)
        };
    }
    if found.status == PathStatus::Complete
        && let Some(last) = waypoints.last_mut()
    {
        *last = project(grid, req.to, goal);
    }
    NavPath {
        status: found.status,
        cost: found.cost,
        waypoints,
        expanded: found.expanded,
    }
}

/// Plans a route from `req.from` to `req.to` over the walkable grid: A* with string pulling, so the
/// waypoints are the few corners a straight-line walker needs. Both ends snap to the nearest node within
/// `snap_radius`; an end with none is [`PathStatus::Unreachable`]. A goal that cannot be reached is
/// [`PathStatus::Partial`] (a path to the reachable node nearest it) unless `partial` is off. Pure
/// and deterministic; it reads the grid and changes nothing.
///
/// # Errors
/// `L4140` before the grid exists (no `.nav(..)` opt-in, or `nav_update` has not run); `L4141` for a
/// non-finite point, a negative or NaN `max_cost`, a negative or non-finite `snap_radius`, or a zero
/// `max_expanded`.
///
/// # Example
/// ```
/// use ludion_engine::{Engine, NavConfig, PathRequest, nav_path};
///
/// let engine = Engine::builder().nav(NavConfig::default()).build().unwrap();
/// // No `nav_update` has run, so there is no grid to plan over yet.
/// let err = nav_path(engine.world(), &PathRequest::new([0.0; 3], [5.0, 0.0, 5.0])).unwrap_err();
/// assert_eq!(err.code.as_str(), "L4140");
/// ```
pub fn nav_path(world: &World, req: &PathRequest) -> Result<NavPath, Diagnostic> {
    let grid = nav_grid(world)?;
    check_request(req)?;
    Ok(plan(grid, req, true))
}

/// The walking distance in metres from `from` to `to` along the grid, or `None` when the goal cannot be
/// reached (either end off the grid, or no route). Ends snap within one metre.
///
/// # Errors
/// As [`nav_path`].
///
/// # Example
/// ```
/// use ludion_engine::{Engine, NavConfig, nav_distance};
///
/// let engine = Engine::builder().nav(NavConfig::default()).build().unwrap();
/// assert!(nav_distance(engine.world(), [0.0; 3], [5.0, 0.0, 5.0]).is_err());
/// ```
pub fn nav_distance(
    world: &World,
    from: [f32; 3],
    to: [f32; 3],
) -> Result<Option<f32>, Diagnostic> {
    let req = PathRequest {
        partial: false,
        ..PathRequest::new(from, to)
    };
    let path = nav_path(world, &req)?;
    Ok((path.status == PathStatus::Complete).then_some(path.cost))
}

/// Whether `to` can be walked to from `from` along the grid. Ends snap within one metre.
///
/// # Errors
/// As [`nav_path`].
///
/// # Example
/// ```
/// use ludion_engine::{Engine, NavConfig, nav_reachable};
///
/// let engine = Engine::builder().nav(NavConfig::default()).build().unwrap();
/// assert!(nav_reachable(engine.world(), [0.0; 3], [5.0, 0.0, 5.0]).is_err());
/// ```
pub fn nav_reachable(world: &World, from: [f32; 3], to: [f32; 3]) -> Result<bool, Diagnostic> {
    Ok(nav_distance(world, from, to)?.is_some())
}

/// The nearest walkable point to `p` within `max_dist` metres: `p` itself lifted or dropped onto the
/// floor when it is over walkable ground, else the centre of the nearest walkable cell; `None` when
/// nothing is that close.
///
/// # Errors
/// `L4140` before the grid exists; `L4141` for a non-finite `p` or a negative or non-finite `max_dist`.
///
/// # Example
/// ```
/// use ludion_engine::{Engine, NavConfig, nav_project};
///
/// let engine = Engine::builder().nav(NavConfig::default()).build().unwrap();
/// assert!(nav_project(engine.world(), [0.0; 3], 2.0).is_err());
/// ```
pub fn nav_project(
    world: &World,
    p: [f32; 3],
    max_dist: f32,
) -> Result<Option<[f32; 3]>, Diagnostic> {
    let grid = nav_grid(world)?;
    check_point("p", p)?;
    check_radius("max_dist", max_dist)?;
    Ok(nearest_node(grid, p, max_dist).map(|node| project(grid, p, node)))
}

/// One agent as the steering phase sees it.
struct Steer {
    entity: EntityId,
    agent: NavAgent,
    pos: [f32; 3],
    /// The walking agent's target node, `None` for a flyer, a routed agent, or a target off the grid.
    node: Option<NodeId>,
    goal: Option<NavGoal>,
    route: Option<NavRoute>,
    route_write: RouteWrite,
}

/// What the serial write-back does to an agent's [`NavRoute`] before applying the steering result.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RouteWrite {
    /// Leave the stored route (a steering advance may still update it).
    Keep,
    /// The plan phase changed `route`: store it.
    Store,
    /// The agent has no walking goal: drop any stored route.
    Remove,
}

/// Whether the stored leg (the previous waypoint to the one being walked) is still walkable. It was at
/// plan time, so only a grid change can break it; the agent's own drift off the line never does.
fn leg_walkable(grid: &NavGrid, route: &NavRoute) -> bool {
    let index = route.index as usize;
    // The route was pulled between node centres; the goal end sits off-centre, so test the centre.
    let centre = |p: [f32; 3]| grid.node_at(p).map_or(p, |n| grid.node_pos(n));
    match (index.checked_sub(1), route.waypoints.get(index)) {
        (Some(prev), Some(&wp)) => walkable_line(grid, centre(route.waypoints[prev]), centre(wp)),
        _ => true,
    }
}

/// The serial plan phase for one routed walker: decides from hashed state and the current grid whether
/// the route needs planning, and plans it while the per-tick `budget` lasts.
fn plan_route(grid: &NavGrid, steer: &mut Steer, goal: NavGoal, budget: &mut u32) {
    let cell = |p| grid.cell_of(p);
    let needs_plan = match &steer.route {
        None => true,
        Some(route) => {
            route.status == NavRouteStatus::Planning
                || cell(goal.at) != cell(route.goal)
                || (goal.replan == NavReplan::OnChange
                    && route.status == NavRouteStatus::Following
                    && !route.partial
                    && !leg_walkable(grid, route))
        }
    };
    if !needs_plan {
        if let Some(route) = steer.route.as_mut()
            && route.goal != goal.at
        {
            // The goal moved inside its cell: follow it without planning again.
            route.goal = goal.at;
            if !route.partial
                && let Some(last) = route.waypoints.last_mut()
            {
                last[0] = goal.at[0];
                last[2] = goal.at[2];
            }
            if route.status == NavRouteStatus::Arrived {
                route.status = NavRouteStatus::Following;
                route.index = route.waypoints.len().saturating_sub(1) as u32;
            }
            steer.route_write = RouteWrite::Store;
        }
        return;
    }
    let mut route = NavRoute {
        goal: goal.at,
        ..NavRoute::default()
    };
    if *budget > 0 {
        *budget -= 1;
        let path = plan(grid, &PathRequest::new(steer.pos, goal.at), false);
        if path.status != PathStatus::Unreachable {
            route.index = 1.min(path.waypoints.len().saturating_sub(1)) as u32;
            route.status = NavRouteStatus::Following;
            route.partial = path.status == PathStatus::Partial;
            route.cost = path.cost;
            route.waypoints = path.waypoints;
        } else {
            route.status = NavRouteStatus::Unreachable;
        }
    }
    steer.route = Some(route);
    steer.route_write = RouteWrite::Store;
}

/// Writes every [`NavAgent`]'s desired velocity into its [`NavSteering`], in ascending entity order.
///
/// A walker follows the shared flow field toward its target (one field per target cell, whatever
/// the agent count) and falls back to a straight line where the field has no direction; with no
/// grid yet its seek is zero. A flyer seeks its target in 3D and never reads the grid. Both add a
/// push away from their nearest [`Index3d`] neighbours on `neighbor_layers` (the walker's flat,
/// the flyer's in 3D). Inside `arrive_radius` the seek is zero. The velocity is the whole result:
/// the game's mover applies it.
///
/// An agent that also carries a [`NavGoal`] takes precedence over `target`: a walker plans a route to
/// `goal.at` (serially, ascending entity, at most `max_path_requests_per_tick` per tick; the rest wait in
/// `planning`) and walks it waypoint by waypoint, writing the [`NavRoute`] component; a flyer seeks
/// `goal.at` in 3D and gets no route. A route is planned again when the goal moves to another cell or,
/// under `replan: on_change`, when the leg being walked is no longer walkable; it never depends on the
/// grid's rebuild count, so a restored world resumes its route.
///
/// Each agent is a pure function of the state at the start of the tick (neighbour centres come
/// frozen from the index), computed over [`ludion_core::par`], so the result is identical at any
/// thread count. **Schedule it** `spatial3d_rebuild` → [`nav_update`] → `nav_steer` → the game's
/// mover. An agent with no world pose is skipped. A no-op for a game with no [`NavAgent`].
///
/// # Example
/// ```
/// use ludion_engine::{Engine, NavConfig, nav_steer, nav_update};
///
/// let engine = Engine::builder()
///     .nav(NavConfig::default())
///     .system("nav_update", nav_update)
///     .system("nav_steer", nav_steer)
///     .build()
///     .unwrap();
/// # let _ = engine;
/// ```
pub fn nav_steer(world: &mut World, _ctx: &SystemCtx) {
    let mut agents: Vec<(EntityId, NavAgent)> = world
        .query_ids::<(&NavAgent,)>()
        .map(|(id, (agent,))| (id, *agent))
        .collect();
    if agents.is_empty() {
        return;
    }
    agents.sort_by_key(|(entity, _)| *entity);
    let mut steers: Vec<Steer> = agents
        .into_iter()
        .filter_map(|(entity, agent)| {
            let pos = world_transform(world, entity)?.trs.translation;
            Some(Steer {
                entity,
                agent,
                pos,
                node: None,
                goal: world.get::<NavGoal>(entity).ok().flatten().copied(),
                route: world.get::<NavRoute>(entity).ok().flatten().cloned(),
                route_write: RouteWrite::Keep,
            })
        })
        .collect();
    // A route belongs to a walking agent with a goal; anything else loses a stale one.
    for steer in steers
        .iter_mut()
        .filter(|s| s.route.is_some() && (s.goal.is_none() || s.agent.flying))
    {
        steer.route = None;
        steer.route_write = RouteWrite::Remove;
    }

    let mut nav = world.remove_resource::<Nav>();
    if let Some(nav) = nav.as_mut() {
        let mut budget = nav.config.max_path_requests_per_tick;
        let (grid, cache) = nav.parts();
        if let Some(grid) = grid {
            for steer in steers.iter_mut().filter(|s| !s.agent.flying) {
                if let Some(goal) = steer.goal {
                    plan_route(grid, steer, goal, &mut budget);
                    continue;
                }
                steer.node = grid.node_at(steer.agent.target);
                if let Some(node) = steer.node {
                    cache.field(grid, node);
                }
            }
        }
    }

    let empty;
    let index = match world.resource::<Index3d>() {
        Some(index) => index,
        None => {
            let mut fallback = Index3d::new();
            fallback.build();
            empty = fallback;
            &empty
        }
    };
    let grid = nav.as_ref().and_then(Nav::grid);
    let cache = nav.as_ref().map(|n| &n.cache);
    let chunks = par::par_map_chunks(&steers, STEER_CHUNK, |_, chunk| {
        let mut scratch: Vec<Neighbor> = Vec::new();
        chunk
            .iter()
            .map(|steer| steer_one(steer, grid, cache, index, &mut scratch))
            .collect::<Vec<([f32; 3], Option<(u32, NavRouteStatus)>)>>()
    });

    for (steer, (velocity, advance)) in steers.iter_mut().zip(chunks.into_iter().flatten()) {
        match world.get_mut::<NavSteering>(steer.entity) {
            Ok(Some(out)) => out.velocity = velocity,
            _ => {
                let _ = world.insert(steer.entity, NavSteering { velocity });
            }
        }
        write_route(world, steer, advance);
    }
    if let Some(mut nav) = nav {
        nav.cache.end_frame();
        world.insert_resource(nav);
    }
}

/// Applies the plan phase's change and the steering advance to the stored [`NavRoute`].
fn write_route(world: &mut World, steer: &mut Steer, advance: Option<(u32, NavRouteStatus)>) {
    if steer.route_write == RouteWrite::Remove {
        let _ = world.remove::<NavRoute>(steer.entity);
        return;
    }
    let Some(route) = steer.route.as_mut() else {
        return;
    };
    let mut store = steer.route_write == RouteWrite::Store;
    if let Some((index, status)) = advance
        && (route.index, route.status) != (index, status)
    {
        (route.index, route.status) = (index, status);
        store = true;
    }
    if !store {
        return;
    }
    match world.get_mut::<NavRoute>(steer.entity) {
        Ok(Some(slot)) => *slot = route.clone(),
        _ => {
            let _ = world.insert(steer.entity, route.clone());
        }
    }
}

fn steer_one(
    steer: &Steer,
    grid: Option<&NavGrid>,
    cache: Option<&FlowCache>,
    index: &Index3d,
    scratch: &mut Vec<Neighbor>,
) -> ([f32; 3], Option<(u32, NavRouteStatus)>) {
    let Steer {
        entity, agent, pos, ..
    } = steer;
    let mut advance = None;
    let mut velocity = if agent.flying {
        let target = steer.goal.map_or(agent.target, |g| g.at);
        seek_flying(&NavAgent { target, ..*agent }, *pos)
    } else if steer.goal.is_some() {
        let (velocity, adv) = seek_routed(steer, grid);
        advance = adv;
        velocity
    } else {
        seek_walking(steer, grid, cache)
    };
    if agent.neighbor_layers != 0 && agent.separation_neighbors > 0 {
        let filter = Filter::new(agent.neighbor_layers).excluding(Handle::entity(*entity));
        index.k_nearest(
            *pos,
            agent.separation_neighbors as usize,
            agent.separation_radius,
            &filter,
            scratch,
        );
        let mut push = [0.0f32; 3];
        for n in scratch.iter() {
            for (axis, p) in push.iter_mut().enumerate() {
                *p += pos[axis] - n.center[axis];
            }
        }
        if !agent.flying {
            push[1] = 0.0;
        }
        for (v, p) in velocity.iter_mut().zip(push) {
            *v += p * agent.separation_strength;
        }
    }
    (velocity, advance)
}

/// A routed walker seeks the waypoint it is on, advancing past the ones it reaches; only a `following`
/// route moves. Returns the velocity and the route's `(index, status)` after this tick.
fn seek_routed(steer: &Steer, grid: Option<&NavGrid>) -> ([f32; 3], Option<(u32, NavRouteStatus)>) {
    let (Some(grid), Some(route)) = (grid, &steer.route) else {
        return ([0.0; 3], None);
    };
    let waypoints = &route.waypoints;
    if route.status != NavRouteStatus::Following || waypoints.is_empty() {
        return ([0.0; 3], None);
    }
    let (agent, pos) = (&steer.agent, steer.pos);
    let last = waypoints.len() - 1;
    let offset = |i: usize| [waypoints[i][0] - pos[0], waypoints[i][2] - pos[2]];
    let dist = |i: usize| {
        let o = offset(i);
        sqrt(o[0] * o[0] + o[1] * o[1])
    };
    let mut at = (route.index as usize).min(last);
    if at == last && dist(last) <= agent.arrive_radius {
        return ([0.0; 3], Some((at as u32, NavRouteStatus::Arrived)));
    }
    let pass = agent.arrive_radius.max(0.5 * grid.params().cell_size);
    while at < last && dist(at) <= pass {
        at += 1;
    }
    let d = dist(at);
    let advance = Some((at as u32, NavRouteStatus::Following));
    if d <= 0.0 {
        return ([0.0; 3], advance);
    }
    let o = offset(at);
    (
        [o[0] / d * agent.speed, 0.0, o[1] / d * agent.speed],
        advance,
    )
}

fn seek_flying(agent: &NavAgent, pos: [f32; 3]) -> [f32; 3] {
    let to = [
        agent.target[0] - pos[0],
        agent.target[1] - pos[1],
        agent.target[2] - pos[2],
    ];
    let d = sqrt(to[0] * to[0] + to[1] * to[1] + to[2] * to[2]);
    if d <= agent.arrive_radius || d <= 0.0 {
        return [0.0; 3];
    }
    to.map(|c| c / d * agent.speed)
}

fn seek_walking(steer: &Steer, grid: Option<&NavGrid>, cache: Option<&FlowCache>) -> [f32; 3] {
    let (agent, pos) = (&steer.agent, steer.pos);
    let Some(grid) = grid else {
        return [0.0; 3];
    };
    let to = [agent.target[0] - pos[0], agent.target[2] - pos[2]];
    let d = sqrt(to[0] * to[0] + to[1] * to[1]);
    if d <= agent.arrive_radius || d <= 0.0 {
        return [0.0; 3];
    }
    let dir = steer
        .node
        .and_then(|node| cache?.get(node))
        .and_then(|field| field.flow_at(grid, pos))
        .unwrap_or([to[0] / d, to[1] / d]);
    [dir[0] * agent.speed, 0.0, dir[1] * agent.speed]
}

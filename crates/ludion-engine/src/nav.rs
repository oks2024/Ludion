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

use ludion_components::{NavAgent, NavSteering};
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::math::sqrt;
use ludion_core::par;
use ludion_ecs::{SystemCtx, World};
use ludion_nav::{ColumnSource, FloorSample, FlowCache, GridParams, NavGrid, NodeId};
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
        }
    }
}

impl NavConfig {
    /// Checks the config.
    ///
    /// # Errors
    /// `L4032` if a value is not finite, a size or radius is not positive, the grid has no columns
    /// or more than 4194304, `y_min >= y_max`, the slope is outside `(0, 90)`, or a layer name is
    /// empty.
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

/// One agent as the steering phase sees it.
struct Steer {
    entity: EntityId,
    agent: NavAgent,
    pos: [f32; 3],
    /// The walking agent's target node, `None` for a flyer or a target off the grid.
    node: Option<NodeId>,
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
            })
        })
        .collect();

    let mut nav = world.remove_resource::<Nav>();
    if let Some(nav) = nav.as_mut() {
        let (grid, cache) = nav.parts();
        if let Some(grid) = grid {
            for steer in steers.iter_mut().filter(|s| !s.agent.flying) {
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
            .collect::<Vec<[f32; 3]>>()
    });

    for (steer, velocity) in steers.iter().zip(chunks.into_iter().flatten()) {
        match world.get_mut::<NavSteering>(steer.entity) {
            Ok(Some(out)) => out.velocity = velocity,
            _ => {
                let _ = world.insert(steer.entity, NavSteering { velocity });
            }
        }
    }
    if let Some(mut nav) = nav {
        nav.cache.end_frame();
        world.insert_resource(nav);
    }
}

fn steer_one(
    steer: &Steer,
    grid: Option<&NavGrid>,
    cache: Option<&FlowCache>,
    index: &Index3d,
    scratch: &mut Vec<Neighbor>,
) -> [f32; 3] {
    let Steer {
        entity, agent, pos, ..
    } = steer;
    let mut velocity = if agent.flying {
        seek_flying(agent, *pos)
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
    velocity
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

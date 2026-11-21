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
//!
//! Several agent sizes share one world through [`NavConfig::classes`]: one grid, flow cache and link
//! table per [`NavClass`], all derived, built from one shared floor down-walk. A [`NavAgent`], a
//! [`PathRequest`] and a [`NavLink`] pick a class by name; an empty name is the first class.
//!
//! A `NavLink` entity adds a directed edge between two nodes (a ladder, a vault). `nav_update` rebuilds
//! the derived link table every call; routes plan across it, and a walker that reaches a link start stops
//! with `NavRoute.status = at_link` until the game calls [`nav_link_done`]. The engine never moves the body.

use ludion_components::nav_route::{NavGoal, NavReplan, NavRoute, NavRouteStatus};
use ludion_components::{NavAgent, NavLink, NavObstacle, NavObstacleState, NavSteering};
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::math::sqrt;
use ludion_core::par;
use ludion_core::rng::DeterministicRng;
use ludion_ecs::{SystemCtx, World};
use ludion_nav::{
    AvoidAgent, AvoidNeighbor, ColumnSource, FloorSample, FlowCache, GridParams, NavGrid, NavLinks,
    NodeId, NodeMark, OffMeshLink, OrcaLine, PathOptions, PathStatus, avoid_velocity,
    find_path_linked, line_exit, nearest_node, reachable_within, string_pull_marked, walkable_line,
};
use ludion_physics::{LayerSet, PhysicsError, PhysicsWorld, QueryFilter, StaticBody};
use ludion_renderer::DebugSegment;
use ludion_spatial::Shape;
use ludion_spatial::spatial3d::{Filter, Handle, Index3d, Neighbor};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::diagnostics::{offender_key, report_full};
use crate::hierarchy::world_transform;
use crate::nav_blockers::{BlockerTable, build_blockers};

/// Agents per parallel steering chunk.
const STEER_CHUNK: usize = 64;
/// Floors a column keeps at most (a down-walk stops here).
const MAX_COLUMN_LAYERS: usize = 32;
/// Columns per parallel chunk.
const SAMPLE_CHUNK: usize = 64;
/// Most columns a grid may have.
const MAX_COLUMNS: u64 = 1 << 22;

/// One agent size: the grid built for it drops floors it cannot stand on or fit under.
///
/// # Example
/// ```
/// use ludion_engine::NavClass;
///
/// let forklift = NavClass { name: "forklift".into(), radius: 1.0, ..NavClass::default() };
/// assert_eq!(forklift.height, 1.8);
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NavClass {
    /// The name agents, requests and links pick the class by; non-empty and unique.
    pub name: String,
    /// The agent's radius: a floor is dropped where a capsule of this radius collides.
    pub radius: f32,
    /// The agent's height: a floor needs this much headroom.
    pub height: f32,
    /// Largest step up or down on flat ground.
    pub max_step: f32,
    /// Steepest walkable surface, degrees from horizontal.
    pub max_slope_deg: f32,
}

impl Default for NavClass {
    fn default() -> Self {
        Self {
            name: String::new(),
            radius: 0.4,
            height: 1.8,
            max_step: 0.4,
            max_slope_deg: 45.0,
        }
    }
}

/// Most agent classes a config may define.
const MAX_CLASSES: usize = 8;

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
    #[schemars(with = "Vec<String>")]
    pub ignore_layers: LayerSet,
    /// Most routes `nav_steer` plans per tick; agents over the cap wait in `planning` until the next
    /// tick (default `8`, at least `1`).
    #[serde(default = "default_max_path_requests")]
    pub max_path_requests_per_tick: u32,
    /// Agent classes, one grid each (default none: one class named `""` from `agent_radius`,
    /// `agent_height`, `max_step` and `max_slope_deg`, which are ignored when this is set). At most 8.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub classes: Vec<NavClass>,
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
            ignore_layers: LayerSet::default(),
            max_path_requests_per_tick: default_max_path_requests(),
            classes: Vec::new(),
        }
    }
}

impl NavConfig {
    /// Checks the config.
    ///
    /// # Errors
    /// `L4032` if a value is not finite, a size or radius is not positive, the grid has no columns
    /// or more than 4194304, `y_min >= y_max`, the slope is outside `(0, 90)`, a layer name is
    /// empty, `max_path_requests_per_tick` is 0, or `classes` has more than 8 entries, an empty or
    /// repeated name, or a class value out of range (paths `/classes/{i}/...`).
    pub fn validate(&self) -> Result<(), Diagnostic> {
        let invalid = invalid_config;
        let legacy = self.classes.is_empty();
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
            let agent_field = path.starts_with("/agent_");
            if (legacy || !agent_field) && (!v.is_finite() || v <= 0.0) {
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
        if legacy && (!self.max_step.is_finite() || self.max_step < 0.0) {
            return Err(invalid(
                "/max_step",
                format!(
                    "nav `max_step` is {}, not a finite number >= 0",
                    self.max_step
                ),
                "use the tallest step the agent climbs, in metres",
            ));
        }
        if legacy && !(self.max_slope_deg > 0.0 && self.max_slope_deg < 90.0) {
            return Err(invalid(
                "/max_slope_deg",
                format!(
                    "nav `max_slope_deg` is {}, outside (0, 90)",
                    self.max_slope_deg
                ),
                "use the steepest walkable angle in degrees, e.g. 45.0",
            ));
        }
        if let Some(i) = self.ignore_layers.iter().position(str::is_empty) {
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
        if self.classes.len() > MAX_CLASSES {
            return Err(invalid(
                "/classes",
                format!(
                    "nav `classes` has {} entries, more than the {MAX_CLASSES} allowed",
                    self.classes.len()
                ),
                "merge classes of similar size, or drop the ones no agent uses",
            ));
        }
        for (i, class) in self.classes.iter().enumerate() {
            if class.name.is_empty() {
                return Err(invalid(
                    &format!("/classes/{i}/name"),
                    format!("nav class {i} has an empty `name`"),
                    "name the class; agents, requests and links pick it by name",
                ));
            }
            if self.classes[..i].iter().any(|c| c.name == class.name) {
                return Err(invalid(
                    &format!("/classes/{i}/name"),
                    format!("nav class name `{}` is used twice", class.name),
                    "give every class its own name",
                ));
            }
            check_agent(&format!("/classes/{i}/"), class)?;
        }
        Ok(())
    }

    /// The classes grids are built for: `classes`, or the legacy agent fields as one class named `""`.
    fn resolved_classes(&self) -> Vec<NavClass> {
        if self.classes.is_empty() {
            vec![NavClass {
                name: String::new(),
                radius: self.agent_radius,
                height: self.agent_height,
                max_step: self.max_step,
                max_slope_deg: self.max_slope_deg,
            }]
        } else {
            self.classes.clone()
        }
    }

    /// The index of the class called `name`; `""` is the first class. `None` for a name no class has.
    pub fn class_index(&self, name: &str) -> Option<usize> {
        if name.is_empty() {
            return Some(0);
        }
        self.classes.iter().position(|c| c.name == name)
    }

    /// The class names, for a diagnostic's help.
    fn class_names(&self) -> String {
        if self.classes.is_empty() {
            "no classes are defined: leave the class empty".to_string()
        } else {
            self.classes
                .iter()
                .map(|c| format!("`{}`", c.name))
                .collect::<Vec<_>>()
                .join(", ")
        }
    }

    fn grid_params(&self, class: &NavClass) -> GridParams {
        GridParams {
            origin: self.origin,
            cell_size: self.cell_size,
            nx: self.nx,
            nz: self.nz,
            agent_height: class.height,
            max_step: class.max_step,
            max_slope_deg: class.max_slope_deg,
        }
    }
}

fn invalid_config(path: &str, what: String, help: &str) -> Diagnostic {
    Diagnostic::new(code::NAV_CONFIG_INVALID, what)
        .with_path(path)
        .with_help(help)
}

/// Checks a class's numbers; `base` is the pointer prefix, `/classes/{i}/`.
fn check_agent(base: &str, class: &NavClass) -> Result<(), Diagnostic> {
    for (field, v) in [("radius", class.radius), ("height", class.height)] {
        if !v.is_finite() || v <= 0.0 {
            return Err(invalid_config(
                &format!("{base}{field}"),
                format!(
                    "nav class `{}` `{field}` is {v}, not a positive finite number",
                    class.name
                ),
                "use a finite value above zero, in metres",
            ));
        }
    }
    if !class.max_step.is_finite() || class.max_step < 0.0 {
        return Err(invalid_config(
            &format!("{base}max_step"),
            format!(
                "nav class `{}` `max_step` is {}, not a finite number >= 0",
                class.name, class.max_step
            ),
            "use the tallest step the agent climbs, in metres",
        ));
    }
    if !(class.max_slope_deg > 0.0 && class.max_slope_deg < 90.0) {
        return Err(invalid_config(
            &format!("{base}max_slope_deg"),
            format!(
                "nav class `{}` `max_slope_deg` is {}, outside (0, 90)",
                class.name, class.max_slope_deg
            ),
            "use the steepest walkable angle in degrees, e.g. 45.0",
        ));
    }
    Ok(())
}

/// The nav resource: config, one grid per agent class once built, and per class the flow-field cache
/// that shares one field per target cell and the off-mesh link table.
///
/// Installed by the engine when the builder is given `.nav(..)`. The unqualified accessors
/// ([`grid`](Nav::grid), [`cache`](Nav::cache), [`links`](Nav::links)) read the **first class**;
/// [`grid_for`](Nav::grid_for) reads a class by name.
pub struct Nav {
    config: NavConfig,
    classes: Vec<NavClass>,
    grids: Vec<NavGrid>,
    caches: Vec<FlowCache>,
    last_statics: Vec<StaticBody>,
    full_rebuild: bool,
    rebuilds: u64,
    cells: Vec<[u32; 2]>,
    tables: Vec<LinkTable>,
    blocker_tables: Vec<BlockerTable>,
}

/// One enabled `NavLink` as the table holds it; its index in [`LinkTable::entries`] is the edge's tag.
struct LinkEntry {
    entity: EntityId,
    from: NodeId,
    to: NodeId,
    bidirectional: bool,
}

/// The derived off-mesh link table: the search edges and the link entity behind each. Rebuilt by every
/// [`nav_update`]; never hashed or snapshotted.
#[derive(Default)]
struct LinkTable {
    links: NavLinks,
    entries: Vec<LinkEntry>,
}

impl LinkTable {
    /// The lowest link entity whose snapped ends are the nodes under `start` and `end`, in either order
    /// for a bidirectional link.
    fn resolve(&self, grid: &NavGrid, start: [f32; 3], end: [f32; 3]) -> Option<EntityId> {
        let (a, b) = (grid.node_at(start)?, grid.node_at(end)?);
        self.entries
            .iter()
            .find(|e| (e.from, e.to) == (a, b) || (e.bidirectional && (e.to, e.from) == (a, b)))
            .map(|e| e.entity)
    }
}

impl Nav {
    /// A resource with no grid yet; the first [`nav_update`] builds it.
    ///
    /// # Errors
    /// `L4032` if the config is invalid.
    pub fn new(config: NavConfig) -> Result<Self, Diagnostic> {
        config.validate()?;
        let classes = config.resolved_classes();
        Ok(Self {
            caches: classes
                .iter()
                .map(|_| FlowCache::new(config.flow_radius))
                .collect(),
            tables: classes.iter().map(|_| LinkTable::default()).collect(),
            blocker_tables: classes.iter().map(|_| BlockerTable::default()).collect(),
            classes,
            config,
            grids: Vec::new(),
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

    /// The first class's grid, `None` until the first [`nav_update`] with a physics world.
    pub fn grid(&self) -> Option<&NavGrid> {
        self.grids.first()
    }

    /// The grid of the class called `class` (`""` is the first); `None` for an unknown class or before
    /// the first [`nav_update`].
    pub fn grid_for(&self, class: &str) -> Option<&NavGrid> {
        self.grids.get(self.config.class_index(class)?)
    }

    /// The first class's off-mesh edges the last [`nav_update`] built from the enabled `NavLink`s; empty
    /// until then.
    pub fn links(&self) -> &NavLinks {
        &self.tables[0].links
    }

    /// The first class's flow-field cache, read-only: [`FlowCache::get`] answers only for a field already
    /// built.
    pub fn cache(&self) -> &FlowCache {
        &self.caches[0]
    }

    /// The first class's flow-field cache. Borrow the [`grid`](Nav::grid) separately; they are distinct
    /// fields when read through [`parts`](Nav::parts).
    pub fn cache_mut(&mut self) -> &mut FlowCache {
        &mut self.caches[0]
    }

    /// The first class's grid and cache together, so a field can be asked for from the cache against
    /// the grid.
    pub fn parts(&mut self) -> (Option<&NavGrid>, &mut FlowCache) {
        (self.grids.first(), &mut self.caches[0])
    }

    fn split_class(&mut self, i: usize) -> (Option<&NavGrid>, &mut FlowCache, &LinkTable) {
        (self.grids.get(i), &mut self.caches[i], &self.tables[i])
    }

    /// Replaces the config and schedules a full rebuild at the next [`nav_update`].
    ///
    /// # Errors
    /// `L4032` if the config is invalid; the old config stays.
    pub fn set_config(&mut self, config: NavConfig) -> Result<(), Diagnostic> {
        config.validate()?;
        self.classes = config.resolved_classes();
        let n = self.classes.len();
        self.caches = (0..n).map(|_| FlowCache::new(config.flow_radius)).collect();
        self.grids.truncate(n);
        self.tables.truncate(n);
        self.tables.resize_with(n, LinkTable::default);
        self.blocker_tables.clear();
        self.blocker_tables.resize_with(n, BlockerTable::default);
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

/// A column source over a pre-sampled rectangle of columns (one per class).
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

/// Samples the floors of the column centred on `(x, z)`, one list per class: the down-walk and the headroom
/// ray are shared, the capsule overlap and the height derive are the class's own.
fn sample_column(
    physics: &PhysicsWorld,
    filter: &QueryFilter,
    config: &NavConfig,
    classes: &[NavClass],
    x: f32,
    z: f32,
) -> Result<Vec<Vec<FloorSample>>, PhysicsError> {
    let mut out: Vec<Vec<FloorSample>> = classes.iter().map(|_| Vec::new()).collect();
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
        for (class, floors) in classes.iter().zip(&mut out) {
            let bottom = y + class.max_step;
            let top = (bottom + 2.0 * class.radius).max(y + class.height);
            let shape = Shape::Capsule {
                half_height: ((top - bottom) * 0.5 - class.radius).max(1e-4),
                radius: class.radius,
            };
            let centre = [x, (top + bottom) * 0.5, z];
            if !physics
                .overlap(&shape, centre, [0.0, 0.0, 0.0, 1.0], filter)?
                .is_empty()
            {
                continue;
            }
            floors.push(FloorSample {
                y,
                normal,
                headroom,
            });
        }
    }
    Ok(out)
}

/// Samples every cell in `cells` in parallel, in cell order; one [`Presampled`] per class.
fn presample(
    physics: &PhysicsWorld,
    filter: &QueryFilter,
    config: &NavConfig,
    classes: &[NavClass],
    cells: &[[u32; 2]],
    min: [u32; 2],
    width: u32,
) -> Result<Vec<Presampled>, PhysicsError> {
    let chunks = par::par_map_chunks(cells, SAMPLE_CHUNK, |_, chunk| {
        chunk
            .iter()
            .map(|&[i, j]| {
                let x = config.origin[0] + (i as f32 + 0.5) * config.cell_size;
                let z = config.origin[1] + (j as f32 + 0.5) * config.cell_size;
                sample_column(physics, filter, config, classes, x, z)
            })
            .collect::<Result<Vec<_>, _>>()
    });
    let mut sources: Vec<Presampled> = classes
        .iter()
        .map(|_| Presampled {
            min,
            width,
            columns: Vec::with_capacity(cells.len()),
        })
        .collect();
    for chunk in chunks {
        for column in chunk? {
            for (source, floors) in sources.iter_mut().zip(column) {
                source.columns.push(floors);
            }
        }
    }
    Ok(sources)
}

/// The inclusive cell rectangle the statics that differ between `old` and `new` can affect, or
/// `None` when nothing differs or the change lies off the grid. Both lists are sorted by entity. `radius`
/// is the widest class radius.
fn dirty_rect(
    old: &[StaticBody],
    new: &[StaticBody],
    config: &NavConfig,
    radius: f32,
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
    let pad = radius + config.cell_size;
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
    let (unsnapped, unknown, unknown_blockers) =
        if !nav.grids.is_empty() && nav.grids.len() == nav.classes.len() {
            let (overlays, blocker_tables, unknown_blockers) =
                build_blockers(world, &nav.grids, &nav.classes);
            for (grid, overlay) in nav.grids.iter_mut().zip(overlays) {
                grid.set_overlay(overlay);
            }
            nav.blocker_tables = blocker_tables;
            let (tables, unsnapped, unknown) = build_links(world, &nav.grids, &nav.classes);
            nav.tables = tables;
            (unsnapped, unknown, unknown_blockers)
        } else {
            (Vec::new(), Vec::new(), Vec::new())
        };
    let names = nav.config.class_names();
    world.insert_resource(nav);
    for (entity, index, name) in unknown {
        let key = format!("{}/classes/{index}", offender_key(world, entity));
        report_full(
            world,
            key,
            Diagnostic::new(
                code::NAV_CLASS_UNKNOWN,
                format!("the `NavLink` `classes` entry `{name}` is not a nav class, so it permits nothing"),
            )
            .with_path(format!("/components/NavLink/classes/{index}"))
            .with_help(format!("use one of: {names}; `ludion explain L4147`")),
        );
    }
    for (entity, index, name) in unknown_blockers {
        let key = format!("{}/blocker/classes/{index}", offender_key(world, entity));
        report_full(
            world,
            key,
            Diagnostic::new(
                code::NAV_CLASS_UNKNOWN,
                format!("the `NavBlocker` `classes` entry `{name}` is not a nav class, so it applies to nothing"),
            )
            .with_path(format!("/components/NavBlocker/classes/{index}"))
            .with_help(format!("use one of: {names}; `ludion explain L4147`")),
        );
    }
    for (entity, end, nearest) in unsnapped {
        let help = match nearest {
            Some(d) => format!(
                "the nearest walkable node is {d:.2} m away: move the `{end}` end onto walkable ground or raise `snap_radius`"
            ),
            None => format!("no walkable node is near: move the `{end}` end onto walkable ground"),
        };
        let key = offender_key(world, entity);
        report_full(
            world,
            key,
            Diagnostic::new(
                code::NAV_LINK_UNSNAPPED,
                format!("the `{end}` end of a `NavLink` snaps to no walkable node, so the link is left out"),
            )
            .with_path(format!("/components/NavLink/{end}"))
            .with_help(help),
        );
    }
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

/// A link end that snapped to nothing: its entity, `"from"` or `"to"`, and the nearest node's distance.
type Unsnapped = (EntityId, &'static str, Option<f32>);

/// A `NavLink` `classes` entry no class has: the link entity, the entry's index and its text.
type UnknownClass = (EntityId, usize, String);

/// The link tables from the enabled `NavLink`s, ascending by entity, one per class: each end snapped to the
/// class's grid, a link left out of a class its `classes` does not permit or where an end does not snap.
/// Also the ends that snapped on no permitted class as `(entity, "from" | "to", nearest node distance on
/// the first class's grid)`, and the `classes` entries naming no class.
fn build_links(
    world: &mut World,
    grids: &[NavGrid],
    classes: &[NavClass],
) -> (Vec<LinkTable>, Vec<Unsnapped>, Vec<UnknownClass>) {
    let mut found: Vec<(EntityId, NavLink)> = world
        .query_ids::<(&NavLink,)>()
        .map(|(id, (link,))| (id, link.clone()))
        .collect();
    found.sort_by_key(|(id, _)| *id);
    let mut tables: Vec<LinkTable> = grids.iter().map(|_| LinkTable::default()).collect();
    let mut edges: Vec<Vec<OffMeshLink>> = grids.iter().map(|_| Vec::new()).collect();
    let mut unsnapped = Vec::new();
    let mut unknown = Vec::new();
    for (entity, link) in found.into_iter().filter(|(_, l)| l.enabled) {
        let affine = crate::hierarchy::world_affine(world, entity);
        let posed = |p: [f32; 3]| affine.as_ref().map_or(p, |a| a.transform_point(p));
        let snap = |grid: &NavGrid, p: [f32; 3]| {
            if p.iter().all(|c| c.is_finite()) {
                nearest_node(grid, p, link.snap_radius)
            } else {
                None
            }
        };
        for (i, name) in link.classes.iter().enumerate() {
            if !classes.iter().any(|c| c.name == *name) {
                unknown.push((entity, i, name.clone()));
            }
        }
        let ends = [posed(link.from), posed(link.to)];
        let mut permitted = false;
        let mut end_snapped = [false; 2];
        for (k, grid) in grids.iter().enumerate() {
            if !link.classes.is_empty() && !link.classes.iter().any(|n| *n == classes[k].name) {
                continue;
            }
            permitted = true;
            let (from, to) = (snap(grid, ends[0]), snap(grid, ends[1]));
            end_snapped[0] |= from.is_some();
            end_snapped[1] |= to.is_some();
            let (Some(from), Some(to)) = (from, to) else {
                continue;
            };
            let (a, b) = (grid.node_pos(from), grid.node_pos(to));
            let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let table = &mut tables[k];
            edges[k].push(OffMeshLink {
                from,
                to,
                cost: sqrt(d[0] * d[0] + d[1] * d[1] + d[2] * d[2]) + link.cost,
                bidirectional: link.bidirectional,
                tag: table.entries.len() as u32,
            });
            table.entries.push(LinkEntry {
                entity,
                from,
                to,
                bidirectional: link.bidirectional,
            });
        }
        if !permitted {
            continue;
        }
        for (end, p, snapped) in [
            ("from", ends[0], end_snapped[0]),
            ("to", ends[1], end_snapped[1]),
        ] {
            if snapped {
                continue;
            }
            let wide = if p.iter().all(|c| c.is_finite()) {
                nearest_node(&grids[0], p, 1.0e6)
            } else {
                None
            };
            let distance = wide.map(|n| {
                let q = grids[0].node_pos(n);
                let d = [q[0] - p[0], q[1] - p[1], q[2] - p[2]];
                sqrt(d[0] * d[0] + d[1] * d[1] + d[2] * d[2])
            });
            unsnapped.push((entity, end, distance));
        }
    }
    for (table, edges) in tables.iter_mut().zip(edges) {
        table.links = NavLinks::new(edges);
    }
    (tables, unsnapped, unknown)
}

fn update(nav: &mut Nav, physics: &PhysicsWorld) -> Result<(), PhysicsError> {
    let mut statics: Vec<StaticBody> = physics.static_bodies().collect();
    statics.sort_by_key(|s| s.entity);
    let config = nav.config.clone();
    let classes = nav.classes.clone();
    let radius = classes.iter().map(|c| c.radius).fold(0.0, f32::max);
    let filter = QueryFilter::new()
        .only_fixed()
        .ignore_set(&config.ignore_layers);
    let full = nav.full_rebuild || nav.grids.len() != classes.len();
    let (min, max) = if full {
        ([0, 0], [config.nx - 1, config.nz - 1])
    } else if let Some(rect) = dirty_rect(&nav.last_statics, &statics, &config, radius) {
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
    let sources = presample(physics, &filter, &config, &classes, &nav.cells, min, width)?;
    if full {
        nav.grids = classes
            .iter()
            .zip(&sources)
            .map(|(class, source)| NavGrid::build(config.grid_params(class), source))
            .collect();
        // A fresh grid restarts its generation, which would pass off stale fields as current.
        nav.caches = classes
            .iter()
            .map(|_| FlowCache::new(config.flow_radius))
            .collect();
    } else {
        for (grid, source) in nav.grids.iter_mut().zip(&sources) {
            grid.rebuild_region(source, min, max);
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
/// assert!(req.partial && req.snap_radius == 1.0 && req.class.is_empty());
/// ```
#[derive(Clone, Debug, PartialEq)]
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
    /// The `NavConfig.classes` entry to plan for; empty is the first class (default empty).
    pub class: String,
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
            class: String::new(),
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
    /// Waypoint indices where a `NavLink` traversal starts: waypoint `i` is the link start, `i + 1` its end.
    pub links: Vec<u32>,
    /// Waypoint indices whose next leg enters an `interact` `NavBlocker` region: a routed walker stops there.
    pub blockers: Vec<u32>,
}

impl NavPath {
    fn unreachable(expanded: u32) -> Self {
        Self {
            status: PathStatus::Unreachable,
            cost: f32::INFINITY,
            waypoints: Vec::new(),
            expanded,
            links: Vec::new(),
            blockers: Vec::new(),
        }
    }
}

/// The grid and link table of `class`: `L4140` before the grids exist, `L4147` for an unknown class.
fn nav_class<'a>(
    world: &'a World,
    class: &str,
) -> Result<(&'a NavGrid, &'a LinkTable), Diagnostic> {
    let nav = world
        .resource::<Nav>()
        .filter(|nav| !nav.grids.is_empty())
        .ok_or_else(|| {
            Diagnostic::new(
                code::NAV_NOT_READY,
                "a navigation query ran before the nav grid exists",
            )
            .with_help("opt in with `Engine::builder().nav(NavConfig::default())` and run `nav_update` before querying")
        })?;
    let index = nav
        .config
        .class_index(class)
        .filter(|&i| i < nav.grids.len())
        .ok_or_else(|| {
            Diagnostic::new(
                code::NAV_CLASS_UNKNOWN,
                format!("navigation class `{class}` is not defined"),
            )
            .with_path("/class")
            .with_help(format!(
                "use one of: {}; `ludion explain L4147`",
                nav.config.class_names()
            ))
        })?;
    Ok((&nav.grids[index], &nav.tables[index]))
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
fn plan(grid: &NavGrid, table: &LinkTable, req: &PathRequest, project_start: bool) -> NavPath {
    let (Some(start), Some(goal)) = (
        nearest_node(grid, req.from, req.snap_radius),
        nearest_node(grid, req.to, req.snap_radius),
    ) else {
        return NavPath::unreachable(0);
    };
    let found = find_path_linked(
        grid,
        &table.links,
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
    let pulled = string_pull_marked(grid, &found);
    let mut waypoints = pulled.points;
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
        links: pulled.links.into_iter().map(|(i, _)| i).collect(),
        blockers: pulled.stops.into_iter().map(|(i, _)| i).collect(),
    }
}

/// Plans a route from `req.from` to `req.to` over the walkable grid: A* with string pulling, so the
/// waypoints are the few corners a straight-line walker needs. Both ends snap to the nearest node within
/// `snap_radius`; an end with none is [`PathStatus::Unreachable`]. A goal that cannot be reached is
/// [`PathStatus::Partial`] (a path to the reachable node nearest it) unless `partial` is off. Pure
/// and deterministic; it reads the grid and changes nothing. Enabled `NavLink`s are edges of the search:
/// `links` on the answer lists the waypoints where a traversal starts.
///
/// # Errors
/// `L4140` before the grid exists (no `.nav(..)` opt-in, or `nav_update` has not run); `L4147` for a
/// `class` the config does not define; `L4141` for a non-finite point, a negative or NaN `max_cost`, a negative or non-finite `snap_radius`, or a zero
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
    let (grid, table) = nav_class(world, &req.class)?;
    check_request(req)?;
    Ok(plan(grid, table, req, true))
}

/// The walking distance in metres from `from` to `to` along the grid, or `None` when the goal cannot be
/// reached (either end off the grid, or no route). Ends snap within one metre. `class` names the agent
/// class (`""` is the first).
///
/// # Errors
/// As [`nav_path`].
///
/// # Example
/// ```
/// use ludion_engine::{Engine, NavConfig, nav_distance};
///
/// let engine = Engine::builder().nav(NavConfig::default()).build().unwrap();
/// assert!(nav_distance(engine.world(), [0.0; 3], [5.0, 0.0, 5.0], "").is_err());
/// ```
pub fn nav_distance(
    world: &World,
    from: [f32; 3],
    to: [f32; 3],
    class: &str,
) -> Result<Option<f32>, Diagnostic> {
    let req = PathRequest {
        partial: false,
        class: class.to_string(),
        ..PathRequest::new(from, to)
    };
    let path = nav_path(world, &req)?;
    Ok((path.status == PathStatus::Complete).then_some(path.cost))
}

/// Whether `to` can be walked to from `from` along the `class` grid. Ends snap within one metre.
///
/// # Errors
/// As [`nav_path`].
///
/// # Example
/// ```
/// use ludion_engine::{Engine, NavConfig, nav_reachable};
///
/// let engine = Engine::builder().nav(NavConfig::default()).build().unwrap();
/// assert!(nav_reachable(engine.world(), [0.0; 3], [5.0, 0.0, 5.0], "").is_err());
/// ```
pub fn nav_reachable(
    world: &World,
    from: [f32; 3],
    to: [f32; 3],
    class: &str,
) -> Result<bool, Diagnostic> {
    Ok(nav_distance(world, from, to, class)?.is_some())
}

/// The nearest walkable point to `p` within `max_dist` metres: `p` itself lifted or dropped onto the
/// floor when it is over walkable ground, else the centre of the nearest walkable cell; `None` when
/// nothing is that close. `class` names the agent class (`""` is the first).
///
/// # Errors
/// `L4140` before the grid exists; `L4147` for an unknown `class`; `L4141` for a non-finite `p` or a
/// negative or non-finite `max_dist`.
///
/// # Example
/// ```
/// use ludion_engine::{Engine, NavConfig, nav_project};
///
/// let engine = Engine::builder().nav(NavConfig::default()).build().unwrap();
/// assert!(nav_project(engine.world(), [0.0; 3], "", 2.0).is_err());
/// ```
pub fn nav_project(
    world: &World,
    p: [f32; 3],
    class: &str,
    max_dist: f32,
) -> Result<Option<[f32; 3]>, Diagnostic> {
    let (grid, _) = nav_class(world, class)?;
    check_point("p", p)?;
    check_radius("max_dist", max_dist)?;
    Ok(nearest_node(grid, p, max_dist).map(|node| project(grid, p, node)))
}

/// Where a straight walk from `from` toward `to` first leaves the `class` grid: the point on the line
/// where the next step has no link (a wall, a drop, a slope too steep for the class), at the floor height
/// of the last node reached. `None` when the whole line is walkable; `Some(from)` when `from` itself is
/// off the grid.
///
/// # Errors
/// `L4140` before the grid exists; `L4147` for an unknown `class`; `L4141` for a non-finite point.
///
/// # Example
/// ```
/// use ludion_engine::{Engine, NavConfig, nav_raycast};
///
/// let engine = Engine::builder().nav(NavConfig::default()).build().unwrap();
/// assert!(nav_raycast(engine.world(), [0.0; 3], [5.0, 0.0, 5.0], "").is_err());
/// ```
pub fn nav_raycast(
    world: &World,
    from: [f32; 3],
    to: [f32; 3],
    class: &str,
) -> Result<Option<[f32; 3]>, Diagnostic> {
    let (grid, _) = nav_class(world, class)?;
    check_point("from", from)?;
    check_point("to", to)?;
    Ok(line_exit(grid, from, to))
}

/// A uniformly random walkable point among the nodes within `radius` metres of walking distance of
/// `around` on the `class` grid (grid links only, no off-mesh links), drawn from `rng` with one
/// `gen_range_usize` call. `around` snaps to a node within one metre; `None` when it is off the grid.
/// The nodes are taken in ascending node order, so the answer is a pure function of the grid and the
/// generator state.
///
/// # Errors
/// `L4140` before the grid exists; `L4147` for an unknown `class`; `L4141` for a non-finite `around` or
/// a negative or non-finite `radius`.
///
/// # Example
/// ```
/// use ludion_core::rng::DeterministicRng;
/// use ludion_engine::{Engine, NavConfig, nav_random_point};
///
/// let engine = Engine::builder().nav(NavConfig::default()).build().unwrap();
/// let mut rng = DeterministicRng::new(7);
/// assert!(nav_random_point(engine.world(), [0.0; 3], 5.0, "", &mut rng).is_err());
/// ```
pub fn nav_random_point(
    world: &World,
    around: [f32; 3],
    radius: f32,
    class: &str,
    rng: &mut DeterministicRng,
) -> Result<Option<[f32; 3]>, Diagnostic> {
    let (grid, _) = nav_class(world, class)?;
    check_point("around", around)?;
    check_radius("radius", radius)?;
    let Some(start) = nearest_node(grid, around, 1.0) else {
        return Ok(None);
    };
    let nodes = reachable_within(grid, start, radius);
    if nodes.is_empty() {
        return Ok(None);
    }
    Ok(Some(
        grid.node_pos(nodes[rng.gen_range_usize(0..nodes.len())]),
    ))
}

/// One agent as the steering phase sees it.
struct Steer {
    entity: EntityId,
    agent: NavAgent,
    /// The agent's class index; `None` for a name the config does not define.
    class: Option<usize>,
    pos: [f32; 3],
    /// The walking agent's target node, `None` for a flyer, a routed agent, or a target off the grid.
    node: Option<NodeId>,
    goal: Option<NavGoal>,
    route: Option<NavRoute>,
    route_write: RouteWrite,
    /// The agent's previous `NavSteering.velocity` (zero when absent): its own motion for avoidance.
    velocity: [f32; 3],
    /// The avoidance radius: `avoidance.radius`, else the class radius (0 for an unknown class).
    radius: f32,
}

/// One walking agent as its neighbours' avoidance sees it: start-of-tick state only.
struct AvoidBody {
    radius: f32,
    velocity: [f32; 2],
    has_avoidance: bool,
    priority: u32,
}

/// One enabled [`NavObstacle`] as avoiding agents see it: its pose and last-step motion.
struct ObstacleBody {
    position: [f32; 3],
    velocity: [f32; 3],
    radius: f32,
}

/// The walking agents and enabled obstacles by ascending entity, built only when some agent has `avoidance`.
struct AvoidTable {
    bodies: Vec<(EntityId, AvoidBody)>,
    obstacles: Vec<ObstacleBody>,
    max_body_radius: f32,
    dt: f32,
}

/// Per-chunk scratch for [`steer_one`]'s avoidance, so the parallel phase does not allocate per agent.
#[derive(Default)]
struct AvoidScratch {
    lines: Vec<OrcaLine>,
    neighbors: Vec<AvoidNeighbor>,
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
///
/// With blockers on the grid every remaining leg is tested, since a blocker can close a leg anywhere ahead.
/// A leg that starts at a link or blocker waypoint is the traversal or the wait, not a line to walk.
fn leg_walkable(grid: &NavGrid, route: &NavRoute) -> bool {
    let index = route.index as usize;
    let last = route.waypoints.len().saturating_sub(1);
    let through = if grid.has_overlay() { last } else { index };
    // The route was pulled between node centres; the goal end sits off-centre, so test the centre.
    let centre = |p: [f32; 3]| grid.node_at(p).map_or(p, |n| grid.node_pos(n));
    (index.max(1)..=through.min(last)).all(|to| {
        let prev = to - 1;
        route.links.contains(&(prev as u32))
            || route.blockers.contains(&(prev as u32))
            || walkable_line(
                grid,
                centre(route.waypoints[prev]),
                centre(route.waypoints[to]),
            )
    })
}

/// Whether the first link traversal still ahead on `route` no longer resolves to an enabled link.
fn pending_link_missing(grid: &NavGrid, table: &LinkTable, route: &NavRoute) -> bool {
    let Some(&i) = route.links.iter().find(|&&i| i >= route.index) else {
        return false;
    };
    let i = i as usize;
    match (route.waypoints.get(i), route.waypoints.get(i + 1)) {
        (Some(&start), Some(&end)) => table.resolve(grid, start, end).is_none(),
        _ => true,
    }
}

/// Why a routed walker plans again; `as_str` is the `nav` trace channel's `reason`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlanReason {
    New,
    Retry,
    GoalMoved,
    LinkMissing,
    BlockerClosed,
    LegBlocked,
}

impl PlanReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Retry => "retry",
            Self::GoalMoved => "goal_moved",
            Self::LinkMissing => "link_missing",
            Self::BlockerClosed => "blocker_closed",
            Self::LegBlocked => "leg_blocked",
        }
    }
}

/// What a planned route came out as, for the `nav` trace channel.
pub(crate) struct PlanOutcome {
    pub(crate) status: PathStatus,
    pub(crate) expanded: u32,
    pub(crate) cost: f32,
    pub(crate) waypoints: u32,
    pub(crate) links: u32,
    pub(crate) blockers: u32,
}

/// One route plan `nav_steer` attempted; `outcome` is `None` when the per-tick cap deferred it.
pub(crate) struct NavPlanLog {
    pub(crate) tick: u64,
    pub(crate) entity: EntityId,
    pub(crate) class: String,
    pub(crate) reason: PlanReason,
    pub(crate) from: [f32; 3],
    pub(crate) to: [f32; 3],
    pub(crate) outcome: Option<PlanOutcome>,
}

/// The opt-in `nav` trace sink: a plain resource, never hashed or snapshotted, present only while traced.
#[derive(Default)]
pub(crate) struct NavTrace {
    pub(crate) plans: Vec<NavPlanLog>,
}

impl NavTrace {
    pub(crate) fn take(&mut self) -> Vec<NavPlanLog> {
        std::mem::take(&mut self.plans)
    }
}

/// Why `route` must be planned again for `goal`, `None` when it stands.
fn plan_reason(
    grid: &NavGrid,
    table: &LinkTable,
    route: Option<&NavRoute>,
    goal: &NavGoal,
) -> Option<PlanReason> {
    let cell = |p| grid.cell_of(p);
    let Some(route) = route else {
        return Some(PlanReason::New);
    };
    match route.status {
        // A walker stopped at a link waits for `nav_link_done`; nothing replans it.
        NavRouteStatus::AtLink => None,
        // A walker stopped at a blocker waits for the game to open it; only a blocked region (or a goal
        // that moved on) plans again.
        NavRouteStatus::AtBlocker => {
            if cell(goal.at) != cell(route.goal) {
                Some(PlanReason::GoalMoved)
            } else if pending_blocker_blocked(grid, route) {
                Some(PlanReason::BlockerClosed)
            } else {
                None
            }
        }
        status => {
            let following = status == NavRouteStatus::Following;
            if status == NavRouteStatus::Planning {
                Some(PlanReason::Retry)
            } else if cell(goal.at) != cell(route.goal) {
                Some(PlanReason::GoalMoved)
            } else if following && pending_link_missing(grid, table, route) {
                Some(PlanReason::LinkMissing)
            } else if following && pending_blocker_blocked(grid, route) {
                Some(PlanReason::BlockerClosed)
            } else if goal.replan == NavReplan::OnChange
                && following
                && !route.partial
                && !leg_walkable(grid, route)
            {
                Some(PlanReason::LegBlocked)
            } else {
                None
            }
        }
    }
}

/// The serial plan phase for one routed walker: decides from hashed state and the current grid whether
/// the route needs planning, and plans it while the per-tick `budget` lasts. Returns the attempt's log
/// when a plan was needed and `tick` is `Some` (tracing).
fn plan_route(
    grid: &NavGrid,
    table: &LinkTable,
    steer: &mut Steer,
    goal: NavGoal,
    budget: &mut u32,
    tick: Option<u64>,
) -> Option<NavPlanLog> {
    let reason = plan_reason(grid, table, steer.route.as_ref(), &goal);
    let Some(reason) = reason else {
        if let Some(route) = steer.route.as_mut()
            && route.status != NavRouteStatus::AtLink
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
        return None;
    };
    let mut route = NavRoute {
        goal: goal.at,
        ..NavRoute::default()
    };
    let mut outcome = None;
    if *budget > 0 {
        *budget -= 1;
        let path = plan(grid, table, &PathRequest::new(steer.pos, goal.at), false);
        if tick.is_some() {
            outcome = Some(PlanOutcome {
                status: path.status,
                expanded: path.expanded,
                cost: path.cost,
                waypoints: path.waypoints.len() as u32,
                links: path.links.len() as u32,
                blockers: path.blockers.len() as u32,
            });
        }
        if path.status != PathStatus::Unreachable {
            // A route that opens on a link start or a blocker must stop there, not walk past it.
            route.index = if path.links.first() == Some(&0) || path.blockers.first() == Some(&0) {
                0
            } else {
                1.min(path.waypoints.len().saturating_sub(1)) as u32
            };
            route.links = path.links;
            route.blockers = path.blockers;
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
    tick.map(|tick| NavPlanLog {
        tick,
        entity: steer.entity,
        class: steer.agent.class.clone(),
        reason,
        from: steer.pos,
        to: goal.at,
        outcome,
    })
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
/// A walker with `NavAgent.avoidance` gets an ORCA velocity (`ludion_nav::avoid_velocity`) in place of that
/// push: its neighbours are the walking agents in the index, each at its previous `NavSteering.velocity`. The
/// result is kept only if a cell-length walk along it stays on the class grid (blocker overlay included);
/// otherwise the seek stands. An agent at a link or blocker does not avoid.
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
pub fn nav_steer(world: &mut World, ctx: &SystemCtx) {
    let obstacles = update_obstacles(world, ctx);
    let mut agents: Vec<(EntityId, NavAgent)> = world
        .query_ids::<(&NavAgent,)>()
        .map(|(id, (agent,))| (id, agent.clone()))
        .collect();
    if agents.is_empty() {
        return;
    }
    agents.sort_by_key(|(entity, _)| *entity);
    let mut steers: Vec<Steer> = agents
        .into_iter()
        .filter_map(|(entity, agent)| {
            let pos = world_transform(world, entity)?.trs.translation;
            let class = world
                .resource::<Nav>()
                .map_or(Some(0), |nav| nav.config.class_index(&agent.class));
            let radius = agent.avoidance.and_then(|a| a.radius).unwrap_or_else(|| {
                world
                    .resource::<Nav>()
                    .zip(class)
                    .and_then(|(nav, c)| nav.classes.get(c))
                    .map_or(0.0, |c| c.radius)
            });
            Some(Steer {
                entity,
                agent,
                class,
                pos,
                node: None,
                goal: world.get::<NavGoal>(entity).ok().flatten().copied(),
                route: world.get::<NavRoute>(entity).ok().flatten().cloned(),
                route_write: RouteWrite::Keep,
                velocity: world
                    .get::<NavSteering>(entity)
                    .ok()
                    .flatten()
                    .map_or([0.0; 3], |s| s.velocity),
                radius,
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

    let tick = world
        .resource::<NavTrace>()
        .is_some()
        .then(|| ctx.frame.as_u64());
    let mut plan_logs: Vec<NavPlanLog> = Vec::new();
    let mut nav = world.remove_resource::<Nav>();
    if let Some(nav) = nav.as_mut() {
        let mut budget = nav.config.max_path_requests_per_tick;
        for steer in steers.iter_mut().filter(|s| !s.agent.flying) {
            let Some(class) = steer.class else {
                continue;
            };
            let (grid, cache, table) = nav.split_class(class);
            let Some(grid) = grid else {
                continue;
            };
            if let Some(goal) = steer.goal {
                plan_logs.extend(plan_route(grid, table, steer, goal, &mut budget, tick));
                continue;
            }
            steer.node = grid.node_at(steer.agent.target);
            if let Some(node) = steer.node {
                cache.field(grid, node);
            }
        }
    }
    if tick.is_some()
        && let Some(sink) = world.resource_mut::<NavTrace>()
    {
        sink.plans.append(&mut plan_logs);
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
    let views: Vec<ClassView> = nav.as_ref().map_or_else(Vec::new, |n| {
        (0..n.classes.len())
            .map(|i| ClassView {
                grid: n.grids.get(i),
                cache: &n.caches[i],
                table: &n.tables[i],
                blockers: &n.blocker_tables[i],
                height: n.classes[i].height,
            })
            .collect()
    });
    let avoid_table = steers
        .iter()
        .any(|s| s.agent.avoidance.is_some() && !s.agent.flying)
        .then(|| {
            let bodies: Vec<(EntityId, AvoidBody)> = steers
                .iter()
                .filter(|s| !s.agent.flying && s.class.is_some())
                .map(|s| {
                    (
                        s.entity,
                        AvoidBody {
                            radius: s.radius,
                            velocity: [s.velocity[0], s.velocity[2]],
                            has_avoidance: s.agent.avoidance.is_some(),
                            priority: s.agent.avoidance.map_or(0, |a| a.priority),
                        },
                    )
                })
                .collect();
            let max_body_radius = obstacles.iter().map(|o| o.radius).fold(
                bodies.iter().fold(0.0f32, |m, (_, b)| m.max(b.radius)),
                f32::max,
            );
            AvoidTable {
                bodies,
                obstacles,
                max_body_radius,
                dt: ctx.dt.max(1e-4),
            }
        });
    let chunks = par::par_map_chunks(&steers, STEER_CHUNK, |_, chunk| {
        let mut scratch: Vec<Neighbor> = Vec::new();
        let mut avoid_scratch = AvoidScratch::default();
        chunk
            .iter()
            .map(|steer| {
                let view = steer.class.and_then(|c| views.get(c));
                steer_one(
                    steer,
                    view,
                    index,
                    avoid_table.as_ref(),
                    &mut scratch,
                    &mut avoid_scratch,
                )
            })
            .collect::<Vec<([f32; 3], Option<Advance>)>>()
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
    let names = nav.as_ref().map(|n| n.config.class_names());
    if let Some(mut nav) = nav {
        for cache in &mut nav.caches {
            cache.end_frame();
        }
        world.insert_resource(nav);
    }
    for steer in steers
        .iter()
        .filter(|s| s.class.is_none() && !s.agent.flying)
    {
        let key = offender_key(world, steer.entity);
        report_full(
            world,
            key,
            Diagnostic::new(
                code::NAV_CLASS_UNKNOWN,
                format!(
                    "the `NavAgent` class `{}` is not a nav class, so the agent stands still",
                    steer.agent.class
                ),
            )
            .with_path("/components/NavAgent/class")
            .with_help(format!(
                "use one of: {}; `ludion explain L4147`",
                names.as_deref().unwrap_or("none")
            )),
        );
    }
}

/// One class's derived state as the parallel steering phase reads it.
struct ClassView<'a> {
    grid: Option<&'a NavGrid>,
    cache: &'a FlowCache,
    table: &'a LinkTable,
    blockers: &'a BlockerTable,
    height: f32,
}

/// Tells a routed agent its game-played link traversal is finished: the route continues from the link's
/// end (`index` moves to the waypoint after the link start, `status` back to `following`, `link` clears).
/// The game moves the body to the link's end first; the engine never does.
///
/// # Errors
/// `L4146` when the entity has no `NavRoute`, or its route is not `at_link`.
///
/// # Example
/// ```
/// use ludion_engine::{Engine, nav_link_done};
///
/// let mut engine = Engine::builder().build().unwrap();
/// let agent = engine.world_mut().spawn();
/// // No route, so nothing is waiting at a link.
/// let err = nav_link_done(engine.world_mut(), agent).unwrap_err();
/// assert_eq!(err.code.as_str(), "L4146");
/// ```
pub fn nav_link_done(world: &mut World, agent: EntityId) -> Result<(), Diagnostic> {
    let not_at_link = || {
        Diagnostic::new(
            code::NAV_NOT_AT_LINK,
            format!("entity {agent:?} is not stopped at a nav link"),
        )
        .with_help("call `nav_link_done` only while its `NavRoute.status` is `at_link`; `ludion explain L4146`")
    };
    let Ok(Some(route)) = world.get_mut::<NavRoute>(agent) else {
        return Err(not_at_link());
    };
    if route.status != NavRouteStatus::AtLink {
        return Err(not_at_link());
    }
    let last = route.waypoints.len().saturating_sub(1) as u32;
    route.index = (route.index + 1).min(last);
    route.status = NavRouteStatus::Following;
    route.link = None;
    Ok(())
}

/// A route's `index`, `status`, `link` and `blocker` after this tick's steering.
#[derive(Clone, Copy, PartialEq)]
struct Advance {
    index: u32,
    status: NavRouteStatus,
    link: Option<EntityId>,
    blocker: Option<EntityId>,
}

/// The lowest `interact` blocker whose region the leg from waypoint `at` enters, `None` once the waypoint after
/// `at` is no longer inside one.
fn blocker_at(
    grid: &NavGrid,
    table: &BlockerTable,
    route: &NavRoute,
    at: usize,
) -> Option<EntityId> {
    let node = grid.node_at(*route.waypoints.get(at + 1)?)?;
    table.entries.get(grid.stop_tag(node)? as usize).copied()
}

/// Whether the first blocker stop still ahead on `route` now enters a blocked region.
fn pending_blocker_blocked(grid: &NavGrid, route: &NavRoute) -> bool {
    route
        .blockers
        .iter()
        .find(|&&i| i >= route.index)
        .and_then(|&i| route.waypoints.get(i as usize + 1))
        .and_then(|&p| grid.node_at(p))
        .is_some_and(|n| grid.mark(n) == NodeMark::Block)
}

/// Applies the plan phase's change and the steering advance to the stored [`NavRoute`].
fn write_route(world: &mut World, steer: &mut Steer, advance: Option<Advance>) {
    if steer.route_write == RouteWrite::Remove {
        let _ = world.remove::<NavRoute>(steer.entity);
        return;
    }
    let Some(route) = steer.route.as_mut() else {
        return;
    };
    let mut store = steer.route_write == RouteWrite::Store;
    if let Some(next) = advance
        && (route.index, route.status, route.link, route.blocker)
            != (next.index, next.status, next.link, next.blocker)
    {
        (route.index, route.status, route.link, route.blocker) =
            (next.index, next.status, next.link, next.blocker);
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

/// Reads every [`NavObstacle`] pose (ascending entity, enabled or not) into its [`NavObstacleState`] and returns
/// the enabled ones. Velocity is the motion since the last tick, zero on the first.
fn update_obstacles(world: &mut World, ctx: &SystemCtx) -> Vec<ObstacleBody> {
    let mut found: Vec<(EntityId, NavObstacle)> = world
        .query_ids::<(&NavObstacle,)>()
        .map(|(id, (obstacle,))| (id, *obstacle))
        .collect();
    found.sort_by_key(|(entity, _)| *entity);
    let mut out = Vec::new();
    for (entity, obstacle) in found {
        let Some(position) = world_transform(world, entity).map(|t| t.trs.translation) else {
            continue;
        };
        let previous = world
            .get::<NavObstacleState>(entity)
            .ok()
            .flatten()
            .copied();
        let velocity = match previous {
            Some(state) if ctx.dt > 0.0 => {
                let mut v = [0.0f32; 3];
                for (axis, c) in v.iter_mut().enumerate() {
                    *c = (position[axis] - state.position[axis]) / ctx.dt;
                }
                v
            }
            _ => [0.0; 3],
        };
        let _ = world.insert(entity, NavObstacleState { position, velocity });
        if obstacle.enabled {
            out.push(ObstacleBody {
                position,
                velocity,
                radius: obstacle.radius,
            });
        }
    }
    out
}

fn steer_one(
    steer: &Steer,
    view: Option<&ClassView>,
    index: &Index3d,
    avoid: Option<&AvoidTable>,
    scratch: &mut Vec<Neighbor>,
    avoid_scratch: &mut AvoidScratch,
) -> ([f32; 3], Option<Advance>) {
    let Steer {
        entity, agent, pos, ..
    } = steer;
    // A walker of an unknown class stands still.
    if !agent.flying && steer.class.is_none() {
        return ([0.0; 3], None);
    }
    let grid = view.and_then(|v| v.grid);
    let table = view.map(|v| v.table);
    let cache = view.map(|v| v.cache);
    let mut advance = None;
    let mut velocity = if agent.flying {
        let target = steer.goal.map_or(agent.target, |g| g.at);
        seek_flying(agent, target, *pos)
    } else if steer.goal.is_some() {
        let (velocity, adv) = seek_routed(steer, grid, table, view.map(|v| v.blockers));
        advance = adv;
        velocity
    } else {
        seek_walking(steer, grid, cache)
    };
    // An agent traversing a link, or waiting at a blocker, stands exactly still.
    let waiting = |status| matches!(status, NavRouteStatus::AtLink | NavRouteStatus::AtBlocker);
    let at_link = match advance {
        Some(a) => waiting(a.status),
        None => steer.route.as_ref().is_some_and(|r| waiting(r.status)),
    };
    if let (Some(avoidance), Some(table), false, false) =
        (agent.avoidance, avoid, agent.flying, at_link)
    {
        let height = view.map_or(0.0, |v| v.height);
        let filter = Filter::new(agent.neighbor_layers).excluding(Handle::entity(*entity));
        index.k_nearest(
            *pos,
            avoidance.max_neighbors as usize,
            steer.radius + table.max_body_radius + agent.speed * avoidance.horizon_s,
            &filter,
            scratch,
        );
        avoid_scratch.neighbors.clear();
        for n in scratch.iter() {
            let Some(other) = n.handle.as_entity() else {
                continue;
            };
            let Ok(slot) = table.bodies.binary_search_by_key(&other, |(e, _)| *e) else {
                continue;
            };
            let body = &table.bodies[slot].1;
            if (n.center[1] - pos[1]).abs() > height {
                continue;
            }
            let responsibility = if !body.has_avoidance || body.priority > avoidance.priority {
                1.0
            } else if body.priority == avoidance.priority {
                0.5
            } else {
                0.0
            };
            avoid_scratch.neighbors.push(AvoidNeighbor {
                position: [n.center[0], n.center[2]],
                velocity: body.velocity,
                radius: body.radius,
                responsibility,
            });
        }
        let reach = steer.radius + table.max_body_radius + agent.speed * avoidance.horizon_s;
        for obstacle in &table.obstacles {
            let dx = obstacle.position[0] - pos[0];
            let dz = obstacle.position[2] - pos[2];
            if dx * dx + dz * dz > reach * reach || (obstacle.position[1] - pos[1]).abs() > height {
                continue;
            }
            avoid_scratch.neighbors.push(AvoidNeighbor {
                position: [obstacle.position[0], obstacle.position[2]],
                velocity: [obstacle.velocity[0], obstacle.velocity[2]],
                radius: obstacle.radius,
                responsibility: 1.0,
            });
        }
        let me = AvoidAgent {
            position: [pos[0], pos[2]],
            velocity: [steer.velocity[0], steer.velocity[2]],
            preferred: [velocity[0], velocity[2]],
            radius: steer.radius,
            max_speed: agent.speed,
            horizon_s: avoidance.horizon_s,
            dt: table.dt,
        };
        let avoided = avoid_velocity(&me, &avoid_scratch.neighbors, &mut avoid_scratch.lines);
        let speed = sqrt(avoided[0] * avoided[0] + avoided[1] * avoided[1]);
        let on_grid = grid.is_some_and(|g| {
            speed <= 1e-6 || {
                let step = g.params().cell_size / speed;
                walkable_line(
                    g,
                    *pos,
                    [
                        pos[0] + avoided[0] * step,
                        pos[1],
                        pos[2] + avoided[1] * step,
                    ],
                )
            }
        });
        if on_grid {
            velocity = [avoided[0], 0.0, avoided[1]];
        }
    } else if agent.neighbor_layers != 0 && agent.separation_neighbors > 0 && !at_link {
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
fn seek_routed(
    steer: &Steer,
    grid: Option<&NavGrid>,
    table: Option<&LinkTable>,
    blockers: Option<&BlockerTable>,
) -> ([f32; 3], Option<Advance>) {
    let (Some(grid), Some(route)) = (grid, &steer.route) else {
        return ([0.0; 3], None);
    };
    let waypoints = &route.waypoints;
    if !matches!(
        route.status,
        NavRouteStatus::Following | NavRouteStatus::AtBlocker
    ) || waypoints.is_empty()
    {
        return ([0.0; 3], None);
    }
    let at_blocker = |at: usize| {
        let entity = blocker_at(grid, blockers?, route, at)?;
        Some(Advance {
            index: at as u32,
            status: NavRouteStatus::AtBlocker,
            link: None,
            blocker: Some(entity),
        })
    };
    // A walker stopped at a blocker stays while its region is still an `interact` blocker; otherwise it
    // continues as a following route from the same waypoint.
    if route.status == NavRouteStatus::AtBlocker
        && let Some(stay) = at_blocker((route.index as usize).min(waypoints.len() - 1))
    {
        return ([0.0; 3], Some(stay));
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
        return (
            [0.0; 3],
            Some(Advance {
                index: at as u32,
                status: NavRouteStatus::Arrived,
                link: None,
                blocker: None,
            }),
        );
    }
    let pass = agent.arrive_radius.max(0.5 * grid.params().cell_size);
    while at < last && dist(at) <= pass {
        if route.links.contains(&(at as u32))
            && let Some(link) =
                table.and_then(|t| t.resolve(grid, waypoints[at], waypoints[at + 1]))
        {
            return (
                [0.0; 3],
                Some(Advance {
                    index: at as u32,
                    status: NavRouteStatus::AtLink,
                    link: Some(link),
                    blocker: None,
                }),
            );
        }
        if route.blockers.contains(&(at as u32))
            && let Some(stop) = at_blocker(at)
        {
            return ([0.0; 3], Some(stop));
        }
        at += 1;
    }
    let d = dist(at);
    let advance = Some(Advance {
        index: at as u32,
        status: NavRouteStatus::Following,
        link: None,
        blocker: None,
    });
    if d <= 0.0 {
        return ([0.0; 3], advance);
    }
    let o = offset(at);
    (
        [o[0] / d * agent.speed, 0.0, o[1] / d * agent.speed],
        advance,
    )
}

fn seek_flying(agent: &NavAgent, target: [f32; 3], pos: [f32; 3]) -> [f32; 3] {
    let to = [target[0] - pos[0], target[1] - pos[1], target[2] - pos[2]];
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

/// Metres around the camera inside which the `nav` view draws grid nodes and flow arrows.
const NAV_VIEW_RADIUS: f32 = 30.0;
/// Most segments the `nav` view draws; the grid is drawn last, so it is what gets truncated.
const NAV_MAX_SEGMENTS: usize = 200_000;
/// Metres the `nav` lines sit above the floor.
const NAV_LIFT: f32 = 0.05;

/// Which `nav` parts [`debug_lines`] draws.
#[derive(Clone, Copy, Debug)]
pub(crate) struct NavDebugParts {
    pub(crate) grid: bool,
    pub(crate) routes: bool,
    pub(crate) flow: bool,
}

const ROUTE_AHEAD: [f32; 4] = [0.0, 0.85, 0.85, 1.0];
const ROUTE_PARTIAL: [f32; 4] = [1.0, 0.55, 0.1, 1.0];
const ROUTE_UNREACHABLE: [f32; 4] = [1.0, 0.15, 0.15, 1.0];
const FLOW_COLOR: [f32; 4] = [1.0, 0.85, 0.1, 1.0];
const NODE_COLOR: [f32; 4] = [0.7, 0.7, 0.7, 1.0];
const LINK_FLAT_COLOR: [f32; 4] = [0.5, 0.5, 0.5, 1.0];
const LINK_STEP_COLOR: [f32; 4] = [0.3, 0.3, 0.3, 1.0];
const LINK_OFFMESH_COLOR: [f32; 4] = [1.0, 0.2, 0.9, 1.0];

fn lifted(p: [f32; 3]) -> [f32; 3] {
    [p[0], p[1] + NAV_LIFT, p[2]]
}

fn within(eye: Option<[f32; 3]>, p: [f32; 3]) -> bool {
    eye.is_none_or(|e| {
        let d = [p[0] - e[0], p[1] - e[1], p[2] - e[2]];
        d[0] * d[0] + d[1] * d[1] + d[2] * d[2] <= NAV_VIEW_RADIUS * NAV_VIEW_RADIUS
    })
}

/// The entities with a `NavRoute`, ascending.
fn route_entities(world: &World) -> Vec<EntityId> {
    let mut ids: Vec<EntityId> = world
        .entities()
        .filter(|id| world.has::<NavRoute>(*id))
        .collect();
    ids.sort_unstable();
    ids
}

/// The distinct nodes walking agents without a `NavGoal` head for, ascending: the flow fields they read.
fn flow_targets(world: &World, grid: &NavGrid) -> Vec<NodeId> {
    let mut targets: Vec<NodeId> = world
        .entities()
        .filter(|id| !world.has::<NavGoal>(*id))
        .filter_map(|id| {
            let agent = world.get::<NavAgent>(id).ok().flatten()?;
            (!agent.flying)
                .then(|| grid.node_at(agent.target))
                .flatten()
        })
        .collect();
    targets.sort_unstable();
    targets.dedup();
    targets
}

/// The `nav` debug category, drawn at extraction from the [`Nav`] resource and the `NavRoute` and
/// `NavAgent` components; read-only, it never builds a field. Routes come first and are never culled,
/// then flow arrows, then the grid, so [`NAV_MAX_SEGMENTS`] only ever truncates the grid. Flow arrows and
/// grid nodes within [`NAV_VIEW_RADIUS`] of `eye` are drawn (all of them when `eye` is `None`).
pub(crate) fn debug_lines(
    world: &World,
    parts: NavDebugParts,
    eye: Option<[f32; 3]>,
) -> Vec<DebugSegment> {
    let mut lines = Vec::new();
    let seg = |a, b, color| DebugSegment { a, b, color };
    if parts.routes {
        for id in route_entities(world) {
            let Ok(Some(route)) = world.get::<NavRoute>(id) else {
                continue;
            };
            let index = (route.index as usize).min(route.waypoints.len());
            let ahead = if route.partial {
                ROUTE_PARTIAL
            } else {
                ROUTE_AHEAD
            };
            let behind = [ahead[0] * 0.4, ahead[1] * 0.4, ahead[2] * 0.4, 1.0];
            let done = &route.waypoints[..route.waypoints.len().min(index + 1)];
            for pair in done.windows(2) {
                lines.push(seg(lifted(pair[0]), lifted(pair[1]), behind));
            }
            if let Some(pose) = world_transform(world, id) {
                let mut at = lifted(pose.trs.translation);
                for waypoint in route.waypoints.iter().skip(index) {
                    lines.push(seg(at, lifted(*waypoint), ahead));
                    at = lifted(*waypoint);
                }
            }
            let color = if route.status == NavRouteStatus::Unreachable {
                ROUTE_UNREACHABLE
            } else {
                ahead
            };
            let g = lifted(route.goal);
            for axis in [[0.15, 0.0, 0.0], [0.0, 0.0, 0.15]] {
                let a = [g[0] - axis[0], g[1], g[2] - axis[2]];
                let b = [g[0] + axis[0], g[1], g[2] + axis[2]];
                lines.push(seg(a, b, color));
            }
        }
    }
    let Some(nav) = world.resource::<Nav>() else {
        return lines;
    };
    let Some(grid) = nav.grid() else {
        return lines;
    };
    let cell = grid.params().cell_size;
    if parts.flow {
        let (cos, sin) = (0.866_025_4_f32, 0.5_f32);
        for target in flow_targets(world, grid) {
            let Some(field) = nav.cache().get(target) else {
                continue;
            };
            for &node in field.reached() {
                let p = lifted(grid.node_pos(NodeId(node)));
                let flow = field.flow(NodeId(node));
                if flow == [0.0; 2] || !within(eye, p) {
                    continue;
                }
                let len = 0.4 * cell;
                let tip = [p[0] + flow[0] * len, p[1], p[2] + flow[1] * len];
                lines.push(seg(p, tip, FLOW_COLOR));
                for sign in [1.0_f32, -1.0] {
                    let (bx, bz) = (-flow[0], -flow[1]);
                    let (s, c) = (sin * sign, cos);
                    let head = [
                        tip[0] + 0.3 * len * (bx * c - bz * s),
                        p[1],
                        tip[2] + 0.3 * len * (bx * s + bz * c),
                    ];
                    lines.push(seg(tip, head, FLOW_COLOR));
                }
            }
        }
    }
    if parts.grid {
        for entry in &nav.tables[0].entries {
            let (a, b) = (grid.node_pos(entry.from), grid.node_pos(entry.to));
            if (within(eye, a) || within(eye, b)) && lines.len() < NAV_MAX_SEGMENTS {
                lines.push(seg(lifted(a), lifted(b), LINK_OFFMESH_COLOR));
            }
        }
        let half = 0.2 * cell;
        'nodes: for n in 0..grid.node_count() {
            #[allow(clippy::cast_possible_truncation)]
            let id = NodeId(n as u32);
            let p = grid.node_pos(id);
            if !within(eye, p) {
                continue;
            }
            let q = lifted(p);
            let cross = [
                seg(
                    [q[0] - half, q[1], q[2]],
                    [q[0] + half, q[1], q[2]],
                    NODE_COLOR,
                ),
                seg(
                    [q[0], q[1], q[2] - half],
                    [q[0], q[1], q[2] + half],
                    NODE_COLOR,
                ),
            ];
            for line in cross {
                if lines.len() >= NAV_MAX_SEGMENTS {
                    break 'nodes;
                }
                lines.push(line);
            }
            for dir in 0..8 {
                let Some(other) = grid.link(id, dir).filter(|other| *other > id) else {
                    continue;
                };
                if lines.len() >= NAV_MAX_SEGMENTS {
                    break 'nodes;
                }
                let o = grid.node_pos(other);
                let color = if (o[1] - p[1]).abs() <= 0.01 {
                    LINK_FLAT_COLOR
                } else {
                    LINK_STEP_COLOR
                };
                lines.push(seg(q, lifted(o), color));
            }
        }
    }
    lines.truncate(NAV_MAX_SEGMENTS);
    lines
}

/// The `nav` parts with something to draw, for `drawn`: `nav:grid` when the grid has a node,
/// `nav:routes` when a `NavRoute` has a waypoint, `nav:flow` when a flowing walker's field is cached;
/// `nav` itself when any of them is.
pub(crate) fn debug_present(world: &World) -> Vec<&'static str> {
    use crate::debug_draw::{NAV, NAV_FLOW, NAV_GRID, NAV_ROUTES};
    let mut present = Vec::new();
    let nav = world.resource::<Nav>();
    let grid = nav.and_then(Nav::grid);
    if grid.is_some_and(|g| g.node_count() > 0) {
        present.push(NAV_GRID);
    }
    if route_entities(world).into_iter().any(|id| {
        world
            .get::<NavRoute>(id)
            .ok()
            .flatten()
            .is_some_and(|route| !route.waypoints.is_empty())
    }) {
        present.push(NAV_ROUTES);
    }
    if let (Some(nav), Some(grid)) = (nav, grid)
        && flow_targets(world, grid)
            .into_iter()
            .any(|target| nav.cache().get(target).is_some())
    {
        present.push(NAV_FLOW);
    }
    if !present.is_empty() {
        present.push(NAV);
    }
    present
}

//! Path followers: an entity that rides a `kind: path` document by writing its own `Transform`
//! (`Plan/character-controller.md` decision 10).
//!
//! [`PathFollower`] names a path and carries the arc-length `phase` it is at; [`path_follow`] advances
//! `phase` on **gameplay** time and poses the entity at `point_at(s)` every tick. It is a sim system
//! the engine installs itself, **before** every game system, so a rider's system reads the platform's
//! pose of *this* tick. It installs only into a world that holds a follower when its schedule is
//! built (scene load, restore, district switch, and the bare `render`/`sweep` hosts), so a game
//! that never uses one pays nothing and its `engine.describe`, profile and hashes are unchanged.
//!
//! A follower spawned at runtime into a world that had none at load has no system to move it:
//! [`report_uninstalled`] reports `L1216` once per entity rather than leaving it silently still.
//!
//! [`PathLibrary`] loads each named path once, lazily, and is engine configuration derived from
//! committed content: never hashed, never snapshotted, rebuilt at every load, restore and switch.
//! **`asset.reload` does not invalidate it**, the [`AnimationLibrary`](crate::animation_events)
//! posture; a path that fails to load raises `L1215` once per entity and the entity stays put.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::number::engine_number;
use ludion_ecs::{Parent, Schedule, SemanticValidate, SystemCtx, World};
use ludion_scene::PathSampler;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::components::Transform;
use crate::debug_draw::debug_draw;
use crate::hierarchy::world_affine;

/// The schedule name of the engine-installed system.
pub(crate) const SYSTEM_NAME: &str = "path_follow";

/// The debug-draw category [`path_follow`] draws each followed path and its follower under.
const DEBUG_CATEGORY: &str = "path";
/// The debug polyline and follower-cross colour (linear RGBA).
const DEBUG_COLOR: [f32; 4] = [1.0, 0.8, 0.1, 1.0];
/// Half-length of the follower's debug cross, in metres.
const DEBUG_CROSS: f32 = 0.25;
/// Equal-arc-length spans in a path's debug polyline.
const DEBUG_SPANS: usize = 64;

/// A path loaded once: its sampler and the debug polyline sampled from it.
struct Loaded {
    sampler: PathSampler,
    polyline: Arc<[[f32; 3]]>,
}

impl Loaded {
    fn new(sampler: PathSampler) -> Self {
        let length = sampler.length();
        let polyline = (0..=DEBUG_SPANS)
            .map(|i| {
                #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
                let at = sampler.point_at(length * i as f64 / DEBUG_SPANS as f64);
                #[allow(clippy::cast_possible_truncation)]
                [at[0] as f32, at[1] as f32, at[2] as f32]
            })
            .collect();
        Loaded { sampler, polyline }
    }
}

/// What a [`PathFollower`] does at the end of its path.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PathMode {
    /// Wraps: past the end it is back at the start (an open path jumps, a closed one is seamless).
    #[default]
    Loop,
    /// Runs to the end and back, forever.
    PingPong,
    /// Runs to the end and stays there (or at the start, going backwards).
    Once,
}

/// Moves an entity along a `kind: path` document: [`path_follow`] writes its `Transform.translation`
/// every tick (rotation and scale are untouched), advancing `phase` by `speed` on gameplay time.
///
/// The pose is in the parent's frame — the path's points are scene coordinates for a root and local
/// to the `Parent` otherwise. A game drives the position
/// itself by writing `phase` with `speed: 0`; the write poses on the next tick.
///
/// # Example
/// ```
/// use ludion_engine::{PathFollower, PathMode};
///
/// let lift: PathFollower = serde_json::from_value(serde_json::json!({
///     "path": "paths/lift.path.yaml",
///     "speed": 2.0,
///     "mode": "ping_pong",
/// }))
/// .unwrap();
/// assert_eq!(lift.mode, PathMode::PingPong);
/// assert_eq!(lift.phase, 0.0);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct PathFollower {
    /// The `kind: path` document, relative to the content root (default `""` — inert).
    #[serde(default)]
    pub path: String,
    /// Arc-length metres per gameplay second; negative runs backwards (default `1.0`).
    #[serde(default = "one")]
    pub speed: f32,
    /// What happens at the end of the path (default `loop`).
    #[serde(default)]
    pub mode: PathMode,
    /// Metres along the path (default `0.0`): the start offset as authored, rewritten every tick.
    #[serde(default)]
    pub phase: f32,
}

fn one() -> f32 {
    1.0
}

impl SemanticValidate for PathFollower {
    /// Rejects a `speed` or `phase` that is not a finite `f32` (`L1269`, pointers `/speed` and
    /// `/phase`): a non-finite number, or one past `f32` range, which the field would store as
    /// infinity. A wrong *shape* is the schema's job and adds nothing here.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        [
            ("speed", "metres per second"),
            ("phase", "metres along the path"),
        ]
        .into_iter()
        .filter_map(|(field, unit)| {
            let number = value.get(field)?.as_f64().map(engine_number)?;
            (!number.is_finite()).then(|| {
                Diagnostic::new(
                    code::PATH_FOLLOWER_INVALID,
                    format!("`PathFollower` `{field}` {number} is not a finite number"),
                )
                .with_path(format!("/{field}"))
                .with_help(format!("give `{field}` a finite number of {unit}"))
            })
        })
        .collect()
    }
}

impl Default for PathFollower {
    fn default() -> Self {
        PathFollower {
            path: String::new(),
            speed: 1.0,
            mode: PathMode::Loop,
            phase: 0.0,
        }
    }
}

/// The samplers [`path_follow`] reads, loaded lazily from `root`. Engine configuration installed at
/// every world rebuild, never hashed; see the module docs.
pub(crate) struct PathLibrary {
    root: PathBuf,
    /// Keyed by `PathFollower.path`; `Err` holds the loader's diagnostics for a path that failed
    /// to load.
    paths: BTreeMap<String, Result<Loaded, Vec<Diagnostic>>>,
}

impl PathLibrary {
    /// An empty library resolving paths against `root`.
    pub(crate) fn new(root: PathBuf) -> Self {
        PathLibrary {
            root,
            paths: BTreeMap::new(),
        }
    }

    /// The path `rel`, loading it on first use; `Err` with the loader's diagnostics.
    fn loaded(&mut self, rel: &str) -> Result<&Loaded, &[Diagnostic]> {
        if !self.paths.contains_key(rel) {
            let loaded = ludion_assets::load_bytes(&self.root, rel)
                .map_err(|d| vec![d])
                .and_then(|bytes| {
                    let text = std::str::from_utf8(&bytes.bytes).map_err(|_| {
                        vec![Diagnostic::new(
                            code::PATH_DOCUMENT_STRUCTURE,
                            format!("path asset `{rel}` is not valid UTF-8 text"),
                        )]
                    })?;
                    ludion_scene::check_path(text).map(|doc| Loaded::new(PathSampler::new(&doc)))
                });
            self.paths.insert(rel.to_string(), loaded);
        }
        match &self.paths[rel] {
            Ok(loaded) => Ok(loaded),
            Err(causes) => Err(causes),
        }
    }
}

/// Whether `world` holds at least one [`PathFollower`] — the key the engine installs
/// [`path_follow`] on.
pub(crate) fn present(world: &World) -> bool {
    world.entities().any(|id| world.has::<PathFollower>(id))
}

/// Adds [`path_follow`] to `schedule` when the world it is built for holds a follower. Called before
/// any game system is added.
pub(crate) fn install(schedule: &mut Schedule, present: bool) {
    if present {
        schedule.add_system(SYSTEM_NAME, path_follow);
    }
}

/// The system's `engine.describe` entry: first in the schedule, no call site to attribute.
pub(crate) fn describe_entry() -> Value {
    serde_json::json!({
        "name": SYSTEM_NAME,
        "order": 0,
        "fn_path": "ludion_engine::path_follow",
        "registered_at": Value::Null,
        "queries": [{
            "components": [
                {"name": "PathFollower", "access": "write"},
                {"name": "Transform", "access": "write"},
            ],
        }],
    })
}

/// Reports `L1216` once per [`PathFollower`] in a world whose schedule lacks [`path_follow`].
pub(crate) fn report_uninstalled(world: &mut World, installed: bool) {
    if installed {
        return;
    }
    let ids: Vec<EntityId> = world
        .query_ids::<(&PathFollower,)>()
        .map(|(id, _)| id)
        .collect();
    for id in ids {
        crate::diagnostics::report(
            world,
            code::PATH_FOLLOWER_NOT_INSTALLED,
            id.to_string(),
            format!(
                "{id} has a `PathFollower`, but this world's schedule has no `path_follow`, so it \
                 never moves: the engine installs it only when a scene, snapshot or district is \
                 built with a follower already in it (author the follower in the scene file, or \
                 put an inert `PathFollower: {{}}` on any entity)"
            ),
        );
    }
}

/// Advances each [`PathFollower`] on gameplay time and poses its entity along its path, in
/// ascending entity order (the module docs have the rest).
///
/// Every tick writes `Transform.translation`, including a held one; `phase` moves only when the
/// gameplay clock advanced. A path that fails to load raises `L1215` once per entity and the entity
/// stays put; so does a follower built in Rust with a non-finite `speed` or `phase` (`L1269`),
/// which no validator saw — it keeps its `phase` and `Transform` untouched.
pub fn path_follow(world: &mut World, ctx: &SystemCtx) {
    let followers: Vec<(EntityId, PathFollower)> = world
        .query_ids::<(&PathFollower,)>()
        .filter(|(_, (follower,))| !follower.path.is_empty())
        .map(|(id, (follower,))| (id, follower.clone()))
        .collect();
    if followers.is_empty() {
        return;
    }
    let advance = ctx.game_advanced().then_some(f64::from(ctx.game_dt));
    let Some(library) = world.resource_mut::<PathLibrary>() else {
        return;
    };
    let mut poses = Vec::with_capacity(followers.len());
    let mut failures = Vec::new();
    let mut non_finite = Vec::new();
    for (id, follower) in followers {
        if let Some(field) = non_finite_field(&follower) {
            non_finite.push((id, field));
            continue;
        }
        match library.loaded(&follower.path) {
            Ok(loaded) => {
                let (phase, position) = pose(&loaded.sampler, &follower, advance);
                poses.push((id, phase, position, Arc::clone(&loaded.polyline)));
            }
            Err(causes) => failures.push((id, follower.path, causes.to_vec())),
        }
    }
    for (id, phase, position, polyline) in poses {
        if let Ok(Some(follower)) = world.get_mut::<PathFollower>(id) {
            follower.phase = phase;
        }
        if let Ok(Some(transform)) = world.get_mut::<Transform>(id) {
            transform.translation = position;
        }
        // A path lives in its parent's frame, so a parented follower's debug geometry is drawn
        // through the parent's world affine.
        let frame = world
            .has::<Parent>(id)
            .then(|| world.get::<Parent>(id).ok().flatten().copied())
            .flatten()
            .and_then(|Parent(parent)| world_affine(world, parent));
        let (polyline, position) = match frame {
            Some(frame) => (
                Arc::from(
                    polyline
                        .iter()
                        .map(|point| frame.transform_point(*point))
                        .collect::<Vec<_>>(),
                ),
                frame.transform_point(position),
            ),
            None => (polyline, position),
        };
        debug_draw(world, DEBUG_CATEGORY)
            .path(&polyline, DEBUG_COLOR)
            .cross(position, DEBUG_CROSS, DEBUG_COLOR);
    }
    for (id, field) in non_finite {
        let diagnostic = Diagnostic::new(
            code::PATH_FOLLOWER_INVALID,
            format!("{id} has a non-finite `{field}`, so it stays where it is"),
        )
        .with_path(format!("/{field}"))
        .with_help(format!("give `{field}` a finite number"));
        crate::diagnostics::report_full(world, format!("{id}/PathFollower/{field}"), diagnostic);
    }
    for (id, path, causes) in failures {
        let diagnostic = Diagnostic::new(
            code::PATH_FOLLOWER_UNLOADED,
            format!("{id} follows `{path}`, which failed to load, so it stays where it is"),
        );
        let diagnostic = causes
            .into_iter()
            .fold(diagnostic, Diagnostic::with_related);
        crate::diagnostics::report_full(world, format!("{id}/PathFollower/path"), diagnostic);
    }
}

/// The first of a follower's `speed` and `phase` that is not finite, by name.
fn non_finite_field(follower: &PathFollower) -> Option<&'static str> {
    if !follower.speed.is_finite() {
        Some("speed")
    } else if !follower.phase.is_finite() {
        Some("phase")
    } else {
        None
    }
}

/// The follower's normalised `phase` and world position after moving by `advance` seconds of
/// gameplay time (`None` when the gameplay clock held).
///
/// The pose is read from the `f32` phase that gets stored, so what a reader recomputes from the
/// component equals what was written.
fn pose(sampler: &PathSampler, follower: &PathFollower, advance: Option<f64>) -> (f32, [f32; 3]) {
    let length = sampler.length();
    let mut phase = f64::from(follower.phase);
    if let Some(dt) = advance {
        phase += f64::from(follower.speed) * dt;
    }
    let normalised = match follower.mode {
        PathMode::Loop => phase.rem_euclid(length),
        PathMode::PingPong => phase.rem_euclid(2.0 * length),
        PathMode::Once => phase.clamp(0.0, length),
    };
    #[allow(clippy::cast_possible_truncation)]
    let stored = normalised as f32;
    let phase = f64::from(stored);
    let s = match follower.mode {
        PathMode::PingPong if phase > length => 2.0 * length - phase,
        _ => phase,
    };
    let at = sampler.point_at(s);
    #[allow(clippy::cast_possible_truncation)]
    let position = [at[0] as f32, at[1] as f32, at[2] as f32];
    (stored, position)
}

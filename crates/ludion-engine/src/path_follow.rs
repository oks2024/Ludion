//! Path followers: an entity that rides a `kind: path` document by writing its own `Transform`
//! (`Plan/character-controller.md` decision 10).
//!
//! [`PathFollower`] names a path and carries the arc-length `phase` it is at; [`path_follow`] advances
//! `phase` on **gameplay** time and poses the entity at `point_at(s)` every tick. It is a sim system
//! the engine installs itself, **before** every game system, so a rider's system reads the platform's
//! pose of *this* tick. It is installed on every schedule (scene load, restore, district switch, and
//! the bare `render`/`sweep` hosts) and costs one empty query in a world without a follower.
//!
//! A follower with an [`orient`](PathFollower::orient) also writes `Transform.rotation`, facing travel.
//!
//! A tick that wraps an open `loop` path, or moves a follower further than its speed allows, is a
//! **teleport**: a `kinematic` body is snapped to the new pose with zero velocity
//! ([`teleport_kinematic`](ludion_physics::teleport_kinematic)) instead of driving its riders at the
//! implied speed. [`path_follower_motion`] reads what the tick did; see [`FollowerMotion`].
//!
//! [`PathLibrary`] loads each named path once, lazily, and is engine configuration derived from
//! committed content: never hashed, never snapshotted, rebuilt at every load, restore and switch.
//! **`asset.reload` does not invalidate it**, the [`AnimationLibrary`](crate::animation_events)
//! posture; a path that fails to load raises `L1215` once per entity and the entity stays put.

use ludion_assets::AssetRoot;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::lane::CentreLine;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::math::{euler_xyz_degrees_to_quat, quat_mul};
use ludion_ecs::{Parent, Schedule, SystemCtx, World};
use ludion_scene::{PathFrame, PathSampler};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use ludion_components::path_follow::{PathFollower, PathMode, PathOrient};

use crate::components::Transform;
use crate::debug_draw::debug_draw;
use crate::hierarchy::{world_affine, world_transform};

/// The schedule name of the engine-installed system.
pub(crate) const SYSTEM_NAME: &str = "path_follow";

/// The debug-draw category [`path_follow`] draws each followed path and its follower under.
const DEBUG_CATEGORY: &str = "path";
/// The debug polyline and follower-cross colour (linear RGBA).
const DEBUG_COLOR: [f32; 4] = [1.0, 0.8, 0.1, 1.0];
/// Half-length of the follower's debug cross, in metres.
const DEBUG_CROSS: f32 = 0.25;
/// Length of each axis of an oriented follower's debug frame, in metres.
const DEBUG_AXES: f32 = 0.5;
/// Horizontal travel below this (per unit tangent) is vertical: `yaw` keeps its rotation.
const ORIENT_VERTICAL_EPS: f64 = 1e-6;
/// Equal-arc-length spans in a path's debug polyline.
const DEBUG_SPANS: usize = 64;

/// Metres a follower may move beyond `|speed|·dt` in one tick before the tick counts as a teleport.
/// The chord of an arc is never longer than the arc, so travel, ping-pong turns and closed wraps stay
/// under `|speed|·dt`; the slack absorbs a game that drives `phase` itself at `speed: 0` (up to
/// 30 m/s at 60 Hz), which must keep carrying its riders. A smaller deliberate jump is the game's to
/// flag with `teleport_kinematic`.
const TELEPORT_SLACK: f32 = 0.5;

/// A path loaded once: its sampler and the debug polyline sampled from it.
struct Loaded {
    sampler: Arc<PathSampler>,
    polyline: Arc<[[f32; 3]]>,
}

impl Loaded {
    fn new(sampler: PathSampler) -> Self {
        let sampler = Arc::new(sampler);
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

/// What a follower's last [`path_follow`] tick did, as [`path_follower_motion`] returns it. Derived
/// by the engine every tick, before any game system runs; never hashed or snapshotted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FollowerMotion {
    /// Metres along the path the follower is posed at (a ping-pong's return leg is folded back).
    pub s: f32,
    /// `-1.0` when the follower runs towards the start of the path, else `1.0`; flips on a
    /// ping-pong's return leg. At `speed` 0 it follows the sign of this tick's change in `s` (a
    /// game-driven `phase`), keeping the last direction when `s` held and on a teleport (`1.0` on
    /// a follower's first tick).
    pub direction: f32,
    /// The translation written this tick minus the one it replaced, in the parent's frame.
    pub delta: [f32; 3],
    /// `delta` per gameplay second: zero on a held tick and on a teleport.
    pub velocity: [f32; 3],
    /// Whether the tick wrapped an open `loop` path or jumped beyond what `speed` allows.
    pub teleported: bool,
}

/// The per-follower [`FollowerMotion`]s of the last [`path_follow`] run, replaced every run. A
/// resource, never hashed; the snapshot carries it ([`entries`](FollowerMotions::entries)) because a
/// `speed: 0` follower reads last tick's `s` to pick its facing.
#[derive(Default)]
pub(crate) struct FollowerMotions(BTreeMap<EntityId, FollowerMotion>);

/// One follower's carried [`FollowerMotion`], as a snapshot holds it.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub(crate) struct FollowerMotionEntry {
    entity: EntityId,
    s: f32,
    direction: f32,
    delta: [f32; 3],
    velocity: [f32; 3],
    teleported: bool,
}

impl FollowerMotionEntry {
    /// Whether every float is finite — JSON has no other kind of number.
    fn round_trips(&self) -> bool {
        self.s.is_finite()
            && self.direction.is_finite()
            && self.delta.iter().all(|v| v.is_finite())
            && self.velocity.iter().all(|v| v.is_finite())
    }
}

impl FollowerMotions {
    /// The carried motions in entity order; an entry with a non-finite float is left out.
    pub(crate) fn entries(&self) -> Vec<FollowerMotionEntry> {
        self.0
            .iter()
            .map(|(&entity, m)| FollowerMotionEntry {
                entity,
                s: m.s,
                direction: m.direction,
                delta: m.delta,
                velocity: m.velocity,
                teleported: m.teleported,
            })
            .filter(FollowerMotionEntry::round_trips)
            .collect()
    }

    /// The motions `entries` carries.
    pub(crate) fn from_entries(entries: Vec<FollowerMotionEntry>) -> Self {
        Self(
            entries
                .into_iter()
                .map(|e| {
                    (
                        e.entity,
                        FollowerMotion {
                            s: e.s,
                            direction: e.direction,
                            delta: e.delta,
                            velocity: e.velocity,
                            teleported: e.teleported,
                        },
                    )
                })
                .collect(),
        )
    }
}

/// What the last [`path_follow`] tick did to `entity`, or `None` when it posed nothing: not a
/// follower, an empty `path`, a path that failed to load, a non-finite `speed` or `phase`, no
/// `Transform`, or no tick has run since the world was built.
///
/// Recomputed every tick before any game system, so a system reads this tick's motion.
///
/// # Example
/// ```
/// use ludion_ecs::World;
/// use ludion_engine::path_follower_motion;
///
/// let mut world = World::new();
/// let entity = world.spawn();
/// assert_eq!(path_follower_motion(&world, entity), None);
/// ```
pub fn path_follower_motion(world: &World, entity: EntityId) -> Option<FollowerMotion> {
    world.resource::<FollowerMotions>()?.0.get(&entity).copied()
}

/// The samplers [`path_follow`] reads, loaded lazily from `root`. Engine configuration installed at
/// every world rebuild, never hashed; see the module docs.
pub(crate) struct PathLibrary {
    root: PathBuf,
    /// Keyed by `PathFollower.path`; `Err` holds the loader's diagnostics for a path that failed
    /// to load.
    paths: BTreeMap<String, Result<Loaded, Vec<Diagnostic>>>,
    /// The near-vertical spans of each path a lane asked about; see `lane_spans`.
    spans: BTreeMap<String, Vec<(f32, f32)>>,
    /// The lane centre line of each path a lane drew; see `lane_centre`.
    centres: BTreeMap<String, CentreLine>,
}

impl PathLibrary {
    /// An empty library resolving paths against `root`.
    pub(crate) fn new(root: PathBuf) -> Self {
        PathLibrary {
            root,
            paths: BTreeMap::new(),
            spans: BTreeMap::new(),
            centres: BTreeMap::new(),
        }
    }

    /// The content root this library resolves paths against.
    pub(crate) fn root(&self) -> &std::path::Path {
        &self.root
    }

    /// The path `rel`, loading it on first use; `Err` with the loader's diagnostics.
    fn loaded(&mut self, rel: &str) -> Result<&Loaded, &[Diagnostic]> {
        if !self.paths.contains_key(rel) {
            let loaded = load(AssetRoot::content(&self.root), rel).map(Loaded::new);
            self.paths.insert(rel.to_string(), loaded);
        }
        match &self.paths[rel] {
            Ok(loaded) => Ok(loaded),
            Err(causes) => Err(causes),
        }
    }

    /// The shared sampler of the path `rel`, loading it on first use; `Err` with the loader's
    /// diagnostics. The `Arc` outlives the library borrow, so a caller can go on to use the world.
    pub(crate) fn sampler(&mut self, rel: &str) -> Result<Arc<PathSampler>, Vec<Diagnostic>> {
        self.loaded(rel)
            .map(|loaded| Arc::clone(&loaded.sampler))
            .map_err(<[Diagnostic]>::to_vec)
    }

    /// The near-vertical spans of the loaded path `rel`, scanned the first time a lane asks (a
    /// follower never pays for the scan); empty for a path that is not loaded.
    pub(crate) fn lane_spans(&mut self, rel: &str) -> &[(f32, f32)] {
        if !self.spans.contains_key(rel) {
            let spans = self
                .loaded_sampler(rel)
                .map(|sampler| crate::lane::vertical_spans(&sampler))
                .unwrap_or_default();
            self.spans.insert(rel.to_string(), spans);
        }
        &self.spans[rel]
    }

    /// The `(position, right)` samples of the loaded path `rel`'s lane centre line, sampled the
    /// first time a lane draws; empty for a path that is not loaded.
    pub(crate) fn lane_centre(&mut self, rel: &str) -> CentreLine {
        if !self.centres.contains_key(rel) {
            let centre = self
                .loaded_sampler(rel)
                .map(|sampler| crate::lane::centre_line(&sampler))
                .unwrap_or_default();
            self.centres.insert(rel.to_string(), centre.into());
        }
        Arc::clone(&self.centres[rel])
    }

    /// The sampler of `rel` when it is already loaded; `None` for a path not loaded or failed.
    fn loaded_sampler(&self, rel: &str) -> Option<Arc<PathSampler>> {
        self.paths
            .get(rel)?
            .as_ref()
            .ok()
            .map(|loaded| Arc::clone(&loaded.sampler))
    }

    /// The debug polyline of the loaded path `rel`; `None` for a path not loaded or failed.
    pub(crate) fn polyline(&self, rel: &str) -> Option<Arc<[[f32; 3]]>> {
        self.paths
            .get(rel)?
            .as_ref()
            .ok()
            .map(|loaded| Arc::clone(&loaded.polyline))
    }

    /// The sampler of `rel` for a reader that holds only `&World`: the loaded one, or on a miss one
    /// loaded now and not cached. `None` for a path that failed to load; the diagnostics are left to
    /// the first `&mut` caller.
    pub(crate) fn peek(&self, rel: &str) -> Option<Arc<PathSampler>> {
        match self.paths.get(rel) {
            Some(loaded) => loaded.as_ref().ok().map(|l| Arc::clone(&l.sampler)),
            None => load(AssetRoot::content(&self.root), rel).ok().map(Arc::new),
        }
    }
}

/// Reads and checks the path asset `rel` under `root`.
fn load(root: ludion_assets::AssetRoot<'_>, rel: &str) -> Result<PathSampler, Vec<Diagnostic>> {
    let bytes = ludion_assets::load_bytes(root, rel).map_err(|d| vec![d])?;
    let text = std::str::from_utf8(&bytes.bytes).map_err(|_| {
        vec![Diagnostic::new(
            code::PATH_DOCUMENT_STRUCTURE,
            format!("path asset `{rel}` is not valid UTF-8 text"),
        )]
    })?;
    ludion_scene::check_path(text).map(|doc| PathSampler::new(&doc))
}

/// Adds [`path_follow`] to `schedule`. Called before any game system is added.
pub(crate) fn install(schedule: &mut Schedule) {
    schedule.add_system(SYSTEM_NAME, path_follow);
}

/// The system's `engine.describe` entry, with no call site to attribute; the caller sets its `order`.
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
        world.insert_resource(FollowerMotions::default());
        return;
    }
    let advance = ctx.game_advanced().then_some(f64::from(ctx.game_dt));
    let previous = world
        .resource::<FollowerMotions>()
        .map(|m| m.0.clone())
        .unwrap_or_default();
    let mut motions = FollowerMotions::default();
    let Some(library) = world.resource_mut::<PathLibrary>() else {
        world.insert_resource(FollowerMotions::default());
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
                let posed = pose(&loaded.sampler, &follower, advance);
                // Both facings, because a held follower's direction is only known once the
                // tick's teleport verdict is.
                let s = f64::from(posed.s);
                let rotations = [
                    orient(&loaded.sampler, follower.orient, s, 1.0),
                    orient(&loaded.sampler, follower.orient, s, -1.0),
                ];
                poses.push((
                    id,
                    follower.speed,
                    follower.orient,
                    rotations,
                    posed,
                    Arc::clone(&loaded.polyline),
                ));
            }
            Err(causes) => failures.push((id, follower.path, causes.to_vec())),
        }
    }
    for (id, speed, orientation, rotations, posed, polyline) in poses {
        let Pose {
            phase,
            position,
            s,
            wrapped,
            mut direction,
        } = posed;
        if let Ok(Some(follower)) = world.get_mut::<PathFollower>(id) {
            follower.phase = phase;
        }
        let mut teleported = false;
        if let Ok(Some(transform)) = world.get_mut::<Transform>(id) {
            let old = transform.translation;
            transform.translation = position;
            let delta = [
                position[0] - old[0],
                position[1] - old[1],
                position[2] - old[2],
            ];
            #[allow(clippy::cast_possible_truncation)]
            let allowed = speed.abs() * advance.map_or(0.0, |dt| dt as f32) + TELEPORT_SLACK;
            teleported = wrapped || norm(delta) > allowed;
            if speed == 0.0 {
                // A game-driven phase: travel is whichever way `s` moved, else as last tick.
                let last = previous.get(&id);
                let kept = last.map_or(1.0, |m| m.direction);
                direction = match last {
                    Some(m) if !teleported && s > m.s => 1.0,
                    Some(m) if !teleported && s < m.s => -1.0,
                    _ => kept,
                };
            }
            if let Some(euler) = rotations[usize::from(direction < 0.0)] {
                transform.rotation = euler;
            }
            #[allow(clippy::cast_possible_truncation)]
            let velocity = match advance {
                Some(dt) if !teleported => delta.map(|d| (f64::from(d) / dt) as f32),
                _ => [0.0; 3],
            };
            motions.0.insert(
                id,
                FollowerMotion {
                    s,
                    direction,
                    delta,
                    velocity,
                    teleported,
                },
            );
        }
        if teleported {
            ludion_physics::teleport_kinematic(world, id);
        }
        // A path lives in its parent's frame, so a parented follower's debug geometry is drawn
        // through the parent's world affine.
        let parent = world
            .has::<Parent>(id)
            .then(|| world.get::<Parent>(id).ok().flatten().copied())
            .flatten();
        let frame = parent.and_then(|Parent(parent)| world_affine(world, parent));
        let parent_rotation = parent
            .and_then(|Parent(parent)| world_transform(world, parent))
            .map(|pose| pose.trs.rotation);
        let current = world
            .get::<Transform>(id)
            .ok()
            .flatten()
            .map(|t| t.rotation);
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
        let mut pen = debug_draw(world, DEBUG_CATEGORY);
        pen.path(&polyline, DEBUG_COLOR)
            .cross(position, DEBUG_CROSS, DEBUG_COLOR);
        if let (false, Some(euler)) = (orientation.is_none(), current) {
            let local = euler_xyz_degrees_to_quat(euler);
            pen.axes(
                position,
                parent_rotation.map_or(local, |parent| quat_mul(parent, local)),
                DEBUG_AXES,
            );
        }
    }
    world.insert_resource(motions);
    for (id, field) in non_finite {
        let diagnostic = Diagnostic::new(
            code::PATH_FOLLOWER_INVALID,
            format!("{id} has a non-finite `{field}`, so it stays where it is"),
        )
        .with_path(format!("/{field}"))
        .with_help(format!("give `{field}` a finite number"));
        let key = format!(
            "{}/PathFollower/{field}",
            crate::diagnostics::offender_key(world, id)
        );
        crate::diagnostics::report_full(world, key, diagnostic);
    }
    for (id, path, causes) in failures {
        let diagnostic = Diagnostic::new(
            code::PATH_FOLLOWER_UNLOADED,
            format!("{id} follows `{path}`, which failed to load, so it stays where it is"),
        );
        let diagnostic = causes
            .into_iter()
            .fold(diagnostic, Diagnostic::with_related);
        let key = format!(
            "{}/PathFollower/path",
            crate::diagnostics::offender_key(world, id)
        );
        crate::diagnostics::report_full(world, key, diagnostic);
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

/// A follower posed for one tick.
struct Pose {
    /// The normalised phase that gets stored.
    phase: f32,
    position: [f32; 3],
    /// Metres along the path, a ping-pong's return leg folded back.
    s: f32,
    /// An open `loop` path was wrapped (or the phase was outside `[0, length)`).
    wrapped: bool,
    direction: f32,
}

/// The follower's normalised `phase` and position after moving by `advance` seconds of gameplay
/// time (`None` when the gameplay clock held).
///
/// The pose is read from the `f32` phase that gets stored, so what a reader recomputes from the
/// component equals what was written.
fn pose(sampler: &PathSampler, follower: &PathFollower, advance: Option<f64>) -> Pose {
    let length = sampler.length();
    let mut phase = f64::from(follower.phase);
    if let Some(dt) = advance {
        phase += f64::from(follower.speed) * dt;
    }
    let wrapped =
        follower.mode == PathMode::Loop && !sampler.closed() && phase.rem_euclid(length) != phase;
    let normalised = match follower.mode {
        PathMode::Loop => phase.rem_euclid(length),
        PathMode::PingPong => phase.rem_euclid(2.0 * length),
        PathMode::Once => phase.clamp(0.0, length),
    };
    #[allow(clippy::cast_possible_truncation)]
    let stored = normalised as f32;
    let phase = f64::from(stored);
    let returning = follower.mode == PathMode::PingPong && phase > length;
    let s = if returning {
        2.0 * length - phase
    } else {
        phase
    };
    let at = sampler.point_at(s);
    #[allow(clippy::cast_possible_truncation)]
    let position = [at[0] as f32, at[1] as f32, at[2] as f32];
    let backwards = (follower.speed < 0.0) != returning;
    Pose {
        phase: stored,
        position,
        #[allow(clippy::cast_possible_truncation)]
        s: s as f32,
        wrapped,
        direction: if backwards { -1.0 } else { 1.0 },
    }
}

/// The Euler XYZ degrees to write for a follower posed at `s` (`None`: leave the rotation alone).
fn orient(sampler: &PathSampler, orient: PathOrient, s: f64, direction: f32) -> Option<[f32; 3]> {
    let facing = f64::from(direction);
    let quat = match orient {
        PathOrient::None => return None,
        PathOrient::Yaw => {
            let t = sampler.tangent_at(s);
            let (dx, dz) = (facing * t[0], facing * t[2]);
            if (dx * dx + dz * dz).sqrt() < ORIENT_VERTICAL_EPS {
                return None;
            }
            #[allow(clippy::cast_possible_truncation)]
            return Some([
                0.0,
                ludion_core::math::f64::atan2(-dx, -dz).to_degrees() as f32,
                0.0,
            ]);
        }
        PathOrient::Frame => {
            let frame = sampler.frame_at(s);
            if direction < 0.0 {
                PathFrame {
                    tangent: frame.tangent.map(|c| -c),
                    right: frame.right.map(|c| -c),
                    ..frame
                }
                .rotation()
            } else {
                frame.rotation()
            }
        }
    };
    let euler = ludion_core::math::f64::quat_to_euler_xyz_degrees(quat);
    #[allow(clippy::cast_possible_truncation)]
    Some(euler.map(|e| e as f32))
}

fn norm(v: [f32; 3]) -> f32 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

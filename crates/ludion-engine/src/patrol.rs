//! Patrols: a guard that walks a `kind: patrol` document as nav legs (`Plan/nav.md` decision 13).
//!
//! [`patrol_update`] writes the guard's `NavGoal` for each stop in turn, so every leg is a nav path
//! around obstacles, and counts each stop's wait and look sequence on **gameplay** time. It never turns
//! the body: [`PatrolState::look_deg`] is the world yaw the game should face (0001's `Perceiver.look`,
//! or a turn-rate of the game's own). A game opts in by scheduling it.
//!
//! **Schedule it** `nav_update` → `patrol_update` → `nav_steer` → the game's mover. Arrival is read from
//! the agent's `NavRoute` at the start of the tick, so a stop begins one tick after the agent got
//! there — deterministic, and `nav_steer` plans the next leg the tick the stop ends.
//!
//! `PatrolState` is hashed and snapshotted, so a restored world resumes the stop it was in.
//! [`PatrolLibrary`] caches each named patrol, loaded lazily through the installed `PathLibrary`'s
//! content root: never hashed, never snapshotted, re-created from disk after a rebuild or restore, and
//! **`asset.reload` does not refresh it** (the `PathLibrary` posture).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use ludion_assets::AssetRoot;
use ludion_components::NavAgent;
use ludion_components::nav_route::{NavGoal, NavRoute, NavRouteStatus};
pub use ludion_components::patrol::{PatrolFollower, PatrolPhase, PatrolResume, PatrolState};
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_ecs::{SystemCtx, World};
use ludion_scene::{PatrolDocument, PatrolMode, PatrolPoint};

use crate::hierarchy::world_transform;
use crate::path_follow::PathLibrary;

/// Horizontal travel below this is no heading: the base yaw is `0`.
const HEADING_EPS: f64 = 1e-9;

/// What the last [`patrol_update`] tick says about a guard, for the game's behaviour and turning code.
#[derive(Clone, Debug, PartialEq)]
pub struct PatrolMotion {
    /// The stop being walked to or stood at.
    pub point: u32,
    /// Where the guard is in the stop.
    pub phase: PatrolPhase,
    /// True only on the tick the stop began.
    pub arrived: bool,
    /// The world yaw, in degrees, to face while stopped; `None` while walking.
    pub look_deg: Option<f32>,
    /// The stop's `tag`; `None` when the patrol is not loaded or the stop has none.
    pub tag: Option<String>,
}

/// The patrol documents [`patrol_update`] reads, keyed by `PatrolFollower.route`.
pub(crate) struct PatrolLibrary {
    root: PathBuf,
    /// `Err` holds the loader's diagnostics for a route that failed to load.
    docs: BTreeMap<String, Result<Arc<PatrolDocument>, Vec<Diagnostic>>>,
}

impl PatrolLibrary {
    fn new(root: PathBuf) -> Self {
        Self {
            root,
            docs: BTreeMap::new(),
        }
    }

    /// The document `rel`, loading it on first use; `Err` with the loader's diagnostics.
    fn load(&mut self, rel: &str) -> Result<Arc<PatrolDocument>, Vec<Diagnostic>> {
        if !self.docs.contains_key(rel) {
            let loaded = read(AssetRoot::content(&self.root), rel).map(Arc::new);
            self.docs.insert(rel.to_string(), loaded);
        }
        self.docs[rel].clone()
    }

    /// The document `rel` when already loaded.
    fn cached(&self, rel: &str) -> Option<&Arc<PatrolDocument>> {
        self.docs.get(rel)?.as_ref().ok()
    }
}

/// Reads and checks the patrol asset `rel` under `root`.
fn read(root: AssetRoot<'_>, rel: &str) -> Result<PatrolDocument, Vec<Diagnostic>> {
    let bytes = ludion_assets::load_bytes(root, rel).map_err(|d| vec![d])?;
    let text = std::str::from_utf8(&bytes.bytes).map_err(|_| {
        vec![Diagnostic::new(
            code::PATROL_DOCUMENT_STRUCTURE,
            format!("patrol asset `{rel}` is not valid UTF-8 text"),
        )]
    })?;
    ludion_scene::check_patrol(text)
}

/// What the last [`patrol_update`] tick says about `entity`, or `None` when it has no `PatrolState`.
///
/// # Example
/// ```
/// use ludion_ecs::World;
/// use ludion_engine::patrol_motion;
///
/// let mut world = World::new();
/// let entity = world.spawn();
/// assert_eq!(patrol_motion(&world, entity), None);
/// ```
pub fn patrol_motion(world: &World, entity: EntityId) -> Option<PatrolMotion> {
    let state = *world.get::<PatrolState>(entity).ok().flatten()?;
    let tag = world
        .get::<PatrolFollower>(entity)
        .ok()
        .flatten()
        .and_then(|follower| world.resource::<PatrolLibrary>()?.cached(&follower.route))
        .and_then(|doc| doc.points.get(state.point as usize)?.tag.clone());
    Some(PatrolMotion {
        point: state.point,
        phase: state.phase,
        arrived: state.arrived,
        look_deg: state.look_deg,
        tag,
    })
}

/// A diagnostic [`patrol_update`] reports once its borrows are over.
enum Failure {
    NoAgent(EntityId),
    Unloaded(EntityId, String, Vec<Diagnostic>),
    Unreachable(EntityId, u32),
}

/// Advances each [`PatrolFollower`] on gameplay time and writes its `NavGoal`, in ascending entity
/// order (the module docs have the rest).
///
/// A guard walks to its current stop; once its `NavRoute` reports the agent arrived at that stop, it
/// stands for `max(wait, Σ look.hold)` while `look_deg` follows the stop's look sequence, then heads
/// for the next stop by `mode`. A stop the nav grid cannot reach is skipped (`L4143`), a route that
/// fails to load leaves the guard alone (`L4142`), and a guard with no `NavAgent` is skipped
/// (`L4144`), each reported once. While `paused` the guard is left to the game; unpausing rejoins by
/// `resume`. A no-op for a world with no [`PatrolFollower`].
///
/// # Example
/// ```
/// use ludion_engine::{Engine, NavConfig, nav_steer, nav_update, patrol_update};
///
/// let engine = Engine::builder()
///     .nav(NavConfig::default())
///     .system("nav_update", nav_update)
///     .system("patrol_update", patrol_update)
///     .system("nav_steer", nav_steer)
///     .build()
///     .unwrap();
/// # let _ = engine;
/// ```
pub fn patrol_update(world: &mut World, ctx: &SystemCtx) {
    let mut followers: Vec<(EntityId, PatrolFollower)> = world
        .query_ids::<(&PatrolFollower,)>()
        .filter(|(_, (follower,))| !follower.route.is_empty())
        .map(|(id, (follower,))| (id, follower.clone()))
        .collect();
    if followers.is_empty() {
        return;
    }
    followers.sort_by_key(|(id, _)| *id);
    let mut library = match world.remove_resource::<PatrolLibrary>() {
        Some(library) => library,
        None => match world.resource::<PathLibrary>() {
            Some(paths) => PatrolLibrary::new(paths.root().to_path_buf()),
            None => return,
        },
    };
    let dt = if ctx.game_advanced() {
        ctx.game_dt
    } else {
        0.0
    };
    let mut failures = Vec::new();
    for (id, follower) in followers {
        if !matches!(world.get::<NavAgent>(id), Ok(Some(_))) {
            failures.push(Failure::NoAgent(id));
            continue;
        }
        let doc = match library.load(&follower.route) {
            Ok(doc) => doc,
            Err(causes) => {
                failures.push(Failure::Unloaded(id, follower.route.clone(), causes));
                continue;
            }
        };
        step(world, id, &follower, &doc, dt, &mut failures);
    }
    world.insert_resource(library);
    report(world, failures);
}

/// One guard's tick: reads its state, applies the phase's rule, writes the state back.
fn step(
    world: &mut World,
    id: EntityId,
    follower: &PatrolFollower,
    doc: &PatrolDocument,
    dt: f32,
    failures: &mut Vec<Failure>,
) {
    let len = doc.points.len();
    let last = len.saturating_sub(1) as u32;
    let mut state = match world.get::<PatrolState>(id) {
        Ok(Some(state)) => *state,
        _ => PatrolState {
            point: follower.start.min(last),
            ..PatrolState::default()
        },
    };
    state.point = state.point.min(last);
    state.arrived = false;

    if state.phase == PatrolPhase::Done {
        write_state(world, id, state);
        return;
    }
    if follower.paused {
        state.phase = PatrolPhase::Paused;
        state.look_deg = None;
        state.stop_left = 0.0;
        state.stop_elapsed = 0.0;
        write_state(world, id, state);
        return;
    }
    if state.phase == PatrolPhase::Paused {
        match follower.resume {
            PatrolResume::Nearest => state.point = nearest(world, id, doc).unwrap_or(state.point),
            PatrolResume::Next => advance(&mut state, doc.mode, len),
            PatrolResume::Last => {}
        }
        state.phase = PatrolPhase::Walking;
    }

    match state.phase {
        PatrolPhase::Walking => {
            write_goal(world, id, &doc.points[state.point as usize]);
            if let Some(route) = world.get::<NavRoute>(id).ok().flatten() {
                let at = to_f32(doc.points[state.point as usize].at);
                if route.goal == at {
                    // A partial route ends short of the stop, so arriving at it is not arriving.
                    match route.status {
                        NavRouteStatus::Arrived if !route.partial => {
                            begin_stop(&mut state, doc, len);
                        }
                        NavRouteStatus::Arrived | NavRouteStatus::Unreachable => {
                            failures.push(Failure::Unreachable(id, state.point));
                            advance(&mut state, doc.mode, len);
                        }
                        NavRouteStatus::Planning
                        | NavRouteStatus::Following
                        | NavRouteStatus::AtLink => {}
                    }
                }
            }
        }
        PatrolPhase::Stopped => {
            state.stop_elapsed += dt;
            state.stop_left = (state.stop_left - dt).max(0.0);
            state.look_deg = Some(look_yaw(&state, doc, len));
            if state.stop_left == 0.0 {
                advance(&mut state, doc.mode, len);
            }
        }
        PatrolPhase::Paused | PatrolPhase::Done => {}
    }
    if state.phase == PatrolPhase::Walking {
        write_goal(world, id, &doc.points[state.point as usize]);
    }
    write_state(world, id, state);
}

/// Starts the stop at `state.point`: its timers and first facing. A stop that holds for nothing still
/// lasts this tick, so `arrived` and `point` name the stop for the game.
fn begin_stop(state: &mut PatrolState, doc: &PatrolDocument, len: usize) {
    let point = &doc.points[state.point as usize];
    let looks: f64 = point.look.iter().map(|look| look.hold).sum();
    #[allow(clippy::cast_possible_truncation)]
    let total = point.wait.max(looks) as f32;
    state.arrived = true;
    state.stop_elapsed = 0.0;
    state.stop_left = total;
    state.phase = PatrolPhase::Stopped;
    state.look_deg = Some(look_yaw(state, doc, len));
}

/// Moves `state` to the next stop by `mode`; a `once` patrol at its last stop is `Done`.
fn advance(state: &mut PatrolState, mode: PatrolMode, len: usize) {
    let point = state.point as usize;
    state.phase = PatrolPhase::Walking;
    state.look_deg = None;
    state.stop_left = 0.0;
    state.stop_elapsed = 0.0;
    match mode {
        PatrolMode::Loop => state.point = ((point + 1) % len) as u32,
        PatrolMode::PingPong => {
            if len > 1 {
                if state.forward && point + 1 >= len {
                    state.forward = false;
                } else if !state.forward && point == 0 {
                    state.forward = true;
                }
                let next = if state.forward { point + 1 } else { point - 1 };
                state.point = next as u32;
            }
        }
        PatrolMode::Once => {
            if point + 1 >= len {
                state.phase = PatrolPhase::Done;
            } else {
                state.point += 1;
            }
        }
    }
}

/// The world yaw to face at `state`'s stop: the base facing plus the look the elapsed time is in.
fn look_yaw(state: &PatrolState, doc: &PatrolDocument, len: usize) -> f32 {
    let point = &doc.points[state.point as usize];
    let base = point
        .face_deg
        .unwrap_or_else(|| leg_yaw(state, doc, len).unwrap_or(0.0));
    let mut until = 0.0;
    let mut offset = 0.0;
    for look in &point.look {
        until += look.hold;
        if f64::from(state.stop_elapsed) < until {
            offset = look.yaw_deg;
            break;
        }
    }
    #[allow(clippy::cast_possible_truncation)]
    {
        (base + offset) as f32
    }
}

/// The yaw (−Z forward, degrees) of the leg walked to `state.point`, if it had a heading.
fn leg_yaw(state: &PatrolState, doc: &PatrolDocument, len: usize) -> Option<f64> {
    let point = state.point as usize;
    let previous = match doc.mode {
        PatrolMode::Loop => (len > 1).then(|| (point + len - 1) % len),
        PatrolMode::Once => point.checked_sub(1),
        PatrolMode::PingPong if state.forward => point.checked_sub(1),
        PatrolMode::PingPong => (point + 1 < len).then_some(point + 1),
    }?;
    let (to, from) = (doc.points[point].at, doc.points[previous].at);
    let (dx, dz) = (to[0] - from[0], to[2] - from[2]);
    ((dx * dx + dz * dz).sqrt() > HEADING_EPS)
        .then(|| ludion_core::math::f64::atan2(-dx, -dz).to_degrees())
}

/// The stop nearest the guard by horizontal distance (ties to the lowest index).
fn nearest(world: &World, id: EntityId, doc: &PatrolDocument) -> Option<u32> {
    let pos = world_transform(world, id)?.trs.translation;
    let mut best: Option<(f32, usize)> = None;
    for (index, point) in doc.points.iter().enumerate() {
        let at = to_f32(point.at);
        let (dx, dz) = (at[0] - pos[0], at[2] - pos[2]);
        let distance = dx * dx + dz * dz;
        if best.is_none_or(|(d, _)| distance < d) {
            best = Some((distance, index));
        }
    }
    best.map(|(_, index)| index as u32)
}

/// Points the guard's `NavGoal` at `point`, keeping an existing goal's `replan`.
fn write_goal(world: &mut World, id: EntityId, point: &PatrolPoint) {
    let at = to_f32(point.at);
    match world.get_mut::<NavGoal>(id) {
        Ok(Some(goal)) => {
            if goal.at != at {
                goal.at = at;
            }
        }
        _ => {
            let _ = world.insert(
                id,
                NavGoal {
                    at,
                    ..NavGoal::default()
                },
            );
        }
    }
}

fn write_state(world: &mut World, id: EntityId, state: PatrolState) {
    let _ = world.insert(id, state);
}

#[allow(clippy::cast_possible_truncation)]
fn to_f32(at: [f64; 3]) -> [f32; 3] {
    [at[0] as f32, at[1] as f32, at[2] as f32]
}

/// Reports each failure once per entity (and point), after the guards are done.
fn report(world: &mut World, failures: Vec<Failure>) {
    for failure in failures {
        let (id, diagnostic, suffix) = match failure {
            Failure::NoAgent(id) => (
                id,
                Diagnostic::new(
                    code::PATROL_WITHOUT_AGENT,
                    format!("{id} has a `PatrolFollower` but no `NavAgent`, so it does not patrol"),
                )
                .with_help("add a `NavAgent` to the entity"),
                String::new(),
            ),
            Failure::Unloaded(id, route, causes) => (
                id,
                causes.into_iter().fold(
                    Diagnostic::new(
                        code::PATROL_ROUTE_UNLOADED,
                        format!(
                            "{id} patrols `{route}`, which failed to load, so it is left alone"
                        ),
                    ),
                    Diagnostic::with_related,
                ),
                "/route".to_string(),
            ),
            Failure::Unreachable(id, point) => (
                id,
                Diagnostic::new(
                    code::PATROL_POINT_UNREACHABLE,
                    format!("{id} cannot reach patrol point {point}, so it skips it"),
                )
                .with_path(format!("/points/{point}"))
                .with_help("move the point onto walkable ground, or clear what blocks it"),
                format!("/points/{point}"),
            ),
        };
        let key = format!(
            "{}/PatrolFollower{suffix}",
            crate::diagnostics::offender_key(world, id)
        );
        crate::diagnostics::report_full(world, key, diagnostic);
    }
}

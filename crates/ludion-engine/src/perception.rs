//! The engine's host for sight: [`PerceptionConfig`], [`PerceptionResults`] and the
//! [`perception_update`] system.
//!
//! A game opts in with `Engine::builder().perception(config)` and schedules
//! `.system("perception_update", perception_update)` after whatever moves the perceivers and
//! targets. Each tick the system rebuilds the [`PerceptionResults`] resource from every enabled
//! [`Perceiver`] and every [`SightTarget`]: one [`SightSample`] per (perceiver, zone, target) whose
//! target has a sample point inside the zone's wedge, sorted by `(perceiver, zone, target)`.
//!
//! A sample point is *seen* when it is in the wedge and its line of sight is clear: a ray from the
//! eye to the point, cast against the layers the point's `blocked_by` names (less the zone's
//! `see_through` layers inside `see_through_max_range`). All rays of one blocker set run together
//! through [`PhysicsWorld::first_hits`], so the result is the same at any thread count. Results are
//! derived state: not hashed, not snapshotted. Transforms are current; line of sight reads the last
//! completed physics step.

use std::collections::BTreeMap;

use ludion_components::{Perceiver, Require, SightTarget, SweepShape, ZoneSweep};
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::math;
use ludion_core::suggest::nearest;
use ludion_ecs::{SystemCtx, World};
use ludion_physics::{LayerSet, PhysicsWorld, QueryFilter, Sweep};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::diagnostics::report_full;
use crate::hierarchy::world_transform;

const DEG_TO_RAD: f32 = core::f32::consts::PI / 180.0;
const RAD_TO_DEG: f32 = 180.0 / core::f32::consts::PI;
/// Most classes a config declares: each is one bit of a `u32`.
const MAX_CLASSES: usize = 32;

/// The class names perception knows. A [`SightTarget`]'s `class` and a zone's `detects` name them.
///
/// # Example
/// ```
/// use ludion_engine::PerceptionConfig;
///
/// let config = PerceptionConfig { classes: vec!["player".into(), "decoy".into()] };
/// assert!(config.validate().is_ok());
/// assert!(PerceptionConfig { classes: vec!["a".into(), "a".into()] }.validate().is_err());
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PerceptionConfig {
    /// The declared classes, at most 32, each non-empty and unique.
    pub classes: Vec<String>,
}

impl PerceptionConfig {
    /// Checks the class list.
    ///
    /// # Errors
    /// `L4220`, pointer `/classes` for a list over 32 names or `/classes/{j}` for an empty or
    /// repeated name.
    pub fn validate(&self) -> Result<(), Diagnostic> {
        let invalid = |path: String, what: String| {
            Diagnostic::new(code::PERCEPTION_CONFIG_INVALID, what)
                .with_path(path)
                .with_help("list each class once, at most 32; see `ludion docs perception`")
        };
        if self.classes.len() > MAX_CLASSES {
            return Err(invalid(
                "/classes".into(),
                format!(
                    "perception `classes` holds {} names; the most is {MAX_CLASSES}",
                    self.classes.len()
                ),
            ));
        }
        for (j, name) in self.classes.iter().enumerate() {
            if name.is_empty() {
                return Err(invalid(
                    format!("/classes/{j}"),
                    format!("perception `classes[{j}]` is empty"),
                ));
            }
            if self.classes[..j].contains(name) {
                return Err(invalid(
                    format!("/classes/{j}"),
                    format!("perception `classes[{j}]` repeats `{name}`"),
                ));
            }
        }
        Ok(())
    }
}

/// What one perceiver's zone made of one target this tick. Only a target with a sample point inside
/// the zone's wedge has a sample.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct SightSample {
    /// The entity that looks.
    pub perceiver: EntityId,
    /// Index into the perceiver's `sight` list.
    pub zone: u8,
    /// The entity looked at.
    pub target: EntityId,
    /// Whether any sample point is inside the zone's wedge (always `true` for a reported sample).
    pub in_zone: bool,
    /// Whether the target's `require` rule is met.
    pub visible: bool,
    /// A bitmask over the target's `points`: bit `i` is set when point `i` is in the wedge with a
    /// clear line of sight.
    pub points_visible: u8,
    /// The nearest blocker of the lowest-index blocked point, when any point was blocked.
    pub blocker: Option<EntityId>,
    /// Metres from the eye to the target's world origin.
    pub distance: f32,
    /// Degrees between the zone's facing and the direction to the target's world origin.
    pub angle_deg: f32,
    /// The zone's range after visibility scaling.
    pub effective_range: f32,
    /// The gameplay tick the sample was computed on.
    pub evaluated_tick: u64,
}

/// The samples [`perception_update`] produced, rebuilt every tick. A derived resource: not hashed,
/// not snapshotted.
///
/// # Example
/// ```
/// use ludion_engine::PerceptionResults;
///
/// let results = PerceptionResults::default();
/// assert!(results.samples().is_empty());
/// ```
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PerceptionResults {
    tick: u64,
    samples: Vec<SightSample>,
    facings: Vec<(EntityId, u8, [f32; 3])>,
}

impl PerceptionResults {
    /// The gameplay tick these results are from.
    pub fn tick(&self) -> u64 {
        self.tick
    }

    /// Every sample, sorted by `(perceiver, zone, target)`.
    pub fn samples(&self) -> &[SightSample] {
        &self.samples
    }

    /// `perceiver`'s samples, sorted by `(zone, target)`.
    pub fn sight(&self, perceiver: EntityId) -> &[SightSample] {
        let start = self.samples.partition_point(|s| s.perceiver < perceiver);
        let len = self.samples[start..]
            .iter()
            .take_while(|s| s.perceiver == perceiver)
            .count();
        &self.samples[start..start + len]
    }

    /// The world-space unit vector `perceiver`'s zone `zone` faces this tick, after its `look` and
    /// sweep; `None` for a perceiver or zone that was not evaluated.
    pub fn zone_facing(&self, perceiver: EntityId, zone: u8) -> Option<[f32; 3]> {
        let at = self
            .facings
            .binary_search_by_key(&(perceiver, zone), |(e, z, _)| (*e, *z))
            .ok()?;
        Some(self.facings[at].2)
    }
}

/// Rebuilds [`PerceptionResults`] from every enabled [`Perceiver`] and [`SightTarget`]. A no-op in a
/// game that configured no perception. A zone's `detects` names and a target's `class` must be in
/// the [`PerceptionConfig`], and `blocked_by` / `see_through` names must be declared physics layers;
/// otherwise `L1572` is reported once per name and the zone or point detects or sees nothing.
///
/// **Schedule it after whatever moves perceivers and targets**, and declare its queries
/// (`.query::<(&Perceiver,)>().query::<(&SightTarget,)>()`) like any system that reads components.
/// It sweeps with `ctx.game_time`, so a held `GameClock` freezes the sweep.
///
/// # Example
/// ```
/// use ludion_engine::{Engine, PerceptionConfig, perception_update};
///
/// let engine = Engine::builder()
///     .perception(PerceptionConfig { classes: vec!["player".into()] })
///     .system("perception_update", perception_update)
///     .build()
///     .unwrap();
/// # let _ = engine;
/// ```
pub fn perception_update(world: &mut World, ctx: &SystemCtx) {
    let Some(config) = world.resource::<PerceptionConfig>().cloned() else {
        return;
    };
    let mut perceivers: Vec<(EntityId, Perceiver)> = world
        .query_ids::<(&Perceiver,)>()
        .filter(|(_, (p,))| p.enabled)
        .map(|(id, (p,))| (id, p.clone()))
        .collect();
    let mut targets: Vec<(EntityId, SightTarget)> = world
        .query_ids::<(&SightTarget,)>()
        .map(|(id, (t,))| (id, t.clone()))
        .collect();
    perceivers.sort_by_key(|(id, _)| *id);
    targets.sort_by_key(|(id, _)| *id);

    let eyes: Vec<Option<Pose>> = perceivers
        .iter()
        .map(|(id, p)| pose(world, *id).map(|pose| pose.with_eye(p.eye)))
        .collect();
    let bodies: Vec<Option<TargetPoints>> = targets
        .iter()
        .map(|(id, t)| target_points(world, *id, t))
        .collect();

    let mut results = PerceptionResults {
        tick: ctx.game_frame.as_u64(),
        ..PerceptionResults::default()
    };
    let mut reports: Vec<(String, Diagnostic)> = Vec::new();
    let seconds = ctx.game_time.seconds();
    {
        let physics = world.resource::<PhysicsWorld>();
        let mut frame = Frame {
            config: &config,
            physics,
            tick: results.tick,
            layers: BTreeMap::new(),
            reports: &mut reports,
        };
        let mut drafts: Vec<Draft> = Vec::new();
        for (index, (id, perceiver)) in perceivers.iter().enumerate() {
            let Some(eye) = &eyes[index] else { continue };
            let sweep_yaw = perceiver
                .sweep
                .as_ref()
                .map_or(0.0, |s| sweep_offset_deg(s, seconds));
            let (yaw, pitch) = (perceiver.look.yaw_deg + sweep_yaw, perceiver.look.pitch_deg);
            let facing = math::quat_rotate(eye.rotation, local_forward(yaw, pitch));
            for (zone_index, zone) in perceiver.sight.iter().enumerate() {
                let zone_id = zone_index as u8;
                results.facings.push((*id, zone_id, facing));
                let detects = frame.detect_mask(&zone.detects);
                let see_through = frame.known_layers(&zone.see_through);
                for (t_index, (target_id, target)) in targets.iter().enumerate() {
                    if target_id == id {
                        continue;
                    }
                    let Some(body) = &bodies[t_index] else {
                        continue;
                    };
                    let Some(class_bit) = frame.class_bit(&target.class) else {
                        continue;
                    };
                    if detects & class_bit == 0 {
                        continue;
                    }
                    let range = effective_range(
                        zone.range,
                        target.visibility,
                        zone.range_by_visibility,
                        zone.visibility_clamp,
                    );
                    let mut points = Vec::with_capacity(target.points.len());
                    let mut any = false;
                    for (point, world_point) in target.points.iter().zip(&body.points) {
                        let delta = sub(*world_point, eye.origin);
                        let local = to_zone_frame(eye.rotation, yaw, pitch, delta);
                        let inside = length(delta) <= range
                            && in_wedge(local, zone.half_angle_deg, zone.vertical_half_angle_deg);
                        any |= inside;
                        points.push(PointDraft {
                            world: *world_point,
                            inside,
                            blockers: inside
                                .then(|| {
                                    frame.blockers(
                                        &point.blocked_by,
                                        &see_through,
                                        length(delta),
                                        zone.see_through_max_range,
                                    )
                                })
                                .flatten(),
                            ray: None,
                        });
                    }
                    if !any {
                        continue;
                    }
                    let to_origin = sub(body.origin, eye.origin);
                    let distance = length(to_origin);
                    drafts.push(Draft {
                        perceiver: *id,
                        zone: zone_id,
                        target: *target_id,
                        eye: eye.origin,
                        points,
                        require: target.require,
                        distance,
                        angle_deg: angle_between(facing, to_origin, distance),
                        effective_range: range,
                    });
                }
            }
        }
        results.samples = frame.finish(drafts);
    }
    results.facings.sort_by_key(|(e, z, _)| (*e, *z));
    world.insert_resource(results);
    for (key, diagnostic) in reports {
        report_full(world, key, diagnostic);
    }
}

/// One sample under construction.
struct Draft {
    perceiver: EntityId,
    zone: u8,
    target: EntityId,
    eye: [f32; 3],
    points: Vec<PointDraft>,
    require: Require,
    distance: f32,
    angle_deg: f32,
    effective_range: f32,
}

/// One sample point: whether it is in the wedge, and what blocks it. `blockers` is `None` for a
/// point that cannot be seen (an unknown layer), `Some(empty)` for one nothing can block.
struct PointDraft {
    world: [f32; 3],
    inside: bool,
    blockers: Option<Vec<String>>,
    ray: Option<usize>,
}

/// One entity's world pose.
struct Pose {
    origin: [f32; 3],
    rotation: [f32; 4],
    scale: [f32; 3],
}

impl Pose {
    /// The same pose with `origin` moved to the local `eye` offset.
    fn with_eye(self, eye: [f32; 3]) -> Pose {
        Pose {
            origin: add(
                self.origin,
                math::quat_rotate(self.rotation, mul(eye, self.scale)),
            ),
            ..self
        }
    }
}

/// A target's world origin and sample points.
struct TargetPoints {
    origin: [f32; 3],
    points: Vec<[f32; 3]>,
}

fn pose(world: &World, entity: EntityId) -> Option<Pose> {
    let trs = world_transform(world, entity)?.trs;
    Some(Pose {
        origin: trs.translation,
        rotation: trs.rotation,
        scale: trs.scale,
    })
}

fn target_points(world: &World, entity: EntityId, target: &SightTarget) -> Option<TargetPoints> {
    let pose = pose(world, entity)?;
    Some(TargetPoints {
        origin: pose.origin,
        points: target
            .points
            .iter()
            .map(|p| {
                add(
                    pose.origin,
                    math::quat_rotate(pose.rotation, mul(p.offset, pose.scale)),
                )
            })
            .collect(),
    })
}

/// What a tick's evaluation shares: the config, the physics world, and the names already checked.
struct Frame<'a> {
    config: &'a PerceptionConfig,
    physics: Option<&'a PhysicsWorld>,
    tick: u64,
    /// Whether each layer name is declared, with the reason when it is not.
    layers: BTreeMap<String, Result<(), String>>,
    reports: &'a mut Vec<(String, Diagnostic)>,
}

impl Frame<'_> {
    fn warn(&mut self, key: String, message: String, help: String) {
        self.reports.push((
            key,
            Diagnostic::new(code::PERCEPTION_NAME_UNKNOWN, message).with_help(help),
        ));
    }

    /// The class bit for `name`, or `None` (reported) for an undeclared one.
    fn class_bit(&mut self, name: &str) -> Option<u32> {
        let at = self.config.classes.iter().position(|c| c == name);
        if at.is_none() {
            self.unknown_class(name);
        }
        at.map(|i| 1 << i)
    }

    fn unknown_class(&mut self, name: &str) {
        let help = match nearest(name, self.config.classes.iter().map(String::as_str)) {
            Some(near) => {
                format!("did you mean `{near}`? declare classes in `PerceptionConfig.classes`")
            }
            None => "declare the class in `PerceptionConfig.classes`".to_string(),
        };
        self.warn(
            format!("class:{name}"),
            format!("perception class `{name}` is not declared in the `PerceptionConfig`"),
            help,
        );
    }

    /// The bits a `detects` list names; empty means every class, and an undeclared name adds none.
    fn detect_mask(&mut self, detects: &[String]) -> u32 {
        if detects.is_empty() {
            return u32::MAX;
        }
        detects
            .iter()
            .fold(0, |mask, name| mask | self.class_bit(name).unwrap_or(0))
    }

    /// Whether `name` is a declared layer; an undeclared one is reported once.
    fn layer_known(&mut self, name: &str) -> bool {
        let Some(physics) = self.physics else {
            return true;
        };
        let known = self
            .layers
            .entry(name.to_string())
            .or_insert_with(|| {
                physics
                    .sweep_filter(&QueryFilter::new().include_layer(name))
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            })
            .clone();
        if let Err(reason) = &known {
            self.warn(
                format!("layer:{name}"),
                format!("perception layer `{name}` is not a declared physics layer: {reason}"),
                "declare the layer under `physics.layers`".to_string(),
            );
        }
        known.is_ok()
    }

    /// The declared names of `set`, in order.
    fn known_layers(&mut self, set: &LayerSet) -> Vec<String> {
        set.iter()
            .filter(|name| self.layer_known(name))
            .map(String::from)
            .collect()
    }

    /// The blocker set of one point (see [`blockers`]); `None` when `blocked_by` names an
    /// undeclared layer, so the point is never seen.
    fn blockers(
        &mut self,
        blocked_by: &LayerSet,
        see_through: &[String],
        distance: f32,
        max_range: f32,
    ) -> Option<Vec<String>> {
        let mut all_known = true;
        for name in blocked_by.iter() {
            all_known &= self.layer_known(name);
        }
        all_known.then(|| blockers(blocked_by.as_slice(), see_through, distance, max_range))
    }

    /// Casts every ray, grouped by blocker set, and folds the drafts into samples in draft order.
    fn finish(&self, mut drafts: Vec<Draft>) -> Vec<SightSample> {
        struct Ray {
            sweep: Sweep,
            perceiver: EntityId,
            target: EntityId,
        }
        let mut rays: Vec<Ray> = Vec::new();
        let mut groups: BTreeMap<Vec<String>, Vec<usize>> = BTreeMap::new();
        for draft in &mut drafts {
            for point in &mut draft.points {
                let Some(set) = &point.blockers else { continue };
                if !point.inside || set.is_empty() {
                    continue;
                }
                let at = rays.len();
                rays.push(Ray {
                    sweep: Sweep::new(draft.eye, sub(point.world, draft.eye), 0.0)
                        .excluding(draft.target),
                    perceiver: draft.perceiver,
                    target: draft.target,
                });
                groups.entry(set.clone()).or_default().push(at);
                point.ray = Some(at);
            }
        }
        let mut outcomes = vec![Cast::Clear; rays.len()];
        if let Some(physics) = self.physics {
            for (set, members) in &groups {
                let layers = LayerSet::from(set.clone());
                let Ok(filter) = physics.sweep_filter(&QueryFilter::new().include_set(&layers))
                else {
                    for &at in members {
                        outcomes[at] = Cast::Blocked(None);
                    }
                    continue;
                };
                let sweeps: Vec<Sweep> = members.iter().map(|&at| rays[at].sweep).collect();
                let mut hits = vec![None; sweeps.len()];
                physics.first_hits(&sweeps, &filter, &mut hits);
                for (&at, hit) in members.iter().zip(hits) {
                    let ray = &rays[at];
                    // The eye inside the perceiver's own blocking collider is not a blocker.
                    let hit = match hit {
                        Some(h) if h.entity == ray.perceiver => physics.first_hit_skipping(
                            &ray.sweep,
                            &filter,
                            &[ray.perceiver, ray.target],
                        ),
                        other => other,
                    };
                    outcomes[at] = hit.map_or(Cast::Clear, |h| Cast::Blocked(Some(h.entity)));
                }
            }
        }
        drafts
            .into_iter()
            .map(|draft| {
                let cast = |point: &PointDraft| point.ray.map_or(Cast::Clear, |at| outcomes[at]);
                let seen = |point: &PointDraft| {
                    point.inside && point.blockers.is_some() && cast(point) == Cast::Clear
                };
                let visible_points = draft.points.iter().filter(|p| seen(p)).count();
                let mask = draft
                    .points
                    .iter()
                    .enumerate()
                    .fold(0u8, |m, (i, p)| m | (u8::from(seen(p)) << i));
                let total = draft.points.len();
                let visible = match draft.require {
                    Require::Any => visible_points >= 1,
                    Require::All => visible_points == total,
                    Require::Fraction(f) => {
                        visible_points >= ((f * total as f32).ceil() as usize).max(1)
                    }
                };
                let blocker = draft.points.iter().find_map(|p| match cast(p) {
                    Cast::Blocked(entity) if p.ray.is_some() => Some(entity),
                    _ => None,
                });
                SightSample {
                    perceiver: draft.perceiver,
                    zone: draft.zone,
                    target: draft.target,
                    in_zone: true,
                    visible,
                    points_visible: mask,
                    blocker: blocker.flatten(),
                    distance: draft.distance,
                    angle_deg: draft.angle_deg,
                    effective_range: draft.effective_range,
                    evaluated_tick: self.tick,
                }
            })
            .collect()
    }
}

/// How one line-of-sight ray ended: clear, or stopped by an entity (`None` when the filter could
/// not be built, which is treated as blocked).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Cast {
    Clear,
    Blocked(Option<EntityId>),
}

/// The layers that block a point: `blocked_by` less `see_through` when the point is within
/// `max_range` of the eye, sorted and deduplicated. An empty result means nothing blocks the point.
fn blockers(
    blocked_by: &[String],
    see_through: &[String],
    distance: f32,
    max_range: f32,
) -> Vec<String> {
    let strip = max_range > 0.0 && distance <= max_range;
    let mut set: Vec<String> = blocked_by
        .iter()
        .filter(|name| !(strip && see_through.contains(name)))
        .cloned()
        .collect();
    set.sort();
    set.dedup();
    set
}

/// `range`, scaled by `visibility` clamped to `clamp` (or floored at 0) when `by_visibility`.
fn effective_range(
    range: f32,
    visibility: f32,
    by_visibility: bool,
    clamp: Option<[f32; 2]>,
) -> f32 {
    if !by_visibility {
        return range;
    }
    let scale = match clamp {
        Some([lo, hi]) => visibility.clamp(lo, hi),
        None => visibility.max(0.0),
    };
    range * scale
}

/// The yaw offset in degrees a sweep adds at `seconds` of gameplay time. A closed form: `t` is
/// reduced in `f64` before the `f32` math so a long run does not drift.
fn sweep_offset_deg(sweep: &ZoneSweep, seconds: f64) -> f32 {
    let period = f64::from(sweep.period_s);
    let amplitude = f64::from(sweep.amplitude_deg);
    if period.is_nan() || period <= 0.0 {
        return 0.0;
    }
    let t = (seconds + f64::from(sweep.phase_s)).rem_euclid(period);
    let offset = match sweep.shape {
        SweepShape::Sine => amplitude * math::f64::sin(std::f64::consts::TAU * t / period),
        SweepShape::PingPongLinear => {
            let dwell = f64::from(sweep.dwell_s);
            let travel = (period - 2.0 * dwell) / 2.0;
            if t < travel {
                -amplitude + 2.0 * amplitude * t / travel
            } else if t < travel + dwell {
                amplitude
            } else if t < 2.0 * travel + dwell {
                amplitude - 2.0 * amplitude * (t - travel - dwell) / travel
            } else {
                -amplitude
            }
        }
    };
    offset as f32
}

/// The facing in an entity's local frame for `yaw` and `pitch` degrees: yaw 0 is −Z, positive yaw
/// turns left, positive pitch looks down.
fn local_forward(yaw_deg: f32, pitch_deg: f32) -> [f32; 3] {
    let (sy, cy) = math::sin_cos(yaw_deg * DEG_TO_RAD);
    let (sp, cp) = math::sin_cos(pitch_deg * DEG_TO_RAD);
    [-sy * cp, -sp, -cy * cp]
}

/// `delta` (world) in the zone frame, where the facing is −Z, +X is right and +Y is up.
fn to_zone_frame(rotation: [f32; 4], yaw_deg: f32, pitch_deg: f32, delta: [f32; 3]) -> [f32; 3] {
    let conjugate = [-rotation[0], -rotation[1], -rotation[2], rotation[3]];
    let [x, y, z] = math::quat_rotate(conjugate, delta);
    let (sy, cy) = math::sin_cos(yaw_deg * DEG_TO_RAD);
    let (sp, cp) = math::sin_cos(pitch_deg * DEG_TO_RAD);
    let (x1, z1) = (x * cy - z * sy, x * sy + z * cy);
    [x1, y * cp - z1 * sp, y * sp + z1 * cp]
}

/// Whether a point at `frame` (see [`to_zone_frame`]) is inside the wedge: `|h| <= half` and
/// `|v| <= vertical_half`. The eye itself is inside.
fn in_wedge(frame: [f32; 3], half_deg: f32, vertical_half_deg: f32) -> bool {
    let [x, y, z] = frame;
    let flat = math::hypot(x, z);
    if flat == 0.0 && y == 0.0 {
        return true;
    }
    let h = math::atan2(x, -z) * RAD_TO_DEG;
    let v = math::atan2(y, flat) * RAD_TO_DEG;
    h.abs() <= half_deg && v.abs() <= vertical_half_deg
}

fn angle_between(facing: [f32; 3], to: [f32; 3], distance: f32) -> f32 {
    if distance <= 0.0 {
        return 0.0;
    }
    let cos = (dot(facing, to) / distance).clamp(-1.0, 1.0);
    math::acos(cos) * RAD_TO_DEG
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn mul(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] * b[0], a[1] * b[1], a[2] * b[2]]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn length(a: [f32; 3]) -> f32 {
    math::sqrt(dot(a, a))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sweep(shape: SweepShape, amplitude: f32, period: f32, dwell: f32, phase: f32) -> ZoneSweep {
        ZoneSweep {
            amplitude_deg: amplitude,
            period_s: period,
            dwell_s: dwell,
            phase_s: phase,
            shape,
        }
    }

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn ping_pong_travels_dwells_and_returns() {
        // period 8, dwell 1: travel 3 s each way.
        let s = sweep(SweepShape::PingPongLinear, 40.0, 8.0, 1.0, 0.0);
        for (t, want) in [
            (0.0, -40.0),
            (1.5, 0.0),
            (3.0, 40.0),
            (3.5, 40.0),
            (4.0, 40.0),
            (5.5, 0.0),
            (7.0, -40.0),
            (7.5, -40.0),
            (8.0, -40.0),
            (9.5, 0.0),
        ] {
            assert!(near(sweep_offset_deg(&s, t), want), "t={t}");
        }
    }

    #[test]
    fn a_phase_shifts_the_cycle_and_a_negative_time_wraps() {
        let s = sweep(SweepShape::PingPongLinear, 40.0, 8.0, 0.0, 2.0);
        assert!(near(
            sweep_offset_deg(&s, 0.0),
            sweep_offset_deg(&sweep(SweepShape::PingPongLinear, 40.0, 8.0, 0.0, 0.0), 2.0)
        ));
        assert!(sweep_offset_deg(&s, -1.0).is_finite());
    }

    #[test]
    fn sine_is_amplitude_times_sin() {
        let s = sweep(SweepShape::Sine, 30.0, 4.0, 0.0, 0.0);
        assert!(near(sweep_offset_deg(&s, 0.0), 0.0));
        assert!(near(sweep_offset_deg(&s, 1.0), 30.0));
        assert!(near(sweep_offset_deg(&s, 3.0), -30.0));
    }

    #[test]
    fn a_billion_ticks_in_does_not_drift() {
        // 1e9 ticks at 60 Hz is a whole number of 8 s periods plus 4 s: 1e9 / 60 s = 16666666.67 s.
        let s = sweep(SweepShape::PingPongLinear, 40.0, 8.0, 0.0, 0.0);
        let seconds = 1.0e9_f64 / 60.0;
        let expected = sweep_offset_deg(&s, seconds.rem_euclid(8.0));
        assert_eq!(sweep_offset_deg(&s, seconds), expected);
        assert!(expected.abs() <= 40.0);
    }

    #[test]
    fn the_facing_maps_to_the_zone_axis() {
        let identity = [0.0, 0.0, 0.0, 1.0];
        for (yaw, pitch) in [(0.0, 0.0), (35.0, 0.0), (-80.0, 20.0), (120.0, -30.0)] {
            let f = local_forward(yaw, pitch);
            let frame = to_zone_frame(identity, yaw, pitch, f);
            assert!(
                near(frame[0], 0.0) && near(frame[1], 0.0) && near(frame[2], -1.0),
                "{yaw} {pitch}: {frame:?}"
            );
        }
        // Positive yaw turns left (toward −X); positive pitch looks down.
        assert!(local_forward(30.0, 0.0)[0] < 0.0);
        assert!(local_forward(0.0, 30.0)[1] < 0.0);
    }

    #[test]
    fn the_wedge_edges_are_inclusive_in_both_axes() {
        let at = |h_deg: f32, v_deg: f32| {
            let (sh, ch) = math::sin_cos(h_deg * DEG_TO_RAD);
            let (sv, cv) = math::sin_cos(v_deg * DEG_TO_RAD);
            [sh * cv, sv, -ch * cv]
        };
        assert!(in_wedge(at(59.0, 0.0), 60.0, 45.0));
        assert!(!in_wedge(at(61.0, 0.0), 60.0, 45.0));
        assert!(in_wedge(at(-59.0, 44.0), 60.0, 45.0));
        assert!(!in_wedge(at(0.0, 46.0), 60.0, 45.0));
        assert!(!in_wedge(at(0.0, -46.0), 60.0, 45.0));
        assert!(in_wedge([0.0; 3], 10.0, 10.0));
        assert!(in_wedge([0.0, 0.0, 1.0], 180.0, 90.0));
    }

    #[test]
    fn blockers_strip_see_through_only_inside_the_range() {
        let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let blocked = names(&["wall", "cover_overhead", "wall"]);
        let see = names(&["cover_overhead"]);
        assert_eq!(blockers(&blocked, &see, 2.0, 3.0), ["wall"]);
        assert_eq!(blockers(&blocked, &see, 3.0, 3.0), ["wall"]);
        assert_eq!(
            blockers(&blocked, &see, 3.5, 3.0),
            ["cover_overhead", "wall"]
        );
        assert_eq!(
            blockers(&blocked, &see, 0.0, 0.0),
            ["cover_overhead", "wall"]
        );
        assert!(blockers(&[], &see, 1.0, 3.0).is_empty());
        assert!(blockers(&names(&["cover_overhead"]), &see, 1.0, 3.0).is_empty());
    }

    #[test]
    fn the_effective_range_scales_and_clamps() {
        assert_eq!(effective_range(10.0, 0.5, false, None), 10.0);
        assert_eq!(effective_range(10.0, 0.5, true, None), 5.0);
        assert_eq!(effective_range(10.0, 3.0, true, None), 30.0);
        assert_eq!(effective_range(10.0, 3.0, true, Some([0.0, 2.0])), 20.0);
        assert_eq!(effective_range(10.0, 0.1, true, Some([0.25, 2.0])), 2.5);
        assert_eq!(effective_range(10.0, -1.0, true, None), 0.0);
    }

    #[test]
    fn a_config_names_the_field_it_refuses() {
        let path = |classes: Vec<String>| {
            PerceptionConfig { classes }
                .validate()
                .unwrap_err()
                .path
                .unwrap_or_default()
        };
        assert_eq!(path((0..33).map(|n| n.to_string()).collect()), "/classes");
        assert_eq!(path(vec!["a".into(), String::new()]), "/classes/1");
        assert_eq!(path(vec!["a".into(), "b".into(), "a".into()]), "/classes/2");
        assert!(
            PerceptionConfig {
                classes: (0..32).map(|n| n.to_string()).collect()
            }
            .validate()
            .is_ok()
        );
    }
}

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
//!
//! A perceiver with `memory` also keeps one [`StimulusRecord`] entity per `(perceiver, sense,
//! source)`: sight refreshes it every tick the target is visible, [`report_stimulus`] refreshes it for
//! hearing and custom senses, and it is forgotten after `forget_*_s` of gameplay time. Records are
//! ordinary entities, so they are hashed and snapshotted.

use std::collections::{BTreeMap, BTreeSet};

use ludion_components::{
    NoiseZone, NoiseZoneShape, PendingNoise, Perceiver, PerceiverMemory, PerceiverStatus,
    PerceptionState, Require, Sense, SightSampleRecord, SightTarget, StimulusRecord, SweepShape,
    ZoneSweep,
};
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::math;
use ludion_core::suggest::nearest;
use ludion_ecs::{SystemCtx, World};
use ludion_physics::{LayerSet, PhysicsWorld, QueryFilter, SurfaceId, Sweep};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::diagnostics::report_full;
use crate::hierarchy::world_transform;

const DEG_TO_RAD: f32 = core::f32::consts::PI / 180.0;
const RAD_TO_DEG: f32 = 180.0 / core::f32::consts::PI;
/// Most classes a config declares: each is one bit of a `u32`.
const MAX_CLASSES: usize = 32;

/// The class names perception knows, and the noise settings. A [`SightTarget`]'s `class` and a
/// zone's `detects` name the classes; a [`Noise`]'s `kind` and a `hears` list name the noise kinds.
///
/// # Example
/// ```
/// use ludion_engine::PerceptionConfig;
///
/// let config = PerceptionConfig {
///     classes: vec!["player".into(), "decoy".into()],
///     noise_kinds: vec!["footstep".into()],
///     ..Default::default()
/// };
/// assert!(config.validate().is_ok());
/// let twice = PerceptionConfig { classes: vec!["a".into(), "a".into()], ..Default::default() };
/// assert!(twice.validate().is_err());
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PerceptionConfig {
    /// The declared classes, at most 32, each non-empty and unique.
    pub classes: Vec<String>,
    /// The declared noise kinds, at most 32, each non-empty and unique (default `[]`).
    #[serde(default)]
    pub noise_kinds: Vec<String>,
    /// Radius multipliers by physics surface name; an unlisted surface is ×1 (default `[]`).
    #[serde(default)]
    pub surface_noise: Vec<SurfaceNoise>,
    /// The layers whose colliders muffle a noise that has `occlusion`; empty means no occlusion
    /// (default `[]`).
    #[serde(default)]
    pub sound_occluder_layers: LayerSet,
    /// The radius factor per occluder between a noise and a listener, in [0, 1] (default `0.5`).
    #[serde(default = "default_occlusion_factor")]
    pub occlusion_factor: f32,
    /// Most occluders that count against one noise, 1 to 8 (default `2`).
    #[serde(default = "default_max_occluders")]
    pub max_occluders: u32,
    /// Most line-of-sight rays cast per tick; the perceivers that do not fit wait their turn. `None`
    /// is no cap; `Some(0)` is refused. 2048 is the recommended value (default `None`).
    #[serde(default)]
    pub max_sight_rays_per_tick: Option<u32>,
}

fn default_occlusion_factor() -> f32 {
    0.5
}

fn default_max_occluders() -> u32 {
    2
}

impl Default for PerceptionConfig {
    fn default() -> Self {
        Self {
            classes: Vec::new(),
            noise_kinds: Vec::new(),
            surface_noise: Vec::new(),
            sound_occluder_layers: LayerSet::default(),
            occlusion_factor: default_occlusion_factor(),
            max_occluders: default_max_occluders(),
            max_sight_rays_per_tick: None,
        }
    }
}

/// A noise radius multiplier for one physics surface, by the name `physics.surfaces` declares.
///
/// # Example
/// ```
/// use ludion_engine::SurfaceNoise;
///
/// let grate = SurfaceNoise { surface: "metal_grate".into(), multiplier: 1.5 };
/// assert_eq!(grate.multiplier, 1.5);
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SurfaceNoise {
    /// A declared surface name.
    pub surface: String,
    /// Scales the noise radius on that surface (finite, not negative).
    pub multiplier: f32,
}

impl PerceptionConfig {
    /// Checks the class and noise-kind lists and the noise numbers.
    ///
    /// # Errors
    /// `L4220`, pointer `/classes` or `/noise_kinds` for a list over 32 names, `/classes/{j}` or
    /// `/noise_kinds/{j}` for an empty or repeated name, `/surface_noise/{j}/surface` or
    /// `/surface_noise/{j}/multiplier`, `/occlusion_factor`, `/max_occluders`, or
    /// `/max_sight_rays_per_tick` for a budget of 0.
    pub fn validate(&self) -> Result<(), Diagnostic> {
        let invalid = |path: String, what: String| {
            Diagnostic::new(code::PERCEPTION_CONFIG_INVALID, what)
                .with_path(path)
                .with_help("list each name once, at most 32; see `ludion docs perception`")
        };
        for (field, names) in [
            ("classes", &self.classes),
            ("noise_kinds", &self.noise_kinds),
        ] {
            if names.len() > MAX_CLASSES {
                return Err(invalid(
                    format!("/{field}"),
                    format!(
                        "perception `{field}` holds {} names; the most is {MAX_CLASSES}",
                        names.len()
                    ),
                ));
            }
            for (j, name) in names.iter().enumerate() {
                if name.is_empty() {
                    return Err(invalid(
                        format!("/{field}/{j}"),
                        format!("perception `{field}[{j}]` is empty"),
                    ));
                }
                if names[..j].contains(name) {
                    return Err(invalid(
                        format!("/{field}/{j}"),
                        format!("perception `{field}[{j}]` repeats `{name}`"),
                    ));
                }
            }
        }
        for (j, entry) in self.surface_noise.iter().enumerate() {
            if entry.surface.is_empty()
                || self.surface_noise[..j]
                    .iter()
                    .any(|e| e.surface == entry.surface)
            {
                return Err(invalid(
                    format!("/surface_noise/{j}/surface"),
                    format!(
                        "perception `surface_noise[{j}].surface` is empty or repeats `{}`",
                        entry.surface
                    ),
                ));
            }
            if !(entry.multiplier.is_finite() && entry.multiplier >= 0.0) {
                return Err(invalid(
                    format!("/surface_noise/{j}/multiplier"),
                    format!(
                        "perception `surface_noise[{j}].multiplier` {} must be finite and not negative",
                        entry.multiplier
                    ),
                ));
            }
        }
        if !(0.0..=1.0).contains(&self.occlusion_factor) {
            return Err(invalid(
                "/occlusion_factor".into(),
                format!(
                    "perception `occlusion_factor` {} must be in [0, 1]",
                    self.occlusion_factor
                ),
            ));
        }
        if !(1..=8).contains(&self.max_occluders) {
            return Err(invalid(
                "/max_occluders".into(),
                format!(
                    "perception `max_occluders` {} must be in 1 to 8",
                    self.max_occluders
                ),
            ));
        }
        if self.max_sight_rays_per_tick == Some(0) {
            return Err(invalid(
                "/max_sight_rays_per_tick".into(),
                "perception `max_sight_rays_per_tick` 0 would starve every perceiver; leave it unset for no cap (2048 is recommended)".into(),
            ));
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
    heard: Vec<HeardNoise>,
    noises: Vec<NoiseRecord>,
    rays_cast: u32,
    deferred: Vec<EntityId>,
}

/// The id [`emit_noise`] gave a noise; ids rise by one per accepted noise.
///
/// # Example
/// ```
/// use ludion_engine::NoiseId;
///
/// assert!(NoiseId(1) < NoiseId(2));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NoiseId(pub u64);

/// A noise a game emits with [`emit_noise`].
///
/// # Example
/// ```
/// use ludion_engine::Noise;
///
/// let step = Noise {
///     at: [2.0, 0.0, 1.0], radius: 6.0, loudness: 0.8, kind: "footstep".into(),
///     source: None, surface: None, occlusion: true,
/// };
/// assert_eq!(step.kind, "footstep");
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct Noise {
    /// Where it was made, world metres (finite).
    pub at: [f32; 3],
    /// Metres it carries before zones, surface and sensitivity (finite, not negative).
    pub radius: f32,
    /// How loud it was; listeners read it back and memory stores it as `strength` (finite, not negative).
    pub loudness: f32,
    /// A declared `PerceptionConfig.noise_kinds` name.
    pub kind: String,
    /// What made it; `None` for an anonymous one, which merges per `(listener, kind)` in memory.
    pub source: Option<EntityId>,
    /// The surface it was made on, e.g. `MoveReport.ground.surface`; scaled by `surface_noise`.
    pub surface: Option<SurfaceId>,
    /// Whether `sound_occluder_layers` colliders between it and a listener shrink its radius.
    pub occlusion: bool,
}

/// One delivery of a noise to one listener.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct HeardNoise {
    /// The entity that heard it.
    pub listener: EntityId,
    /// The noise.
    pub noise: NoiseId,
    /// Where it was made.
    pub at: [f32; 3],
    /// Its kind.
    pub kind: String,
    /// How loud it was.
    pub loudness: f32,
    /// What made it, when known.
    pub source: Option<EntityId>,
    /// Metres from the listener's ear to `at`.
    pub distance: f32,
    /// The radius this listener's ear applied: zone, surface, sensitivity and occluders included.
    pub effective_radius: f32,
    /// The sound occluders counted between the noise and the ear, after the `max_occluders` cap;
    /// 0 when the noise or config has no occlusion.
    pub occluders: u8,
}

/// One noise as the tick delivered it, from the source's side.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct NoiseRecord {
    /// The noise.
    pub id: NoiseId,
    /// Where it was made.
    pub at: [f32; 3],
    /// Its kind.
    pub kind: String,
    /// How loud it was.
    pub loudness: f32,
    /// What made it, when known.
    pub source: Option<EntityId>,
    /// The radius after the zone and surface multipliers, before any listener's sensitivity or
    /// occluders: the number to draw a ring from.
    pub effective_radius: f32,
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

    /// The noises delivered this tick to `listener`, sorted by noise id.
    pub fn heard(&self, listener: EntityId) -> &[HeardNoise] {
        let start = self.heard.partition_point(|h| h.listener < listener);
        let len = self.heard[start..]
            .iter()
            .take_while(|h| h.listener == listener)
            .count();
        &self.heard[start..start + len]
    }

    /// Every noise delivered this tick, in noise-id order, with the radius the senses used.
    pub fn noises(&self) -> &[NoiseRecord] {
        &self.noises
    }

    /// The line-of-sight rays cast this tick, re-casts after a self-hit included.
    pub fn rays_cast(&self) -> u32 {
        self.rays_cast
    }

    /// The perceivers whose sight was due this tick but did not fit `max_sight_rays_per_tick`,
    /// sorted. Their samples are carried from the last evaluation.
    pub fn deferred(&self) -> &[EntityId] {
        &self.deferred
    }
}

/// Rebuilds [`PerceptionResults`] from every enabled [`Perceiver`] and [`SightTarget`], delivers every
/// pending [`Noise`] (see [`emit_noise`]) to the listeners that hear it, then refreshes
/// and ages the [`StimulusRecord`]s of every perceiver with `memory`. A no-op in a
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
///     .perception(PerceptionConfig { classes: vec!["player".into()], ..Default::default() })
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
    let now = results.tick;
    let budget = config.max_sight_rays_per_tick;
    let carry = Carry::read(world, &perceivers, budget);
    let due: Vec<bool> = perceivers
        .iter()
        .enumerate()
        .map(|(index, (id, p))| {
            eyes[index].is_some()
                && (!carry.persisting.contains(id)
                    || is_due(
                        id.index(),
                        p.period_ticks,
                        now,
                        carry.status.get(id).copied(),
                    ))
        })
        .collect();
    let mut reports: Vec<(String, Diagnostic)> = Vec::new();
    let seconds = ctx.game_time.seconds();
    let rotation: Rotation;
    {
        let physics = world.resource::<PhysicsWorld>();
        let mut frame = Frame {
            config: &config,
            physics,
            tick: results.tick,
            layers: BTreeMap::new(),
            reports: &mut reports,
        };
        let mut plans: Vec<(EntityId, Vec<Draft>)> = Vec::new();
        for (index, (id, perceiver)) in perceivers.iter().enumerate() {
            let Some(eye) = &eyes[index] else { continue };
            let sweep_yaw = perceiver
                .sweep
                .as_ref()
                .map_or(0.0, |s| sweep_offset_deg(s, seconds));
            let (yaw, pitch) = (perceiver.look.yaw_deg + sweep_yaw, perceiver.look.pitch_deg);
            let facing = math::quat_rotate(eye.rotation, local_forward(yaw, pitch));
            for zone_index in 0..perceiver.sight.len() {
                results.facings.push((*id, zone_index as u8, facing));
            }
            if !due[index] {
                continue;
            }
            let mut drafts: Vec<Draft> = Vec::new();
            for (zone_index, zone) in perceiver.sight.iter().enumerate() {
                let zone_id = zone_index as u8;
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
            plans.push((*id, drafts));
        }
        let wanted: Vec<(EntityId, u32)> = plans
            .iter()
            .map(|(id, drafts)| (*id, planned_rays(drafts)))
            .collect();
        rotation = plan_rotation(&wanted, carry.cursor, budget);
        let drafts: Vec<Draft> = plans
            .into_iter()
            .filter(|(id, _)| rotation.served.binary_search(id).is_ok())
            .flat_map(|(_, drafts)| drafts)
            .collect();
        let (fresh, rays) = frame.finish(drafts);
        results.samples = fresh;
        results.rays_cast = rays;
    }
    results.facings.sort_by_key(|(e, z, _)| (*e, *z));
    results.deferred.clone_from(&rotation.deferred);
    let fresh = std::mem::take(&mut results.samples);
    let merged = persist_sight(world, now, &perceivers, &carry, &rotation, &fresh);
    note_starved(&perceivers, &carry, &rotation, now, &mut reports);
    let heard = hear(world, &config, &mut reports);
    results.heard = heard.heard;
    results.noises = heard.noises;
    update_memory(
        world,
        now,
        ctx.game_time.rate().hz(),
        &fresh,
        heard.refreshes,
    );
    results.samples = merged.unwrap_or(fresh);
    world.insert_resource(results);
    for (key, diagnostic) in reports {
        report_full(world, key, diagnostic);
    }
}

/// What a pass reads of earlier ticks: which perceivers persist their sight, when each was last
/// evaluated, the carried sample records and the rotation cursor.
struct Carry {
    /// Enabled perceivers whose sight is staggered (`period_ticks > 1`) or budgeted.
    persisting: BTreeSet<EntityId>,
    status: BTreeMap<EntityId, u64>,
    records: Vec<(EntityId, SightSampleRecord)>,
    cursor: Option<EntityId>,
}

impl Carry {
    fn read(world: &mut World, perceivers: &[(EntityId, Perceiver)], budget: Option<u32>) -> Carry {
        let persisting = perceivers
            .iter()
            .filter(|(_, p)| p.period_ticks > 1 || budget.is_some())
            .map(|(id, _)| *id)
            .collect();
        let status = world
            .query_ids::<(&PerceiverStatus,)>()
            .map(|(id, (s,))| (id, s.evaluated_tick))
            .collect();
        let mut records: Vec<(EntityId, SightSampleRecord)> = world
            .query_ids::<(&SightSampleRecord,)>()
            .map(|(id, (r,))| (id, r.clone()))
            .collect();
        records.sort_by_key(|(id, _)| *id);
        let cursor = world
            .query_ids::<(&PerceptionState,)>()
            .min_by_key(|(id, _)| *id)
            .and_then(|(_, (s,))| s.sight_cursor);
        Carry {
            persisting,
            status,
            records,
            cursor,
        }
    }
}

/// Whether a perceiver of period `period` and entity `index` is due at `now`: one never evaluated
/// waits for its phase (`now % period == index % period`), one evaluated is due `period` ticks after
/// its last evaluation, so a deferred perceiver stays due until served.
fn is_due(index: u32, period: u32, now: u64, evaluated: Option<u64>) -> bool {
    let period = u64::from(period.max(1));
    match evaluated {
        None => now % period == u64::from(index) % period,
        Some(last) => now.saturating_sub(last) >= period,
    }
}

/// The rays a perceiver's drafts will cast: points in a wedge that a non-empty blocker set can hide.
fn planned_rays(drafts: &[Draft]) -> u32 {
    drafts
        .iter()
        .flat_map(|d| &d.points)
        .filter(|p| p.inside && p.blockers.as_ref().is_some_and(|set| !set.is_empty()))
        .count() as u32
}

/// Who the budget serves this tick.
#[derive(Default)]
struct Rotation {
    served: Vec<EntityId>,
    deferred: Vec<EntityId>,
    cursor: Option<EntityId>,
}

/// Serves `due` (sorted by id, each with the rays it plans) from the first id above `cursor`,
/// wrapping. The first perceiver is always served, even over `budget`; the first that does not fit
/// stops the pass and every later one is deferred, which keeps the rotation fair. The cursor becomes
/// the last one served. With no budget everyone is served and the cursor is left alone.
fn plan_rotation(
    due: &[(EntityId, u32)],
    cursor: Option<EntityId>,
    budget: Option<u32>,
) -> Rotation {
    let Some(budget) = budget else {
        return Rotation {
            served: due.iter().map(|(id, _)| *id).collect(),
            deferred: Vec::new(),
            cursor,
        };
    };
    let start = cursor.map_or(0, |c| due.partition_point(|(id, _)| *id <= c));
    let mut rotation = Rotation {
        cursor,
        ..Rotation::default()
    };
    let mut used = 0u32;
    let mut stopped = false;
    for (id, rays) in due[start..].iter().chain(&due[..start]) {
        if !stopped && (rotation.served.is_empty() || used.saturating_add(*rays) <= budget) {
            used = used.saturating_add(*rays);
            rotation.served.push(*id);
            rotation.cursor = Some(*id);
        } else {
            stopped = true;
            rotation.deferred.push(*id);
        }
    }
    rotation.served.sort();
    rotation.deferred.sort();
    rotation
}

fn record_of(sample: &SightSample) -> SightSampleRecord {
    SightSampleRecord {
        perceiver: sample.perceiver,
        target: sample.target,
        blocker: sample.blocker,
        zone: sample.zone,
        in_zone: sample.in_zone,
        visible: sample.visible,
        points_visible: sample.points_visible,
        distance: sample.distance,
        angle_deg: sample.angle_deg,
        effective_range: sample.effective_range,
        evaluated_tick: sample.evaluated_tick,
    }
}

fn sample_of(record: &SightSampleRecord) -> SightSample {
    SightSample {
        perceiver: record.perceiver,
        zone: record.zone,
        target: record.target,
        in_zone: record.in_zone,
        visible: record.visible,
        points_visible: record.points_visible,
        blocker: record.blocker,
        distance: record.distance,
        angle_deg: record.angle_deg,
        effective_range: record.effective_range,
        evaluated_tick: record.evaluated_tick,
    }
}

/// The persistence pass: upserts the [`SightSampleRecord`]s and [`PerceiverStatus`] of every served
/// persisting perceiver, despawns those of a perceiver that is gone, disabled or no longer
/// persisting, advances the rotation cursor, and returns `fresh` merged with the carried samples of
/// the perceivers not served. A deferred perceiver that was never evaluated gets a status one period
/// back, so it stays due every tick until served and `L4221` can measure its wait. `None` when nothing
/// persists and nothing is stored, so a plain world is not touched.
fn persist_sight(
    world: &mut World,
    now: u64,
    perceivers: &[(EntityId, Perceiver)],
    carry: &Carry,
    rotation: &Rotation,
    fresh: &[SightSample],
) -> Option<Vec<SightSample>> {
    if carry.persisting.is_empty() && carry.status.is_empty() && carry.records.is_empty() {
        return None;
    }
    let served = |id: &EntityId| rotation.served.binary_search(id).is_ok();
    let mut wanted: BTreeMap<(EntityId, u8, EntityId), SightSampleRecord> = BTreeMap::new();
    for sample in fresh
        .iter()
        .filter(|s| carry.persisting.contains(&s.perceiver))
    {
        wanted.insert(
            (sample.perceiver, sample.zone, sample.target),
            record_of(sample),
        );
    }
    let mut doomed: Vec<EntityId> = Vec::new();
    let mut existing: BTreeMap<(EntityId, u8, EntityId), (EntityId, &SightSampleRecord)> =
        BTreeMap::new();
    for (entity, record) in &carry.records {
        let key = (record.perceiver, record.zone, record.target);
        if !carry.persisting.contains(&record.perceiver) || existing.contains_key(&key) {
            doomed.push(*entity);
            continue;
        }
        existing.insert(key, (*entity, record));
    }
    let mut merged: Vec<SightSample> = fresh.to_vec();
    for (key, (entity, record)) in &existing {
        if served(&key.0) {
            if !wanted.contains_key(key) {
                doomed.push(*entity);
            }
        } else if world.is_alive(record.target) {
            merged.push(sample_of(record));
        }
    }
    merged.sort_by_key(|s| (s.perceiver, s.zone, s.target));

    doomed.sort();
    let _ = world.despawn_many(&doomed);
    for (key, record) in wanted {
        match existing.get(&key) {
            Some((entity, old)) => {
                if **old != record
                    && let Ok(Some(stored)) = world.get_mut::<SightSampleRecord>(*entity)
                {
                    *stored = record;
                }
            }
            None => {
                let entity = world.spawn();
                let _ = world.insert(entity, record);
            }
        }
    }
    for id in rotation
        .served
        .iter()
        .filter(|id| carry.persisting.contains(id))
    {
        match world.get_mut::<PerceiverStatus>(*id) {
            Ok(Some(status)) => status.evaluated_tick = now,
            _ => {
                let _ = world.insert(
                    *id,
                    PerceiverStatus {
                        evaluated_tick: now,
                    },
                );
            }
        }
    }
    for id in &rotation.deferred {
        if carry.persisting.contains(id)
            && !carry.status.contains_key(id)
            && let Ok(at) = perceivers.binary_search_by_key(id, |(p, _)| *p)
        {
            let period = u64::from(perceivers[at].1.period_ticks.max(1));
            let _ = world.insert(
                *id,
                PerceiverStatus {
                    evaluated_tick: now.saturating_sub(period),
                },
            );
        }
    }
    for entity in carry.status.keys() {
        if !carry.persisting.contains(entity) && world.is_alive(*entity) {
            let _ = world.remove::<PerceiverStatus>(*entity);
        }
    }
    if rotation.cursor != carry.cursor {
        let state = state_entity(world);
        if let Ok(Some(state)) = world.get_mut::<PerceptionState>(state) {
            state.sight_cursor = rotation.cursor;
        }
    }
    Some(merged)
}

/// Queues `L4221` when a deferred perceiver has waited more than four of its periods.
fn note_starved(
    perceivers: &[(EntityId, Perceiver)],
    carry: &Carry,
    rotation: &Rotation,
    now: u64,
    reports: &mut Vec<(String, Diagnostic)>,
) {
    for id in &rotation.deferred {
        let Some(last) = carry.status.get(id) else {
            continue;
        };
        let period = perceivers
            .binary_search_by_key(id, |(p, _)| *p)
            .map_or(1, |at| perceivers[at].1.period_ticks.max(1));
        let waited = now.saturating_sub(*last);
        if waited > 4 * u64::from(period) {
            reports.push((
                "perception/deferred".to_string(),
                Diagnostic::new(
                    code::PERCEPTION_DEFERRED,
                    format!(
                        "perceiver {id:?} has not been evaluated for {waited} ticks (period {period}): `max_sight_rays_per_tick` is too small for the load"
                    ),
                )
                .with_help(
                    "raise `PerceptionConfig.max_sight_rays_per_tick` (2048 is recommended) or the `period_ticks` of idle perceivers; see `ludion docs perception`",
                ),
            ));
            return;
        }
    }
}

/// The `perception` key of `engine.describe`: the rays the last tick cast and the perceivers it
/// deferred. `None` until `perception_update` has run, so a world without perception describes
/// byte-identically to one before this key existed.
pub(crate) fn describe_section(world: &World) -> Option<Value> {
    let results = world.resource::<PerceptionResults>()?;
    Some(json!({
        "rays_cast": results.rays_cast,
        "deferred": results.deferred,
    }))
}

/// A stimulus a game reports to a perceiver's memory: hearing, or a custom sense.
///
/// # Example
/// ```
/// use ludion_engine::{Sense, Stimulus};
///
/// let noise = Stimulus { sense: Sense::Hearing, source: None, at: [4.0, 0.0, -2.0], strength: 0.5 };
/// assert_eq!(noise.sense, Sense::Hearing);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stimulus {
    /// The channel.
    pub sense: Sense,
    /// What made it; `None` for an anonymous one, which merges per `(perceiver, sense)`.
    pub source: Option<EntityId>,
    /// Where it came from, world metres (finite).
    pub at: [f32; 3],
    /// How strong it was (finite).
    pub strength: f32,
}

/// `(perceiver, sense, source, kind)`: the identity of one memory record. `None` sorts first. The
/// noise kind is part of the identity only for an anonymous record, and is empty otherwise.
type RecordKey = (EntityId, Sense, Option<EntityId>, String);

fn record_key(record: &StimulusRecord) -> RecordKey {
    let kind = if record.source.is_none() {
        record.kind.clone()
    } else {
        String::new()
    };
    (record.perceiver, record.sense, record.source, kind)
}

/// One sensing to fold into memory: from a sight sample, or a delivered noise.
struct Refresh {
    perceiver: EntityId,
    sense: Sense,
    source: Option<EntityId>,
    kind: String,
    at: [f32; 3],
    strength: f32,
    zone: u8,
}

/// A record as stored, and as it will be after the tick's pass.
struct Slot {
    entity: Option<EntityId>,
    before: Option<StimulusRecord>,
    record: StimulusRecord,
}

/// Seconds to whole ticks at `hz`, rounding up with slack for the `f32` representation error (the
/// rule `ludion-ai`'s timers use): `0.3` s at 60 Hz is 18 ticks, not 19.
fn seconds_to_ticks(seconds: f32, hz: u32) -> u64 {
    let x = f64::from(seconds.max(0.0)) * f64::from(hz);
    (x - x * 1e-6).ceil() as u64
}

/// Ticks of silence after which a record of `sense` is forgotten; at least 1, so a record sensed this
/// tick is never forgotten on it.
fn forget_ticks(memory: &PerceiverMemory, sense: Sense, hz: u32) -> u64 {
    let seconds = match sense {
        Sense::Sight => memory.forget_sight_s,
        Sense::Hearing | Sense::Custom(_) => memory.forget_hearing_s,
    };
    seconds_to_ticks(seconds, hz).max(1)
}

/// The keys of the records to evict so `records` fits `max`: least recently sensed first, ties by
/// `(sense, source)` ascending.
fn evictions(records: &[(RecordKey, u64)], max: usize) -> Vec<RecordKey> {
    let mut order: Vec<&(RecordKey, u64)> = records.iter().collect();
    order.sort_by(|a, b| {
        let rank = |(key, last): &(RecordKey, u64)| (*last, key.1, key.2, key.3.clone());
        rank(a).cmp(&rank(b))
    });
    order
        .into_iter()
        .take(records.len().saturating_sub(max))
        .map(|(key, _)| key.clone())
        .collect()
}

/// The memory pass of [`perception_update`]: orphans despawn, visible targets refresh or create
/// their sight record, `sensed_now` follows `last_sensed_tick`, aged records are forgotten, each
/// perceiver is trimmed to `max_records`, and new records spawn in key order. `hearings` are the
/// tick's noise deliveries, folded in after sight. A world with no memory and no records is not
/// touched.
fn update_memory(
    world: &mut World,
    now: u64,
    hz: u32,
    samples: &[SightSample],
    hearings: Vec<Refresh>,
) {
    let memories: BTreeMap<EntityId, PerceiverMemory> = world
        .query_ids::<(&Perceiver,)>()
        .filter_map(|(id, (p,))| p.memory.map(|m| (id, m)))
        .collect();
    let mut stored: Vec<(EntityId, StimulusRecord)> = world
        .query_ids::<(&StimulusRecord,)>()
        .map(|(id, (r,))| (id, r.clone()))
        .collect();
    if memories.is_empty() && stored.is_empty() {
        return;
    }
    stored.sort_by_key(|(id, _)| *id);

    let mut doomed: Vec<EntityId> = Vec::new();
    let mut slots: BTreeMap<RecordKey, Slot> = BTreeMap::new();
    for (entity, record) in stored {
        let key = record_key(&record);
        if !memories.contains_key(&record.perceiver) || slots.contains_key(&key) {
            doomed.push(entity);
            continue;
        }
        slots.insert(
            key,
            Slot {
                entity: Some(entity),
                before: Some(record.clone()),
                record,
            },
        );
    }

    // Samples sort by (perceiver, zone, target), so the first visible one of a pair has the lowest zone.
    let mut seen: BTreeMap<(EntityId, EntityId), u8> = BTreeMap::new();
    for sample in samples
        .iter()
        .filter(|s| s.visible && memories.contains_key(&s.perceiver))
    {
        seen.entry((sample.perceiver, sample.target))
            .or_insert(sample.zone);
    }
    let mut refreshes: Vec<Refresh> = Vec::new();
    for ((perceiver, target), zone) in seen {
        let Some(at) = pose(world, target).map(|p| p.origin) else {
            continue;
        };
        refreshes.push(Refresh {
            perceiver,
            sense: Sense::Sight,
            source: Some(target),
            kind: String::new(),
            at,
            strength: 1.0,
            zone,
        });
    }
    refreshes.extend(
        hearings
            .into_iter()
            .filter(|h| memories.contains_key(&h.perceiver)),
    );
    for refresh in refreshes {
        let Refresh {
            perceiver,
            sense,
            source,
            kind,
            at,
            strength,
            zone,
        } = refresh;
        let key = (
            perceiver,
            sense,
            source,
            if source.is_none() {
                kind.clone()
            } else {
                String::new()
            },
        );
        let slot = slots.entry(key).or_insert_with(|| Slot {
            entity: None,
            before: None,
            record: StimulusRecord {
                perceiver,
                source,
                sense,
                last_position: at,
                last_direction: [0.0; 3],
                first_sensed_tick: now,
                last_sensed_tick: now,
                sensed_now: true,
                strength,
                zone,
                kind: kind.clone(),
            },
        });
        let record = &mut slot.record;
        if sense == Sense::Sight {
            let moved = sub(at, record.last_position);
            let distance = length(moved);
            if distance > 1e-4 {
                record.last_direction = [
                    moved[0] / distance,
                    moved[1] / distance,
                    moved[2] / distance,
                ];
            }
        }
        record.last_position = at;
        record.last_sensed_tick = now;
        record.strength = strength;
        record.zone = zone;
        record.kind = kind;
    }

    for slot in slots.values_mut() {
        slot.record.sensed_now = slot.record.last_sensed_tick == now;
    }
    slots.retain(|(perceiver, sense, _, _), slot| {
        let age = now.saturating_sub(slot.record.last_sensed_tick);
        let forgotten = age >= forget_ticks(&memories[perceiver], *sense, hz);
        if forgotten && let Some(entity) = slot.entity {
            doomed.push(entity);
        }
        !forgotten
    });

    let listed: Vec<(RecordKey, u64)> = slots
        .iter()
        .map(|(key, slot)| (key.clone(), slot.record.last_sensed_tick))
        .collect();
    for group in listed.chunk_by(|a, b| a.0.0 == b.0.0) {
        let max = memories[&group[0].0.0].max_records as usize;
        for key in evictions(group, max) {
            if let Some(Slot {
                entity: Some(entity),
                ..
            }) = slots.remove(&key)
            {
                doomed.push(entity);
            }
        }
    }

    doomed.sort();
    let _ = world.despawn_many(&doomed);
    for slot in slots.into_values() {
        match slot.entity {
            Some(entity) if slot.before.as_ref() != Some(&slot.record) => {
                if let Ok(Some(stored)) = world.get_mut::<StimulusRecord>(entity) {
                    *stored = slot.record;
                }
            }
            Some(_) => {}
            None => {
                let entity = world.spawn();
                let _ = world.insert(entity, slot.record);
            }
        }
    }
}

/// Refreshes or creates `perceiver`'s record of `stimulus` now, so a game can feed hearing or a
/// custom sense (sight feeds itself). The record is keyed `(perceiver, sense, source)`; an anonymous
/// stimulus (`source: None`) merges per `(perceiver, sense)`. It sets `sensed_now`, `last_position`,
/// `strength` and `last_sensed_tick`, leaves `last_direction` alone, and trims the perceiver to its
/// `max_records`. A dead perceiver, one with no `memory`, or a non-finite `at` or `strength` drops the
/// stimulus and reports `L1574` once per cause.
///
/// # Example
/// ```
/// use ludion_components::{Perceiver, PerceiverMemory, Sense, SightZone};
/// use ludion_core::time::{FrameIndex, TickRate};
/// use ludion_ecs::{SystemCtx, World};
/// use ludion_engine::{Stimulus, report_stimulus, stimuli};
///
/// let mut world = World::new();
/// let guard = world.spawn();
/// let zone = SightZone {
///     name: String::new(), range: 5.0, half_angle_deg: 60.0, vertical_half_angle_deg: 45.0,
///     detects: vec![], see_through: Default::default(), see_through_max_range: 0.0,
///     range_by_visibility: false, visibility_clamp: None,
/// };
/// let perceiver = Perceiver {
///     enabled: true, eye: [0.0; 3], look: Default::default(), sight: vec![zone], sweep: None,
///     memory: Some(PerceiverMemory::default()), hearing: None, period_ticks: 1,
/// };
/// world.insert(guard, perceiver).unwrap();
///
/// let ctx = SystemCtx::new(FrameIndex::new(30), TickRate::DEFAULT);
/// let bang = Stimulus { sense: Sense::Hearing, source: None, at: [3.0, 0.0, 1.0], strength: 1.0 };
/// report_stimulus(&mut world, &ctx, guard, bang);
/// let heard = stimuli(&mut world, guard);
/// assert_eq!(heard.len(), 1);
/// assert!(heard[0].1.sensed_now && heard[0].1.last_position == [3.0, 0.0, 1.0]);
/// ```
pub fn report_stimulus(
    world: &mut World,
    ctx: &SystemCtx,
    perceiver: EntityId,
    stimulus: Stimulus,
) {
    let cause = if !world.is_alive(perceiver) {
        Some(("dead", format!("perceiver {perceiver:?} is not alive")))
    } else if stimulus.at.iter().any(|c| !c.is_finite()) || !stimulus.strength.is_finite() {
        Some((
            "not_finite",
            "the stimulus `at` or `strength` is not finite".to_string(),
        ))
    } else {
        None
    };
    let memory = world
        .get::<Perceiver>(perceiver)
        .ok()
        .flatten()
        .and_then(|p| p.memory);
    let cause = cause.or_else(|| {
        memory.is_none().then(|| {
            (
                "no_memory",
                format!("perceiver {perceiver:?} has no `Perceiver` with `memory`"),
            )
        })
    });
    if let Some((key, message)) = cause {
        let diagnostic = Diagnostic::new(
            code::STIMULUS_REPORT_INVALID,
            format!("`report_stimulus` dropped a stimulus: {message}"),
        )
        .with_help(
            "give the perceiver `memory:` and pass finite numbers; see `ludion docs perception`",
        );
        report_full(world, format!("stimulus:{key}"), diagnostic);
        return;
    }
    let Some(memory) = memory else { return };
    let now = ctx.game_frame.as_u64();
    let mine: Vec<(EntityId, StimulusRecord)> = world
        .query_ids::<(&StimulusRecord,)>()
        .filter(|(_, (r,))| r.perceiver == perceiver)
        .map(|(id, (r,))| (id, r.clone()))
        .collect();
    let key: RecordKey = (perceiver, stimulus.sense, stimulus.source, String::new());
    let existing = mine
        .iter()
        .filter(|(_, r)| record_key(r) == key)
        .map(|(id, _)| *id)
        .min();
    let mut listed: Vec<(RecordKey, u64)> = mine
        .iter()
        .map(|(_, r)| (record_key(r), r.last_sensed_tick))
        .collect();
    match existing {
        Some(entity) => {
            if let Ok(Some(record)) = world.get_mut::<StimulusRecord>(entity) {
                record.last_position = stimulus.at;
                record.last_sensed_tick = now;
                record.sensed_now = true;
                record.strength = stimulus.strength;
            }
            listed.retain(|(k, _)| *k != key);
        }
        None => {
            let entity = world.spawn();
            let _ = world.insert(
                entity,
                StimulusRecord {
                    perceiver,
                    source: stimulus.source,
                    sense: stimulus.sense,
                    last_position: stimulus.at,
                    last_direction: [0.0; 3],
                    first_sensed_tick: now,
                    last_sensed_tick: now,
                    sensed_now: true,
                    strength: stimulus.strength,
                    zone: 0,
                    kind: String::new(),
                },
            );
        }
    }
    listed.push((key, now));
    let doomed: Vec<EntityId> = evictions(&listed, memory.max_records as usize)
        .into_iter()
        .filter_map(|evicted| {
            world
                .query_ids::<(&StimulusRecord,)>()
                .find(|(_, (r,))| record_key(r) == evicted)
                .map(|(id, _)| id)
        })
        .collect();
    let _ = world.despawn_many(&doomed);
}

/// `perceiver`'s remembered records with their entities, sorted by `(sense, source)` (`None` first).
/// Read `last_position` of one whose `sensed_now` is `false` to search where it was last sensed.
///
/// # Example
/// ```
/// use ludion_ecs::World;
/// use ludion_engine::stimuli;
///
/// let mut world = World::new();
/// let nobody = world.spawn();
/// assert!(stimuli(&mut world, nobody).is_empty());
/// ```
pub fn stimuli(world: &mut World, perceiver: EntityId) -> Vec<(EntityId, StimulusRecord)> {
    let mut found: Vec<(EntityId, StimulusRecord)> = world
        .query_ids::<(&StimulusRecord,)>()
        .filter(|(_, (r,))| r.perceiver == perceiver)
        .map(|(id, (r,))| (id, r.clone()))
        .collect();
    found.sort_by_key(|(id, r)| (r.sense, r.source, *id));
    found
}

/// Forgets every record `perceiver` holds of `source` (`None` is the anonymous ones), across senses.
///
/// # Example
/// ```
/// use ludion_ecs::World;
/// use ludion_engine::{forget, stimuli};
///
/// let mut world = World::new();
/// let guard = world.spawn();
/// forget(&mut world, guard, None);
/// assert!(stimuli(&mut world, guard).is_empty());
/// ```
pub fn forget(world: &mut World, perceiver: EntityId, source: Option<EntityId>) {
    let doomed: Vec<EntityId> = stimuli(world, perceiver)
        .into_iter()
        .filter(|(_, r)| r.source == source)
        .map(|(id, _)| id)
        .collect();
    let _ = world.despawn_many(&doomed);
}

/// Forgets everything `perceiver` remembers.
///
/// # Example
/// ```
/// use ludion_ecs::World;
/// use ludion_engine::{forget_all, stimuli};
///
/// let mut world = World::new();
/// let guard = world.spawn();
/// forget_all(&mut world, guard);
/// assert!(stimuli(&mut world, guard).is_empty());
/// ```
pub fn forget_all(world: &mut World, perceiver: EntityId) {
    let doomed: Vec<EntityId> = stimuli(world, perceiver)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    let _ = world.despawn_many(&doomed);
}

/// Queues `noise` for [`perception_update`] to deliver to every listener (a [`Perceiver`] with
/// `hearing`) that hears its kind within reach, and returns its [`NoiseId`]. A noise emitted before
/// the update in a tick is delivered that tick, one emitted after it the next. The reach of one
/// listener is `radius × NoiseZone multiplier × surface multiplier × sensitivity ×
/// occlusion_factor^occluders`. Pending noises are entities, so a snapshot between emission and
/// delivery loses nothing.
///
/// Returns `None` when nothing was queued: a game with no perception config (silently), a
/// non-finite or negative `at`, `radius` or `loudness` (`L1577`, once), or an undeclared `kind`
/// (`L1572`, once per name).
///
/// # Example
/// ```
/// use ludion_core::time::{FrameIndex, TickRate};
/// use ludion_ecs::{SystemCtx, World};
/// use ludion_engine::{Noise, PerceptionConfig, emit_noise};
///
/// let mut world = World::new();
/// world.insert_resource(PerceptionConfig {
///     noise_kinds: vec!["footstep".into()],
///     ..Default::default()
/// });
/// let ctx = SystemCtx::new(FrameIndex::new(0), TickRate::DEFAULT);
/// let step = Noise {
///     at: [1.0, 0.0, 0.0], radius: 6.0, loudness: 1.0, kind: "footstep".into(),
///     source: None, surface: None, occlusion: false,
/// };
/// assert!(emit_noise(&mut world, &ctx, step.clone()).is_some());
/// let bang = Noise { kind: "bang".into(), ..step };
/// assert!(emit_noise(&mut world, &ctx, bang).is_none());
/// ```
pub fn emit_noise(world: &mut World, _ctx: &SystemCtx, noise: Noise) -> Option<NoiseId> {
    let config = world.resource::<PerceptionConfig>()?;
    let kind_declared = config.noise_kinds.contains(&noise.kind);
    let near = nearest(&noise.kind, config.noise_kinds.iter().map(String::as_str))
        .map(|n| format!("did you mean `{n}`? "))
        .unwrap_or_default();
    let numbers = noise
        .at
        .iter()
        .chain([&noise.radius, &noise.loudness])
        .all(|n| n.is_finite())
        && noise.radius >= 0.0
        && noise.loudness >= 0.0;
    if !numbers {
        let diagnostic = Diagnostic::new(
            code::NOISE_INVALID,
            "`emit_noise` dropped a noise: `at`, `radius` or `loudness` is not finite or is negative",
        )
        .with_help("pass finite, non-negative numbers; see `ludion docs perception`");
        report_full(world, "noise/not_finite".to_string(), diagnostic);
        return None;
    }
    if !kind_declared {
        let diagnostic = Diagnostic::new(
            code::PERCEPTION_NAME_UNKNOWN,
            format!(
                "noise kind `{}` is not declared in the `PerceptionConfig`; the noise was dropped",
                noise.kind
            ),
        )
        .with_help(format!(
            "{near}declare noise kinds in `PerceptionConfig.noise_kinds`"
        ));
        report_full(world, format!("noise/kind:{}", noise.kind), diagnostic);
        return None;
    }
    let state = state_entity(world);
    let id = world
        .get_mut::<PerceptionState>(state)
        .ok()
        .flatten()
        .map(|s| {
            let id = s.next_noise_id;
            s.next_noise_id += 1;
            id
        })?;
    let entity = world.spawn();
    let _ = world.insert(
        entity,
        PendingNoise {
            id,
            at: noise.at,
            radius: noise.radius,
            loudness: noise.loudness,
            kind: noise.kind,
            source: noise.source,
            surface: noise.surface.map(|s| s.0),
            occlusion: noise.occlusion,
        },
    );
    Some(NoiseId(id))
}

/// The [`PerceptionState`] singleton's entity: the lowest one, spawned on first use.
fn state_entity(world: &mut World) -> EntityId {
    let existing = world
        .query_ids::<(&PerceptionState,)>()
        .map(|(id, _)| id)
        .min();
    existing.unwrap_or_else(|| {
        let entity = world.spawn();
        let _ = world.insert(entity, PerceptionState::default());
        entity
    })
}

/// What the hearing pass produced.
#[derive(Default)]
struct HearingPass {
    heard: Vec<HeardNoise>,
    noises: Vec<NoiseRecord>,
    refreshes: Vec<Refresh>,
}

/// A [`NoiseZone`] with its world pose, for containment tests.
struct ZoneAt {
    zone: NoiseZone,
    origin: [f32; 3],
    rotation: [f32; 4],
}

/// Whether `point` is inside `shape` in the pose `origin`/`rotation` (scale is ignored). Boundaries
/// are inclusive.
fn zone_contains(
    shape: &NoiseZoneShape,
    origin: [f32; 3],
    rotation: [f32; 4],
    point: [f32; 3],
) -> bool {
    let delta = sub(point, origin);
    match shape {
        NoiseZoneShape::Sphere { radius } => length(delta) <= *radius,
        NoiseZoneShape::Box { half_extents } => {
            let conjugate = [-rotation[0], -rotation[1], -rotation[2], rotation[3]];
            let local = math::quat_rotate(conjugate, delta);
            (0..3).all(|i| local[i].abs() <= half_extents[i])
        }
    }
}

/// The multiplier of the zone that governs a noise of `kind` at `at`: among the zones containing
/// `at` whose `kinds` include `kind` (empty means all), the highest `priority`, ties to the first in
/// `zones` (sorted by entity). Multipliers never stack; no zone is ×1.
fn zone_multiplier(zones: &[ZoneAt], at: [f32; 3], kind: &str) -> f32 {
    let mut best: Option<&ZoneAt> = None;
    for zone in zones {
        let matches = zone.zone.kinds.is_empty() || zone.zone.kinds.iter().any(|k| k == kind);
        if matches
            && zone_contains(&zone.zone.shape, zone.origin, zone.rotation, at)
            && best.is_none_or(|b| zone.zone.priority > b.zone.priority)
        {
            best = Some(zone);
        }
    }
    best.map_or(1.0, |z| z.zone.multiplier)
}

/// The hearing pass of [`perception_update`]: every pending noise, in id order, is delivered to each
/// enabled listener that hears its kind within reach (listeners ascending), then despawned. A world
/// with no pending noise is not touched.
fn hear(
    world: &mut World,
    config: &PerceptionConfig,
    reports: &mut Vec<(String, Diagnostic)>,
) -> HearingPass {
    let mut pending: Vec<(EntityId, PendingNoise)> = world
        .query_ids::<(&PendingNoise,)>()
        .map(|(id, (n,))| (id, n.clone()))
        .collect();
    if pending.is_empty() {
        return HearingPass::default();
    }
    pending.sort_by_key(|(entity, n)| (n.id, *entity));
    let mut zone_ids: Vec<(EntityId, NoiseZone)> = world
        .query_ids::<(&NoiseZone,)>()
        .map(|(id, (z,))| (id, z.clone()))
        .collect();
    zone_ids.sort_by_key(|(id, _)| *id);
    let zones: Vec<ZoneAt> = zone_ids
        .into_iter()
        .filter_map(|(id, zone)| {
            let pose = pose(world, id)?;
            Some(ZoneAt {
                zone,
                origin: pose.origin,
                rotation: pose.rotation,
            })
        })
        .collect();
    let mut listener_ids: Vec<(EntityId, ludion_components::Hearing, bool)> = world
        .query_ids::<(&Perceiver,)>()
        .filter(|(_, (p,))| p.enabled)
        .filter_map(|(id, (p,))| Some((id, p.hearing.clone()?, p.memory.is_some())))
        .collect();
    listener_ids.sort_by_key(|(id, _, _)| *id);
    let listeners: Vec<(EntityId, ludion_components::Hearing, bool, [f32; 3])> = listener_ids
        .into_iter()
        .filter_map(|(id, hearing, memory)| {
            let ear = pose(world, id)?.with_eye(hearing.ear).origin;
            Some((id, hearing, memory, ear))
        })
        .collect();

    let mut out = HearingPass::default();
    {
        let physics = world.resource::<PhysicsWorld>();
        let occludes = !config.sound_occluder_layers.is_empty();
        for (_, noise) in &pending {
            if !config.noise_kinds.contains(&noise.kind) {
                continue;
            }
            let surface = noise
                .surface
                .and_then(|s| physics.and_then(|p| p.surface_name(SurfaceId(s))))
                .and_then(|name| config.surface_noise.iter().find(|e| e.surface == name))
                .map_or(1.0, |e| e.multiplier);
            let radius = noise.radius * zone_multiplier(&zones, noise.at, &noise.kind) * surface;
            out.noises.push(NoiseRecord {
                id: NoiseId(noise.id),
                at: noise.at,
                kind: noise.kind.clone(),
                loudness: noise.loudness,
                source: noise.source,
                effective_radius: radius,
            });
            for (listener, hearing, memory, ear) in &listeners {
                if !hearing.hears.contains(&noise.kind) {
                    continue;
                }
                let reach = radius * hearing.sensitivity;
                let to_ear = sub(*ear, noise.at);
                let distance = length(to_ear);
                if distance > reach {
                    continue;
                }
                let mut effective = reach;
                let mut occluders = 0u8;
                if noise.occlusion
                    && occludes
                    && distance > 1e-6
                    && let Some(physics) = physics
                {
                    let filter = QueryFilter::new().include_set(&config.sound_occluder_layers);
                    let hits = physics.ray_cast(noise.at, to_ear, distance, &filter);
                    let hits = hits.unwrap_or_else(|error| {
                        reports.push((
                            "noise/occluder_layers".to_string(),
                            Diagnostic::new(
                                code::PERCEPTION_NAME_UNKNOWN,
                                format!(
                                    "perception `sound_occluder_layers` is not usable: {error}"
                                ),
                            )
                            .with_help("declare the layers under `physics.layers`"),
                        ));
                        Vec::new()
                    });
                    let mut blockers: Vec<EntityId> = hits
                        .iter()
                        .map(|h| h.entity)
                        .filter(|e| Some(*e) != noise.source && e != listener)
                        .collect();
                    blockers.sort();
                    blockers.dedup();
                    let count = blockers.len().min(config.max_occluders as usize);
                    for _ in 0..count {
                        effective *= config.occlusion_factor;
                    }
                    occluders = count as u8;
                }
                if distance > effective {
                    continue;
                }
                out.heard.push(HeardNoise {
                    listener: *listener,
                    noise: NoiseId(noise.id),
                    at: noise.at,
                    kind: noise.kind.clone(),
                    loudness: noise.loudness,
                    source: noise.source,
                    distance,
                    effective_radius: effective,
                    occluders,
                });
                if *memory {
                    out.refreshes.push(Refresh {
                        perceiver: *listener,
                        sense: Sense::Hearing,
                        source: noise.source,
                        kind: noise.kind.clone(),
                        at: noise.at,
                        strength: noise.loudness,
                        zone: 0,
                    });
                }
            }
        }
    }
    out.heard.sort_by_key(|h| (h.listener, h.noise));
    let consumed: Vec<EntityId> = pending.iter().map(|(entity, _)| *entity).collect();
    let _ = world.despawn_many(&consumed);
    out
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
    /// Also returns the rays cast, a re-cast after a self-hit included.
    fn finish(&self, mut drafts: Vec<Draft>) -> (Vec<SightSample>, u32) {
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
        let mut cast_count = 0u32;
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
                cast_count += sweeps.len() as u32;
                for (&at, hit) in members.iter().zip(hits) {
                    let ray = &rays[at];
                    // The eye inside the perceiver's own blocking collider is not a blocker.
                    let hit = match hit {
                        Some(h) if h.entity == ray.perceiver => {
                            cast_count += 1;
                            physics.first_hit_skipping(
                                &ray.sweep,
                                &filter,
                                &[ray.perceiver, ray.target],
                            )
                        }
                        other => other,
                    };
                    outcomes[at] = hit.map_or(Cast::Clear, |h| Cast::Blocked(Some(h.entity)));
                }
            }
        }
        let samples = drafts
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
            .collect();
        (samples, cast_count)
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
            PerceptionConfig {
                classes,
                ..Default::default()
            }
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
                classes: (0..32).map(|n| n.to_string()).collect(),
                ..Default::default()
            }
            .validate()
            .is_ok()
        );
    }

    fn id(index: u32) -> EntityId {
        EntityId::new(index, 0)
    }

    #[test]
    fn a_never_evaluated_perceiver_waits_for_its_phase_and_an_evaluated_one_for_its_period() {
        assert!(is_due(4, 1, 7, None));
        assert!(is_due(4, 3, 7, None) && !is_due(4, 3, 8, None));
        assert!(!is_due(4, 3, 8, Some(7)) && is_due(4, 3, 10, Some(7)));
        assert!(is_due(0, 3, 9, Some(1)));
        assert!(
            !is_due(0, 3, 0, Some(5)),
            "a restored-earlier clock is not due"
        );
        assert!(is_due(0, 0, 3, None), "a zero period is read as 1");
    }

    #[test]
    fn the_rotation_starts_after_the_cursor_and_stops_at_the_first_misfit() {
        let due = [(id(1), 4), (id(2), 4), (id(3), 4), (id(4), 4)];
        let none = plan_rotation(&due, None, None);
        assert_eq!((none.served.len(), none.deferred.len()), (4, 0));
        assert_eq!(none.cursor, None);

        let first = plan_rotation(&due, None, Some(8));
        assert_eq!(first.served, [id(1), id(2)]);
        assert_eq!(first.deferred, [id(3), id(4)]);
        assert_eq!(first.cursor, Some(id(2)));

        let wrapped = plan_rotation(&due, Some(id(3)), Some(8));
        assert_eq!(wrapped.served, [id(1), id(4)]);
        assert_eq!(wrapped.deferred, [id(2), id(3)]);
        assert_eq!(wrapped.cursor, Some(id(1)));
    }

    #[test]
    fn the_first_perceiver_is_served_over_budget_and_a_misfit_blocks_later_small_ones() {
        let due = [(id(1), 50), (id(2), 1)];
        let big = plan_rotation(&due, None, Some(10));
        assert_eq!(big.served, [id(1)]);
        assert_eq!(big.deferred, [id(2)]);
        let unfair = [(id(1), 1), (id(2), 9), (id(3), 1)];
        let stopped = plan_rotation(&unfair, None, Some(5));
        assert_eq!(stopped.served, [id(1)]);
        assert_eq!(stopped.deferred, [id(2), id(3)]);
        assert_eq!(plan_rotation(&[], Some(id(1)), Some(5)).cursor, Some(id(1)));
    }

    #[test]
    fn a_config_refuses_bad_noise_settings() {
        let path = |edit: &dyn Fn(&mut PerceptionConfig)| {
            let mut config = PerceptionConfig {
                noise_kinds: vec!["footstep".into()],
                ..Default::default()
            };
            edit(&mut config);
            config.validate().unwrap_err().path.unwrap_or_default()
        };
        assert_eq!(
            path(&|c| c.noise_kinds = (0..33).map(|n| n.to_string()).collect()),
            "/noise_kinds"
        );
        assert_eq!(
            path(&|c| c.noise_kinds.push("footstep".into())),
            "/noise_kinds/1"
        );
        let entry = |surface: &str, multiplier: f32| SurfaceNoise {
            surface: surface.into(),
            multiplier,
        };
        assert_eq!(
            path(&|c| c.surface_noise = vec![entry("a", 1.0), entry("a", 1.0)]),
            "/surface_noise/1/surface"
        );
        assert_eq!(
            path(&|c| c.surface_noise = vec![entry("a", -1.0)]),
            "/surface_noise/0/multiplier"
        );
        assert_eq!(path(&|c| c.occlusion_factor = 1.5), "/occlusion_factor");
        assert_eq!(path(&|c| c.max_occluders = 9), "/max_occluders");
        assert_eq!(path(&|c| c.max_occluders = 0), "/max_occluders");
        assert_eq!(
            path(&|c| c.max_sight_rays_per_tick = Some(0)),
            "/max_sight_rays_per_tick"
        );
        let capped = PerceptionConfig {
            max_sight_rays_per_tick: Some(1),
            ..Default::default()
        };
        assert!(capped.validate().is_ok());
    }

    fn zone_at(shape: NoiseZoneShape, origin: [f32; 3], priority: i32, mult: f32) -> ZoneAt {
        ZoneAt {
            zone: NoiseZone {
                shape,
                multiplier: mult,
                kinds: Vec::new(),
                priority,
            },
            origin,
            rotation: [0.0, 0.0, 0.0, 1.0],
        }
    }

    #[test]
    fn zones_contain_inclusively_and_a_box_follows_its_rotation() {
        let identity = [0.0, 0.0, 0.0, 1.0];
        let sphere = NoiseZoneShape::Sphere { radius: 2.0 };
        assert!(zone_contains(
            &sphere,
            [1.0, 0.0, 0.0],
            identity,
            [3.0, 0.0, 0.0]
        ));
        assert!(!zone_contains(
            &sphere,
            [1.0, 0.0, 0.0],
            identity,
            [3.1, 0.0, 0.0]
        ));
        let slab = NoiseZoneShape::Box {
            half_extents: [4.0, 1.0, 1.0],
        };
        assert!(zone_contains(&slab, [0.0; 3], identity, [4.0, 1.0, -1.0]));
        assert!(!zone_contains(&slab, [0.0; 3], identity, [0.0, 0.0, 3.0]));
        // A quarter turn about +Y swings the long axis from X to -Z.
        let half = core::f32::consts::FRAC_1_SQRT_2;
        let turned = [0.0, half, 0.0, half];
        assert!(zone_contains(&slab, [0.0; 3], turned, [0.0, 0.0, 3.5]));
        assert!(!zone_contains(&slab, [0.0; 3], turned, [3.5, 0.0, 0.0]));
    }

    #[test]
    fn the_highest_priority_zone_wins_and_ties_go_to_the_first() {
        let big = NoiseZoneShape::Sphere { radius: 10.0 };
        let zones = [
            zone_at(big, [0.0; 3], 0, 0.5),
            zone_at(big, [0.0; 3], 0, 0.25),
            zone_at(big, [0.0; 3], 3, 2.0),
            zone_at(big, [50.0, 0.0, 0.0], 9, 7.0),
        ];
        assert_eq!(zone_multiplier(&zones, [0.0; 3], "x"), 2.0);
        assert_eq!(zone_multiplier(&zones[..2], [0.0; 3], "x"), 0.5);
        assert_eq!(zone_multiplier(&zones, [0.0, 20.0, 0.0], "x"), 1.0);
        let mut quiet = zone_at(big, [0.0; 3], 0, 0.1);
        quiet.zone.kinds = vec!["footstep".into()];
        assert_eq!(zone_multiplier(&[quiet], [0.0; 3], "bang"), 1.0);
    }
}

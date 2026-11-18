//! [`move_character`]: depenetration, collide-and-slide, ground probe and ceiling for a
//! [`CharacterMotor`](crate::CharacterMotor).

use std::cell::OnceCell;
use std::rc::Rc;

use ludion_core::ids::EntityId;
use ludion_core::math::{cos, sqrt};
use ludion_core::time::FrameIndex;
use ludion_ecs::World;
use ludion_spatial::ParentPose;
use rapier3d::prelude::{BoundingVolume, SharedShape};

use super::carry::{rigid_carry, rotate, rotate_yaw};
use super::component::{CharacterMotor, CharacterMotorState, PlatformPose, check, motor_in_force};
use super::gather::Gather;
use super::report::{GroundHit, GroundProbe, MotorError, MoveReport, PlatformCarry, SurfaceHit};
use super::surface::SurfaceVelocity;
use super::sweep::{Capsule, Contact};
use crate::pose::PhysicsPose;
use crate::queries::{QueryFilter, QueryHit};
use crate::sweep::SweepFilter;
use crate::world::{PhysicsWorld, to_pose};

/// Depenetration passes before the motor gives up and reports `stuck`.
const DEPENETRATION_PASSES: usize = 4;
/// A displacement shorter than this is not worth a cast.
const MIN_MOVE: f32 = 1e-5;
/// Height differences under this are float rounding of the ground ray, not motion.
const REST_NOISE: f32 = 1e-6;
/// The least a hit normal may oppose the motion for the skin back-off along the motion to stay
/// bounded.
const MIN_OPPOSING: f32 = 0.2;
/// Two wall normals of one entity closer than this are one wall: coplanar triangles of a mesh.
const WALL_SAME_PLANE: f32 = 0.999;

/// Reshapes the horizontal motion the motor applies: a game's lane, rail or plane lock.
///
/// [`project`](MoveConstraint::project) runs with the feet and the displacement the motor is about
/// to cast: the requested move and each slide iteration's remainder, and both platform carry passes.
/// Only its x and z are taken; vertical motion, depenetration and the ground probe are the motor's.
/// A constraint that needs the tick's `dt` reads [`PhysicsWorld::gameplay_dt`].
///
/// [`correction`](MoveConstraint::correction) is an optional capped pull (a lane's return to its
/// line), asked once per call after the requested move, only when that move is not stuck. It is a
/// separate pass because a pull folded into the displacement is projected away by the slide loop
/// after the first floor hit; the pass is locked to the pull's own axis so a pull into an angled
/// wall stops instead of creeping along it. [`corrected`](MoveConstraint::corrected) reports what
/// the pass achieved.
pub trait MoveConstraint {
    /// The step to cast instead of `step`, from `feet`.
    fn project(&mut self, feet: [f32; 3], step: [f32; 3]) -> [f32; 3];

    /// A horizontal displacement to sweep from `feet`, the pose the requested move left. Its y is
    /// dropped; a length under the motor's minimum move means none.
    fn correction(&mut self, _feet: [f32; 3]) -> [f32; 3] {
        [0.0; 3]
    }

    /// Called once after a correction was swept: the net feet change, and whether the travel along
    /// the correction fell short of its length (a wall or ceiling clipped it).
    fn corrected(&mut self, _applied: [f32; 3], _blocked: bool) {}
}

/// Keeps only the component of a step along `axis`, never backwards: the correction pass.
struct AxisLock {
    axis: [f32; 3],
}

impl MoveConstraint for AxisLock {
    fn project(&mut self, _feet: [f32; 3], step: [f32; 3]) -> [f32; 3] {
        let along = dot(step, self.axis).max(0.0);
        [self.axis[0] * along, step[1], self.axis[2] * along]
    }
}

/// Moves `entity`'s feet by `displacement` through the world: depenetrates, slides along what it
/// hits, probes the ground and writes the pose `P` and the [`CharacterMotorState`].
///
/// `P` is the game's position component (the engine's `Transform` for
/// `ludion_engine::move_character`). The kinematic capsule follows through the physics write pass,
/// so the character pushes dynamic props and nothing pushes it: rigid props are pushed, not
/// walked around; a pinned prop blocks. A rigid collider blocks as ground or ceiling, but a
/// wall-class hit on one does not stop the slide, and depenetration ignores it, unless the prop
/// itself cannot move on (a non-rigid collider is in its way): then it blocks like a static one.
/// Only the first prop is tested, so a row of crates against a wall can still squeeze.
///
/// # Platform carry
///
/// A motor that ended its last call grounded on a non-rigid support (the `static` or `kinematic`
/// collider, or another motor, in [`CharacterMotorState::ground`]) is carried once per tick, on
/// its first call of the tick: the support's translation and yaw about its pivot since the motor
/// last called, applied to the feet where the move left them (so a rider who walked on a turning
/// platform turns about the right point),
/// plus the support's [`SurfaceVelocity`] over the tick's gameplay time (nothing
/// while a `GameClock` hold froze it). The yaw turns the rider too. The carry is swept like a
/// move: it stops at walls and ceilings (reported in `walls`/`ceiling`), never steps up and is
/// shaped by a [`MoveConstraint`]. The surface displacement is swept against the support's own
/// collider too (a belt's end stop holds the rider); the rigid part ignores it. It is reported in
/// [`MoveReport::platform`] with its intended values even when blocked, whenever it ran, and
/// `moved` includes it. A game's takeoff momentum is `(translation + surface) / dt`.
///
/// **On a stuck tick the carry is not applied**: `platform` is the default, and the grounded,
/// ground and platform record is kept, so the next tick carries from where it left off.
///
/// **The carry is applied after the displacement's slide and ground probe, not before.** Casts
/// see colliders as of the last step, so the support's collider sits where the motor left it a
/// tick ago. Depenetrating, sliding and probing in that frame keeps the rider's relation to its
/// support exact; carrying first would put a rider on a descending platform inside the stale
/// collider, and depenetration would push it back up every tick.
///
/// **A rigid support is never carried.** Its collider is already at this tick's pose after the
/// step, so the stale-frame argument above does not hold and carrying would double-count the
/// platform's motion; `ground.entity` still names it. Dynamic bodies get no surface velocity.
///
/// Platforms move before the motor is called — a `PathFollower` already does; a platform moved
/// after the motor is carried next tick.
///
/// Casts see colliders as of the last step, and skip sensors ([`QueryFilter::include_sensors`] is
/// never set). The motor's own capsule and the layers its `layer` ignores are excluded.
///
/// This function draws nothing: the engine's wrapper draws the `motor` debug category.
///
/// Call it from a system registered **before** [`install`](crate::install)'s: the report is
/// stamped with the frame the physics write pass runs in next. The physics world does not exist
/// until the first write pass, so the first tick of a world returns [`MotorError::NoPhysicsWorld`].
///
/// ```no_run
/// # use ludion_ecs::{SystemCtx, World};
/// # use ludion_physics::{move_character, ParentPose, PhysicsPose};
/// # use ludion_core::ids::EntityId;
/// fn walk<P: PhysicsPose, R: ParentPose>(world: &mut World, _ctx: &SystemCtx, hero: EntityId) {
///     let gravity = -0.2;
///     if let Ok(report) = move_character::<P, R>(world, hero, [0.05, gravity, 0.0]) {
///         if report.landed { /* play the thud */ }
///     }
/// }
/// ```
pub fn move_character<P: PhysicsPose, R: ParentPose>(
    world: &mut World,
    entity: EntityId,
    displacement: [f32; 3],
) -> Result<MoveReport, MotorError> {
    move_character_with::<P, R>(world, entity, displacement, None)
}

/// [`move_character`] with a [`MoveConstraint`] shaping every horizontal displacement and adding
/// its correction.
/// The same ordering rule applies: call it before [`install`](crate::install)'s systems.
pub fn move_character_with<P: PhysicsPose, R: ParentPose>(
    world: &mut World,
    entity: EntityId,
    displacement: [f32; 3],
    constraint: Option<&mut dyn MoveConstraint>,
) -> Result<MoveReport, MotorError> {
    let prepared = prepare::<P, R>(world, entity, displacement)?;
    let (outcome, platform_now) = {
        let physics = world
            .resource::<PhysicsWorld>()
            .ok_or(MotorError::NoPhysicsWorld)?;
        let solver = Solver::new(&*world, physics, entity, &prepared.checked);
        let outcome = solver.solve(
            prepared.feet,
            displacement,
            prepared.was_grounded,
            constraint,
            prepared.carry.as_ref(),
        );
        let platform_now = platform_after::<P, R>(world, physics, &outcome);
        (outcome, platform_now)
    };
    Ok(apply::<P>(
        world,
        entity,
        prepared.tick,
        outcome,
        platform_now,
    ))
}

/// Everything a call reads before it solves: the validated motor, the feet and the carry due.
pub(super) struct Prepared {
    pub(super) feet: [f32; 3],
    pub(super) displacement: [f32; 3],
    pub(super) checked: CharacterMotor,
    pub(super) was_grounded: bool,
    pub(super) carry: Option<CarryPlan>,
    pub(super) tick: u64,
}

/// Reads and validates a call's inputs and plans its carry, in the order the errors are reported.
pub(super) fn prepare<P: PhysicsPose, R: ParentPose>(
    world: &World,
    entity: EntityId,
    displacement: [f32; 3],
) -> Result<Prepared, MotorError> {
    let motor = world
        .get::<CharacterMotor>(entity)
        .ok()
        .flatten()
        .cloned()
        .ok_or(MotorError::NoMotor(entity))?;
    let feet = world
        .get::<P>(entity)
        .ok()
        .flatten()
        .map(|pose| pose.translation())
        .ok_or(MotorError::NoPose(entity))?;
    if world
        .get::<CharacterMotorState>(entity)
        .ok()
        .flatten()
        .is_some_and(|state| state.suspended)
    {
        return Err(MotorError::Suspended(entity));
    }
    if !displacement.iter().all(|c| c.is_finite()) {
        return Err(MotorError::NonFiniteDisplacement(displacement));
    }
    let state = world.get::<CharacterMotorState>(entity).ok().flatten();
    let was_grounded = state.is_some_and(|state| state.grounded);
    let checked = motor_in_force(&motor, state);
    if let Err((pointer, reason)) = check(&checked) {
        return Err(MotorError::InvalidMotor {
            entity,
            reason: format!("{pointer}: {reason}"),
        });
    }
    let physics = world
        .resource::<PhysicsWorld>()
        .ok_or(MotorError::NoPhysicsWorld)?;
    let tick = physics.last_write_frame.map_or(0, |frame| frame + 1);
    let carry = plan_carry::<P, R>(world, physics, state, tick);
    Ok(Prepared {
        feet,
        displacement,
        checked,
        was_grounded,
        carry,
        tick,
    })
}

/// The support pose the motor stands on after `outcome`, `None` unless it ended grounded on a
/// non-rigid support with a pose.
pub(super) fn platform_after<P: PhysicsPose, R: ParentPose>(
    world: &World,
    physics: &PhysicsWorld,
    outcome: &Outcome,
) -> Option<PlatformPose> {
    outcome
        .report
        .ground
        .filter(|_| outcome.report.grounded)
        .filter(|hit| !physics.is_rigid(hit.entity))
        .and_then(|hit| world_pose::<P, R>(world, hit.entity))
        .map(|(translation, rotation)| PlatformPose {
            translation,
            rotation,
        })
}

/// Writes a solved call: the pose `P`, the [`CharacterMotorState`] and the physics world's move
/// report. Returns the report.
pub(super) fn apply<P: PhysicsPose>(
    world: &mut World,
    entity: EntityId,
    tick: u64,
    mut outcome: Outcome,
    platform_now: Option<PlatformPose>,
) -> MoveReport {
    let frame = FrameIndex::new(tick);
    outcome.report.posture = world
        .get::<CharacterMotorState>(entity)
        .ok()
        .flatten()
        .and_then(|state| state.posture.clone());

    if let Ok(Some(pose)) = world.get_mut::<P>(entity) {
        pose.set_translation(outcome.feet);
        let yaw = outcome.report.platform.yaw;
        if yaw != 0.0 {
            pose.set_rotation_quat(rotate_yaw(pose.rotation_quat(), yaw));
        }
    }
    let ground_entity = outcome.report.ground.filter(|_| outcome.report.grounded);
    let record = |state: &mut CharacterMotorState| {
        if outcome.carry_skipped {
            state.carry_tick = Some(tick);
            return;
        }
        state.grounded = outcome.report.grounded;
        state.ground = ground_entity.map(|hit| hit.entity);
        state.platform = platform_now;
        state.carry_tick = Some(tick);
    };
    if let Ok(Some(state)) = world.get_mut::<CharacterMotorState>(entity) {
        record(state);
    } else {
        let mut state = CharacterMotorState::default();
        record(&mut state);
        let _ = world.insert(entity, state);
    }
    if let Some(physics) = world.resource_mut::<PhysicsWorld>() {
        physics
            .move_reports
            .insert(entity, (frame, outcome.report.clone()));
    }
    outcome.report
}

/// How far below the feet `move_character`'s ground probe looks: `snap_distance + skin_width` when
/// the motor `was_grounded` and is not moving up, else `2 * skin_width`. The debug draw reads the
/// same rule the mover applies.
pub fn ground_probe_reach(
    motor: &CharacterMotor,
    was_grounded: bool,
    displacement: [f32; 3],
) -> f32 {
    probe_reach(
        motor.snap_distance,
        motor.skin_width,
        was_grounded && displacement[1] <= 0.0,
    )
}

/// Whether the motor on `entity` would stand at `feet`, within walk reach: the motor's own ground
/// probe at any feet position, cast from `step_height` above over `step_height + snap_distance +
/// skin_width`, so a ledge one step up or a drop of one snap counts. `Ok(None)` when nothing is
/// under it, or the support is out of reach (above the step height).
///
/// It is pure and deterministic, reads the last completed step, uses the crouched height in force,
/// does no headroom test (see [`fits_character`](crate::fits_character)), and filters out the
/// motor's ignored layers and sensors as a move does. It works on a suspended motor too.
///
/// ```no_run
/// # use ludion_ecs::{SystemCtx, World};
/// # use ludion_physics::{probe_character_ground, PhysicsPose};
/// # use ludion_core::ids::EntityId;
/// fn safe_to_land<P: PhysicsPose>(world: &World, _ctx: &SystemCtx, hero: EntityId) -> bool {
///     probe_character_ground::<P>(world, hero, [2.0, 0.0, 0.0])
///         .ok()
///         .flatten()
///         .is_some_and(|probe| probe.rest_y.is_some())
/// }
/// ```
pub fn probe_character_ground<P: PhysicsPose>(
    world: &World,
    entity: EntityId,
    feet: [f32; 3],
) -> Result<Option<GroundProbe>, MotorError> {
    let motor = world
        .get::<CharacterMotor>(entity)
        .ok()
        .flatten()
        .cloned()
        .ok_or(MotorError::NoMotor(entity))?;
    world
        .get::<P>(entity)
        .ok()
        .flatten()
        .ok_or(MotorError::NoPose(entity))?;
    if !feet.iter().all(|c| c.is_finite()) {
        return Err(MotorError::NonFiniteFeet(feet));
    }
    let state = world.get::<CharacterMotorState>(entity).ok().flatten();
    let checked = motor_in_force(&motor, state);
    if let Err((pointer, reason)) = check(&checked) {
        return Err(MotorError::InvalidMotor {
            entity,
            reason: format!("{pointer}: {reason}"),
        });
    }
    let physics = world
        .resource::<PhysicsWorld>()
        .ok_or(MotorError::NoPhysicsWorld)?;
    let solver = Solver::new(world, physics, entity, &checked);
    let lift = solver.step_height;
    let origin = [feet[0], feet[1] + lift, feet[2]];
    let reach = lift + solver.snap + solver.skin;
    let under = solver.radius + lift + solver.snap + solver.skin;
    let (ground, grounded, settled) = solver.probe(origin, reach, under, true);
    let Some(hit) = ground else {
        return Ok(None);
    };
    if solver.above_step(feet[1], hit.point[1]) {
        return Ok(None);
    }
    Ok(Some(GroundProbe {
        ground: GroundHit {
            distance: hit.distance - lift,
            ..hit
        },
        rest_y: grounded.then_some(settled[1]),
    }))
}

fn probe_reach(snap: f32, skin: f32, snap_allowed: bool) -> f32 {
    if snap_allowed {
        snap + skin
    } else {
        2.0 * skin
    }
}

/// The query filter a motor's casts and overlaps use: the motor's own capsule excluded and the
/// layers its `layer` ignores dropped. Sensors are skipped by the query default.
pub(crate) fn motor_filter(
    physics: &PhysicsWorld,
    entity: EntityId,
    motor: &CharacterMotor,
) -> QueryFilter {
    let mut filter = QueryFilter::new().exclude(entity).body_layers_only();
    for name in physics.ignored_by(motor.layer.as_deref()) {
        filter = filter.ignore_layer(name);
    }
    filter
}

/// `entity`'s world pose from the game's pose component `P`, `None` when it has none.
fn world_pose<P: PhysicsPose, R: ParentPose>(
    world: &World,
    entity: EntityId,
) -> Option<([f32; 3], [f32; 4])> {
    let pose = world.get::<P>(entity).ok().flatten()?;
    Some(R::world_pose(world, entity).unwrap_or_else(|| (pose.translation(), pose.rotation_quat())))
}

/// The carry a motor is due this call: it stood on a non-rigid support at the end of its last
/// call, has not been carried this tick, and the support still has a pose. The carry is that
/// support's motion since, applied rigidly to the feet, plus its surface velocity over the tick's
/// gameplay time. The poses are kept, not the translation: the feet they act on are the ones
/// after the slide and probe.
fn plan_carry<P: PhysicsPose, R: ParentPose>(
    world: &World,
    physics: &PhysicsWorld,
    state: Option<&CharacterMotorState>,
    tick: u64,
) -> Option<CarryPlan> {
    let state = state.filter(|state| state.grounded && state.carry_tick != Some(tick))?;
    let (ground, stored) = (state.ground?, state.platform?);
    if physics.is_rigid(ground) {
        return None;
    }
    let (translation, rotation) = world_pose::<P, R>(world, ground)?;
    let surface = world
        .get::<SurfaceVelocity>(ground)
        .ok()
        .flatten()
        .map(|belt| belt.velocity)
        .filter(|velocity| velocity.iter().all(|c| c.is_finite()))
        .map_or([0.0; 3], |velocity| {
            scale(rotate(rotation, velocity), physics.gameplay_dt(world))
        });
    Some(CarryPlan {
        entity: ground,
        from: stored,
        to: PlatformPose {
            translation,
            rotation,
        },
        surface,
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Ground,
    Wall,
    Ceiling,
}

pub(super) struct Outcome {
    feet: [f32; 3],
    pub(super) report: MoveReport,
    /// A carry was planned but the move was stuck, so it was not applied.
    carry_skipped: bool,
}

/// What a slide loop has recorded so far, across the displacement and the carry.
#[derive(Default)]
struct Slide {
    ceiling: Option<SurfaceHit>,
    stepped: Option<f32>,
    /// Each wall with the distance travelled when it was hit, for ordering.
    walls: Vec<(f32, SurfaceHit)>,
    travel: f32,
}

/// The platform carry decided at the start of a call: the support, its pose then and now, and its
/// surface displacement.
pub(super) struct CarryPlan {
    entity: EntityId,
    from: PlatformPose,
    to: PlatformPose,
    surface: [f32; 3],
}

/// Slack on the gather box beyond the move's own reach; a query outside the box falls back to the
/// BVH, so it sets speed, never results.
const GATHER_SLACK: f32 = 0.05;

/// Sorts `(dist, rank, entity)`: equal gaps fall to spawn rank, which unrelated entity churn leaves
/// alone, and `EntityId` is the last deterministic tie.
fn sort_by_gap_then_rank(contacts: &mut [Contact], rank: impl Fn(EntityId) -> u64) {
    contacts.sort_unstable_by(|a, b| {
        a.dist
            .total_cmp(&b.dist)
            .then_with(|| rank(a.entity).cmp(&rank(b.entity)))
            .then(a.entity.cmp(&b.entity))
    });
}

pub(super) struct Solver<'a> {
    physics: &'a PhysicsWorld,
    /// The ECS world, for the spawn rank that breaks equal-gap contact ties: the motor runs before
    /// the write, so the physics world's own rank table may not be filled yet.
    world: &'a World,
    filter: SweepFilter,
    /// The colliders around the solve's start, walked once; empty outside `solve`.
    gather: OnceCell<Rc<Gather>>,
    /// The motor's capsule, the skin-inflated one `contacts` queries and the wider one `refine`
    /// queries, each built once for the solve.
    shape: SharedShape,
    inflated: SharedShape,
    wide: SharedShape,
    height: f32,
    radius: f32,
    skin: f32,
    snap: f32,
    step_height: f32,
    max_slides: u32,
    cos_limit: f32,
}

impl<'a> Solver<'a> {
    pub(super) fn new(
        world: &'a World,
        physics: &'a PhysicsWorld,
        entity: EntityId,
        motor: &CharacterMotor,
    ) -> Self {
        let capsule = Capsule {
            half_height: motor.half_height_for(motor.height),
            radius: motor.radius,
        };
        let margin = 2.0 * motor.skin_width + 1e-3;
        let filter = physics
            .sweep_filter(&motor_filter(physics, entity, motor))
            .expect("a motor's ignored layers come from the layer table");
        Solver {
            physics,
            world,
            filter,
            gather: OnceCell::new(),
            shape: capsule.built(physics),
            inflated: Capsule {
                radius: motor.radius + motor.skin_width,
                ..capsule
            }
            .built(physics),
            wide: Capsule {
                radius: motor.radius + margin,
                ..capsule
            }
            .built(physics),
            height: motor.height,
            radius: motor.radius,
            skin: motor.skin_width,
            snap: motor.snap_distance,
            step_height: motor.step_height,
            max_slides: motor.max_slides,
            cos_limit: cos(motor.slope_limit_deg.to_radians()),
        }
    }

    /// The gather the solve's queries share, once `solve` has built it.
    fn gathered(&self) -> Option<&Gather> {
        self.gather.get().map(|gather| &**gather)
    }

    /// Walks the BVH once over the neighbourhood this move can reach from `start`.
    fn gather_around(&self, start: [f32; 3], displacement: [f32; 3]) {
        self.gather.get_or_init(|| {
            let reach = len(displacement)
                + self.radius
                + self.step_height
                + self.snap
                + 4.0 * self.skin
                + GATHER_SLACK;
            let capsule = self
                .shape
                .compute_aabb(&to_pose(self.center(start), [0.0, 0.0, 0.0, 1.0]));
            let bounds = capsule.loosened(reach);
            Rc::new(self.physics.gather(bounds, &self.filter))
        });
    }

    fn center(&self, feet: [f32; 3]) -> [f32; 3] {
        [feet[0], feet[1] + self.height / 2.0, feet[2]]
    }

    fn classify(&self, normal: [f32; 3]) -> Class {
        if normal[1] >= self.cos_limit {
            Class::Ground
        } else if normal[1] <= -self.cos_limit {
            Class::Ceiling
        } else {
            Class::Wall
        }
    }

    /// Whether `prop` cannot travel `need` (plus a skin) along `direction`: a non-rigid collider is
    /// in its way. Only the prop itself is tested, so a row of crates against a wall can still
    /// squeeze.
    fn pinned(&self, prop: EntityId, direction: [f32; 3], need: f32) -> bool {
        let limit = need + self.skin;
        self.physics
            .prop_free_distance(prop, direction, limit, &self.filter, self.gathered())
            < limit
    }

    /// A wall-class hit on a rigid prop that is free to move does not stop the slide: the
    /// kinematic drive pushes it. A pinned prop blocks like a static collider.
    fn is_pushable(&self, hit: &QueryHit, direction: [f32; 3], length: f32) -> bool {
        if !self.physics.is_rigid(hit.entity) || self.classify(normalize(hit.normal)) != Class::Wall
        {
            return false;
        }
        let push = normalize([direction[0], 0.0, direction[2]]);
        let need = (length - hit.distance).max(0.0);
        !self.pinned(hit.entity, push, need)
    }

    fn cast(
        &self,
        feet: [f32; 3],
        direction: [f32; 3],
        length: f32,
        pushing: bool,
    ) -> Option<QueryHit> {
        self.physics.cast_capsule(
            &*self.shape,
            self.center(feet),
            direction,
            length,
            &self.filter,
            self.gathered(),
            |hit| pushing && self.is_pushable(hit, direction, length),
        )
    }

    /// Whether `contact` is a prop that can be pushed away: it never pushes the motor back.
    fn is_free_prop(&self, contact: &Contact) -> bool {
        let away = normalize([-contact.normal[0], 0.0, -contact.normal[2]]);
        self.physics.is_rigid(contact.entity)
            && self.classify(contact.normal) == Class::Wall
            && !self.pinned(contact.entity, away, self.radius / 2.0)
    }

    /// The contacts of the capsule within the skin, sorted by gap. They are queried on a capsule
    /// inflated by the skin, because parry's normal at a gap of a few micrometres is noise while
    /// the same surface a skin deep inside the inflated shape gives an exact one; `dist` is mapped
    /// back to the real gap. A rigid wall-class contact is dropped
    /// while its prop is free to be pushed away.
    fn contacts(&self, feet: [f32; 3]) -> Vec<Contact> {
        let mut contacts = self.physics.capsule_contacts(
            &*self.inflated,
            self.center(feet),
            0.0,
            &self.filter,
            self.gathered(),
        );
        for contact in &mut contacts {
            contact.dist += self.skin;
        }
        contacts.retain(|contact| !self.is_free_prop(contact));
        sort_by_gap_then_rank(&mut contacts, |entity| {
            self.world.spawn_rank(entity).unwrap_or(u64::MAX)
        });
        contacts
    }

    /// `entity`'s surface normal at the capsule's pose, from a contact query a margin deep inside
    /// a wider capsule; `fallback` (a cast's normal) when it is not within reach. A mesh reports its
    /// deepest contact, which may be another face of the same collider than the one the cast hit
    /// at `point`: a contact far from `point` is not the surface in question.
    fn refine(
        &self,
        feet: [f32; 3],
        entity: EntityId,
        point: [f32; 3],
        fallback: [f32; 3],
    ) -> [f32; 3] {
        let margin = 2.0 * self.skin + 1e-3;
        self.physics
            .capsule_contacts(
                &*self.wide,
                self.center(feet),
                margin / 2.0,
                &self.filter,
                self.gathered(),
            )
            .into_iter()
            .find(|contact| {
                contact.entity == entity && len(sub(contact.point, point)) <= self.radius / 4.0
            })
            .map_or_else(|| normalize(fallback), |contact| normalize(contact.normal))
    }

    /// Pushes the capsule out of what it is closer to than half the skin, back to a full skin of
    /// clearance; returns the least-penetrating pose found and whether even that one overlaps by
    /// more than the skin. The feet never move more than a radius: when no clear pose lies within
    /// it the feet stay put and the call is stuck, so a closing press cannot eject its rider
    /// through itself.
    fn depenetrate(&self, feet: [f32; 3]) -> ([f32; 3], bool) {
        let mut current = feet;
        let mut best = (f32::INFINITY, feet);
        for pass in 0..=DEPENETRATION_PASSES {
            let contacts = self.contacts(current);
            let deepest = contacts
                .first()
                .filter(|contact| contact.dist < self.skin / 2.0);
            let depth = contacts
                .first()
                .map_or(0.0, |contact| (-contact.dist).max(0.0));
            if depth <= best.0 {
                best = (depth, current);
            }
            let Some(deepest) = deepest else { break };
            if pass == DEPENETRATION_PASSES {
                break;
            }
            let next = add(current, scale(deepest.normal, self.skin - deepest.dist));
            if len(sub(next, feet)) > self.radius {
                break;
            }
            current = next;
        }
        let stuck = best.0 > self.skin;
        (if stuck { feet } else { best.1 }, stuck)
    }

    /// One move: depenetrate, slide along what the displacement hits, sweep the constraint's
    /// correction, probe the ground, then sweep the platform `carry` through the same slide loop,
    /// shaped by the constraint, without a step-up and without a second probe: first the surface
    /// displacement against everything (the support's end stop holds), then the rigid part with the
    /// support excluded (a rider moves rigidly with its support, whose collider casts see a tick
    /// late). A stuck move applies no carry.
    pub(super) fn solve(
        &self,
        start: [f32; 3],
        displacement: [f32; 3],
        was_grounded: bool,
        mut constraint: Option<&mut dyn MoveConstraint>,
        carry: Option<&CarryPlan>,
    ) -> Outcome {
        self.gather_around(start, displacement);
        let (mut feet, mut stuck) = self.depenetrate(start);
        let rise_from = feet[1];
        let mut slide = Slide::default();
        if !stuck {
            self.slide(
                &mut slide,
                &mut feet,
                displacement,
                was_grounded,
                true,
                &mut constraint,
            );
            let pull = constraint.as_mut().map(|c| c.correction(feet));
            if let Some(pull) = pull
                .map(|p| [p[0], 0.0, p[2]])
                .filter(|p| len(*p) >= MIN_MOVE)
            {
                let (applied, blocked) = self.correct(&mut slide, &mut feet, pull, was_grounded);
                if let Some(constraint) = constraint.as_mut() {
                    constraint.corrected(applied, blocked);
                }
            }
            let (settled, still_stuck) = self.depenetrate(feet);
            feet = settled;
            stuck = still_stuck;
        }

        let snap_allowed = was_grounded && displacement[1] <= 0.0;
        let reach = probe_reach(self.snap, self.skin, snap_allowed);
        let (ground, grounded, settled) = self.probe(
            feet,
            reach,
            self.radius + self.snap + self.skin,
            snap_allowed,
        );
        feet = settled;
        // A snap back down undoes a climb: only the rise that survived the tick is a step.
        let rise = feet[1] - rise_from;
        slide.stepped = slide.stepped.map(|c| c.min(rise)).filter(|c| *c > 0.0);
        // A stuck tick carries nothing: the support's record is kept for the next one.
        let carry_skipped = stuck && carry.is_some();
        let carry = carry.filter(|_| !stuck).map(|carry| {
            // The belt's displacement meets the support's own collider (the rider still sits in its
            // stale frame, so an end stop holds); the rigid part acts on the feet as they are after
            // it, and ignores the support.
            self.slide(
                &mut slide,
                &mut feet,
                carry.surface,
                was_grounded,
                false,
                &mut constraint,
            );
            let (translation, yaw) = rigid_carry(
                feet,
                carry.from.translation,
                carry.from.rotation,
                carry.to.translation,
                carry.to.rotation,
            );
            self.without(carry.entity).slide(
                &mut slide,
                &mut feet,
                translation,
                was_grounded,
                false,
                &mut constraint,
            );
            (carry, translation, yaw)
        });
        let Slide {
            ceiling,
            stepped,
            mut walls,
            ..
        } = slide;
        walls.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.entity.cmp(&b.1.entity)));
        let report = MoveReport {
            grounded,
            landed: grounded && !was_grounded,
            ground,
            ceiling,
            walls: walls.into_iter().map(|(_, wall)| wall).collect(),
            stepped,
            platform: carry.map_or_else(PlatformCarry::default, |(carry, translation, yaw)| {
                PlatformCarry {
                    entity: Some(carry.entity),
                    translation,
                    yaw,
                    surface: carry.surface,
                }
            }),
            moved: sub(feet, start),
            stuck,
            height: self.height,
            posture: None,
            lane: None,
        };
        Outcome {
            feet,
            report,
            carry_skipped,
        }
    }

    /// Sweeps a constraint's `correction` along its own axis, without a step-up. Returns the net
    /// feet change and whether the travel along the axis fell short of the correction.
    fn correct(
        &self,
        slide: &mut Slide,
        feet: &mut [f32; 3],
        correction: [f32; 3],
        was_grounded: bool,
    ) -> ([f32; 3], bool) {
        let length = len(correction);
        let axis = scale(correction, 1.0 / length);
        let before = *feet;
        let mut lock = AxisLock { axis };
        self.slide(
            slide,
            feet,
            correction,
            was_grounded,
            false,
            &mut Some(&mut lock),
        );
        let applied = sub(*feet, before);
        (applied, dot(applied, axis) < length - MIN_MOVE)
    }

    /// This solver with `entity`'s collider ignored as well.
    fn without(&self, entity: EntityId) -> Solver<'a> {
        Solver {
            filter: self.filter.exclude_also(entity),
            gather: self.gather.clone(),
            shape: self.shape.clone(),
            inflated: self.inflated.clone(),
            wide: self.wide.clone(),
            ..*self
        }
    }

    /// Collide-and-slide `displacement` from `feet` for up to `max_slides` iterations, recording
    /// what it hit into `slide`. A wall-class hit climbs a step when `allow_step` and the motor
    /// `was_grounded`.
    fn slide(
        &self,
        slide: &mut Slide,
        feet: &mut [f32; 3],
        displacement: [f32; 3],
        was_grounded: bool,
        allow_step: bool,
        constraint: &mut Option<&mut dyn MoveConstraint>,
    ) {
        let mut planes: Vec<[f32; 3]> = Vec::new();
        let mut remaining = displacement;
        for _ in 0..self.max_slides {
            if let Some(constraint) = constraint.as_mut() {
                let shaped = constraint.project(*feet, remaining);
                remaining = [shaped[0], remaining[1], shaped[2]];
            }
            let length = len(remaining);
            if length.is_nan() || length < MIN_MOVE || !length.is_finite() {
                break;
            }
            let direction = scale(remaining, 1.0 / length);
            let from = *feet;
            let Some(hit) = self.cast(*feet, direction, length + self.skin, true) else {
                *feet = add(*feet, remaining);
                break;
            };
            // Stop a full skin short of the surface *along its normal*, so a grazing approach does not
            // end closer than the depenetration pass would then push it back out.
            let opposing = -dot(normalize(hit.normal), direction);
            let backoff = self.skin / opposing.max(MIN_OPPOSING);
            let advance = (hit.distance - backoff).clamp(0.0, length);
            *feet = add(*feet, scale(direction, advance));
            slide.travel += advance;
            let leftover = scale(direction, length - advance);
            let normal = self.refine(*feet, hit.entity, hit.point, hit.normal);
            let surface = SurfaceHit {
                entity: hit.entity,
                point: hit.point,
                normal,
            };
            let class = self.classify(normal);
            let mut next = match class {
                Class::Ground => self.slide_ground(leftover, normal),
                Class::Wall => {
                    if allow_step
                        && was_grounded
                        && let Some((pose, climb)) = self.step_up(
                            from,
                            [remaining[0], 0.0, remaining[2]],
                            &hit,
                            displacement[1],
                        )
                    {
                        *feet = pose;
                        slide.stepped = Some(slide.stepped.unwrap_or(0.0) + climb);
                        break;
                    }
                    if !slide.walls.iter().any(|(_, wall)| {
                        wall.entity == hit.entity && dot(wall.normal, normal) > WALL_SAME_PLANE
                    }) {
                        slide.walls.push((slide.travel, surface));
                    }
                    slide_wall(leftover, normal)
                }
                Class::Ceiling => {
                    if slide.ceiling.is_none() {
                        slide.ceiling = Some(surface);
                    }
                    [leftover[0], leftover[1].min(0.0), leftover[2]]
                }
            };
            if !planes.iter().any(|plane| dot(*plane, normal) > 0.9999) {
                planes.push(normal);
            }
            if planes.len() >= 3 {
                break;
            }
            if planes.len() == 2
                && planes
                    .iter()
                    .any(|plane| dot(next, *plane) < -MIN_MOVE * 10.0)
            {
                let crease = cross(planes[0], planes[1]);
                let crease_len = len(crease);
                next = if crease_len < 1e-4 {
                    [0.0; 3]
                } else {
                    let along = scale(crease, 1.0 / crease_len);
                    scale(along, dot(leftover, along))
                };
            }
            remaining = next;
        }
    }

    /// Climbs the obstacle `hit` blocking `horizontal` from `from`: up by the step height, forward,
    /// then down onto walkable ground. Returns the landing pose and the net climb, or `None` when
    /// the obstacle rises above the step height, the headroom or the way forward is blocked, or
    /// the landing is not walkable. A capsule landing on the step's edge counts as standing on the
    /// face past it ([`rim_support`](Self::rim_support)).
    fn step_up(
        &self,
        from: [f32; 3],
        horizontal: [f32; 3],
        hit: &QueryHit,
        displacement_y: f32,
    ) -> Option<([f32; 3], f32)> {
        let length = len(horizontal);
        if self.step_height <= 0.0 || length < MIN_MOVE {
            return None;
        }
        // The obstruction's top, from the floor the capsule stands on (a skin below its feet).
        if self.above_step(from[1], hit.point[1]) {
            return None;
        }
        let up = [0.0, 1.0, 0.0];
        if self
            .cast(from, up, self.step_height + self.skin, false)
            .is_some()
        {
            return None;
        }
        let raised = add(from, scale(up, self.step_height));
        let direction = scale(horizontal, 1.0 / length);
        if self
            .cast(raised, direction, length + self.skin, true)
            .is_some()
        {
            return None;
        }
        let over = add(raised, horizontal);
        let snap = if displacement_y <= 0.0 {
            self.snap
        } else {
            0.0
        };
        let land = self.cast(
            over,
            [0.0, -1.0, 0.0],
            self.step_height + self.skin + snap,
            false,
        )?;
        let normal = self.refine(over, land.entity, land.point, land.normal);
        if normal[1] < self.cos_limit && self.rim_support(land.point, over, land.entity).is_none() {
            return None;
        }
        let pose = [
            over[0],
            over[1] - (land.distance - self.skin).max(0.0),
            over[2],
        ];
        let climb = pose[1] - from[1];
        let (settled, stuck) = self.depenetrate(pose);
        (climb > 0.0 && climb <= self.step_height && !stuck && settled == pose)
            .then_some((pose, climb))
    }

    /// Whether a surface point at `point_y` rises above the step height from the floor a skin
    /// below `floor_feet_y`.
    fn above_step(&self, floor_feet_y: f32, point_y: f32) -> bool {
        point_y - (floor_feet_y - self.skin) > self.step_height
    }

    /// The leftover after hitting walkable ground: downward motion is spent, and the horizontal
    /// part follows the plane at its full horizontal length.
    fn slide_ground(&self, leftover: [f32; 3], normal: [f32; 3]) -> [f32; 3] {
        let v = [leftover[0], leftover[1].max(0.0), leftover[2]];
        let flat = sqrt(v[0] * v[0] + v[2] * v[2]);
        let on_plane = sub(v, scale(normal, dot(v, normal)));
        let on_plane_flat = sqrt(on_plane[0] * on_plane[0] + on_plane[2] * on_plane[2]);
        if flat > 0.0 && on_plane_flat > 1e-6 {
            scale(on_plane, flat / on_plane_flat)
        } else {
            on_plane
        }
    }

    /// The height that rests the capsule at `feet` a skin above the walkable plane under its axis,
    /// within a skin up and `reach` down of where it is, or `None` when the ray down the axis does
    /// not first meet a walkable face of `entity`. The face may lie below the capsule's own contact
    /// (an edge of the same collider), so the caller bounds the result by the cast. It is read from the ray's plane height,
    /// not from a swept cast: a cast's time of impact is only good to about a tenth of a
    /// millimetre, which a per-tick snap would ratchet into a visible sink, and a height computed
    /// from the plane is the same bits however the capsule got there.
    fn plane_rest(&self, feet: [f32; 3], entity: EntityId, reach: f32) -> Option<f32> {
        let origin = [feet[0], feet[1] + self.radius, feet[2]];
        let under = self
            .physics
            .probe_down(origin, self.radius + reach, &self.filter, self.gathered())
            .filter(|under| under.entity == entity && under.normal[1] >= self.cos_limit)?;
        let up = normalize(under.normal)[1];
        let rest = under.point[1] + (self.radius + self.skin) / up - self.radius;
        // The ray's height is rounded off a sum with the origin: below a micrometre it is noise,
        // and following it would flip the last bits of a rested capsule every tick.
        let rest = rest.clamp(feet[1] - reach, feet[1] + self.skin);
        Some(if (rest - feet[1]).abs() < REST_NOISE {
            feet[1]
        } else {
            rest
        })
    }

    /// The walkable face `entity` presents past the contact `point` of a capsule whose axis is at
    /// `axis`: a short ray down just beyond the contact, away from the axis, raised by a skin. A
    /// capsule resting on an edge touches only the edge, and its cast normal there is tilted off
    /// the face it stands on; this finds the face. Returns that face's normal.
    fn rim_support(&self, point: [f32; 3], axis: [f32; 3], entity: EntityId) -> Option<[f32; 3]> {
        let away = normalize([point[0] - axis[0], 0.0, point[2] - axis[2]]);
        let origin = [
            point[0] + away[0] * 1e-3,
            point[1] + self.skin,
            point[2] + away[2] * 1e-3,
        ];
        self.physics
            .probe_down(origin, 2.0 * self.skin, &self.filter, self.gathered())
            .filter(|under| under.entity == entity && under.normal[1] >= self.cos_limit)
            .map(|under| normalize(under.normal))
    }

    /// Casts down from `origin` over `reach` for the support; settles onto it when `settle`.
    /// `under` is the length of the ray down the axis from a radius above `origin`, used when the
    /// cast contact is too steep. Returns the support, whether it is ground, and the feet after
    /// settling (`origin` when not settled).
    fn probe(
        &self,
        origin: [f32; 3],
        reach: f32,
        under: f32,
        settle: bool,
    ) -> (Option<GroundHit>, bool, [f32; 3]) {
        let mut feet = origin;
        let Some(hit) = self.cast(feet, [0.0, -1.0, 0.0], reach, false) else {
            return (None, false, feet);
        };
        if hit.normal[1] <= 0.0 {
            return (None, false, feet);
        }
        let mut normal = normalize(hit.normal);
        let mut rim = false;
        let mut walkable = normal[1] >= self.cos_limit;
        if !walkable {
            let origin = [feet[0], feet[1] + self.radius, feet[2]];
            if let Some(under) = self
                .physics
                .probe_down(origin, under, &self.filter, self.gathered())
                .filter(|under| under.entity == hit.entity && under.normal[1] >= self.cos_limit)
            {
                normal = normalize(under.normal);
                rim = true;
                walkable = true;
            } else if let Some(face) = self.rim_support(hit.point, feet, hit.entity) {
                normal = face;
                rim = true;
                walkable = true;
            }
        }
        let grounded = walkable && hit.distance <= reach;
        if grounded && settle {
            let cast_rest = feet[1] - (hit.distance - self.skin).max(0.0);
            // A plane under the axis lower than the cast contact belongs to a lower face of the
            // same collider (a mesh's floor below its own step): resting there sinks the capsule.
            feet[1] = self
                .plane_rest(feet, hit.entity, reach)
                .filter(|rest| *rest >= cast_rest - self.skin / 2.0)
                .unwrap_or(cast_rest);
        }
        if !rim {
            normal = self.refine(feet, hit.entity, hit.point, normal);
        }
        (
            Some(GroundHit {
                entity: hit.entity,
                point: hit.point,
                normal,
                walkable,
                distance: hit.distance,
            }),
            grounded,
            feet,
        )
    }
}

/// The leftover after hitting a wall: the horizontal part loses its inward component along the
/// horizontal normal (a steep face cannot be climbed); downward motion on an upward-facing steep
/// face follows the true plane, which is the slide-off under gravity; upward motion is kept.
fn slide_wall(leftover: [f32; 3], normal: [f32; 3]) -> [f32; 3] {
    let horizontal = normalize([normal[0], 0.0, normal[2]]);
    let flat = [leftover[0], 0.0, leftover[2]];
    let inward = dot(flat, horizontal);
    let mut out = if inward < 0.0 {
        sub(flat, scale(horizontal, inward))
    } else {
        flat
    };
    if leftover[1] < 0.0 && normal[1] > 0.0 {
        let down = [0.0, leftover[1], 0.0];
        out = add(out, sub(down, scale(normal, dot(down, normal))));
    } else {
        out[1] += leftover[1];
    }
    out
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale(a: [f32; 3], k: f32) -> [f32; 3] {
    [a[0] * k, a[1] * k, a[2] * k]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn len(a: [f32; 3]) -> f32 {
    sqrt(dot(a, a))
}

/// `a` scaled to unit length, or `a` itself when it has none.
fn normalize(a: [f32; 3]) -> [f32; 3] {
    let length = len(a);
    if length > 1e-12 {
        scale(a, 1.0 / length)
    } else {
        a
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contact(entity: u32, dist: f32) -> Contact {
        Contact {
            entity: EntityId::new(entity, 0),
            dist,
            normal: [0.0, 1.0, 0.0],
            point: [0.0; 3],
        }
    }

    #[test]
    fn equal_gaps_follow_rank_and_a_smaller_gap_still_wins() {
        // Entity order 1 < 2 < 3, rank order 3 < 1 < 2.
        let rank = |e: EntityId| match e.index() {
            3 => 0,
            1 => 1,
            _ => 2,
        };
        let mut contacts = [
            contact(1, 0.1),
            contact(2, 0.1),
            contact(3, 0.1),
            contact(2, -0.5),
        ];
        sort_by_gap_then_rank(&mut contacts, rank);
        let order: Vec<u32> = contacts.iter().map(|c| c.entity.index()).collect();
        assert_eq!(order, [2, 3, 1, 2]);
    }
}

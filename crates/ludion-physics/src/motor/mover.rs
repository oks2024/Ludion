//! [`move_character`]: depenetration, collide-and-slide, ground probe and ceiling for a
//! [`CharacterMotor`](crate::CharacterMotor).

use ludion_core::ids::EntityId;
use ludion_core::math::{cos, sqrt};
use ludion_core::time::FrameIndex;
use ludion_ecs::World;

use super::component::{CharacterMotor, CharacterMotorState, check, height_in_force};
use super::report::{GroundHit, MotorError, MoveReport, PlatformCarry, SurfaceHit};
use super::sweep::{Capsule, Contact};
use crate::pose::PhysicsPose;
use crate::queries::{QueryFilter, QueryHit};
use crate::world::PhysicsWorld;

/// Depenetration passes before the motor gives up and reports `stuck`.
const DEPENETRATION_PASSES: usize = 4;
/// A displacement shorter than this is not worth a cast.
const MIN_MOVE: f32 = 1e-5;
/// The least a hit normal may oppose the motion for the skin back-off along the motion to stay
/// bounded.
const MIN_OPPOSING: f32 = 0.2;

/// Reshapes a move before the motor casts it: a game's lane, rail or plane lock.
///
/// The motor calls [`project`](SlideConstraint::project) with the feet and the displacement it is
/// about to cast, for the initial displacement and again for each slide iteration's remainder; the
/// return value replaces it. A constraint that needs the tick's `dt` reads
/// [`PhysicsWorld::tick_rate`].
pub trait SlideConstraint {
    /// The step to cast instead of `step`, from `feet`.
    fn project(&mut self, feet: [f32; 3], step: [f32; 3]) -> [f32; 3];
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
/// Casts see colliders as of the last step, and skip sensors ([`QueryFilter::include_sensors`] is
/// never set). The motor's own capsule and the layers its `layer` ignores are excluded.
///
/// Call it from a system registered **before** [`install`](crate::install)'s: the report is
/// stamped with the frame the physics write pass runs in next. The physics world does not exist
/// until the first write pass, so the first tick of a world returns [`MotorError::NoPhysicsWorld`].
///
/// ```no_run
/// # use ludion_ecs::{SystemCtx, World};
/// # use ludion_physics::{move_character, PhysicsPose};
/// # use ludion_core::ids::EntityId;
/// fn walk<P: PhysicsPose>(world: &mut World, _ctx: &SystemCtx, hero: EntityId) {
///     let gravity = -0.2;
///     if let Ok(report) = move_character::<P>(world, hero, [0.05, gravity, 0.0]) {
///         if report.landed { /* play the thud */ }
///     }
/// }
/// ```
pub fn move_character<P: PhysicsPose>(
    world: &mut World,
    entity: EntityId,
    displacement: [f32; 3],
) -> Result<MoveReport, MotorError> {
    move_character_with::<P>(world, entity, displacement, None)
}

/// [`move_character`] with a [`SlideConstraint`] reshaping the displacement before each cast.
/// The same ordering rule applies: call it before [`install`](crate::install)'s systems.
pub fn move_character_with<P: PhysicsPose>(
    world: &mut World,
    entity: EntityId,
    displacement: [f32; 3],
    constraint: Option<&mut dyn SlideConstraint>,
) -> Result<MoveReport, MotorError> {
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
    if !displacement.iter().all(|c| c.is_finite()) {
        return Err(MotorError::NonFiniteDisplacement(displacement));
    }
    let state = world.get::<CharacterMotorState>(entity).ok().flatten();
    let height = height_in_force(&motor, state);
    let was_grounded = state.is_some_and(|state| state.grounded);
    let checked = CharacterMotor {
        height,
        ..motor.clone()
    };
    if let Err((pointer, reason)) = check(&checked) {
        return Err(MotorError::InvalidMotor {
            entity,
            reason: format!("{pointer}: {reason}"),
        });
    }
    let physics = world
        .resource::<PhysicsWorld>()
        .ok_or(MotorError::NoPhysicsWorld)?;
    let solver = Solver::new(physics, entity, &checked);
    let outcome = solver.solve(feet, displacement, was_grounded, constraint);
    let frame = physics
        .last_write_frame
        .map_or(FrameIndex::ZERO, |frame| FrameIndex::new(frame + 1));

    if let Ok(Some(pose)) = world.get_mut::<P>(entity) {
        pose.set_translation(outcome.feet);
    }
    let ground_entity = outcome.report.ground.filter(|_| outcome.report.grounded);
    if world
        .get::<CharacterMotorState>(entity)
        .ok()
        .flatten()
        .is_some()
    {
        if let Ok(Some(state)) = world.get_mut::<CharacterMotorState>(entity) {
            state.grounded = outcome.report.grounded;
            state.ground = ground_entity.map(|hit| hit.entity);
        }
    } else {
        let _ = world.insert(
            entity,
            CharacterMotorState {
                grounded: outcome.report.grounded,
                ground: ground_entity.map(|hit| hit.entity),
                ..CharacterMotorState::default()
            },
        );
    }
    if let Some(physics) = world.resource_mut::<PhysicsWorld>() {
        physics
            .move_reports
            .insert(entity, (frame, outcome.report.clone()));
    }
    Ok(outcome.report)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Ground,
    Wall,
    Ceiling,
}

struct Outcome {
    feet: [f32; 3],
    report: MoveReport,
}

struct Solver<'a> {
    physics: &'a PhysicsWorld,
    filter: QueryFilter,
    capsule: Capsule,
    height: f32,
    radius: f32,
    skin: f32,
    snap: f32,
    max_slides: u32,
    cos_limit: f32,
}

impl<'a> Solver<'a> {
    fn new(physics: &'a PhysicsWorld, entity: EntityId, motor: &CharacterMotor) -> Self {
        let mut filter = QueryFilter::new().exclude(entity);
        for name in physics.ignored_by(motor.layer.as_deref()) {
            filter = filter.ignore_layer(name);
        }
        Solver {
            physics,
            filter,
            capsule: Capsule {
                half_height: motor.half_height_for(motor.height),
                radius: motor.radius,
            },
            height: motor.height,
            radius: motor.radius,
            skin: motor.skin_width,
            snap: motor.snap_distance,
            max_slides: motor.max_slides,
            cos_limit: cos(motor.slope_limit_deg.to_radians()),
        }
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
            .prop_free_distance(prop, direction, limit, &self.filter)
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
            self.capsule,
            self.center(feet),
            direction,
            length,
            &self.filter,
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
        let inflated = Capsule {
            radius: self.radius + self.skin,
            ..self.capsule
        };
        let mut contacts =
            self.physics
                .capsule_contacts(inflated, self.center(feet), 0.0, &self.filter);
        for contact in &mut contacts {
            contact.dist += self.skin;
        }
        contacts.retain(|contact| !self.is_free_prop(contact));
        contacts.sort_unstable_by(|a, b| a.dist.total_cmp(&b.dist).then(a.entity.cmp(&b.entity)));
        contacts
    }

    /// `entity`'s surface normal at the capsule's pose, from a contact query a margin deep inside
    /// a wider capsule; `fallback` (a cast's normal) when it is not within reach.
    fn refine(&self, feet: [f32; 3], entity: EntityId, fallback: [f32; 3]) -> [f32; 3] {
        let margin = 2.0 * self.skin + 1e-3;
        let wide = Capsule {
            radius: self.radius + margin,
            ..self.capsule
        };
        self.physics
            .capsule_contacts(wide, self.center(feet), margin / 2.0, &self.filter)
            .into_iter()
            .find(|contact| contact.entity == entity)
            .map_or_else(|| normalize(fallback), |contact| normalize(contact.normal))
    }

    /// Pushes the capsule out of what it is closer to than half the skin, back to a full skin of
    /// clearance; returns the least-penetrating pose found and whether even that one overlaps by
    /// more than the skin.
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
            current = add(current, scale(deepest.normal, self.skin - deepest.dist));
        }
        (best.1, best.0 > self.skin)
    }

    fn solve(
        &self,
        start: [f32; 3],
        displacement: [f32; 3],
        was_grounded: bool,
        mut constraint: Option<&mut dyn SlideConstraint>,
    ) -> Outcome {
        let (mut feet, mut stuck) = self.depenetrate(start);
        let mut ceiling = None;
        let mut walls: Vec<(f32, SurfaceHit)> = Vec::new();
        if !stuck {
            let mut planes: Vec<[f32; 3]> = Vec::new();
            let mut remaining = displacement;
            let mut travel = 0.0;
            for _ in 0..self.max_slides {
                if let Some(constraint) = constraint.as_mut() {
                    remaining = constraint.project(feet, remaining);
                }
                let length = len(remaining);
                if length.is_nan() || length < MIN_MOVE || !length.is_finite() {
                    break;
                }
                let direction = scale(remaining, 1.0 / length);
                let Some(hit) = self.cast(feet, direction, length + self.skin, true) else {
                    feet = add(feet, remaining);
                    break;
                };
                // Stop a full skin short of the surface *along its normal*, so a grazing approach does not
                // end closer than the depenetration pass would then push it back out.
                let opposing = -dot(normalize(hit.normal), direction);
                let backoff = self.skin / opposing.max(MIN_OPPOSING);
                let advance = (hit.distance - backoff).clamp(0.0, length);
                feet = add(feet, scale(direction, advance));
                travel += advance;
                let leftover = scale(direction, length - advance);
                let normal = self.refine(feet, hit.entity, hit.normal);
                let surface = SurfaceHit {
                    entity: hit.entity,
                    point: hit.point,
                    normal,
                };
                let class = self.classify(normal);
                let mut next = match class {
                    Class::Ground => self.slide_ground(leftover, normal),
                    Class::Wall => {
                        if !walls.iter().any(|(_, wall)| wall.entity == hit.entity) {
                            walls.push((travel, surface));
                        }
                        slide_wall(leftover, normal)
                    }
                    Class::Ceiling => {
                        if ceiling.is_none() {
                            ceiling = Some(surface);
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
        if !stuck {
            let (settled, still_stuck) = self.depenetrate(feet);
            feet = settled;
            stuck = still_stuck;
        }

        let (ground, grounded) = self.probe(&mut feet, displacement[1], was_grounded);
        walls.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.entity.cmp(&b.1.entity)));
        let report = MoveReport {
            grounded,
            landed: grounded && !was_grounded,
            ground,
            ceiling,
            walls: walls.into_iter().map(|(_, wall)| wall).collect(),
            stepped: None,
            platform: PlatformCarry::default(),
            moved: sub(feet, start),
            stuck,
            height: self.height,
        };
        Outcome { feet, report }
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

    /// Casts down from `feet` for the support, snapping onto it when allowed. Returns the support
    /// and whether the motor is grounded on it.
    fn probe(
        &self,
        feet: &mut [f32; 3],
        displacement_y: f32,
        was_grounded: bool,
    ) -> (Option<GroundHit>, bool) {
        let snap_allowed = was_grounded && displacement_y <= 0.0;
        let reach = if snap_allowed {
            self.snap + self.skin
        } else {
            2.0 * self.skin
        };
        let Some(hit) = self.cast(*feet, [0.0, -1.0, 0.0], reach, false) else {
            return (None, false);
        };
        if hit.normal[1] <= 0.0 {
            return (None, false);
        }
        let mut normal = normalize(hit.normal);
        let mut rim = false;
        let mut walkable = normal[1] >= self.cos_limit;
        if !walkable {
            let origin = [feet[0], feet[1] + self.radius, feet[2]];
            let length = self.radius + self.snap + self.skin;
            if let Some(under) = self
                .physics
                .probe_down(origin, length, &self.filter)
                .filter(|under| under.normal[1] >= self.cos_limit)
            {
                normal = normalize(under.normal);
                rim = true;
                walkable = true;
            }
        }
        let grounded = walkable && hit.distance <= reach;
        if grounded && snap_allowed {
            feet[1] -= (hit.distance - self.skin).max(0.0);
        }
        if !rim {
            normal = self.refine(*feet, hit.entity, normal);
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

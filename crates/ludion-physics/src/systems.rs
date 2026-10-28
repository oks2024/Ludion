//! The system triple and the one verb that registers it.

use std::any::TypeId;
use std::collections::BTreeSet;
use std::sync::Arc;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_ecs::{Parent, Schedule, SystemCtx, World};
use ludion_spatial::{Body, Collider, Joint, JointMotor, Plane, Shape, Velocity};

use crate::PhysicsPose;
use crate::meshes::ColliderMeshes;
use crate::motor::{CharacterMotor, CharacterMotorState, check, height_in_force, motor_invalid};
use crate::world::{DEFAULT_GRAVITY, MeshInput, PhysicsWorld, ShapeRejection, mesh_hash_of};

/// The registration name of the system that pushes authored state into the solver.
pub const WRITE: &str = "physics_write";
/// The registration name of the system that advances the solver by one fixed timestep.
pub const STEP: &str = "physics_step";
/// The registration name of the system that writes the solver's result back into components.
pub const MIRROR: &str = "physics_mirror";

/// Whether the world's [`Plane`] constrains the bodies physics creates.
///
/// The choice is made at the [`install`] call because it cannot be inferred: a host installs a
/// `Plane` resource whether or not the game is two-dimensional, and the default `Plane::Xz` locks
/// the world `y` axis — the one gravity points down. Inferring "plane present ⇒ planar" would
/// therefore turn a 3D pile into a pile that never falls, with nothing to read that says why.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhysicsSpace {
    /// Full three-dimensional motion: the engine's [`Plane`] contributes no locks, and a body moves
    /// and tumbles on every axis. The launcher and ragdoll case, and how a 3D game opts out of a
    /// `Plane` its host installed anyway. A body's own
    /// [`AxisLock`](ludion_spatial::AxisLock) still applies.
    Free,
    /// The world's [`Plane`] locks the off-plane translation axis and the two in-plane rotation
    /// axes on every rigid body, leaving rotation about the plane normal free — so a 2D game is the
    /// 3D solver with axis locks rather than a second engine. With no `Plane` resource in the world
    /// this is indistinguishable from [`Free`](PhysicsSpace::Free): there is no plane to lock
    /// against.
    Planar,
}

/// The default number of solver position-correction iterations per step — the pinned rapier
/// build's own default (`IntegrationParameters::default().num_solver_iterations`), pinned again
/// here by a unit test in `world.rs` so a pin upgrade that changes it fails loudly.
pub const DEFAULT_SOLVER_ITERATIONS: u32 = 4;
/// The default number of internal stabilization iterations per step — the pinned rapier build's
/// own default (`IntegrationParameters::default().num_internal_stabilization_iterations`).
pub const DEFAULT_STABILIZATION_ITERATIONS: u32 = 1;
/// The scene-wide default for [`RigidBody::sleep_after_ticks`](ludion_spatial::RigidBody), used by
/// any rigid body that omits its own.
pub const DEFAULT_SLEEP_AFTER_TICKS: u32 = 30;

/// The most layers a [`CollisionLayers`] table can hold, `default` included: one per bit of a mask.
pub const MAX_COLLISION_LAYERS: usize = 32;

/// A scene's collision-layer table as the solver reads it: the layer names, `default` first, and for
/// each the bitmask of layers it never collides with.
///
/// The masks are already symmetric — bit `j` of layer `i`'s mask is set when either layer ignores the
/// other — which is the shape `ludion_scene::LayerMatrix` resolves, so a host maps one onto the
/// other without this crate learning what a scene is. A layer's index is its bit position.
///
/// # Example
/// ```
/// use ludion_physics::CollisionLayers;
///
/// // `default` (bit 0) and `ghost` (bit 1), with `ghost` ignoring `default`.
/// let layers = CollisionLayers::new(
///     vec!["default".to_string(), "ghost".to_string()],
///     vec![0b10, 0b01],
/// );
/// assert_eq!(layers.index_of("ghost"), Some(1));
/// assert_eq!(layers.index_of("ghots"), None);
/// assert_eq!(layers.names(), ["default", "ghost"]);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollisionLayers {
    names: Vec<String>,
    ignores: Vec<u32>,
}

impl CollisionLayers {
    /// A table over `names` (`default` first) with `ignores[i]` the symmetric ignore mask of layer
    /// `i`. Both lists must be the same length and hold at most 32 layers (one per bit of the
    /// mask); a table that breaks either is a host bug, not scene data, and panics.
    ///
    /// # Panics
    /// When the lists differ in length or name more than [`MAX_COLLISION_LAYERS`] layers.
    pub fn new(names: Vec<String>, ignores: Vec<u32>) -> Self {
        assert_eq!(names.len(), ignores.len(), "one ignore mask per layer name");
        assert!(
            names.len() <= MAX_COLLISION_LAYERS,
            "a collision-layer table holds at most {MAX_COLLISION_LAYERS} layers (`default` included), got {}",
            names.len()
        );
        debug_assert_eq!(names.first().map(String::as_str), Some("default"));
        CollisionLayers { names, ignores }
    }

    /// The index (bit position) of `name`, or `None` when the table does not declare it.
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.names.iter().position(|candidate| candidate == name)
    }

    /// Every layer name, `default` first.
    pub fn names(&self) -> &[String] {
        &self.names
    }

    pub(crate) fn ignores_mask(&self, index: usize) -> u32 {
        self.ignores[index]
    }
}

/// The solver tunables a scene's `physics:` block sets: the space, gravity, the solver and
/// stabilization iteration counts, the scene-wide sleep default, and the collision-layer table.
///
/// Built from [`PhysicsSpace`] alone via [`From`], or with gravity, the iteration counts and sleep
/// default overridden. [`PhysicsWorld::apply_settings`](crate::PhysicsWorld::apply_settings)
/// re-applies gravity and the two iteration counts to the solver every tick, so editing them takes
/// effect without recreating any body. The space (and the plane's axis locks) and each body's
/// resolved sleep default are creation-time only: they are baked in when the body is created and do
/// not move if the settings change afterward. The layer table is re-applied when it changes: every
/// body's collision groups follow it, and a body whose layer it stops declaring is dropped.
#[derive(Clone, Debug, PartialEq)]
pub struct PhysicsSettings {
    /// The space new rigid bodies are created in.
    pub space: PhysicsSpace,
    /// The constant acceleration applied to every rigid body. A top-down game sets zero.
    pub gravity: [f32; 3],
    /// `rapier`'s `num_solver_iterations` (`1..=64`).
    pub solver_iterations: u32,
    /// `rapier`'s `num_internal_stabilization_iterations` (`1..=64`).
    pub stabilization_iterations: u32,
    /// The scene-wide default for a rigid body that omits its own `sleep_after_ticks`.
    pub sleep_after_ticks: u32,
    /// The declared collision layers, or `None` for a scene declaring none: every collider then
    /// keeps the solver's default groups and only `default` (or no `layer`) is a valid layer name.
    pub layers: Option<CollisionLayers>,
}

impl PhysicsSettings {
    /// `space` with every other tunable at its pinned default.
    pub fn new(space: PhysicsSpace) -> Self {
        PhysicsSettings {
            space,
            gravity: DEFAULT_GRAVITY,
            solver_iterations: DEFAULT_SOLVER_ITERATIONS,
            stabilization_iterations: DEFAULT_STABILIZATION_ITERATIONS,
            sleep_after_ticks: DEFAULT_SLEEP_AFTER_TICKS,
            layers: None,
        }
    }
}

impl From<PhysicsSpace> for PhysicsSettings {
    fn from(space: PhysicsSpace) -> Self {
        PhysicsSettings::new(space)
    }
}

/// Appends the whole physics tick — [`WRITE`], [`STEP`], [`MIRROR`] — to `schedule`, in that
/// order, at the position the game chooses, simulating under `settings`.
///
/// # Why one verb and not three
///
/// The three systems are order-critical and a wrong order fails *silently*: mirror before step
/// writes back last tick's pose, write after step throws this tick's contacts away. Neither
/// produces an error — just a simulation that is subtly wrong and reproducibly so. Registering
/// them as one verb makes the order unstatable rather than merely documented, which is the
/// `spatial::rebuild` precedent without its footgun.
///
/// `P` is the game's position component: the whole coupling between this crate and a game's data
/// is [`PhysicsPose`]'s four methods, which is why `ludion-physics` does not depend on the engine.
/// Register the game's own systems **before** this call if they write velocities, teleport
/// bodies or queue impulses ([`PhysicsWorld::apply_impulse`] and its siblings), and after it if
/// they read the result.
///
/// The [`PhysicsWorld`] resource appears on the first tick that claims an entity (see
/// [`claims`]), at the schedule's tick rate, with the plane [`space`](PhysicsSpace) asks for.
/// Until then nothing is allocated and [`STEP`]/[`MIRROR`] are no-ops, so a game that registers
/// this verb and never authors a rigid body pays one query per tick and nothing else.
///
/// # Example
/// ```
/// use ludion_ecs::Schedule;
/// use ludion_physics::{install, MIRROR, PhysicsSpace, STEP, WRITE};
/// # use ludion_physics::PhysicsPose;
/// # use schemars::JsonSchema;
/// # use serde::{Deserialize, Serialize};
/// # #[derive(Serialize, Deserialize, JsonSchema)]
/// # struct Pose { translation: [f32; 3], rotation: [f32; 4] }
/// # impl PhysicsPose for Pose {
/// #     fn translation(&self) -> [f32; 3] { self.translation }
/// #     fn set_translation(&mut self, t: [f32; 3]) { self.translation = t; }
/// #     fn rotation_quat(&self) -> [f32; 4] { self.rotation }
/// #     fn set_rotation_quat(&mut self, r: [f32; 4]) { self.rotation = r; }
/// # }
/// let mut schedule = Schedule::new();
/// install::<Pose>(&mut schedule, PhysicsSpace::Free);
/// assert_eq!(schedule.system_names().collect::<Vec<_>>(), vec![WRITE, STEP, MIRROR]);
/// ```
pub fn install<P: PhysicsPose>(schedule: &mut Schedule, settings: impl Into<PhysicsSettings>) {
    let settings = settings.into();
    schedule
        .add_system_shared(
            WRITE,
            Arc::new(move |world: &mut World, ctx: &SystemCtx| write::<P>(world, ctx, &settings)),
        )
        .add_system(STEP, step)
        .add_system(MIRROR, mirror::<P>);
}

/// Whether physics owns `collider`, and therefore the exact complement of what
/// [`spatial::rebuild`](ludion_spatial::spatial::rebuild) skips: a 3D shape, or a `rigid` body of
/// any shape.
///
/// **Two tiers, not two ways.** `spatial::*` answers "what is near this point on the plane this
/// frame" with no bodies and no solver; `physics::*` simulates. An entity is in one tier or the
/// other and never both, and the two predicates are complementary rather than merely disjoint, so
/// a collider cannot fall through the gap either. In practice: a `circle` or `aabb` reaches the
/// solver only when its body is `rigid` — the plane-locked 2D-physics case — and a *static* or
/// *kinematic* in-plane collider is `ludion-spatial`'s, which is why a physics scene's level
/// geometry is authored with 3D shapes (`box`, `heightfield`, `convex`) rather than `aabb`.
///
/// # Example
/// ```
/// use ludion_physics::claims;
/// use ludion_spatial::{Body, Collider, RigidBody, Shape};
///
/// let wall = Collider { shape: Shape::Aabb { half_extents: [1.0, 1.0] }, body: Body::new_static(), layer: None, sensor: false };
/// assert!(!claims(&wall)); // the 2D index owns it
///
/// let crate_ = Collider { shape: Shape::Box { half_extents: [0.5; 3] }, body: Body::new_static(), layer: None, sensor: false };
/// assert!(claims(&crate_)); // a 3D shape is never in the 2D index
///
/// let puck = Collider {
///     shape: Shape::Circle { radius: 0.5 },
///     body: Body::Rigid(RigidBody::default()),
///     layer: None,
///     sensor: false,
/// };
/// assert!(claims(&puck)); // a rigid body is never in the 2D index
/// ```
pub fn claims(collider: &Collider) -> bool {
    collider.is_solver_owned()
}

/// One claimed entity this tick: its id, pose, and — only when its `Collider` is stale — the new
/// `Collider` to [`rebuild`](PhysicsWorld::rebuild) from.
/// The resolved [`MeshInput`] rides along, for every path that builds a body.
type Claimed = (EntityId, [f32; 3], [f32; 4], Option<Collider>, MeshInput);

/// The mesh a collider's shape needs from the library: `Ok(None)` for a shape that is not a `mesh`.
fn resolve_mesh(meshes: Option<&mut ColliderMeshes>, collider: &Collider) -> MeshInput {
    let Shape::Mesh { path } = &collider.shape else {
        return Ok(None);
    };
    match meshes {
        Some(meshes) => meshes.get(path).map(Some),
        None => Err(Diagnostic::new(
            code::COLLIDER_MESH_UNAVAILABLE,
            "no collider-mesh library is installed on this world; the host installs one at load",
        )),
    }
}

/// The kinematic bodies a game asked to teleport this tick, drained by [`WRITE`] every run; never
/// hashed or snapshotted. Filled by [`teleport_kinematic`].
#[derive(Debug, Default)]
pub struct KinematicTeleports(BTreeSet<EntityId>);

/// Teleports a kinematic body: the next physics write places it at its pose component with zero
/// velocity, instead of driving it there over the tick.
///
/// A driven kinematic body carries what rests on it (and flings it when it jumps far in one tick);
/// a teleport leaves riders behind, not launched. A no-op in a world without a [`PhysicsWorld`], and
/// a rigid or static body ignores it (a rigid body's pose write already is a teleport). Call it
/// from a system registered before [`install`], after writing the pose. A call from a system
/// scheduled after the physics write (presentation, say) only takes effect next tick, and a
/// snapshot does not carry it.
///
/// # Example
/// ```
/// use ludion_ecs::World;
/// use ludion_physics::{KinematicTeleports, teleport_kinematic};
///
/// let mut world = World::new();
/// let entity = world.spawn();
/// teleport_kinematic(&mut world, entity); // no physics world: nothing is recorded
/// assert!(world.resource::<KinematicTeleports>().is_none());
/// ```
pub fn teleport_kinematic(world: &mut World, entity: EntityId) {
    if world.resource::<PhysicsWorld>().is_none() {
        return;
    }
    if let Some(KinematicTeleports(entities)) = world.resource_mut::<KinematicTeleports>() {
        entities.insert(entity);
    } else {
        world.insert_resource(KinematicTeleports(BTreeSet::from([entity])));
    }
}

/// Reconciles the body set with the colliders physics [`claims`] and pushes this tick's authored
/// state in.
///
/// Bodies are created and retired in **ascending entity order** — the typed join's order, which is
/// a pure function of world content and independent of spawn order — so the creation-order handle
/// table is reproducible from the scene alone.
fn write<P: PhysicsPose>(world: &mut World, ctx: &SystemCtx, settings: &PhysicsSettings) {
    // First, before any early return, so the record never outlives the tick it was made in.
    let teleports = world
        .remove_resource::<KinematicTeleports>()
        .map(|KinematicTeleports(entities)| entities)
        .unwrap_or_default();
    // Taken out before the query so the staleness check below can read it: `query_ids` borrows
    // `world` mutably, and a `PhysicsWorld` still living in `world`'s resources would not be
    // readable at the same time.
    let physics_taken = world.remove_resource::<PhysicsWorld>();
    // Taken for the same reason: resolving a mesh needs it mutably beside the query.
    let mut meshes = world.remove_resource::<ColliderMeshes>();

    let mut live: Vec<Claimed> = world
        .query_ids::<(&Collider, &P)>()
        .filter(|(_, (collider, _))| claims(collider))
        .map(|(entity, (collider, pose))| {
            let mesh = resolve_mesh(meshes.as_mut(), collider);
            let stale = physics_taken
                .as_ref()
                .is_some_and(|physics| physics.is_stale(entity, collider, mesh_hash_of(&mesh)));
            let source = stale.then(|| collider.clone());
            (
                entity,
                pose.translation(),
                pose.rotation_quat(),
                source,
                mesh,
            )
        })
        .collect();
    if let Some(meshes) = meshes {
        world.insert_resource(meshes);
    }
    // A motor owns its entity's capsule: a `Collider` beside it is dropped, so the motor row below is
    // the only one. Reported (not retried per tick) through the rejection queue once a world exists.
    let motor_rows = motor_rows::<P>(world);
    let mut motor_rejections = Vec::new();
    if !motor_rows.is_empty() {
        live.retain(|(entity, ..)| {
            let doubled = motor_rows.iter().any(|row| row.entity == *entity);
            if doubled {
                motor_rejections.push((
                    *entity,
                    code::CHARACTER_MOTOR_WITH_COLLIDER,
                    "the entity carries a Collider beside its CharacterMotor; the motor owns its capsule".to_string(),
                ));
            }
            !doubled
        });
    }
    // A pose relative to a parent is replaced by its world pose, and only a kinematic body may have
    // one: a rigid or static body under a parent is dropped from `live`, so `retire` below removes any
    // body it had. `entity_indices_with` records no query access, and a world with no `Parent` skips
    // the pass entirely.
    if world
        .entity_indices_with(TypeId::of::<Parent>())
        .next()
        .is_some()
    {
        live.retain_mut(|(entity, translation, rotation, _, _)| {
            let Some((world_translation, world_rotation)) = P::derived_world_pose(world, *entity)
            else {
                return true;
            };
            if !matches!(world.get::<Collider>(*entity), Ok(Some(c)) if matches!(c.body, Body::Kinematic(_))) {
                return false;
            }
            (*translation, *rotation) = (world_translation, world_rotation);
            true
        });
    }
    // Motor rows join after the parent pass: a motor writes world-space feet, so it is never
    // re-derived from a parent. A row is built from the motor's synthesized collider, which stays
    // `Some` until the body exists.
    for row in motor_rows {
        match row.collider {
            Err(reason) => {
                motor_rejections.push((row.entity, code::CHARACTER_MOTOR_INVALID, reason))
            }
            Ok(collider) => {
                let stale = physics_taken.as_ref().is_none_or(|physics| {
                    !physics.has_body(row.entity) || physics.is_stale(row.entity, &collider, None)
                });
                live.push((
                    row.entity,
                    row.centre,
                    [0.0, 0.0, 0.0, 1.0],
                    stale.then_some(collider),
                    Ok(None),
                ));
            }
        }
    }
    live.sort_by_key(|(entity, ..)| *entity);
    // Nothing claimed and no world yet: allocate nothing. A 2D game that registers this verb pays
    // the query and no more, and the resource only ever exists for a scene that has physics in it.
    if live.is_empty() && physics_taken.is_none() && motor_rejections.is_empty() {
        return;
    }
    // A sorted side list rather than a map: both walks are ascending, so one cursor joins them.
    let velocities: Vec<(EntityId, Velocity)> = world
        .query_ids::<(&Velocity,)>()
        .map(|(entity, (velocity,))| (entity, *velocity))
        .collect();

    // `PhysicsSpace::Free` reads no `Plane` at all: a game may build one whether or not it is
    // two-dimensional, so a 3D world must be able to ignore it rather than merely not have one.
    // `PhysicsSpace::Planar` reads `Plane::of` — an absent resource means `Plane::Xz` — so a bare
    // host (`ludion render`, `ludion sweep`) locks the same axis a game with no `Plane` resource
    // would, rather than applying no locks at all.
    let plane = match settings.space {
        PhysicsSpace::Free => None,
        PhysicsSpace::Planar => Some(Plane::of(world)),
    };
    let mut physics =
        physics_taken.unwrap_or_else(|| PhysicsWorld::new(ctx.time.rate()).with_plane(plane));
    physics.apply_settings(settings);
    for (entity, code, reason) in motor_rejections {
        physics.queue_rejection(ShapeRejection {
            entity,
            code,
            reason,
            related: Vec::new(),
        });
    }

    let present: Vec<EntityId> = live.iter().map(|(entity, ..)| *entity).collect();
    let gone: Vec<EntityId> = physics
        .retire(&present)
        .into_iter()
        .filter(|entity| !world.is_alive(*entity))
        .collect();
    physics.note_despawned(&gone);

    let mut cursor = 0;
    for (entity, translation, rotation, source, mesh) in &live {
        let (entity, translation, rotation) = (*entity, *translation, *rotation);
        let mesh = mesh.clone();
        while cursor < velocities.len() && velocities[cursor].0 < entity {
            cursor += 1;
        }
        let velocity = velocities
            .get(cursor)
            .filter(|(candidate, _)| *candidate == entity)
            .map(|&(_, velocity)| velocity);
        if let Some(collider) = source {
            if physics.retune_in_place(entity, collider, mesh_hash_of(&mesh)) {
                physics.push(entity, translation, rotation, velocity);
                if teleports.contains(&entity) {
                    physics.snap_kinematic(entity, translation, rotation);
                }
            } else {
                physics.rebuild(entity, collider, translation, rotation, velocity, mesh);
            }
        } else if physics.has_body(entity) {
            physics.push(entity, translation, rotation, velocity);
            if teleports.contains(&entity) {
                physics.snap_kinematic(entity, translation, rotation);
            }
        } else if let Ok(Some(collider)) = world.get::<Collider>(entity) {
            physics.create(entity, collider, translation, rotation, velocity, mesh);
        }
    }

    let joints: Vec<(EntityId, Joint)> = world
        .query_ids::<(&Joint,)>()
        .map(|(entity, (joint,))| (entity, joint.clone()))
        .collect();
    physics.reconcile_joints(&joints);
    let motors: Vec<(EntityId, JointMotor)> = world
        .query_ids::<(&JointMotor,)>()
        .map(|(entity, (motor,))| (entity, motor.clone()))
        .collect();
    physics.drive_joints(&motors);
    physics.apply_queued_impulses();

    world.insert_resource(physics);
}

/// One motor entity this tick: the body-centre pose it writes, and the synthesized `Collider` its
/// body is built from — or the `L1294` reason the motor cannot have one.
struct MotorRow {
    entity: EntityId,
    centre: [f32; 3],
    collider: Result<Collider, String>,
}

/// Every `CharacterMotor` with a pose, ascending by entity. The pose is the **feet**; the body
/// centre sits half the height in force above it.
fn motor_rows<P: PhysicsPose>(world: &mut World) -> Vec<MotorRow> {
    let motors: Vec<(EntityId, CharacterMotor, [f32; 3])> = world
        .query_ids::<(&CharacterMotor, &P)>()
        .map(|(entity, (motor, pose))| (entity, motor.clone(), pose.translation()))
        .collect();
    motors
        .into_iter()
        .map(|(entity, motor, feet)| {
            let state = world.get::<CharacterMotorState>(entity).ok().flatten();
            let height = height_in_force(&motor, state);
            let overridden = CharacterMotor { height, ..motor };
            let collider = match check(&overridden) {
                Ok(()) => Ok(overridden.solver_collider(height)),
                Err((pointer, reason)) => Err(motor_invalid(&reason, pointer).message),
            };
            MotorRow {
                entity,
                centre: [feet[0], feet[1] + height / 2.0, feet[2]],
                collider,
            }
        })
        .collect()
}

/// Advances the solver by one fixed timestep. A world with no [`PhysicsWorld`] resource has
/// nothing to step — no collider in it has been claimed yet.
fn step(world: &mut World, _ctx: &SystemCtx) {
    if let Some(physics) = world.resource_mut::<PhysicsWorld>() {
        physics.step();
    }
}

/// Writes each rigid body's resolved pose and velocity back into its components, in handle-table
/// order, then rebases the edit-detection baseline to the pose the components read back — a pose
/// component may store the pose lossily (Euler angles), and the next tick's write compares against
/// what the component reports, not what the solver held.
///
/// A rigid body whose entity has no [`Velocity`] component simply does not expose its velocity —
/// the mirror never inserts one, because a component appearing under a game is a change to the
/// world hash that nothing asked for.
fn mirror<P: PhysicsPose>(world: &mut World, _ctx: &SystemCtx) {
    let Some(mut physics) = world.remove_resource::<PhysicsWorld>() else {
        return;
    };
    let resolved = physics.resolved();
    let broken: Vec<EntityId> = physics
        .joint_events()
        .iter()
        .map(|event| event.joint)
        .collect();
    world.insert_resource(physics);
    let mut read_back = Vec::with_capacity(resolved.len());
    for (entity, translation, rotation, velocity) in resolved {
        if let Ok(Some(pose)) = world.get_mut::<P>(entity) {
            pose.set_translation(translation);
            pose.set_rotation_quat(rotation);
            read_back.push((entity, pose.translation(), pose.rotation_quat()));
        }
        if let Ok(Some(existing)) = world.get_mut::<Velocity>(entity) {
            *existing = velocity;
        }
    }
    if let Some(physics) = world.resource_mut::<PhysicsWorld>() {
        physics.rebase_mirrored(&read_back);
    }
    // A joint exists iff its component does (`Plan/physics.md` design decision 4): the break
    // itself already removed the rapier joint in `step`, this just catches the component up.
    for carrier in broken {
        let _ = world.remove::<Joint>(carrier);
    }
}

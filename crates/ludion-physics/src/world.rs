//! [`PhysicsWorld`]: the solver state as one world resource, and the mapping from the authored
//! [`Collider`]/[`Velocity`] surface onto it.

use bincode::Options;
use core::fmt;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use ludion_core::codes::code;
use ludion_core::collections::StableMap;
use ludion_core::diagnostics::{Diagnostic, DiagnosticCode};
use ludion_core::ids::EntityId;
use ludion_core::math::euler_xyz_degrees_to_quat;
use ludion_core::suggest::nearest;
use ludion_core::time::{FrameIndex, GameClock, TickRate};
use ludion_ecs::World;
use ludion_spatial::{
    AxisLock, Body, Collider, CombineRule, CompoundPart, Joint, JointKind, JointMotor, Plane,
    RigidBody, Shape, Velocity,
};
use rapier3d::parry::utils::PoseOpt;
use rapier3d::prelude::{
    Array2, CoefficientCombineRule, Collider as RapierCollider, ColliderBuilder, ColliderHandle,
    ContactPair, Group, ImpulseJointHandle, InteractionGroups, InteractionTestMode, JointAxis,
    LockedAxes, NEW_CONTACT_BIT, PhysicsWorld as RapierWorld, Pose, Real, RigidBodyBuilder,
    RigidBodyHandle, RigidBodyType, Rotation, ShapeType, SharedShape, TriMeshFlags, Vector,
};
use serde::{Deserialize, Serialize};

use crate::contacts::{ContactEvent, ContactPhase};
use crate::impulse::{BlastHit, ImpulseMiss, QueuedImpulse};
use crate::joints::{
    JointBroken, JointEntry, MotorRows, apply_motor, build_generic_joint, current_motor,
    desired_motor, motor_off_target,
};
use crate::layer_set::LayerSet;
use crate::meshes::TriangleMesh;
use crate::motor::{LaneReport, MoveReport};
use crate::queries::QueryFilter;
use crate::sensors::SensorEvent;
use crate::surface_tag::{SurfaceId, SurfaceTable};
use crate::systems::{CollisionLayers, DEFAULT_SLEEP_AFTER_TICKS, PhysicsSettings};

/// Gravity applied to every rigid body when nothing else is asked for: one standard gravity down
/// the world's `y` axis.
pub const DEFAULT_GRAVITY: [f32; 3] = [0.0, -9.81, 0.0];

/// The layer a collider with no `layer` is on, and layer index 0 of every table.
pub(crate) const DEFAULT_LAYER: &str = "default";

/// The `L1186` reason for a layer `name` no table in `declared` names, with the nearest declared
/// name when there is one.
fn unknown_layer(name: &str, declared: &[String]) -> String {
    let hint = match nearest(name, declared.iter().map(String::as_str)) {
        Some(near) => format!("nearest match is `{near}`"),
        None => "declare it under `physics.layers`".to_string(),
    };
    format!("layer `{name}` names no layer the scene's `physics.layers` declares; {hint}")
}

/// What went wrong reading or writing a [`PhysicsWorld`] snapshot, or performing a query against
/// one.
///
/// A collider whose authored shape cannot be built gets no body (see [`PhysicsWorld::shape_kind`]);
/// that rejection is queued on [`take_rejections`](PhysicsWorld::take_rejections) rather than
/// turned into a [`PhysicsError`] here, since only the caller can see the engine's diagnostics sink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhysicsError {
    /// The solver state could not be encoded to bytes.
    Encode(String),
    /// The bytes are not a physics snapshot this build can read — a truncated blob, or one written
    /// by a different codec or a different solver pin.
    Decode(String),
    /// A query's shape could not be built by the solver — the same faults
    /// [`PhysicsWorld::shape_kind`] reports for a scene-authored collider, `L1181` for a degenerate
    /// `convex` hull and `L1182` for a malformed `heightfield` grid.
    QueryShape {
        /// `L1181` or `L1182`.
        code: DiagnosticCode,
        /// Why the shape could not be built.
        reason: String,
    },
    /// A query or call argument was non-finite, zero where a direction may not be, negative where a
    /// distance may not be, or outside a time scale's `[MIN_TIME_SCALE, 1]`. The message names the argument and
    /// the value found.
    QueryInput(String),
}

impl fmt::Display for PhysicsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PhysicsError::Encode(why) => {
                write!(f, "could not encode the physics snapshot: {why}")
            }
            PhysicsError::Decode(why) => write!(
                f,
                "could not decode the physics snapshot: {why}; the bytes were written by a \
                 different snapshot codec or solver pin"
            ),
            PhysicsError::QueryShape { code, reason } => {
                write!(f, "the query shape could not be built: {reason} ({code})")
            }
            PhysicsError::QueryInput(why) => write!(f, "bad query input: {why}"),
        }
    }
}

impl std::error::Error for PhysicsError {}

/// The parry shape an authored [`Shape`] maps to — the observable half of the shape-mapping
/// contract, so a test or a tool can assert the mapping without naming a solver type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShapeKind {
    /// A sphere: what `sphere` and the in-plane `circle` both build.
    Ball,
    /// An axis-aligned box: what `box` and the in-plane `aabb` both build.
    Cuboid,
    /// A capsule along the world `y` axis.
    Capsule,
    /// A cylinder along the world `y` axis.
    Cylinder,
    /// A convex hull over the authored points.
    ConvexPolyhedron,
    /// A height grid.
    HeightField,
    /// Several shapes joined into one collider: what `compound` builds.
    Compound,
    /// A triangle mesh with its internal edges fixed: what `mesh` builds.
    TriMesh,
    /// Any other parry shape: unreachable through the authored surface, and the value a mapping
    /// with no name here reads as.
    Other,
}

/// One entity's `Collider` the solver could not build a shape for.
///
/// This crate cannot see the engine's `Diagnostics` resource (it does not depend on
/// `ludion-engine`), so a rejection is queued here rather than reported directly;
/// [`PhysicsWorld::take_rejections`] hands the queue to a caller that can report it once per
/// `(code, entity)` and keep the retry from repeating every tick.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShapeRejection {
    /// The entity whose `Collider` was rejected.
    pub entity: EntityId,
    /// `L1181` for a degenerate `convex` hull, `L1182` for a malformed `heightfield` grid, `L1186`
    /// for a `layer` the scene's `physics.layers` does not declare.
    pub code: DiagnosticCode,
    /// Why the shape could not be built, in the same wording the `Collider` validator uses for the
    /// same fault.
    pub reason: String,
    /// The loader's own diagnostics behind an `L1212`; empty otherwise.
    pub related: Vec<Diagnostic>,
}

/// One carrier's `JointMotor` that cannot drive its `Joint`.
///
/// Queued here rather than reported for the [`ShapeRejection`] reason;
/// [`PhysicsWorld::take_motor_mismatches`] hands the queue to a caller that can report it as
/// `L3037`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MotorMismatch {
    /// The entity carrying the mismatched `JointMotor` and its `Joint`.
    pub carrier: EntityId,
    /// Why the motor cannot drive the joint: a `fixed` kind, a `target` arity the kind does not
    /// take, or a non-finite or negative value that bypassed `L1188`.
    pub reason: String,
}

/// Whether the solver moves a body, and who owns its pose.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum BodyKind {
    /// Never moves.
    Fixed,
    /// The ECS owns the pose; it is pushed in every tick and never written back.
    Kinematic,
    /// The solver owns the pose; the ECS writes are teleports and impulses.
    Rigid,
}

/// The last values the mirror wrote into the ECS for one rigid body.
///
/// This is what makes a game edit detectable without a per-float tolerance: the write system
/// compares the component against **the exact bits the component read back after the mirror last
/// wrote it** (velocity: the bits written), never against a prediction, so a difference is an edit
/// by definition and an untouched component is bit-equal.
/// It is also what lets a resting body sleep — pushing an unchanged velocity in every tick would
/// wake it forever.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
struct Mirrored {
    translation: [f32; 3],
    rotation: [f32; 4],
    linear: [f32; 3],
    angular: [f32; 3],
}

/// One `static` body as [`PhysicsWorld::static_bodies`] reports it.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct StaticBody {
    /// The entity that owns the body.
    pub entity: EntityId,
    /// Minimum corner of the posed collider's world AABB.
    pub min: [f32; 3],
    /// Maximum corner of the posed collider's world AABB.
    pub max: [f32; 3],
    /// Changes when the collider is rebuilt or the body's pose changes, even when the AABB does not
    /// (a cube turned about its own axis).
    pub revision: u64,
}

/// One row of the creation-order handle table.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct BodyEntry {
    pub(crate) entity: EntityId,
    pub(crate) kind: BodyKind,
    pub(crate) body: RigidBodyHandle,
    pub(crate) collider: ColliderHandle,
    mirrored: Option<Mirrored>,
    /// The `Collider` this body was built from — compared with `PartialEq` against the entity's
    /// current one every tick to detect a live edit (shape, body kind, mass, lock or layer) that
    /// [`create`](PhysicsWorld::create) baked in once and never re-reads (an edit of only the
    /// contact material, or a rigid body's damping, `gravity_scale` or `sleep_after_ticks`, is
    /// applied in place by `retune_in_place` instead). Rides the snapshot, since a restore must have it to
    /// compare against on the very next tick; encoded through [`source_as_json`] rather than
    /// derived straight into bincode (see its doc for why).
    #[serde(with = "source_as_json")]
    pub(crate) source: Collider,
    /// The [`TriangleMesh::hash`] a `mesh` body was built from, `None` for every other shape. A
    /// restore compares it against the freshly installed library, so an edited mesh document is
    /// rebuilt into its body while an unchanged one is left alone.
    pub(crate) mesh_hash: Option<u64>,
}

/// What the write system resolved for one collider's mesh: `Ok(None)` for a shape that is not a
/// `mesh`, `Ok(Some(_))` for a mesh that loaded, and the loader's diagnostic for one that did not.
pub(crate) type MeshInput = Result<Option<Arc<TriangleMesh>>, Diagnostic>;

/// The content hash of a resolved mesh, `None` when there is none.
pub(crate) fn mesh_hash_of(mesh: &MeshInput) -> Option<u64> {
    mesh.as_ref()
        .ok()
        .and_then(Option::as_ref)
        .map(|mesh| mesh.hash)
}

/// (De)serializes a [`BodyEntry::source`] `Collider` or a [`JointEntry`](crate::joints::JointEntry)
/// `source` `Joint` as JSON bytes rather than letting bincode encode it directly.
///
/// Both types carry a `#[serde(skip_serializing_if = "Option::is_none")]` field —
/// `RigidBody::sleep_after_ticks` and `Joint::break_impulse` — right for the scene writer's
/// minimal-diff YAML, wrong for bincode's positional struct encoding: skipping the field on
/// `Serialize` when it is `None` writes one fewer value than the derived `Deserialize`
/// unconditionally reads back, desyncing the two byte-for-byte the moment the field is unset. JSON
/// is self-describing (a missing field just falls back to `#[serde(default)]`), so wrapping the
/// value in a JSON byte string sidesteps the mismatch entirely; the cost lands only in
/// [`to_bytes`](PhysicsWorld::to_bytes)/[`from_bytes`](PhysicsWorld::from_bytes), never in the
/// per-tick [`is_stale`](PhysicsWorld::is_stale)/staleness comparisons, which still run against the
/// decoded value in memory.
pub(crate) mod source_as_json {
    use serde::de::DeserializeOwned;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub(crate) fn serialize<T: Serialize, S: Serializer>(
        value: &T,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let json = serde_json::to_vec(value).map_err(serde::ser::Error::custom)?;
        json.serialize(serializer)
    }

    pub(crate) fn deserialize<'de, T: DeserializeOwned, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<T, D::Error> {
        let json = Vec::<u8>::deserialize(deserializer)?;
        serde_json::from_slice(&json).map_err(serde::de::Error::custom)
    }
}

/// [`Plane`] as snapshot bytes. `Plane` is pure data in `ludion-spatial` and carries no serde
/// derives, so the snapshot spells it out rather than growing a dependency's type a serde impl.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum PlaneCode {
    Xz,
    Xy,
}

impl From<Plane> for PlaneCode {
    fn from(plane: Plane) -> Self {
        match plane {
            Plane::Xz => PlaneCode::Xz,
            Plane::Xy => PlaneCode::Xy,
        }
    }
}

impl From<PlaneCode> for Plane {
    fn from(code: PlaneCode) -> Self {
        match code {
            PlaneCode::Xz => Plane::Xz,
            PlaneCode::Xy => Plane::Xy,
        }
    }
}

/// A `Started` that is still summing its pair's impulse over the first base tick of simulated
/// time after the touch. `event.impulse` is the running sum; `elapsed` counts base ticks.
#[derive(Clone, Copy, Serialize, Deserialize)]
struct ContactWindow {
    event: ContactEvent,
    elapsed: f32,
}

/// Simulated time, in base ticks, a `Started` collects its impulse over. The tolerance absorbs
/// f32 sums of small scales (20 steps of 0.05).
const IMPULSE_WINDOW: f32 = 1.0 - 1e-4;

/// First bytes of a tagged physics blob. An untagged blob starts with an arena length, which cannot
/// spell this.
const PHYSICS_BLOB_MAGIC: [u8; 4] = *b"LPB1";

/// The layout version of a tagged blob's body (the field set of `SnapshotOwned`). A blob written
/// at another version is refused rather than decoded positionally into the wrong shape.
pub const PHYSICS_BLOB_VERSION: u32 = 1;

/// The snapshot as written. Borrowed, because the solver state is not `Clone`.
#[derive(Serialize)]
struct SnapshotRef<'a> {
    rapier: &'a RapierWorld,
    table: &'a [BodyEntry],
    plane: Option<PlaneCode>,
    touching: &'a [(EntityId, EntityId)],
    contacts: &'a [ContactEvent],
    windows: &'a [ContactWindow],
    joints: &'a [JointEntry],
    joint_events: &'a [JointBroken],
    blast_hits: &'a [BlastHit],
    fresh_colliders: &'a [ColliderHandle],
    last_step_dt: f32,
    sensing: &'a [(EntityId, EntityId)],
    sensor_events: &'a [SensorEvent],
    despawned: &'a [EntityId],
    last_write_frame: Option<u64>,
    last_write_held: Option<u64>,
}

/// The snapshot as read — the same fields in the same order, so the two encode identically. The
/// encoding is positional: **any field change here, or in a type it holds, bumps
/// [`PHYSICS_BLOB_VERSION`].**
#[derive(Deserialize)]
struct SnapshotOwned {
    rapier: RapierWorld,
    table: Vec<BodyEntry>,
    plane: Option<PlaneCode>,
    touching: Vec<(EntityId, EntityId)>,
    contacts: Vec<ContactEvent>,
    windows: Vec<ContactWindow>,
    joints: Vec<JointEntry>,
    joint_events: Vec<JointBroken>,
    blast_hits: Vec<BlastHit>,
    fresh_colliders: Vec<ColliderHandle>,
    last_step_dt: f32,
    sensing: Vec<(EntityId, EntityId)>,
    sensor_events: Vec<SensorEvent>,
    despawned: Vec<EntityId>,
    last_write_frame: Option<u64>,
    last_write_held: Option<u64>,
}

/// Contact pairs of one body-kind combination in the narrow phase's contact graph.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PairCount {
    /// Pairs the broad phase tracks: overlapping bounding boxes, the broad-phase cost.
    pub tracked: usize,
    /// Pairs with an active contact: the narrow-phase and solver cost.
    pub touching: usize,
}

/// The contact graph's pairs by body kind, as returned by [`PhysicsWorld::contact_pair_census`].
/// A collider with no parent body counts as static; both kinematic body types count as kinematic.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ContactPairCensus {
    /// Static against static.
    pub static_static: PairCount,
    /// Kinematic against static.
    pub kinematic_static: PairCount,
    /// Kinematic against kinematic.
    pub kinematic_kinematic: PairCount,
    /// Rigid against static.
    pub rigid_static: PairCount,
    /// Rigid against kinematic.
    pub rigid_kinematic: PairCount,
    /// Rigid against rigid.
    pub rigid_rigid: PairCount,
}

/// One step's solver timings (milliseconds, from rapier's profiler timers) and body counts, as
/// returned by [`PhysicsWorld::step_breakdown`]. The stage timers overlap: `collision_detection_ms`
/// contains the broad and narrow phases, `solver_ms` contains the velocity timers, and
/// `substeps` is the solver iteration count, which doubles as the substep count in the pinned build.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StepBreakdown {
    /// The whole `PhysicsPipeline::step`.
    pub step_ms: f64,
    /// Propagating user changes into the solver sets.
    pub user_changes_ms: f64,
    /// Broad plus narrow phase.
    pub collision_detection_ms: f64,
    /// The broad phase alone.
    pub broad_phase_ms: f64,
    /// The narrow phase alone.
    pub narrow_phase_ms: f64,
    /// The closing broad-phase update that keeps scene queries valid.
    pub final_broad_phase_ms: f64,
    /// Island construction and sleeping.
    pub island_construction_ms: f64,
    /// Solver contact-graph maintenance.
    pub constraints_collection_ms: f64,
    /// Kinematic and dynamic updates.
    pub update_ms: f64,
    /// Constraint resolution and position update.
    pub solver_ms: f64,
    /// Assembling the velocity constraints.
    pub velocity_assembly_ms: f64,
    /// Resolving the velocity constraints.
    pub velocity_resolution_ms: f64,
    /// Updating body velocities.
    pub velocity_update_ms: f64,
    /// Writing forces back to user-visible data.
    pub velocity_writeback_ms: f64,
    /// Continuous collision detection.
    pub ccd_ms: f64,
    /// Rigid bodies in the world.
    pub bodies: usize,
    /// Bodies awake after the step.
    pub awake_bodies: usize,
    /// Contact pairs with an active contact after the step.
    pub contact_pairs: usize,
    /// Impulse joints in the world.
    pub impulse_joints: usize,
    /// The solver iteration count (also the substep count).
    pub substeps: u32,
    /// Whether this step reused the detect phase [`prime`](crate::prime) ran at scene commit, so
    /// the user-change, broad-phase and narrow-phase timers above read zero for it.
    pub primed: bool,
}

/// The solver state for one world: the rigid-body, collider, joint, island, broad-phase and
/// narrow-phase sets, the integration parameters built from the session's [`TickRate`], and the
/// creation-order handle table that binds them to ECS entities.
///
/// A world resource, not a component: it is invisible to `WorldHash`, and what reaches the hash is
/// the pose and [`Velocity`] the mirror writes back into components. A divergence in the solver
/// therefore shows up in the hash trace one tick later, which is what the per-tick trace gate
/// checks.
///
/// # The handle table is the only iteration order
///
/// The table is a `Vec` in **creation order**, and every loop in this crate walks it. Nothing
/// whose order reaches an output iterates a rapier set unsorted, and the table is never rebuilt by
/// walking one — it rides in the snapshot instead. Two walks do visit rapier's own order, because
/// their result does not depend on it: `record_detection_poses` (one independent write per
/// collider) and `collect_contacts` and `collect_sensors` (both sort the pairs by entity before using them). The
/// entity→row map beside it is a lookup accelerator only; nothing that reaches the world hash
/// iterates it.
///
/// # Gravity
///
/// [`DEFAULT_GRAVITY`] is one standard gravity down `y`, which is what a side-view
/// [`Plane::Xy`] game wants unchanged. A top-down [`Plane::Xz`] game usually wants **zero** — its
/// ground plane is the screen — and sets it through [`PhysicsSettings::gravity`], the scene's
/// `physics.gravity`; nothing infers it from the plane. [`apply_settings`](PhysicsWorld::apply_settings)
/// writes gravity into the solver every tick, so it is never set any other way.
pub struct PhysicsWorld {
    pub(crate) rapier: RapierWorld,
    pub(crate) table: Vec<BodyEntry>,
    pub(crate) rows: StableMap<EntityId, usize>,
    /// Each live claimed entity's ECS spawn rank, entity-sorted, refreshed by every write before
    /// anything reads it. Not part of the snapshot (the write refills it); never hashed.
    ranks: Vec<(EntityId, u64)>,
    /// Each entity's authored `QueryLayers`, entity-sorted, and the same list resolved to layer
    /// bitmasks against the current layer table. Queries see them; the solver never does. Not part
    /// of the snapshot (the write refills them); never hashed.
    query_layers: Vec<(EntityId, LayerSet)>,
    query_masks: Vec<(EntityId, u32)>,
    /// Whether `query_masks` was resolved with rejections queued; a restore resolves silently
    /// (the layer table is not applied yet) and the next write reports.
    query_reported: bool,
    /// The scene's declared surfaces, each entity's authored `SurfaceTag` name (entity-sorted) and
    /// the same list resolved to ids. Hits report them; the solver never does. Not part of the
    /// snapshot (the write refills them); never hashed.
    surfaces: Option<SurfaceTable>,
    surface_tags: Vec<(EntityId, String)>,
    surface_ids: Vec<(EntityId, SurfaceId)>,
    /// Whether `surface_ids` was resolved with rejections queued; a restore resolves silently.
    surfaces_reported: bool,
    /// Each entity's authored `SurfaceOverlay` (entity-sorted) and the resolved list sorted
    /// `(Reverse(priority), entity)`: the first overlay containing a point wins.
    surface_overlays: Vec<(EntityId, String, i32)>,
    overlay_ids: Vec<(EntityId, SurfaceId, i32)>,
    pub(crate) plane: Option<Plane>,
    tick_rate: TickRate,
    sleep_after_ticks: u32,
    /// The layer table [`apply_settings`](PhysicsWorld::apply_settings) last applied. Not part of the
    /// snapshot: the groups ride inside the serialized colliders, and the first `apply_settings`
    /// after a restore re-derives the table from the loaded scene.
    layers: Option<CollisionLayers>,
    /// The `Collider` last rejected for each entity still carrying one, as it was when rejected,
    /// with the mesh hash it saw (a mesh that fails retries once its content changes) and the code
    /// it was rejected under — so `create` can tell "still the same bad shape" (skip
    /// the rebuild attempt) from "the game changed it" (try again) without keeping a second copy of
    /// every collider that did build, and a layer-table change retries only the layer rejections.
    rejected: StableMap<EntityId, (Collider, Option<u64>, DiagnosticCode)>,
    /// Rejections since the last [`take_rejections`](PhysicsWorld::take_rejections), in the order
    /// `create` produced them. Not part of the snapshot: see [`take_rejections`](PhysicsWorld::take_rejections).
    pending_rejections: Vec<ShapeRejection>,
    /// The pairs that were touching as of the last step, canonical (`a < b`) and sorted ascending
    /// — this step's comparison baseline for [`collect_contacts`](PhysicsWorld::collect_contacts).
    /// Part of the snapshot, since a restore must know what was already touching to avoid emitting
    /// a spurious `Started` for a pair the uninterrupted run would have seen as already in contact.
    touching: Vec<(EntityId, EntityId)>,
    /// This step's contact transitions. See [`contact_events`](PhysicsWorld::contact_events).
    contacts: Vec<ContactEvent>,
    /// Pairs whose `Started` is still collecting impulse, ascending by pair; each is also in
    /// `touching`. Part of the snapshot, so a restore mid-window emits the same event.
    windows: Vec<ContactWindow>,
    /// Every collider's pose as the step's collision detection saw it, by collider arena index,
    /// refilled before each step into one reused buffer. The contact manifolds' local frames are
    /// relative to these poses, not to the post-step ones. Not part of the snapshot.
    detection_poses: Vec<Option<Pose>>,
    /// The joint handle table — creation order, the only iteration order, the `table` precedent.
    /// Rides the snapshot.
    pub(crate) joints: Vec<JointEntry>,
    /// Lookup accelerator for `joints`, carrier → row. Never iterated for anything a trace or the
    /// world hash depends on; not part of the snapshot, rebuilt by
    /// [`reindex_joints`](PhysicsWorld::reindex_joints) after restore, the `rows` precedent.
    joint_rows: StableMap<EntityId, usize>,
    /// This step's joint breaks. See [`joint_events`](PhysicsWorld::joint_events).
    joint_events: Vec<JointBroken>,
    /// Carriers `reconcile_joints` found missing a body since the
    /// last [`take_missing_joints`](PhysicsWorld::take_missing_joints). Not part of the snapshot —
    /// the `pending_rejections` precedent.
    pending_missing: Vec<EntityId>,
    /// Carriers whose `JointMotor` [`drive_joints`](PhysicsWorld::drive_joints) currently finds
    /// unable to drive their `Joint`, sorted ascending — the latch that queues a mismatch only on
    /// the transition into it. Not part of the snapshot: a restored world re-detects and re-queues,
    /// which the latch on the reading side absorbs.
    motor_mismatched: Vec<EntityId>,
    /// Mismatches since the last [`take_motor_mismatches`](PhysicsWorld::take_motor_mismatches).
    /// Not part of the snapshot, the `pending_missing` precedent.
    pending_motor_mismatches: Vec<MotorMismatch>,
    /// Impulse calls waiting for the write system, in call order. Never part of the snapshot:
    /// [`to_bytes`](PhysicsWorld::to_bytes) refuses a world holding any.
    pub(crate) pending_impulses: Vec<QueuedImpulse>,
    /// How many blasts are queued this tick — the next [`radial_impulse`](PhysicsWorld::radial_impulse)
    /// call's index. Not part of the snapshot, reset when the queue drains.
    pub(crate) blasts_queued: u32,
    /// The last tick's blast hits. See [`blast_hits`](PhysicsWorld::blast_hits). Part of the
    /// snapshot, the `contacts` precedent.
    pub(crate) blast_hits: Vec<BlastHit>,
    /// Dropped at-centre and at-point impulses since the last
    /// [`take_impulse_misses`](PhysicsWorld::take_impulse_misses). Not part of the snapshot, the
    /// `pending_motor_mismatches` precedent.
    pub(crate) pending_impulse_misses: Vec<ImpulseMiss>,
    /// Colliders inserted since the last step, so absent from the last step's BVH. Rapier resolves a
    /// BVH leaf by slot and ignores its generation, so a stale leaf can land on one of these; a
    /// blast skips them. Cleared by a real step; rides the snapshot, since a frozen step keeps it
    /// across the tick boundary.
    pub(crate) fresh_colliders: Vec<ColliderHandle>,
    /// The smallest [`scale_time`](PhysicsWorld::scale_time) asked for since the last step, `1.0`
    /// when none. Not part of the snapshot: [`to_bytes`](PhysicsWorld::to_bytes) refuses a world
    /// holding one below `1.0`.
    pub(crate) time_scale: f32,
    /// Whether [`freeze`](PhysicsWorld::freeze) was called since the last step. Not part of the
    /// snapshot, guarded like `time_scale`.
    pub(crate) frozen: bool,
    /// See [`last_step_dt`](PhysicsWorld::last_step_dt). Rides the snapshot.
    pub(crate) last_step_dt: f32,
    /// The `(sensor, other)` pairs overlapping as of the last non-frozen step, sorted ascending —
    /// the baseline `collect_sensors` diffs against. Rides the snapshot, the `touching` precedent.
    pub(crate) sensing: Vec<(EntityId, EntityId)>,
    /// `sensing`'s second column, so `sensor_occupants` can hand out a slice. Derived: rebuilt
    /// after every collect and on restore, never snapshotted.
    pub(crate) occupants: Vec<EntityId>,
    /// This step's sensor transitions. See [`sensor_events`](PhysicsWorld::sensor_events). Rides
    /// the snapshot.
    pub(crate) sensor_events: Vec<SensorEvent>,
    /// Entities the write system found despawned, sorted, kept until a non-frozen step has judged
    /// the exits they cause. Rides the snapshot, since a frozen step keeps it across ticks.
    pub(crate) despawned: Vec<EntityId>,
    /// The frame the write pass last ran in, which stamps a motor's report with the frame of the
    /// tick it was called in. Rides the snapshot.
    pub(crate) last_write_frame: Option<u64>,
    /// The `GameClock` frames held at that write pass, so a motor can tell whether gameplay time
    /// advanced into its tick. Rides the snapshot.
    pub(crate) last_write_held: Option<u64>,
    /// See [`last_step_primed`](PhysicsWorld::last_step_primed). Measurement only; never read by
    /// the simulation and not part of the snapshot.
    pub(crate) last_step_primed: bool,
    /// Each motor's last `move_character` report and the frame it ran in. Never hashed or
    /// snapshotted: a restore empties it.
    pub(crate) move_reports: StableMap<EntityId, (FrameIndex, MoveReport)>,
    /// Feature edges of each mesh content hash, for the wireframe read. A host read cache: no sim
    /// path reads it, and it is not part of the snapshot.
    pub(crate) wire_edges: Mutex<BTreeMap<u64, Arc<[[u32; 2]]>>>,
}

impl PhysicsWorld {
    /// An empty world stepping at `tick_rate`, with [`DEFAULT_GRAVITY`] and no plane locks.
    ///
    /// Every solver tunable other than the timestep stays at the pinned build's default — solver
    /// iteration counts, contact softness, the prediction distance, the length unit — so a scene's
    /// behaviour is a function of the pin and the authored `Collider`s alone
    /// (`ludion devdocs ludion-physics` tabulates which parameter each authored field reaches).
    ///
    /// # Example
    /// ```
    /// use ludion_core::time::TickRate;
    /// use ludion_physics::{PhysicsWorld, DEFAULT_GRAVITY};
    ///
    /// let world = PhysicsWorld::new(TickRate::DEFAULT);
    /// assert_eq!(world.gravity(), DEFAULT_GRAVITY);
    /// assert_eq!(world.body_count(), 0);
    /// ```
    pub fn new(tick_rate: TickRate) -> Self {
        let mut rapier = RapierWorld::new();
        rapier.integration_parameters.dt = seconds_per_tick(tick_rate);
        rapier.gravity = to_vector(DEFAULT_GRAVITY);
        let mut world = PhysicsWorld {
            rapier,
            table: Vec::new(),
            rows: StableMap::new(),
            ranks: Vec::new(),
            query_layers: Vec::new(),
            query_masks: Vec::new(),
            query_reported: true,
            surfaces: None,
            surface_tags: Vec::new(),
            surface_ids: Vec::new(),
            surfaces_reported: true,
            surface_overlays: Vec::new(),
            overlay_ids: Vec::new(),
            plane: None,
            tick_rate,
            sleep_after_ticks: DEFAULT_SLEEP_AFTER_TICKS,
            layers: None,
            rejected: StableMap::new(),
            pending_rejections: Vec::new(),
            touching: Vec::new(),
            contacts: Vec::new(),
            windows: Vec::new(),
            detection_poses: Vec::new(),
            joints: Vec::new(),
            joint_rows: StableMap::new(),
            joint_events: Vec::new(),
            pending_missing: Vec::new(),
            motor_mismatched: Vec::new(),
            pending_motor_mismatches: Vec::new(),
            pending_impulses: Vec::new(),
            blasts_queued: 0,
            blast_hits: Vec::new(),
            pending_impulse_misses: Vec::new(),
            fresh_colliders: Vec::new(),
            time_scale: 1.0,
            frozen: false,
            last_step_dt: seconds_per_tick(tick_rate),
            sensing: Vec::new(),
            occupants: Vec::new(),
            sensor_events: Vec::new(),
            despawned: Vec::new(),
            last_write_frame: None,
            last_write_held: None,
            last_step_primed: false,
            move_reports: StableMap::new(),
            wire_edges: Mutex::default(),
        };
        world.sync_solver_pool();
        world
    }

    /// Builder form of [`set_plane`](PhysicsWorld::set_plane).
    #[must_use]
    pub fn with_plane(mut self, plane: Option<Plane>) -> Self {
        self.set_plane(plane);
        self
    }

    /// The constant acceleration applied to every rigid body, as last written by
    /// [`apply_settings`](PhysicsWorld::apply_settings).
    pub fn gravity(&self) -> [f32; 3] {
        from_vector(self.rapier.gravity)
    }

    /// The plane whose locks every rigid body created from here on carries, or `None` for a world
    /// with no plane constraint at all.
    ///
    /// `Some(plane)` locks the off-plane translation axis and the two in-plane rotation axes, so a
    /// 2D game is the 3D solver with axis locks rather than a second engine. The locks are OR'd
    /// with the body's own [`AxisLock`], and they are baked in at creation: changing the plane does
    /// not re-lock bodies that already exist.
    ///
    /// **`None` is not [`Plane::Xz`] here.** `Plane::of` reads an absent resource as the ground
    /// plane, because a 2D query has to pick a plane to answer in; a 3D world has no plane and must
    /// not be flattened into one, so this crate reads an absent resource as "no locks" instead. An
    /// in-plane `circle`/`aabb` shape in such a world is still laid out on the ground plane, since
    /// its two authored half-extents have to land on some pair of axes.
    pub fn set_plane(&mut self, plane: Option<Plane>) {
        self.plane = plane;
    }

    /// The plane whose locks new rigid bodies carry, if any.
    pub fn plane(&self) -> Option<Plane> {
        self.plane
    }

    /// The tick rate the integration timestep was built from.
    pub fn tick_rate(&self) -> TickRate {
        self.tick_rate
    }

    /// The seconds gameplay time advanced into the tick a motor is being moved in: `0` when the
    /// previous frame was held, otherwise one tick scaled by the clock's applied scale (the clock
    /// a `PathFollower` reads).
    pub fn gameplay_dt(&self, world: &World) -> f32 {
        let clock = world.resource::<GameClock>();
        let held_now = clock.map(|clock| clock.held());
        if held_now
            .zip(self.last_write_held)
            .is_some_and(|(now, last)| now > last)
        {
            0.0
        } else {
            seconds_per_tick(self.tick_rate) * clock.map_or(1.0, |clock| clock.applied_scale())
        }
    }

    /// Applies a scene's `physics:` block: gravity, the solver and stabilization iteration counts,
    /// and the scene-wide sleep default a rigid body falls back to when it omits its own.
    ///
    /// Called every tick — including on a world just restored from bytes — so a session always
    /// simulates under the loaded scene's settings, with no codec change to carry them. It does not
    /// touch the plane, which stays creation-time.
    ///
    /// Re-applies collision groups when the layer table changes: each body's groups are recomputed
    /// from its `Collider` and, where they differ, set and the body (with what it touches) woken; a
    /// body whose layer the new table does not declare is removed and rejected `L1186`, and every
    /// standing layer rejection is retried against the new table. An unchanged table — including a
    /// restored world's first call — sets nothing and wakes nothing.
    pub fn apply_settings(&mut self, settings: &PhysicsSettings) {
        self.rapier.gravity = to_vector(settings.gravity);
        self.rapier.integration_parameters.num_solver_iterations = settings.solver_iterations as _;
        self.rapier
            .integration_parameters
            .num_internal_stabilization_iterations = settings.stabilization_iterations as _;
        self.sleep_after_ticks = settings.sleep_after_ticks;
        if settings.layers != self.layers {
            self.layers.clone_from(&settings.layers);
            self.reapply_groups();
            self.resolve_query_masks(true);
        }
        if settings.surfaces != self.surfaces {
            self.surfaces.clone_from(&settings.surfaces);
            self.resolve_surface_ids(true);
        }
    }

    /// The collision groups `collider` gets under the current layer table: `None` for the solver's
    /// own default (no table and no layer, or `default`), otherwise the layer's bit as membership
    /// and everything but its ignored layers as the filter — rapier's `And` test then makes the
    /// pair rule symmetric. `Err` carries the `L1186` reason for a name the table does not declare.
    fn groups_for(&self, collider: &Collider) -> Result<Option<InteractionGroups>, String> {
        let name = collider.layer.as_deref().unwrap_or(DEFAULT_LAYER);
        let Some(table) = &self.layers else {
            return if name == DEFAULT_LAYER {
                Ok(None)
            } else {
                Err(unknown_layer(name, &[]))
            };
        };
        let Some(index) = table.index_of(name) else {
            return Err(unknown_layer(name, table.names()));
        };
        Ok(Some(InteractionGroups::new(
            Group::from_bits_truncate(1 << index),
            Group::from_bits_truncate(!table.ignores_mask(index)),
            InteractionTestMode::And,
        )))
    }

    /// Walks the table in creation order after the layer table changed: sets the groups that moved
    /// and drops the bodies whose layer is gone.
    fn reapply_groups(&mut self) {
        // A layer rejection may now build; `create` retries it next tick. A shape rejection stays.
        self.rejected = std::mem::take(&mut self.rejected)
            .into_iter()
            .filter(|(_, (_, _, rejected_as))| *rejected_as != code::COLLIDER_LAYER_UNKNOWN)
            .collect();
        let mut gone = Vec::new();
        for row in 0..self.table.len() {
            let (entity, handle, body) = {
                let entry = &self.table[row];
                (entry.entity, entry.collider, entry.body)
            };
            match self.groups_for(&self.table[row].source) {
                Ok(groups) => {
                    let target = groups.unwrap_or_else(InteractionGroups::all);
                    let Some(collider) = self.rapier.colliders.get_mut(handle) else {
                        continue;
                    };
                    if collider.collision_groups() != target {
                        collider.set_collision_groups(target);
                        self.rapier.wake_up(body, true);
                        self.wake_touching(entity);
                    }
                }
                Err(reason) => {
                    let entry = &self.table[row];
                    gone.push((entity, entry.source.clone(), entry.mesh_hash, reason));
                }
            }
        }
        for (entity, source, mesh_hash, reason) in gone {
            self.remove_entry(entity);
            self.reject(
                entity,
                &source,
                mesh_hash,
                code::COLLIDER_LAYER_UNKNOWN,
                reason,
                Vec::new(),
            );
        }
    }

    /// `rapier`'s solver position-correction iteration count, as applied by the last
    /// [`apply_settings`](PhysicsWorld::apply_settings).
    pub fn solver_iterations(&self) -> u32 {
        self.rapier.integration_parameters.num_solver_iterations as u32
    }

    /// The last step's per-stage solver timings and body counts, read off rapier's own profiler
    /// timers; `None` unless the crate is built with the `profiler` feature. Measurement only: the
    /// timers never feed the simulation.
    ///
    /// ```no_run
    /// # use ludion_physics::PhysicsWorld;
    /// # fn show(world: &PhysicsWorld) {
    /// if let Some(step) = world.step_breakdown() {
    ///     println!("solver {:.3} ms of {:.3} ms", step.solver_ms, step.step_ms);
    /// }
    /// # }
    /// ```
    pub fn step_breakdown(&self) -> Option<StepBreakdown> {
        #[cfg(feature = "profiler")]
        {
            let counters = &self.rapier.physics_pipeline.counters;
            let (stages, cd, solver) = (&counters.stages, &counters.cd, &counters.solver);
            Some(StepBreakdown {
                step_ms: counters.step_time.time_ms(),
                user_changes_ms: stages.user_changes.time_ms(),
                collision_detection_ms: stages.collision_detection_time.time_ms(),
                broad_phase_ms: cd.broad_phase_time.time_ms(),
                narrow_phase_ms: cd.narrow_phase_time.time_ms(),
                final_broad_phase_ms: cd.final_broad_phase_time.time_ms(),
                island_construction_ms: stages.island_construction_time.time_ms(),
                constraints_collection_ms: stages.island_constraints_collection_time.time_ms(),
                update_ms: stages.update_time.time_ms(),
                solver_ms: stages.solver_time.time_ms(),
                velocity_assembly_ms: solver.velocity_assembly_time.time_ms(),
                velocity_resolution_ms: solver.velocity_resolution_time.time_ms(),
                velocity_update_ms: solver.velocity_update_time.time_ms(),
                velocity_writeback_ms: solver.velocity_writeback_time.time_ms(),
                ccd_ms: stages.ccd_time.time_ms(),
                bodies: self.rapier.bodies.len(),
                awake_bodies: self.rapier.islands.num_active_bodies(),
                contact_pairs: self
                    .rapier
                    .contact_pairs()
                    .filter(|pair| pair.has_any_active_contact())
                    .count(),
                impulse_joints: self.rapier.impulse_joints.len(),
                substeps: self.solver_iterations(),
                primed: self.last_step_primed,
            })
        }
        #[cfg(not(feature = "profiler"))]
        {
            None
        }
    }

    /// The contact graph's pairs by body kind: `tracked` counts the overlapping pairs the broad
    /// phase keeps, which are only those `active_collision_types` and the collision groups admit,
    /// so under the defaults kinematic-kinematic and kinematic-static stay 0. `touching` counts
    /// those with an active contact, which is what the narrow phase and the solver pay for.
    /// Measurement only: it walks the graph read-only and never feeds the simulation.
    ///
    /// ```no_run
    /// # use ludion_physics::PhysicsWorld;
    /// # fn show(world: &PhysicsWorld) {
    /// let census = world.contact_pair_census();
    /// println!(
    ///     "kinematic pairs: {} tracked, {} touching",
    ///     census.kinematic_kinematic.tracked, census.kinematic_kinematic.touching
    /// );
    /// # }
    /// ```
    pub fn contact_pair_census(&self) -> ContactPairCensus {
        let rank = |handle: ColliderHandle| match self
            .rapier
            .colliders
            .get(handle)
            .and_then(|collider| collider.parent())
            .and_then(|body| self.rapier.bodies.get(body))
            .map(|body| body.body_type())
        {
            None | Some(RigidBodyType::Fixed) => 0,
            Some(RigidBodyType::Dynamic) => 2,
            Some(_) => 1,
        };
        let mut census = ContactPairCensus::default();
        for pair in self.rapier.narrow_phase.contact_pairs() {
            let (one, other) = (rank(pair.collider1), rank(pair.collider2));
            let slot = match (one.min(other), one.max(other)) {
                (0, 0) => &mut census.static_static,
                (0, 1) => &mut census.kinematic_static,
                (1, 1) => &mut census.kinematic_kinematic,
                (0, _) => &mut census.rigid_static,
                (1, _) => &mut census.rigid_kinematic,
                _ => &mut census.rigid_rigid,
            };
            slot.tracked += 1;
            if pair.has_any_active_contact() {
                slot.touching += 1;
            }
        }
        census
    }

    /// `rapier`'s internal stabilization iteration count, as applied by the last
    /// [`apply_settings`](PhysicsWorld::apply_settings).
    pub fn stabilization_iterations(&self) -> u32 {
        self.rapier
            .integration_parameters
            .num_internal_stabilization_iterations as u32
    }

    /// The scene-wide sleep default a rigid body falls back to when it omits its own
    /// `sleep_after_ticks`.
    pub fn sleep_after_ticks(&self) -> u32 {
        self.sleep_after_ticks
    }

    /// This step's contact transitions, canonical and sorted ascending by `(a, b)`. See
    /// [`ContactEvent`] for the full contract.
    pub fn contact_events(&self) -> &[ContactEvent] {
        &self.contacts
    }

    /// Takes every [`ShapeRejection`] queued since the last call, in the order `create` produced
    /// them.
    ///
    /// This crate does not depend on `ludion-engine` and so cannot report through its
    /// `Diagnostics` resource; a caller that can (`ludion-engine`'s `scene_physics` module) drains
    /// this every tick and reports each one, relying on the engine's own once-per-`(code, key)`
    /// latch to keep a standing rejection from printing twice. Not part of the snapshot: a restored
    /// world's already-rejected entities are simply retried once more on the next tick and re-queued
    /// here, which the latch on the reading side absorbs.
    pub fn take_rejections(&mut self) -> Vec<ShapeRejection> {
        std::mem::take(&mut self.pending_rejections)
    }

    /// Queues a [`ShapeRejection`] for a fault found before any shape is built (a character motor
    /// refused by the write pass), for the same once-per-`(code, entity)` reporting as `create`'s.
    pub(crate) fn queue_rejection(&mut self, rejection: ShapeRejection) {
        // The write pass repeats a standing fault every tick; a host that never drains the queue
        // must not see it grow.
        if self
            .pending_rejections
            .iter()
            .any(|p| p.entity == rejection.entity && p.code == rejection.code)
        {
            return;
        }
        self.pending_rejections.push(rejection);
    }

    /// This step's joint breaks, sorted ascending by carrier. See [`JointBroken`] for the full
    /// contract.
    pub fn joint_events(&self) -> &[JointBroken] {
        &self.joint_events
    }

    /// How many carriers hold a `Joint` this crate is tracking, attached or inert.
    pub fn joint_count(&self) -> usize {
        self.joints.len()
    }

    /// Whether `carrier`'s joint currently has a live rapier joint attached, or `None` if `carrier`
    /// carries no `Joint` this crate is tracking.
    pub fn is_joint_active(&self, carrier: EntityId) -> Option<bool> {
        let &row = self.joint_rows.get(&carrier)?;
        Some(self.joints[row].handle.is_some())
    }

    /// Takes every carrier `reconcile_joints` found missing a
    /// body since the last call — the [`take_rejections`](PhysicsWorld::take_rejections) contract,
    /// for `L3035` instead of a rejected shape.
    pub fn take_missing_joints(&mut self) -> Vec<EntityId> {
        std::mem::take(&mut self.pending_missing)
    }

    /// Takes every [`MotorMismatch`] queued since the last call — the
    /// [`take_missing_joints`](PhysicsWorld::take_missing_joints) contract, for `L3037`. A carrier
    /// is queued only on the transition into a mismatch, so a standing one is not re-queued every
    /// tick.
    pub fn take_motor_mismatches(&mut self) -> Vec<MotorMismatch> {
        std::mem::take(&mut self.pending_motor_mismatches)
    }

    /// How many bodies the handle table holds.
    pub fn body_count(&self) -> usize {
        self.table.len()
    }

    /// How many bodies are rigid or kinematic — every solver body but the `static` ones. Compared
    /// against [`GATED_BODIES`](crate::GATED_BODIES) for `L4034`.
    ///
    /// # Example
    /// ```
    /// use ludion_core::time::TickRate;
    /// use ludion_physics::PhysicsWorld;
    ///
    /// assert_eq!(PhysicsWorld::new(TickRate::DEFAULT).non_static_body_count(), 0);
    /// ```
    pub fn non_static_body_count(&self) -> usize {
        self.table
            .iter()
            .filter(|entry| entry.kind != BodyKind::Fixed)
            .count()
    }

    /// The entities that have a body, in creation order — the table's own order.
    pub fn entities(&self) -> impl Iterator<Item = EntityId> + '_ {
        self.table.iter().map(|entry| entry.entity)
    }

    /// The `static` bodies, in creation order, each with its posed world AABB and a `revision` that
    /// changes whenever the collider is rebuilt or the body is re-posed. A consumer that caches
    /// geometry derived from static colliders diffs two scans by `entity` and refreshes what moved.
    ///
    /// Reads the state as of the last completed step, like the queries.
    ///
    /// # Example
    /// ```
    /// use ludion_core::time::TickRate;
    /// use ludion_physics::PhysicsWorld;
    ///
    /// assert_eq!(PhysicsWorld::new(TickRate::DEFAULT).static_bodies().count(), 0);
    /// ```
    pub fn static_bodies(&self) -> impl Iterator<Item = StaticBody> + '_ {
        self.table
            .iter()
            .filter(|entry| entry.kind == BodyKind::Fixed)
            .filter_map(|entry| {
                let collider = self.rapier.colliders.get(entry.collider)?;
                let aabb = collider.compute_aabb();
                let (index, generation) = entry.collider.into_raw_parts();
                let pose = self.rapier.bodies.get(entry.body)?.position();
                let (t, r) = (pose.translation, pose.rotation);
                let mut revision = (u64::from(index) << 32) | u64::from(generation);
                for bits in [t.x, t.y, t.z, r.x, r.y, r.z, r.w].map(f32::to_bits) {
                    revision = (revision ^ u64::from(bits)).wrapping_mul(0x0000_0100_0000_01b3);
                }
                Some(StaticBody {
                    entity: entry.entity,
                    min: from_vector(aabb.mins),
                    max: from_vector(aabb.maxs),
                    revision,
                })
            })
    }

    /// The parry shape `entity`'s collider was built as, or `None` if it has no body.
    ///
    /// An entity gets no body when its authored shape cannot be built — a `convex` hull that
    /// degenerates, a `heightfield` whose `rows`/`cols` do not match its `heights` or describe a
    /// grid smaller than 2×2. The rejection is queued once on
    /// [`take_rejections`](PhysicsWorld::take_rejections) and the creation attempt is not retried
    /// until the entity's `Collider` changes.
    pub fn shape_kind(&self, entity: EntityId) -> Option<ShapeKind> {
        let entry = self.entry(entity)?;
        let shape = self.rapier.colliders.get(entry.collider)?.shape();
        Some(match shape.shape_type() {
            ShapeType::Ball => ShapeKind::Ball,
            ShapeType::Cuboid => ShapeKind::Cuboid,
            ShapeType::Capsule => ShapeKind::Capsule,
            ShapeType::Cylinder => ShapeKind::Cylinder,
            ShapeType::ConvexPolyhedron => ShapeKind::ConvexPolyhedron,
            ShapeType::HeightField => ShapeKind::HeightField,
            ShapeType::Compound => ShapeKind::Compound,
            ShapeType::TriMesh => ShapeKind::TriMesh,
            _ => ShapeKind::Other,
        })
    }

    /// The axis locks `entity`'s body actually carries — the body's own [`AxisLock`] OR'd with the
    /// plane's — or `None` if it has no body.
    pub fn locked_axes(&self, entity: EntityId) -> Option<AxisLock> {
        let entry = self.entry(entity)?;
        let locked = self.rapier.bodies.get(entry.body)?.locked_axes();
        Some(AxisLock {
            translation: [
                locked.contains(LockedAxes::TRANSLATION_LOCKED_X),
                locked.contains(LockedAxes::TRANSLATION_LOCKED_Y),
                locked.contains(LockedAxes::TRANSLATION_LOCKED_Z),
            ],
            rotation: [
                locked.contains(LockedAxes::ROTATION_LOCKED_X),
                locked.contains(LockedAxes::ROTATION_LOCKED_Y),
                locked.contains(LockedAxes::ROTATION_LOCKED_Z),
            ],
        })
    }

    /// Whether `entity`'s body is asleep, or `None` if it has no body. A sleeping body costs
    /// nothing to step and its mirrored pose stops changing.
    pub fn is_sleeping(&self, entity: EntityId) -> Option<bool> {
        let entry = self.entry(entity)?;
        Some(self.rapier.bodies.get(entry.body)?.is_sleeping())
    }

    /// `entity`'s body's solver mass, or `None` if it has no body. Reflects the `Collider` the
    /// body was last built or `rebuild`-ed from, not necessarily the one
    /// the entity carries this tick — the write system rebuilds a stale body before this could
    /// observe a mismatch.
    ///
    /// # Example
    /// ```
    /// use ludion_core::time::TickRate;
    /// use ludion_core::ids::EntityId;
    /// use ludion_physics::PhysicsWorld;
    ///
    /// let world = PhysicsWorld::new(TickRate::DEFAULT);
    /// assert_eq!(world.mass(EntityId::new(0, 0)), None);
    /// ```
    pub fn mass(&self, entity: EntityId) -> Option<f32> {
        let entry = self.entry(entity)?;
        Some(self.rapier.bodies.get(entry.body)?.mass())
    }

    /// `entity`'s body's gravity multiplier, or `None` if it has no body. Reflects the `Collider`
    /// the body was last built from or retuned to; `1.0` for a static or kinematic body.
    ///
    /// # Example
    /// A levitation system: an `overlap` picks the rigid bodies in a zone, their `Collider` is
    /// edited to `gravity_scale: 0.0`, and a per-entity counter in game state edits it back.
    /// ```
    /// use ludion_core::time::{FrameIndex, TickRate};
    /// use ludion_ecs::{Schedule, SystemCtx, World};
    /// use ludion_physics::{NoParent, PhysicsPose, PhysicsSettings, PhysicsSpace, PhysicsWorld, QueryFilter, install};
    /// use ludion_spatial::{Body, Collider, RigidBody, Shape};
    /// use schemars::JsonSchema;
    /// use serde::{Deserialize, Serialize};
    ///
    /// #[derive(Serialize, Deserialize, JsonSchema, Clone, Copy)]
    /// struct Transform {
    ///     translation: [f32; 3],
    ///     rotation: [f32; 4],
    /// }
    /// impl PhysicsPose for Transform {
    ///     fn translation(&self) -> [f32; 3] { self.translation }
    ///     fn set_translation(&mut self, t: [f32; 3]) { self.translation = t; }
    ///     fn rotation_quat(&self) -> [f32; 4] { self.rotation }
    ///     fn set_rotation_quat(&mut self, r: [f32; 4]) { self.rotation = r; }
    /// }
    ///
    /// /// Levitated steps still to run, counting the one the edit lands in.
    /// #[derive(Serialize, Deserialize, JsonSchema, Clone, Copy)]
    /// struct Floating {
    ///     ticks: u32,
    /// }
    ///
    /// fn set_scale(world: &mut World, entity: ludion_core::ids::EntityId, scale: f32) {
    ///     let collider = world.get_mut::<Collider>(entity).unwrap().unwrap();
    ///     if let Body::Rigid(body) = &mut collider.body {
    ///         body.gravity_scale = scale;
    ///     }
    /// }
    ///
    /// fn levitate(world: &mut World, ctx: &SystemCtx) {
    ///     let mut done = Vec::new();
    ///     for (entity, (floating,)) in world.query_ids::<(&mut Floating,)>() {
    ///         floating.ticks -= 1;
    ///         if floating.ticks == 0 {
    ///             done.push(entity);
    ///         }
    ///     }
    ///     for entity in done {
    ///         set_scale(world, entity, 1.0);
    ///         world.remove::<Floating>(entity).unwrap();
    ///     }
    ///     let Some(physics) = world.resource::<PhysicsWorld>() else { return };
    ///     if ctx.frame.as_u64() == 1 {
    ///         let zone = Shape::Sphere { radius: 2.0 };
    ///         let level = [0.0, 0.0, 0.0, 1.0];
    ///         let inside = physics.overlap(&zone, [0.0; 3], level, &QueryFilter::new()).unwrap();
    ///         for entity in inside {
    ///             let rigid = matches!(
    ///                 world.get::<Collider>(entity).unwrap().map(|c| &c.body),
    ///                 Some(Body::Rigid(_))
    ///             );
    ///             if rigid {
    ///                 set_scale(world, entity, 0.0);
    ///                 world.insert(entity, Floating { ticks: 3 }).unwrap();
    ///             }
    ///         }
    ///     }
    /// }
    ///
    /// let mut world = World::new();
    /// let ball = world.spawn();
    /// let at_origin = Transform { translation: [0.0; 3], rotation: [0.0, 0.0, 0.0, 1.0] };
    /// world.insert(ball, at_origin).unwrap();
    /// let body = Body::Rigid(RigidBody::default());
    /// let collider = Collider { shape: Shape::Sphere { radius: 0.5 }, body, layer: None, sensor: false };
    /// world.insert(ball, collider).unwrap();
    ///
    /// let mut schedule = Schedule::new();
    /// schedule.add_system("levitate", levitate);
    /// install::<Transform, NoParent>(&mut schedule, PhysicsSettings::new(PhysicsSpace::Free));
    /// let mut run = |world: &mut World, frames: std::ops::Range<u64>| {
    ///     for frame in frames {
    ///         let ctx = SystemCtx::new(FrameIndex::new(frame), TickRate::DEFAULT);
    ///         schedule.run(world, &ctx);
    ///     }
    ///     world.resource::<PhysicsWorld>().unwrap().gravity_scale(ball)
    /// };
    ///
    /// // Frame 1 floats the ball, and frames 1 to 3 step it at scale 0.
    /// assert_eq!(run(&mut world, 0..4), Some(0.0));
    /// // The counter runs out on frame 4 and the collider is edited back before that step.
    /// assert_eq!(run(&mut world, 4..5), Some(1.0));
    /// ```
    pub fn gravity_scale(&self, entity: EntityId) -> Option<f32> {
        let entry = self.entry(entity)?;
        Some(self.rapier.bodies.get(entry.body)?.gravity_scale())
    }

    /// `entity`'s collider's friction coefficient, or `None` if it has no body. Reflects the
    /// `Collider` the body was last built from or retuned to; a static or kinematic body reads
    /// the solver's default.
    ///
    /// # Example
    /// ```
    /// use ludion_core::time::{FrameIndex, TickRate};
    /// use ludion_ecs::{Schedule, SystemCtx, World};
    /// use ludion_physics::{NoParent, PhysicsPose, PhysicsSettings, PhysicsSpace, PhysicsWorld, install};
    /// use ludion_spatial::{Body, Collider, RigidBody, Shape};
    /// use schemars::JsonSchema;
    /// use serde::{Deserialize, Serialize};
    ///
    /// #[derive(Serialize, Deserialize, JsonSchema, Clone, Copy)]
    /// struct Transform {
    ///     translation: [f32; 3],
    ///     rotation: [f32; 4],
    /// }
    /// impl PhysicsPose for Transform {
    ///     fn translation(&self) -> [f32; 3] { self.translation }
    ///     fn set_translation(&mut self, t: [f32; 3]) { self.translation = t; }
    ///     fn rotation_quat(&self) -> [f32; 4] { self.rotation }
    ///     fn set_rotation_quat(&mut self, r: [f32; 4]) { self.rotation = r; }
    /// }
    ///
    /// let mut world = World::new();
    /// let ball = world.spawn();
    /// let at_origin = Transform { translation: [0.0; 3], rotation: [0.0, 0.0, 0.0, 1.0] };
    /// world.insert(ball, at_origin).unwrap();
    /// let body = Body::Rigid(RigidBody::default());
    /// let collider = Collider { shape: Shape::Sphere { radius: 0.5 }, body, layer: None, sensor: false };
    /// world.insert(ball, collider).unwrap();
    ///
    /// let mut schedule = Schedule::new();
    /// install::<Transform, NoParent>(&mut schedule, PhysicsSettings::new(PhysicsSpace::Free));
    /// let mut step = |world: &mut World, frame: u64| {
    ///     let ctx = SystemCtx::new(FrameIndex::new(frame), TickRate::DEFAULT);
    ///     schedule.run(world, &ctx);
    /// };
    /// step(&mut world, 0);
    /// let before = world.resource::<PhysicsWorld>().unwrap().friction(ball).unwrap();
    ///
    /// // A live friction edit reaches the body in place on the next tick.
    /// if let Some(Collider { body: Body::Rigid(tunables), .. }) = world.get_mut::<Collider>(ball).unwrap() {
    ///     tunables.friction = before + 1.0;
    /// }
    /// step(&mut world, 1);
    /// assert_eq!(world.resource::<PhysicsWorld>().unwrap().friction(ball), Some(before + 1.0));
    /// ```
    pub fn friction(&self, entity: EntityId) -> Option<f32> {
        let entry = self.entry(entity)?;
        Some(self.rapier.colliders.get(entry.collider)?.friction())
    }

    /// `entity`'s collider's restitution coefficient, or `None` if it has no body. Reflects the
    /// `Collider` the body was last built from or retuned to; see [`friction`](PhysicsWorld::friction)
    /// for a runnable edit-and-read example.
    pub fn restitution(&self, entity: EntityId) -> Option<f32> {
        let entry = self.entry(entity)?;
        Some(self.rapier.colliders.get(entry.collider)?.restitution())
    }

    /// `entity`'s collider's friction combine rule, or `None` if it has no body.
    pub fn friction_combine(&self, entity: EntityId) -> Option<CombineRule> {
        let entry = self.entry(entity)?;
        Some(combine_rule(
            self.rapier
                .colliders
                .get(entry.collider)?
                .friction_combine_rule(),
        ))
    }

    /// `entity`'s collider's restitution combine rule, or `None` if it has no body. When two
    /// colliders touch and their rules differ, the stronger wins (max over multiply over min over
    /// average), so a bouncy part set to `Max` rebounds off a ground left at `Average`.
    ///
    /// # Example
    /// ```
    /// use ludion_core::time::{FrameIndex, TickRate};
    /// use ludion_ecs::{Schedule, SystemCtx, World};
    /// use ludion_physics::{NoParent, PhysicsPose, PhysicsSettings, PhysicsSpace, PhysicsWorld, install};
    /// use ludion_spatial::{Body, Collider, CombineRule, ContactMaterial, RigidBody, Shape};
    /// use schemars::JsonSchema;
    /// use serde::{Deserialize, Serialize};
    ///
    /// #[derive(Serialize, Deserialize, JsonSchema, Clone, Copy)]
    /// struct Transform {
    ///     translation: [f32; 3],
    ///     rotation: [f32; 4],
    /// }
    /// impl PhysicsPose for Transform {
    ///     fn translation(&self) -> [f32; 3] { self.translation }
    ///     fn set_translation(&mut self, t: [f32; 3]) { self.translation = t; }
    ///     fn rotation_quat(&self) -> [f32; 4] { self.rotation }
    ///     fn set_rotation_quat(&mut self, r: [f32; 4]) { self.rotation = r; }
    /// }
    ///
    /// let mut world = World::new();
    /// let at = |y: f32| Transform { translation: [0.0, y, 0.0], rotation: [0.0, 0.0, 0.0, 1.0] };
    /// let ground = world.spawn();
    /// world.insert(ground, at(0.0)).unwrap();
    /// let dead = ContactMaterial { restitution: 0.0, ..ContactMaterial::default() };
    /// let shape = Shape::Box { half_extents: [5.0, 0.5, 5.0] };
    /// world.insert(ground, Collider { shape, body: Body::Static(dead), layer: None, sensor: false }).unwrap();
    ///
    /// let ball = world.spawn();
    /// world.insert(ball, at(3.0)).unwrap();
    /// let bouncy = RigidBody {
    ///     restitution: 0.8,
    ///     restitution_combine: CombineRule::Max,
    ///     ..RigidBody::default()
    /// };
    /// let shape = Shape::Sphere { radius: 0.5 };
    /// world.insert(ball, Collider { shape, body: Body::Rigid(bouncy), layer: None, sensor: false }).unwrap();
    ///
    /// let mut schedule = Schedule::new();
    /// install::<Transform, NoParent>(&mut schedule, PhysicsSettings::new(PhysicsSpace::Free));
    /// let ctx = SystemCtx::new(FrameIndex::new(0), TickRate::DEFAULT);
    /// schedule.run(&mut world, &ctx);
    /// let physics = world.resource::<PhysicsWorld>().unwrap();
    /// assert_eq!(physics.restitution_combine(ball), Some(CombineRule::Max));
    /// assert_eq!(physics.restitution_combine(ground), Some(CombineRule::Average));
    /// ```
    pub fn restitution_combine(&self, entity: EntityId) -> Option<CombineRule> {
        let entry = self.entry(entity)?;
        Some(combine_rule(
            self.rapier
                .colliders
                .get(entry.collider)?
                .restitution_combine_rule(),
        ))
    }

    /// The whole solver state as bytes: a bincode encoding of the nine persistent solver
    /// structures plus the creation-order handle table and the plane.
    ///
    /// The step pipeline and the CCD workspace are **not** state — they carry no information across
    /// a step — and are rebuilt fresh by [`from_bytes`](PhysicsWorld::from_bytes). Bytes rather
    /// than a structured value because the narrow phase's 128-bit solver masks do not fit in a JSON
    /// number; wrapping these bytes for a text document (base64) is the caller's job.
    ///
    /// The blob opens with a tag: the magic `LPB1`, [`PHYSICS_BLOB_VERSION`] (`u32` LE) and the
    /// solver pin's version string (`u16` LE length, then bytes). A bincode major bump changes the
    /// encoding, so it is a snapshot-format event rather than an ordinary dependency update. The last step's contact events and touching set ride in the
    /// blob too, since tick `N + 1` reads what tick `N`'s step produced; so do the blast hits, the
    /// colliders a frozen step left out of the next blast, and the last step's timestep. The sensor
    /// overlap baseline, the last step's sensor events and the not-yet-judged despawns ride too.
    ///
    /// A world holding queued impulse calls refuses with [`PhysicsError::Encode`]: the queue is
    /// not state a restore could carry, so a snapshot taken between a call and the write system
    /// that applies it would silently drop the calls. Under a scene `physics:` block the write
    /// system runs after every game system, so a snapshot between ticks never sees a queue. A
    /// pending [`scale_time`](PhysicsWorld::scale_time) below `1.0` or a pending
    /// [`freeze`](PhysicsWorld::freeze) refuses the same way.
    pub fn to_bytes(&self) -> Result<Vec<u8>, PhysicsError> {
        if !self.pending_impulses.is_empty() {
            return Err(PhysicsError::Encode(format!(
                "{} impulse calls are queued but not applied; queue them from a system that runs \
                 before the physics write system",
                self.pending_impulses.len()
            )));
        }
        if self.time_scale < 1.0 || self.frozen {
            return Err(PhysicsError::Encode(
                "a time scale or freeze is pending but not stepped; call scale_time and freeze \
                 from a system that runs before the physics write system"
                    .to_string(),
            ));
        }
        let pin = rapier3d::VERSION.as_bytes();
        let pin_len = u16::try_from(pin.len())
            .map_err(|_| PhysicsError::Encode("the solver pin name is too long".to_string()))?;
        let mut bytes = Vec::with_capacity(10 + pin.len());
        bytes.extend_from_slice(&PHYSICS_BLOB_MAGIC);
        bytes.extend_from_slice(&PHYSICS_BLOB_VERSION.to_le_bytes());
        bytes.extend_from_slice(&pin_len.to_le_bytes());
        bytes.extend_from_slice(pin);
        let body = bincode::serialize(&SnapshotRef {
            rapier: &self.rapier,
            table: &self.table,
            plane: self.plane.map(PlaneCode::from),
            touching: &self.touching,
            contacts: &self.contacts,
            windows: &self.windows,
            joints: &self.joints,
            joint_events: &self.joint_events,
            blast_hits: &self.blast_hits,
            fresh_colliders: &self.fresh_colliders,
            last_step_dt: self.last_step_dt,
            sensing: &self.sensing,
            sensor_events: &self.sensor_events,
            despawned: &self.despawned,
            last_write_frame: self.last_write_frame,
            last_write_held: self.last_write_held,
        })
        .map_err(|why| PhysicsError::Encode(why.to_string()))?;
        bytes.extend_from_slice(&body);
        Ok(bytes)
    }

    /// Rebuilds a world from [`to_bytes`](PhysicsWorld::to_bytes), stepping at `tick_rate`.
    ///
    /// The restored world continues the simulation bit-identically: warm-start impulses, island
    /// membership and the contact graph all ride in the bytes, and the entity→row map is rebuilt by
    /// walking the restored table in its own order, never by walking a solver set. The timestep is
    /// taken from `tick_rate`, not from the bytes, so a session restores at the rate it runs at.
    ///
    /// # Errors
    /// [`PhysicsError::Decode`] for a truncated or trailing-byte blob, one tagged with another
    /// blob version or solver pin, or one with no tag at all (it predates the version tag).
    pub fn from_bytes(bytes: &[u8], tick_rate: TickRate) -> Result<Self, PhysicsError> {
        let Some(rest) = bytes.strip_prefix(&PHYSICS_BLOB_MAGIC) else {
            return Err(PhysicsError::Decode(
                "the blob carries no version tag: it predates the version tag".to_string(),
            ));
        };
        let body = check_blob_tag(rest)?;
        // `bincode::deserialize` is this chain plus `allow_trailing_bytes`; a blob with a tail is corrupt.
        let snapshot: SnapshotOwned = bincode::DefaultOptions::new()
            .with_fixint_encoding()
            .reject_trailing_bytes()
            .deserialize(body)
            .map_err(|why| PhysicsError::Decode(why.to_string()))?;
        let mut world = PhysicsWorld {
            rapier: snapshot.rapier,
            table: snapshot.table,
            rows: StableMap::new(),
            ranks: Vec::new(),
            query_layers: Vec::new(),
            query_masks: Vec::new(),
            query_reported: true,
            surfaces: None,
            surface_tags: Vec::new(),
            surface_ids: Vec::new(),
            surfaces_reported: true,
            surface_overlays: Vec::new(),
            overlay_ids: Vec::new(),
            plane: snapshot.plane.map(Plane::from),
            tick_rate,
            sleep_after_ticks: DEFAULT_SLEEP_AFTER_TICKS,
            layers: None,
            rejected: StableMap::new(),
            pending_rejections: Vec::new(),
            touching: snapshot.touching,
            contacts: snapshot.contacts,
            windows: snapshot.windows,
            detection_poses: Vec::new(),
            joints: snapshot.joints,
            joint_rows: StableMap::new(),
            joint_events: snapshot.joint_events,
            pending_missing: Vec::new(),
            motor_mismatched: Vec::new(),
            pending_motor_mismatches: Vec::new(),
            pending_impulses: Vec::new(),
            blasts_queued: 0,
            blast_hits: snapshot.blast_hits,
            pending_impulse_misses: Vec::new(),
            fresh_colliders: snapshot.fresh_colliders,
            time_scale: 1.0,
            frozen: false,
            last_step_dt: snapshot.last_step_dt,
            occupants: snapshot.sensing.iter().map(|pair| pair.1).collect(),
            sensing: snapshot.sensing,
            sensor_events: snapshot.sensor_events,
            despawned: snapshot.despawned,
            last_write_frame: snapshot.last_write_frame,
            last_write_held: snapshot.last_write_held,
            last_step_primed: false,
            move_reports: StableMap::new(),
            wire_edges: Mutex::default(),
        };
        world.rapier.integration_parameters.dt = seconds_per_tick(tick_rate);
        world.reindex();
        world.reindex_joints();
        world.sync_solver_pool();
        Ok(world)
    }

    /// Sizes the solver's worker pool to `par::threads()`. A `parallel` rapier with no pool of its
    /// own falls back to rayon's global one, which `LUDION_THREADS`/`--threads` do not govern; the
    /// count is thread-local and a world can be built under one and stepped under another, so the
    /// size is re-checked wherever it could have drifted (build, decode, every step).
    fn sync_solver_pool(&mut self) {
        let want = ludion_core::par::threads();
        if self.rapier.physics_pipeline.num_threads() == Some(want) {
            return;
        }
        if let Err(why) = self.rapier.physics_pipeline.configure_thread_pool(want) {
            panic!("could not start the physics solver's {want} worker threads: {why}");
        }
        if let Some(pool) = self.rapier.physics_pipeline.thread_pool() {
            // Rayon returns from `build` before its threads have run: their startup allocations
            // would land in whatever the caller does next, on a loaded box.
            pool.broadcast(|_| ());
        }
    }

    /// Advances the simulation by one fixed timestep scaled by the pending
    /// [`scale_time`](PhysicsWorld::scale_time) (or not at all when frozen), with no hooks and no
    /// event handler, then diffs the solver's contact graph into this step's
    /// [`contact_events`](PhysicsWorld::contact_events). The scale and the freeze reset here.
    pub(crate) fn step(&mut self) {
        self.sync_solver_pool();
        self.last_step_primed = false;
        self.contacts.clear();
        self.sensor_events.clear();
        self.joint_events.clear();
        let scale = std::mem::replace(&mut self.time_scale, 1.0);
        if std::mem::take(&mut self.frozen) {
            self.last_step_dt = 0.0;
            // No step ran, so the fresh colliders are still absent from the BVH.
            return;
        }
        let base = seconds_per_tick(self.tick_rate);
        let dt = base * scale;
        self.rapier.integration_parameters.dt = dt;
        self.record_detection_poses();
        let retuned = (scale < 1.0).then(|| self.scale_joint_softness(base, scale));
        self.rapier.step();
        self.last_step_primed = self.rapier.physics_pipeline.last_step_used_prime();
        if let Some(saved) = retuned {
            self.restore_joint_softness(saved);
        }
        self.rapier.integration_parameters.dt = base;
        self.last_step_dt = dt;
        self.fresh_colliders.clear();
        self.collect_contacts(scale);
        self.collect_broken_joints();
        self.collect_sensors();
        self.despawned.clear();
    }

    /// Runs the first step's detect phase (user changes, broad and narrow phase) ahead of time, at
    /// the timestep `step` will use. The step that follows reuses it only when nothing was
    /// modified in between, so the result is the one an unprimed world computes. `fresh_colliders`
    /// is left alone: only a real step clears it.
    pub(crate) fn prime_detection(&mut self) {
        self.sync_solver_pool();
        self.rapier.integration_parameters.dt = seconds_per_tick(self.tick_rate);
        self.rapier.prime();
    }

    /// Whether the last step reused the collision detection [`prime`](crate::prime) ran at scene
    /// commit, and so skipped its own detect phase. A measurement only: it never
    /// feeds the simulation, and a world that was not primed, or whose first tick found anything
    /// changed, reports `false` with the same result.
    pub fn last_step_primed(&self) -> bool {
        self.last_step_primed
    }

    /// Fills `detection_poses` with the pose each collider will have when the step's collision
    /// detection runs: its parent body's current pose composed with its offset, which is where a
    /// pose written this tick by the write system lands, since rapier propagates it to the collider
    /// only inside the step.
    fn record_detection_poses(&mut self) {
        self.detection_poses
            .iter_mut()
            .for_each(|pose| *pose = None);
        for (handle, collider) in self.rapier.colliders.iter() {
            let pose = match (collider.parent(), collider.position_wrt_parent()) {
                (Some(body), Some(offset)) => self
                    .rapier
                    .bodies
                    .get(body)
                    .map_or(*collider.position(), |body| *body.position() * *offset),
                _ => *collider.position(),
            };
            let slot = handle.into_raw_parts().0 as usize;
            if slot >= self.detection_poses.len() {
                self.detection_poses.resize(slot + 1, None);
            }
            self.detection_poses[slot] = Some(pose);
        }
    }

    /// Diffs the narrow phase's touching pairs against the previous step's touching set, filling
    /// this step's [`contact_events`](PhysicsWorld::contact_events) with the transitions.
    ///
    /// The narrow phase is walked once: every active pair is mapped to its two entities through
    /// the collider `user_data` [`create`](PhysicsWorld::create) sets, ordered canonically
    /// (`a < b`), and collected with the step's `total_impulse_magnitude` and the picked point and
    /// normal (see [`ContactEvent`]). A collider whose
    /// `user_data` cannot be resolved — never true for a pair this crate created, since every
    /// collider it inserts carries an entity — is skipped rather than panicking. Sorting that list
    /// and merge-diffing it against the previous step's already-sorted `touching` produces the
    /// output sorted with no second sort: a pair only in the new set **and
    /// engaged** (see `engaged`) opens an impulse window and joins `touching`; the window's
    /// [`Started`](ContactPhase::Started) carries the impulse summed over one base tick of simulated
    /// time (`scale` per step) and the touch's point and normal. A pair only in the old set is
    /// [`Stopped`](ContactPhase::Stopped) with zeros, or, if its window is still open, flushes its
    /// `Started` and stays touching one more step so the next reports the stop; a pair in both is
    /// neither.
    /// Nothing here depends on rapier's own iteration order — the key is this function's sort.
    fn collect_contacts(&mut self, scale: f32) {
        // The point, normal and `engaged` walk each pair's manifolds, so only a pair that is not
        // already touching pays for them: the rest need nothing but their summed impulse.
        let mut current: Vec<(EntityId, EntityId, f32, &ContactPair)> = self
            .rapier
            .narrow_phase
            .contact_pairs()
            .filter(|pair| pair.has_any_active_contact())
            .filter_map(|pair| {
                let co1 = self.rapier.colliders.get(pair.collider1)?;
                let co2 = self.rapier.colliders.get(pair.collider2)?;
                let one = entity_from_bits(co1.user_data);
                let other = entity_from_bits(co2.user_data);
                let (a, b) = if one < other {
                    (one, other)
                } else {
                    (other, one)
                };
                Some((a, b, pair.total_impulse_magnitude(), pair))
            })
            .collect();
        current.sort_unstable_by_key(|&(a, b, ..)| (a, b));
        let started = |pair: &ContactPair, a: EntityId, b: EntityId, impulse: f32| {
            let (Some(co1), Some(co2)) = (
                self.rapier.colliders.get(pair.collider1),
                self.rapier.colliders.get(pair.collider2),
            ) else {
                unreachable!("the pair was collected with both colliders present");
            };
            let one = entity_from_bits(co1.user_data);
            let other = entity_from_bits(co2.user_data);
            // An active pair always has a solver contact; zeros rather than a panic if not.
            let pose = |handle: ColliderHandle, collider: &RapierCollider| {
                let slot = handle.into_raw_parts().0 as usize;
                self.detection_poses
                    .get(slot)
                    .copied()
                    .flatten()
                    .unwrap_or(*collider.position())
            };
            let poses = [pose(pair.collider1, co1), pose(pair.collider2, co2)];
            let (point, normal) = contact_point(pair, poses).unwrap_or_default();
            // The zero fallback stays exact: negating it would give `-0.0`.
            let normal = if one > other && normal != [0.0; 3] {
                normal.map(|c| -c)
            } else {
                normal
            };
            let event = ContactEvent {
                a,
                b,
                phase: ContactPhase::Started,
                impulse,
                point,
                normal,
            };
            (event, engaged(pair))
        };

        // `None` when both cursors are exhausted; otherwise the merge order, with an exhausted
        // cursor reading as "on the far side of everything left in the other one" so the loop
        // below has one arm per outcome instead of one per cursor combination.
        use std::cmp::Ordering;
        let mut next_touching = Vec::with_capacity(current.len());
        let old_windows = std::mem::take(&mut self.windows);
        let mut next_windows = Vec::new();
        // Emits a window whose base tick has passed; otherwise it keeps collecting.
        let mut settle = |window: ContactWindow, contacts: &mut Vec<ContactEvent>| {
            if window.elapsed >= IMPULSE_WINDOW {
                contacts.push(window.event);
            } else {
                next_windows.push(window);
            }
        };
        let window_of = |pair: (EntityId, EntityId)| {
            old_windows
                .binary_search_by_key(&pair, |window| (window.event.a, window.event.b))
                .ok()
                .map(|index| old_windows[index])
        };
        let mut old = self.touching.iter().copied().peekable();
        let mut new = current.iter().copied().peekable();
        loop {
            let order = match (old.peek(), new.peek()) {
                (Some(&o), Some(&(a, b, ..))) => o.cmp(&(a, b)),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => break,
            };
            match order {
                Ordering::Less => {
                    let pair = old.next().unwrap();
                    if let Some(window) = window_of(pair) {
                        // Separated mid-window: the `Started` with what it gathered, and the pair
                        // stays touching one more step so the next one reports its `Stopped`.
                        self.contacts.push(window.event);
                        next_touching.push(pair);
                    } else {
                        self.contacts.push(ContactEvent {
                            a: pair.0,
                            b: pair.1,
                            phase: ContactPhase::Stopped,
                            impulse: 0.0,
                            point: [0.0; 3],
                            normal: [0.0; 3],
                        });
                    }
                }
                Ordering::Greater => {
                    let (a, b, impulse, pair) = new.next().unwrap();
                    let (event, engaged) = started(pair, a, b, impulse);
                    if engaged {
                        next_touching.push((event.a, event.b));
                        let window = ContactWindow {
                            event,
                            elapsed: scale,
                        };
                        settle(window, &mut self.contacts);
                    }
                }
                Ordering::Equal => {
                    let pair = old.next().unwrap();
                    let (_, _, impulse, _) = new.next().unwrap();
                    next_touching.push(pair);
                    if let Some(mut window) = window_of(pair) {
                        window.event.impulse += impulse;
                        window.elapsed += scale;
                        settle(window, &mut self.contacts);
                    }
                }
            }
        }
        self.windows = next_windows;
        self.touching = next_touching;
    }

    /// Drops the bodies of every entity not in `live` (entity-ascending, the join's own order, each
    /// with its spawn rank), walking the table in creation order — and drops any rejected-shape
    /// entry for an entity that has left too, so a despawned entity's dead shape does not sit in the
    /// map forever. Returns the entities whose bodies it removed, in table order.
    pub(crate) fn retire(&mut self, live: &[(EntityId, u64)]) -> Vec<EntityId> {
        let mut retired = Vec::new();
        let table_dirty = self.table.iter().any(|entry| !is_live(live, entry.entity));
        let rejected_dirty = self.rejected.keys().any(|entity| !is_live(live, *entity));
        if !table_dirty && !rejected_dirty {
            return retired;
        }
        if table_dirty {
            let mut kept = Vec::with_capacity(self.table.len());
            for entry in std::mem::take(&mut self.table) {
                if is_live(live, entry.entity) {
                    kept.push(entry);
                } else {
                    retired.push(entry.entity);
                    self.move_reports.remove(&entry.entity);
                    self.rapier.remove_body(entry.body);
                }
            }
            self.table = kept;
            self.reindex();
        }
        if rejected_dirty {
            self.rejected = std::mem::take(&mut self.rejected)
                .into_iter()
                .filter(|(entity, _)| is_live(live, *entity))
                .collect();
        }
        retired
    }

    /// Whether `entity` already has a body — the cheap guard that keeps the write system from
    /// reading a `Collider` it does not need.
    pub(crate) fn has_body(&self, entity: EntityId) -> bool {
        self.rows.contains_key(&entity)
    }

    /// Whether `entity` has a body built from a different `Collider` than the one it carries now
    /// — one comparison against [`BodyEntry::source`] and the mesh hash it was built from, no
    /// tolerance. `false` for an entity with no body at all: staleness only ever calls for a
    /// `rebuild`, never a [`create`](PhysicsWorld::create).
    pub(crate) fn is_stale(
        &self,
        entity: EntityId,
        collider: &Collider,
        current_mesh_hash: Option<u64>,
    ) -> bool {
        self.entry(entity)
            .is_some_and(|entry| entry.source != *collider || entry.mesh_hash != current_mesh_hash)
    }

    /// Appends a body for `entity` at the authored pose, or queues a [`ShapeRejection`] and does
    /// nothing if the shape cannot be built. `velocity` is the body's initial velocity, which only a
    /// rigid body carries. Also the tail of `rebuild`: a rebuilt entity's
    /// new body is created the same way a never-claimed one's is.
    ///
    /// A shape that was already rejected for this exact `Collider` is not retried — `create` is
    /// called again every tick an unclaimed entity has no body, and rebuilding the same doomed shape
    /// every tick would be wasted work on top of the once-per-session latch on the reporting side.
    /// The retry resumes the moment the entity's `Collider` changes (including changing back).
    pub(crate) fn create(
        &mut self,
        entity: EntityId,
        collider: &Collider,
        translation: [f32; 3],
        rotation: [f32; 4],
        velocity: Option<Velocity>,
        mesh: MeshInput,
    ) {
        let mesh_hash = mesh_hash_of(&mesh);
        if self
            .rejected
            .get(&entity)
            .is_some_and(|(rejected, hash, _)| rejected == collider && *hash == mesh_hash)
        {
            return;
        }
        let is_mesh = matches!(collider.shape, Shape::Mesh { .. });
        if matches!(collider.body, Body::Rigid(_)) {
            let kind = match collider.shape {
                Shape::Mesh { .. } => Some("a mesh collider"),
                Shape::Heightfield { .. } => Some("a heightfield collider"),
                _ => None,
            };
            if let Some(kind) = kind {
                let why = if is_mesh {
                    "a triangle mesh"
                } else {
                    "a heightfield"
                };
                self.reject(
                    entity,
                    collider,
                    mesh_hash,
                    code::COLLIDER_MESH_ON_RIGID,
                    format!("{kind} cannot be on a rigid body: {why} has no interior"),
                    Vec::new(),
                );
                return;
            }
        }
        if collider.sensor {
            let refusal = if matches!(collider.body, Body::Rigid(_)) {
                Some((
                    code::COLLIDER_SENSOR_ON_RIGID,
                    "a sensor cannot be a rigid body: it detects overlaps and never collides",
                ))
            } else if matches!(
                collider.shape,
                Shape::Mesh { .. } | Shape::Heightfield { .. }
            ) {
                Some((
                    code::COLLIDER_SENSOR_NO_INTERIOR,
                    "a mesh or heightfield sensor has no interior, so it could only fire on a surface crossing",
                ))
            } else {
                None
            };
            if let Some((refused, reason)) = refusal {
                self.reject(
                    entity,
                    collider,
                    mesh_hash,
                    refused,
                    reason.to_string(),
                    Vec::new(),
                );
                return;
            }
        }
        let groups = match self.groups_for(collider) {
            Ok(groups) => groups,
            Err(reason) => {
                let layer = code::COLLIDER_LAYER_UNKNOWN;
                self.reject(entity, collider, mesh_hash, layer, reason, Vec::new());
                return;
            }
        };
        let mesh_ref = match &mesh {
            Ok(mesh) => mesh.as_deref(),
            Err(cause) if is_mesh => {
                let unavailable = code::COLLIDER_MESH_UNAVAILABLE;
                let (reason, related) = match &collider.shape {
                    Shape::Mesh { path } if cause.code != unavailable => (
                        format!("the mesh `{path}` failed to load"),
                        vec![cause.clone()],
                    ),
                    _ => (cause.message.clone(), Vec::new()),
                };
                self.reject(entity, collider, mesh_hash, unavailable, reason, related);
                return;
            }
            Err(_) => None,
        };
        let shape = match shape_builder(&collider.shape, self.plane, mesh_ref) {
            Ok(shape) => shape,
            Err((code, reason)) => {
                self.reject(entity, collider, mesh_hash, code, reason, Vec::new());
                return;
            }
        };
        self.rejected.remove(&entity);
        let shape = shape.user_data(entity_bits(entity));
        let shape = match groups {
            Some(groups) => shape.collision_groups(groups),
            None => shape,
        };
        let pose = to_pose(translation, rotation);
        let (kind, tunables) = match &collider.body {
            Body::Static(_) => (BodyKind::Fixed, None),
            Body::Kinematic(_) => (BodyKind::Kinematic, None),
            Body::Rigid(tunables) => (BodyKind::Rigid, Some(tunables)),
        };
        let mut body = match kind {
            BodyKind::Fixed => RigidBodyBuilder::fixed(),
            BodyKind::Kinematic => RigidBodyBuilder::kinematic_position_based(),
            BodyKind::Rigid => RigidBodyBuilder::dynamic(),
        }
        .pose(pose);
        let mut collider_builder = shape;
        if let Some(tunables) = tunables {
            body = body
                .linear_damping(tunables.linear_damping)
                .angular_damping(tunables.angular_damping)
                .gravity_scale(tunables.gravity_scale)
                .locked_axes(locked_axes(tunables.lock, self.plane));
            if let Some(velocity) = velocity {
                body = body
                    .linvel(to_vector(velocity.linear))
                    .angvel(to_vector(velocity.angular));
            }
            collider_builder = collider_builder.mass(tunables.mass);
        }
        let material = collider.body.material();
        let mut collider_builder = collider_builder
            .friction(material.friction)
            .restitution(material.restitution)
            .friction_combine_rule(rapier_rule(material.friction_combine))
            .restitution_combine_rule(rapier_rule(material.restitution_combine));
        if collider.sensor {
            collider_builder = collider_builder.sensor(true);
        }
        let (body_handle, collider_handle) = self.rapier.insert(body, collider_builder);
        self.fresh_colliders.push(collider_handle);
        if let Some(tunables) = tunables {
            let sleep_after_ticks = tunables.sleep_after_ticks.unwrap_or(self.sleep_after_ticks);
            let activation = self.rapier.bodies[body_handle].activation_mut();
            activation.time_until_sleep = time_until_sleep(sleep_after_ticks, self.tick_rate);
            if let Some(threshold) = tunables.sleep_threshold {
                activation.normalized_linear_threshold = threshold;
            }
            if let Some(threshold) = tunables.sleep_angular_threshold {
                activation.angular_threshold = threshold;
            }
        }
        self.table.push(BodyEntry {
            entity,
            kind,
            body: body_handle,
            collider: collider_handle,
            mirrored: None,
            source: collider.clone(),
            mesh_hash,
        });
        self.rows.insert(entity, self.table.len() - 1);
    }

    /// Applies an edit of in-place tunables to `entity`'s body, returning `true` when it did. It
    /// does so only when the entity has a body, the recorded source and `collider` are the same
    /// body kind, and the two differ in nothing but the contact material (`friction`,
    /// `restitution` and both combine rules, any body kind) and, for a [`Body::Rigid`],
    /// `linear_damping`, `angular_damping`, `gravity_scale`, `sleep_after_ticks`, `sleep_threshold` and `sleep_angular_threshold`; otherwise it
    /// returns `false` and touches nothing, leaving the caller to `rebuild`.
    ///
    /// A rebuild replaces the body handle, which re-captures the zero of every `Joint` naming the
    /// body (`joints.rs::reconcile_joints` re-attaches on a vanished handle) and drops its contacts;
    /// these are plain scalars the solver re-reads every step, so they skip that. Shape, body
    /// kind, `mass`, `lock` and `layer` change the collider or the mass properties and rebuild.
    pub(crate) fn retune_in_place(
        &mut self,
        entity: EntityId,
        collider: &Collider,
        current_mesh_hash: Option<u64>,
    ) -> bool {
        let Some(&row) = self.rows.get(&entity) else {
            return false;
        };
        // A mesh edited in the same tick as the material still needs its triangles rebuilt.
        if self.table[row].mesh_hash != current_mesh_hash {
            return false;
        }
        // An unchanged collider reaching here means the mesh behind it changed: a rebuild.
        if self.table[row].source == *collider {
            return false;
        }
        let mut expected = self.table[row].source.clone();
        expected.body = match (&self.table[row].source.body, &collider.body) {
            (Body::Rigid(old), Body::Rigid(new)) => Body::Rigid(RigidBody {
                friction: new.friction,
                restitution: new.restitution,
                friction_combine: new.friction_combine,
                restitution_combine: new.restitution_combine,
                linear_damping: new.linear_damping,
                angular_damping: new.angular_damping,
                gravity_scale: new.gravity_scale,
                sleep_after_ticks: new.sleep_after_ticks,
                sleep_threshold: new.sleep_threshold,
                sleep_angular_threshold: new.sleep_angular_threshold,
                ..*old
            }),
            (Body::Static(_), Body::Static(new)) => Body::Static(*new),
            (Body::Kinematic(_), Body::Kinematic(new)) => Body::Kinematic(*new),
            _ => return false,
        };
        if expected != *collider {
            return false;
        }
        let material = collider.body.material();
        let entry = &self.table[row];
        let (body_handle, collider_handle) = (entry.body, entry.collider);
        let mut resting_on_it = Vec::new();
        if let Body::Rigid(new) = collider.body {
            let sleep_after_ticks = new.sleep_after_ticks.unwrap_or(self.sleep_after_ticks);
            let body = &mut self.rapier.bodies[body_handle];
            body.set_linear_damping(new.linear_damping);
            body.set_angular_damping(new.angular_damping);
            body.set_gravity_scale(new.gravity_scale, true);
            let activation = body.activation_mut();
            activation.time_until_sleep = time_until_sleep(sleep_after_ticks, self.tick_rate);
            activation.normalized_linear_threshold = new.sleep_threshold.unwrap_or(
                rapier3d::dynamics::RigidBodyActivation::default_normalized_linear_threshold(),
            );
            activation.angular_threshold = new
                .sleep_angular_threshold
                .unwrap_or(rapier3d::dynamics::RigidBodyActivation::default_angular_threshold());
            // The setters clear the sleeping flag at most, not the rest timer, so a body already
            // past its threshold would fall straight back asleep before the edit moved it. A
            // sleeping pair is also not re-solved, so the new material would go unread.
            body.wake_up(true);
        } else {
            // The same reason, for whatever rests on this static or kinematic body.
            for pair in self.rapier.narrow_phase.contact_pairs_with(collider_handle) {
                let other = if pair.collider1 == collider_handle {
                    pair.collider2
                } else {
                    pair.collider1
                };
                if let Some(parent) = self.rapier.colliders.get(other).and_then(|c| c.parent()) {
                    resting_on_it.push(parent);
                }
            }
        }
        for parent in resting_on_it {
            self.rapier.wake_up(parent, true);
        }
        let target = &mut self.rapier.colliders[collider_handle];
        target.set_friction(material.friction);
        target.set_restitution(material.restitution);
        target.set_friction_combine_rule(rapier_rule(material.friction_combine));
        target.set_restitution_combine_rule(rapier_rule(material.restitution_combine));
        self.table[row].source = collider.clone();
        true
    }

    /// Rebuilds `entity`'s body from its new `Collider`, at its current `translation`/`rotation`
    /// and `velocity`: the old body is removed and a fresh one built through
    /// [`create`](PhysicsWorld::create), so a live edit to shape, body kind, mass, lock or layer
    /// reaches the solver instead of sitting unread in the component forever (an edit of only the
    /// contact material, or a rigid body's damping, `gravity_scale` or `sleep_after_ticks`, is
    /// applied in place by `retune_in_place` instead).
    ///
    /// Every body still touching `entity` (this crate's own `touching` set) is woken
    /// (`wake_up(true)`) **before** the old body is removed, so nothing is left resting asleep on
    /// a shape that just shrank or vanished. `touching` itself is untouched — contacts are keyed by
    /// entity pair, so the next step's [`collect_contacts`](PhysicsWorld::collect_contacts) diff
    /// sees the rebuild as invisible: a pair still in contact emits nothing, a pair that stops
    /// emits `Stopped` as usual. If the new `Collider`'s shape cannot be built, the entity is left
    /// bodiless by `create`'s own rejection path, exactly as an unclaimed entity's first attempt
    /// would be.
    pub(crate) fn rebuild(
        &mut self,
        entity: EntityId,
        collider: &Collider,
        translation: [f32; 3],
        rotation: [f32; 4],
        velocity: Option<Velocity>,
        mesh: MeshInput,
    ) {
        // A rigid body with no `Velocity` component keeps its momentum across the rebuild.
        let velocity = velocity.or_else(|| {
            let entry = self.entry(entity)?;
            if entry.kind != BodyKind::Rigid {
                return None;
            }
            let body = self.rapier.bodies.get(entry.body)?;
            Some(Velocity {
                linear: from_vector(body.linvel()),
                angular: from_vector(body.angvel()),
            })
        });
        self.remove_entry(entity);
        self.create(entity, collider, translation, rotation, velocity, mesh);
    }

    /// Wakes every body still touching `entity` (this crate's own `touching` set), so nothing is
    /// left resting asleep on a shape that changed or vanished. Wakes go in spawn-rank order, never
    /// the entity-pair order `touching` is kept in; an unranked partner sorts last, and the row only
    /// makes the order total.
    fn wake_touching(&mut self, entity: EntityId) {
        let mut partners = Vec::new();
        for &(a, b) in &self.touching {
            let partner = if a == entity {
                b
            } else if b == entity {
                a
            } else {
                continue;
            };
            if let Some(&row) = self.rows.get(&partner) {
                partners.push((self.spawn_rank(partner), row));
            }
        }
        partners.sort_unstable();
        for (_, row) in partners {
            self.rapier.wake_up(self.table[row].body, true);
        }
    }

    /// Wakes what touches `entity`, then removes its body and table row. `touching` itself is left
    /// alone: contacts are keyed by entity pair, so the next step's diff sees the removal as a
    /// `Stopped`.
    fn remove_entry(&mut self, entity: EntityId) {
        self.wake_touching(entity);
        if let Some(&row) = self.rows.get(&entity) {
            self.rapier.remove_body(self.table[row].body);
            self.table.remove(row);
            self.reindex();
        }
    }

    /// Records `collider` as rejected for `entity` and queues the report.
    fn reject(
        &mut self,
        entity: EntityId,
        collider: &Collider,
        mesh_hash: Option<u64>,
        code: DiagnosticCode,
        reason: String,
        related: Vec<Diagnostic>,
    ) {
        self.rejected
            .insert(entity, (collider.clone(), mesh_hash, code));
        self.pending_rejections.push(ShapeRejection {
            entity,
            code,
            reason,
            related,
        });
    }

    /// Pushes this tick's authored state into the solver: a kinematic body's target pose, and a
    /// rigid body's pose and velocity when the game changed either since the last mirror.
    pub(crate) fn push(
        &mut self,
        entity: EntityId,
        translation: [f32; 3],
        rotation: [f32; 4],
        velocity: Option<Velocity>,
    ) {
        let Some(&row) = self.rows.get(&entity) else {
            return;
        };
        let entry = &self.table[row];
        let (kind, handle, mirrored) = (entry.kind, entry.body, entry.mirrored);
        let Some(body) = self.rapier.bodies.get_mut(handle) else {
            return;
        };
        match kind {
            BodyKind::Fixed => {
                // The ECS owns a static body's pose; a move since the solver last saw it re-poses
                // the body, and whatever rests on it is woken so a sleeper is not left in the air.
                let pose = to_pose(translation, rotation);
                if *body.position() != pose {
                    body.set_position(pose, false);
                    self.wake_touching(entity);
                }
            }
            BodyKind::Kinematic => {
                body.set_next_kinematic_position(to_pose(translation, rotation));
            }
            BodyKind::Rigid => {
                // Before the first mirror there is nothing to compare against: the authored pose
                // and velocity went in at creation.
                let Some(mirrored) = mirrored else {
                    return;
                };
                if mirrored.translation != translation || mirrored.rotation != rotation {
                    body.set_position(to_pose(translation, rotation), true);
                }
                if let Some(velocity) = velocity {
                    if mirrored.linear != velocity.linear {
                        body.set_linvel(to_vector(velocity.linear), true);
                    }
                    if mirrored.angular != velocity.angular {
                        body.set_angvel(to_vector(velocity.angular), true);
                    }
                }
            }
        }
    }

    /// Snaps a kinematic body to this pose with zero velocity — a teleport, not a move — and wakes what
    /// touches it, so a sleeper left at the old pose falls instead of hanging. No-op for any other body.
    pub(crate) fn snap_kinematic(
        &mut self,
        entity: EntityId,
        translation: [f32; 3],
        rotation: [f32; 4],
    ) {
        let Some(entry) = self.entry(entity) else {
            return;
        };
        if entry.kind != BodyKind::Kinematic {
            return;
        }
        let handle = entry.body;
        if let Some(body) = self.rapier.bodies.get_mut(handle) {
            body.set_position(to_pose(translation, rotation), true);
        }
        self.wake_touching(entity);
    }

    /// Reads every rigid body's resolved pose and velocity in table order, records them as the new
    /// comparison baseline, and hands them back for the mirror system to write into components.
    /// The mirror then [rebases](PhysicsWorld::rebase_mirrored) the pose half to what each
    /// component reads back, so a lossy pose type is not mistaken for an edit.
    ///
    /// Kinematic and static bodies are absent: the ECS owns their pose, and writing it back would
    /// make the pose two-owned — the `Arc<Mutex<World>>` smell in another shape.
    pub(crate) fn resolved(&mut self) -> Vec<(EntityId, [f32; 3], [f32; 4], Velocity)> {
        let mut out = Vec::with_capacity(self.table.len());
        for entry in &mut self.table {
            if entry.kind != BodyKind::Rigid {
                continue;
            }
            let Some(body) = self.rapier.bodies.get(entry.body) else {
                continue;
            };
            let pose = body.position();
            let translation = from_vector(pose.translation);
            let rotation = from_rotation(pose.rotation);
            let velocity = Velocity {
                linear: from_vector(body.linvel()),
                angular: from_vector(body.angvel()),
            };
            entry.mirrored = Some(Mirrored {
                translation,
                rotation,
                linear: velocity.linear,
                angular: velocity.angular,
            });
            out.push((entry.entity, translation, rotation, velocity));
        }
        out
    }

    /// Replaces each listed body's pose baseline with the pose its component read back after the
    /// mirror wrote it. A [`PhysicsPose`](crate::PhysicsPose) whose round trip is lossy (`Transform`
    /// stores Euler degrees) reports slightly different bits than the solver held; comparing the next
    /// tick's component against the solver's pose would read that difference as an edit and re-pose
    /// and wake the body forever. Velocity is stored losslessly and keeps its baseline.
    pub(crate) fn rebase_mirrored(&mut self, read_back: &[(EntityId, [f32; 3], [f32; 4])]) {
        for &(entity, translation, rotation) in read_back {
            let Some(&row) = self.rows.get(&entity) else {
                continue;
            };
            if let Some(mirrored) = &mut self.table[row].mirrored {
                mirrored.translation = translation;
                mirrored.rotation = rotation;
            }
        }
    }

    /// Reconciles the joint table against `live` — every carrier's current `Joint`, ordered by the
    /// carrier's spawn rank (`systems.rs::write`'s sort), so a joint is inserted in an order churn
    /// cannot permute. Called after the body
    /// loop, so `self.rows` already reflects this tick's creations, rebuilds and retirements.
    ///
    /// A carrier not in `live` any more is retired (its rapier joint removed, if any), and so is
    /// every entry marked [`broken`](JointEntry::broken): the flag only has to survive until the
    /// mirror removes its `Joint` component, which happens in the same tick this table saw the
    /// break, so by the next reconcile the row is stale — still present means a fresh `Joint` was
    /// re-added to the same carrier, which the row below then takes the `None` arm for and forms.
    /// Per carrier still present after that purge: a missing body drops the rapier joint (if
    /// attached) and queues the carrier on
    /// [`take_missing_joints`](PhysicsWorld::take_missing_joints) **only on the transition into
    /// inert** (a still-missing carrier is not re-queued every tick — a bare host that never drains
    /// the queue must not grow it without bound); a changed `Joint` drops and rebuilds at the
    /// bodies' current poses; and a handle that is `None`, or no longer present in
    /// `rapier.impulse_joints` (a body rebuild or retirement cascaded its removal), is reattached.
    pub(crate) fn reconcile_joints(&mut self, live: &[(EntityId, Joint)]) {
        // The binary searches below need entity order; the insert loop keeps `live`'s rank order.
        let mut carriers: Vec<EntityId> = live.iter().map(|(carrier, _)| *carrier).collect();
        carriers.sort_unstable();
        let stale = self
            .joints
            .iter()
            .any(|entry| entry.broken || carriers.binary_search(&entry.carrier).is_err());
        if stale {
            let mut kept = Vec::with_capacity(self.joints.len());
            for entry in std::mem::take(&mut self.joints) {
                if !entry.broken && carriers.binary_search(&entry.carrier).is_ok() {
                    kept.push(entry);
                } else if let Some(handle) = entry.handle {
                    self.rapier.remove_impulse_joint(handle);
                }
            }
            self.joints = kept;
            self.reindex_joints();
        }

        for (carrier, joint) in live {
            let bodies = self.joint_bodies(joint.a, joint.b);
            match self.joint_rows.get(carrier).copied() {
                None => {
                    let handle = bodies.map(|(a, b)| self.attach_joint(a, b, joint));
                    if bodies.is_none() {
                        self.pending_missing.push(*carrier);
                    }
                    self.joints.push(JointEntry {
                        carrier: *carrier,
                        handle,
                        broken: false,
                        source: joint.clone(),
                    });
                    self.joint_rows.insert(*carrier, self.joints.len() - 1);
                }
                Some(row) if bodies.is_none() => {
                    // Queue only on the transition into inert — `handle` was `Some` — so a
                    // standing-missing carrier is not re-queued every tick a bare host
                    // (`render`, `sweep`) never drains `take_missing_joints`.
                    if let Some(handle) = self.joints[row].handle.take() {
                        self.rapier.remove_impulse_joint(handle);
                        self.pending_missing.push(*carrier);
                    }
                }
                Some(row) if self.joints[row].source != *joint => {
                    if let Some(handle) = self.joints[row].handle.take() {
                        self.rapier.remove_impulse_joint(handle);
                    }
                    let (a, b) = bodies.expect("checked above");
                    self.joints[row].handle = Some(self.attach_joint(a, b, joint));
                    self.joints[row].source = joint.clone();
                }
                Some(row) => {
                    let needs_attach = match self.joints[row].handle {
                        None => true,
                        Some(handle) => !self.rapier.impulse_joints.contains(handle),
                    };
                    if needs_attach {
                        let (a, b) = bodies.expect("checked above");
                        self.joints[row].handle = Some(self.attach_joint(a, b, joint));
                    }
                }
            }
        }
    }

    /// Pushes each attached joint's `JointMotor` (`motors`, ascending by carrier — the typed join's
    /// own order) onto its rapier joint. The desired motor rows are compared with the rapier
    /// joint's current ones and written, waking both bodies, only on a difference: a retarget
    /// mutates the live joint, never rebuilds it, so its captured zero survives; a freshly attached
    /// joint has no motor, so the same comparison applies it on the tick it forms. A carrier whose
    /// motor cannot drive its joint stays passive and is queued on
    /// [`take_motor_mismatches`](PhysicsWorld::take_motor_mismatches) only on the transition into
    /// the mismatch; a carrier with no `JointMotor` — or no attached joint — is simply passive.
    pub(crate) fn drive_joints(&mut self, motors: &[(EntityId, JointMotor)]) {
        let tracked = &self.joint_rows;
        self.motor_mismatched
            .retain(|carrier| tracked.get(carrier).is_some());
        for row in 0..self.joints.len() {
            let entry = &self.joints[row];
            let Some(handle) = entry.handle else {
                continue;
            };
            if entry.broken {
                continue;
            }
            let carrier = entry.carrier;
            let motor = motors
                .binary_search_by_key(&carrier, |(id, _)| *id)
                .ok()
                .map(|i| &motors[i].1);
            let desired = match desired_motor(&entry.source.kind, motor) {
                Ok(rows) => {
                    if let Ok(at) = self.motor_mismatched.binary_search(&carrier) {
                        self.motor_mismatched.remove(at);
                    }
                    rows
                }
                Err(reason) => {
                    if let Err(at) = self.motor_mismatched.binary_search(&carrier) {
                        self.motor_mismatched.insert(at, carrier);
                        self.pending_motor_mismatches
                            .push(MotorMismatch { carrier, reason });
                    }
                    [None; 6]
                }
            };
            let Some(joint) = self.rapier.impulse_joints.get(handle) else {
                continue;
            };
            if current_motor(&joint.data) != Some(desired)
                && let Some(joint) = self.rapier.impulse_joints.get_mut(handle, true)
            {
                apply_motor(&mut joint.data, &desired);
            }
            if desired.iter().any(Option::is_some) {
                self.keep_motor_awake(handle, &desired);
            }
        }
    }

    /// Wakes a driven joint's sleeping dynamic bodies while its motor is off target, so a steady
    /// drive never stalls asleep short of it. Two flag reads when neither body sleeps.
    fn keep_motor_awake(&mut self, handle: ImpulseJointHandle, rows: &MotorRows) {
        let Some(joint) = self.rapier.impulse_joints.get(handle) else {
            return;
        };
        let (h1, h2) = (joint.body1(), joint.body2());
        let (Some(b1), Some(b2)) = (self.rapier.bodies.get(h1), self.rapier.bodies.get(h2)) else {
            return;
        };
        if !b1.is_sleeping() && !b2.is_sleeping() {
            return;
        }
        if !motor_off_target(*b1.position(), *b2.position(), &joint.data, rows) {
            return;
        }
        for body in [h1, h2] {
            if let Some(body) = self.rapier.bodies.get_mut(body)
                && body.is_dynamic()
                && body.is_sleeping()
            {
                body.wake_up(true);
            }
        }
    }

    /// `a` and `b`'s current body handles, or `None` if either is dead or has no body — a
    /// [`ShapeRejection`], a `static`/`kinematic`/`rigid` `Collider` never claimed, or nothing there
    /// at all.
    fn joint_bodies(&self, a: EntityId, b: EntityId) -> Option<(RigidBodyHandle, RigidBodyHandle)> {
        let &a_row = self.rows.get(&a)?;
        let &b_row = self.rows.get(&b)?;
        Some((self.table[a_row].body, self.table[b_row].body))
    }

    /// Builds `joint` at `a` and `b`'s current solver poses and inserts it, waking both bodies.
    fn attach_joint(
        &mut self,
        a: RigidBodyHandle,
        b: RigidBodyHandle,
        joint: &Joint,
    ) -> ImpulseJointHandle {
        let pa = *self
            .rapier
            .bodies
            .get(a)
            .expect("joint_bodies only returns live handles")
            .position();
        let pb = *self
            .rapier
            .bodies
            .get(b)
            .expect("joint_bodies only returns live handles")
            .position();
        let generic = build_generic_joint(pa, pb, joint);
        self.rapier.insert_impulse_joint(a, b, generic)
    }

    /// Walks the joint table (creation order) after a step, breaking any joint whose linear
    /// constraint impulse this step exceeded its authored `break_impulse`, then sorts this step's
    /// [`joint_events`](PhysicsWorld::joint_events) ascending by carrier — the [`ContactEvent`]
    /// contract.
    ///
    /// rapier writes each locked/limited DOF's impulse back once per solver substep — the pinned
    /// build's `num_solver_iterations` doubles as the substep count
    /// (`staged_island_solver/init.rs`) — so the raw per-substep magnitude read off the joint is
    /// only the *last* substep's share, not the step's. Scaling it by the substep count restores
    /// the per-step quantity `docs/physics-joints.md` and [`JointBroken::impulse`] promise.
    fn collect_broken_joints(&mut self) {
        let substeps = self
            .rapier
            .integration_parameters
            .num_solver_iterations
            .max(1) as f32;
        for row in 0..self.joints.len() {
            let Some(threshold) = self.joints[row].source.break_impulse else {
                continue;
            };
            let Some(handle) = self.joints[row].handle else {
                continue;
            };
            let Some(joint) = self.rapier.impulse_joints.get(handle) else {
                continue;
            };
            let impulses = joint.impulses;
            let mut sum_sq =
                impulses[0] * impulses[0] + impulses[1] * impulses[1] + impulses[2] * impulses[2];
            // A slider's own axis is free, not locked, so its constraint force never lands in
            // `impulses` above — only in its limit's impulse once the ride hits an end stop. That
            // has to count too, or a slider slamming its limit can never break (a ball's/hinge's
            // *angular* limits are excluded on purpose: a break is a linear-impulse quantity).
            if matches!(self.joints[row].source.kind, JointKind::Slider { .. }) {
                let limit = joint.data.limits[JointAxis::LinX as usize].impulse;
                sum_sq += limit * limit;
            }
            let magnitude = sum_sq.sqrt() * substeps;
            if magnitude <= threshold {
                continue;
            }
            self.rapier.remove_impulse_joint(handle);
            let carrier = self.joints[row].carrier;
            let a = self.joints[row].source.a;
            let b = self.joints[row].source.b;
            self.joints[row].handle = None;
            self.joints[row].broken = true;
            self.joint_events.push(JointBroken {
                joint: carrier,
                a,
                b,
                impulse: magnitude,
            });
        }
        self.joint_events.sort_by_key(|event| event.joint);
    }

    /// The layer table, if the scene declares one.
    pub(crate) fn layer_table(&self) -> Option<&CollisionLayers> {
        self.layers.as_ref()
    }

    /// `Err(QueryInput)` for the first of `names` that is neither `default` nor in the layer table.
    pub(crate) fn check_layer_names(&self, names: &[String]) -> Result<(), PhysicsError> {
        let declared: &[String] = match &self.layers {
            Some(table) => table.names(),
            None => &[],
        };
        for name in names {
            let known = name == DEFAULT_LAYER || declared.iter().any(|d| d == name);
            if !known {
                let declared_names: Vec<String> = if declared.is_empty() {
                    vec![DEFAULT_LAYER.to_string()]
                } else {
                    declared.to_vec()
                };
                return Err(PhysicsError::QueryInput(unknown_layer(
                    name,
                    &declared_names,
                )));
            }
        }
        Ok(())
    }

    /// `entity`'s authored shape and current world pose, for casting a body on its own.
    pub(crate) fn shape_and_pose(&self, entity: EntityId) -> Option<(Shape, [f32; 3], [f32; 4])> {
        let entry = self.entry(entity)?;
        let pose = self.rapier.colliders.get(entry.collider)?.position();
        Some((
            entry.source.shape.clone(),
            from_vector(pose.translation),
            from_rotation(pose.rotation),
        ))
    }

    /// The layer names a collider on `layer` ignores (`None` is `default`), ascending by layer index.
    pub(crate) fn ignored_by(&self, layer: Option<&str>) -> Vec<String> {
        let Some(table) = &self.layers else {
            return Vec::new();
        };
        let Some(index) = table.index_of(layer.unwrap_or(DEFAULT_LAYER)) else {
            return Vec::new();
        };
        let mask = table.ignores_mask(index);
        table
            .names()
            .iter()
            .enumerate()
            .filter(|(bit, _)| mask & (1 << bit) != 0)
            .map(|(_, name)| name.clone())
            .collect()
    }

    /// Whether `entity`'s body is rigid, which a motor pushes rather than walks around.
    pub(crate) fn is_rigid(&self, entity: EntityId) -> bool {
        self.entry(entity)
            .is_some_and(|entry| entry.kind == BodyKind::Rigid)
    }

    /// The last `move_character` call's report for `entity` and the frame it ran in, overwritten
    /// by every call. Never hashed or snapshotted: a restore empties it, and a retired entity's
    /// entry is dropped.
    pub fn last_move_report(&self, entity: EntityId) -> Option<(FrameIndex, &MoveReport)> {
        self.move_reports
            .get(&entity)
            .map(|(frame, report)| (*frame, report))
    }

    /// Fills the lane of `entity`'s stored report: the engine's `move_character` knows it only after
    /// the motor call has stored the report. A no-op without a stored report; never hashed.
    pub fn attach_lane_report(&mut self, entity: EntityId, lane: LaneReport) {
        if let Some((_, report)) = self.move_reports.get_mut(&entity) {
            report.lane = Some(lane);
        }
    }

    /// The table row for `entity`, if it has one.
    pub(crate) fn entry(&self, entity: EntityId) -> Option<&BodyEntry> {
        self.rows.get(&entity).map(|&row| &self.table[row])
    }

    /// Test hook: the spawn-rank side table; never hashed, not API.
    #[doc(hidden)]
    pub fn spawn_ranks_view(&self) -> &[(EntityId, u64)] {
        &self.ranks
    }

    /// Test hook: rapier's awake-body order; never hashed, not API.
    #[doc(hidden)]
    pub fn awake_order(&self) -> Vec<EntityId> {
        self.rapier
            .islands
            .active_bodies()
            .filter_map(|handle| self.table.iter().find(|e| e.body == handle))
            .map(|entry| entry.entity)
            .collect()
    }

    /// Replaces the query-only layer side table; `layers` is entity-sorted. Skipped when equal;
    /// otherwise stored and resolved against the current layer table.
    pub(crate) fn set_query_layers(&mut self, layers: &[(EntityId, LayerSet)]) {
        if self.query_reported && self.query_layers == layers {
            return;
        }
        self.query_layers = layers.to_vec();
        self.resolve_query_masks(true);
    }

    /// Refreshes the query-only layers from `world`; call it after a restore, before
    /// [`apply_settings`](PhysicsWorld::apply_settings), so an include query answers before the first
    /// write. The write refreshes them itself every tick.
    pub fn sync_query_layers(&mut self, world: &World) {
        let mut layers: Vec<(EntityId, LayerSet)> = self
            .entities()
            .filter_map(|entity| match world.get::<crate::QueryLayers>(entity) {
                Ok(Some(query)) => Some((entity, query.layers.clone())),
                _ => None,
            })
            .collect();
        layers.sort_unstable_by_key(|(entity, _)| *entity);
        self.query_layers = layers;
        self.resolve_query_masks(false);
    }

    /// Resolves each entity's `QueryLayers` to a bitmask under the current layer table. An entity
    /// naming an undeclared layer gets mask `0` and one `L1186` rejection, and keeps its body.
    fn resolve_query_masks(&mut self, report: bool) {
        let mut masks = Vec::with_capacity(self.query_layers.len());
        let mut rejections = Vec::new();
        for (entity, set) in &self.query_layers {
            match self.check_layer_names(set.as_slice()) {
                Ok(()) => masks.push((*entity, self.layer_mask(set.as_slice()))),
                Err(err) => {
                    masks.push((*entity, 0));
                    let PhysicsError::QueryInput(reason) = err else {
                        continue;
                    };
                    rejections.push(ShapeRejection {
                        entity: *entity,
                        code: code::COLLIDER_LAYER_UNKNOWN,
                        reason: format!("`QueryLayers` {reason}"),
                        related: Vec::new(),
                    });
                }
            }
        }
        self.query_masks = masks;
        self.query_reported = report;
        if report {
            for rejection in rejections {
                self.queue_rejection(rejection);
            }
        }
    }

    /// Replaces the surface-tag side table; `tags` is entity-sorted. Skipped when equal; otherwise
    /// stored and resolved against the current surface table.
    pub(crate) fn set_surface_tags(
        &mut self,
        tags: &[(EntityId, String)],
        overlays: &[(EntityId, String, i32)],
    ) {
        if self.surfaces_reported && self.surface_tags == tags && self.surface_overlays == overlays
        {
            return;
        }
        self.surface_tags = tags.to_vec();
        self.surface_overlays = overlays.to_vec();
        self.resolve_surface_ids(true);
    }

    /// Refreshes the surface tags from `world`; call it after a restore, before
    /// [`apply_settings`](PhysicsWorld::apply_settings), so a hit answers before the first write.
    /// The write refreshes them itself every tick.
    ///
    /// # Example
    /// ```
    /// use ludion_core::time::TickRate;
    /// use ludion_ecs::World;
    /// use ludion_physics::PhysicsWorld;
    ///
    /// let mut physics = PhysicsWorld::new(TickRate::DEFAULT);
    /// physics.sync_surface_tags(&World::new());
    /// assert_eq!(physics.surface_id("concrete"), None);
    /// ```
    pub fn sync_surface_tags(&mut self, world: &World) {
        let mut tags: Vec<(EntityId, String)> = self
            .entities()
            .filter_map(|entity| match world.get::<crate::SurfaceTag>(entity) {
                Ok(Some(tag)) => Some((entity, tag.surface.clone())),
                _ => None,
            })
            .collect();
        tags.sort_unstable_by_key(|(entity, _)| *entity);
        let mut overlays: Vec<(EntityId, String, i32)> = self
            .entities()
            .filter_map(|entity| match world.get::<crate::SurfaceOverlay>(entity) {
                Ok(Some(o)) => Some((entity, o.surface.clone(), o.priority)),
                _ => None,
            })
            .collect();
        overlays.sort_unstable_by_key(|(entity, _, _)| *entity);
        self.surface_tags = tags;
        self.surface_overlays = overlays;
        self.resolve_surface_ids(false);
    }

    /// The `L1690` rejection for `entity`'s `component` naming the undeclared surface `name`.
    fn undeclared_surface(&self, entity: EntityId, component: &str, name: &str) -> ShapeRejection {
        let declared: &[String] = self.surfaces.as_ref().map_or(&[], SurfaceTable::names);
        let hint = match nearest(name, declared.iter().map(String::as_str)) {
            Some(near) => format!("nearest match is `{near}`"),
            None => "declare it under `physics.surfaces`".to_string(),
        };
        ShapeRejection {
            entity,
            code: code::SURFACE_UNKNOWN,
            reason: format!(
                "`{component}` surface `{name}` names no surface the scene's \
                 `physics.surfaces` declares; {hint}"
            ),
            related: Vec::new(),
        }
    }

    /// Resolves each entity's `SurfaceTag` and `SurfaceOverlay` to ids under the current surface
    /// table. An entity naming an undeclared surface gets no id (a tag's hits report the default,
    /// an overlay is inert) and one `L1690` rejection, and keeps its body.
    fn resolve_surface_ids(&mut self, report: bool) {
        let mut ids = Vec::with_capacity(self.surface_tags.len());
        let mut rejections = Vec::new();
        for (entity, name) in &self.surface_tags {
            match self.surfaces.as_ref().and_then(|table| table.id(name)) {
                Some(id) => ids.push((*entity, id)),
                None => rejections.push(self.undeclared_surface(*entity, "SurfaceTag", name)),
            }
        }
        let mut overlay_ids = Vec::with_capacity(self.surface_overlays.len());
        for (entity, name, priority) in &self.surface_overlays {
            match self.surfaces.as_ref().and_then(|table| table.id(name)) {
                Some(id) => overlay_ids.push((*entity, id, *priority)),
                None => rejections.push(self.undeclared_surface(*entity, "SurfaceOverlay", name)),
            }
        }
        overlay_ids
            .sort_unstable_by_key(|&(entity, _, priority)| (std::cmp::Reverse(priority), entity));
        self.overlay_ids = overlay_ids;
        self.surface_ids = ids;
        self.surfaces_reported = report;
        if report {
            for rejection in rejections {
                self.queue_rejection(rejection);
            }
        }
    }

    /// The id of the declared surface `name`, or `None` when the scene declares no such surface.
    ///
    /// # Example
    /// ```
    /// use ludion_core::time::TickRate;
    /// use ludion_physics::{PhysicsSettings, PhysicsSpace, PhysicsWorld, SurfaceId, SurfaceTable};
    ///
    /// let mut world = PhysicsWorld::new(TickRate::DEFAULT);
    /// world.apply_settings(&PhysicsSettings {
    ///     surfaces: Some(SurfaceTable::new(vec!["concrete".into()], None)),
    ///     ..PhysicsSettings::new(PhysicsSpace::Free)
    /// });
    /// assert_eq!(world.surface_id("concrete"), Some(SurfaceId(0)));
    /// assert_eq!(world.surface_name(SurfaceId(0)), Some("concrete"));
    /// ```
    pub fn surface_id(&self, name: &str) -> Option<SurfaceId> {
        self.surfaces.as_ref()?.id(name)
    }

    /// The declared name of `id`, or `None` when out of range.
    pub fn surface_name(&self, id: SurfaceId) -> Option<&str> {
        self.surfaces.as_ref()?.name(id)
    }

    /// The surface `entity`'s collider reports: its `SurfaceTag`, else the scene's
    /// `default_surface`, else `None`. What a `ContactEvent` consumer calls to name a contact's
    /// surface.
    pub fn surface_of(&self, entity: EntityId) -> Option<SurfaceId> {
        self.surface_ids
            .binary_search_by_key(&entity, |&(e, _)| e)
            .map(|i| self.surface_ids[i].1)
            .ok()
            .or_else(|| self.surfaces.as_ref()?.default())
    }

    /// The surface of the first `SurfaceOverlay` (highest priority, then lowest entity) whose
    /// sensor volume contains `point`, as of the last completed step. An overlay with no body is
    /// skipped.
    fn overlay_at(&self, point: [f32; 3]) -> Option<SurfaceId> {
        let p = to_vector(point);
        self.overlay_ids.iter().find_map(|&(entity, id, _)| {
            let &row = self.rows.get(&entity)?;
            let collider = &self.rapier.colliders[self.table[row].collider];
            let aabb = collider.compute_aabb();
            let inside_box = p.x >= aabb.mins.x
                && p.y >= aabb.mins.y
                && p.z >= aabb.mins.z
                && p.x <= aabb.maxs.x
                && p.y <= aabb.maxs.y
                && p.z <= aabb.maxs.z;
            (inside_box && collider.shape().contains_point(collider.position(), p)).then_some(id)
        })
    }

    /// The surface a hit on `entity`'s collider at `point` reports: an overlay containing `point`,
    /// else the entity's `SurfaceTag`, else the scene's `default_surface`, else `None`.
    ///
    /// # Example
    /// ```
    /// use ludion_core::time::TickRate;
    /// use ludion_ecs::World;
    /// use ludion_physics::PhysicsWorld;
    ///
    /// let physics = PhysicsWorld::new(TickRate::DEFAULT);
    /// let entity = World::new().spawn();
    /// assert_eq!(physics.surface_at(entity, [0.0; 3]), None);
    /// ```
    pub fn surface_at(&self, entity: EntityId, point: [f32; 3]) -> Option<SurfaceId> {
        self.overlay_at(point).or_else(|| self.surface_of(entity))
    }

    /// The surface under `point`: an overlay containing it, else the surface of the nearest
    /// collider a downward ray of `reach` meets through `filter` (resolved by
    /// [`surface_at`](PhysicsWorld::surface_at) at the hit point). `Ok(None)` when nothing is
    /// under it. A footstep system and noise emission share this one answer.
    ///
    /// `point` must be finite and `reach` finite and non-negative; anything else is
    /// [`PhysicsError::QueryInput`].
    ///
    /// # Example
    /// ```
    /// use ludion_core::time::TickRate;
    /// use ludion_physics::{PhysicsWorld, QueryFilter};
    ///
    /// let world = PhysicsWorld::new(TickRate::DEFAULT);
    /// let under = world.surface_under([0.0, 1.0, 0.0], 2.0, &QueryFilter::new()).unwrap();
    /// assert_eq!(under, None);
    /// ```
    pub fn surface_under(
        &self,
        point: [f32; 3],
        reach: f32,
        filter: &QueryFilter,
    ) -> Result<Option<SurfaceId>, PhysicsError> {
        crate::queries::check_ray(point, [0.0, -1.0, 0.0], reach)?;
        if let Some(id) = self.overlay_at(point) {
            return Ok(Some(id));
        }
        let hits = self.ray_cast(point, [0.0, -1.0, 0.0], reach, filter)?;
        Ok(hits.first().and_then(|hit| hit.surface))
    }

    /// `entity`'s query-only layer bitmask; `0` when it has none or names an undeclared layer.
    pub(crate) fn query_mask(&self, entity: EntityId) -> u32 {
        self.query_masks
            .binary_search_by_key(&entity, |&(e, _)| e)
            .map_or(0, |i| self.query_masks[i].1)
    }

    /// Replaces the spawn-rank side table; `ranks` is entity-sorted. Reuses the allocation.
    pub(crate) fn set_ranks(&mut self, ranks: &[(EntityId, u64)]) {
        self.ranks.clear();
        self.ranks.extend_from_slice(ranks);
    }

    /// Refreshes the spawn ranks from `world`; call it after a restore, before anything that can
    /// wake a body. The write refreshes them itself every tick. Only bodies with a table row can be
    /// woken or blasted, so those are the entities it ranks.
    pub fn sync_spawn_ranks(&mut self, world: &World) {
        self.ranks = crate::systems::spawn_ranks(world, self.entities());
    }

    /// `entity`'s ECS spawn rank, the order blasts and partner wakes follow; `u64::MAX` for an
    /// entity the table does not rank, a total-order guard.
    pub(crate) fn spawn_rank(&self, entity: EntityId) -> u64 {
        self.ranks
            .binary_search_by_key(&entity, |&(e, _)| e)
            .map_or(u64::MAX, |i| self.ranks[i].1)
    }

    /// `entity`'s body handle if it is `rigid`, or why it is not: no body at all, or a `static` or
    /// `kinematic` one.
    pub(crate) fn rigid_body_of(&self, entity: EntityId) -> Result<RigidBodyHandle, &'static str> {
        match self.entry(entity) {
            None => Err("the entity has no rigid body"),
            Some(entry) if entry.kind != BodyKind::Rigid => {
                Err("the entity's body is static or kinematic, and impulses move only rigid bodies")
            }
            Some(entry) => Ok(entry.body),
        }
    }

    /// Rebuilds the entity→row lookup by walking the table in its own order.
    fn reindex(&mut self) {
        self.rows.clear();
        for (row, entry) in self.table.iter().enumerate() {
            self.rows.insert(entry.entity, row);
        }
    }

    /// Rebuilds the carrier→row lookup for `joints` by walking it in its own order.
    fn reindex_joints(&mut self) {
        self.joint_rows.clear();
        for (row, entry) in self.joints.iter().enumerate() {
            self.joint_rows.insert(entry.carrier, row);
        }
    }
}

/// The fixed timestep as the solver takes it: seconds per tick, narrowed to `f32`.
fn seconds_per_tick(tick_rate: TickRate) -> Real {
    #[allow(clippy::cast_possible_truncation)]
    // the presentation of an exact 1/hz, as SystemCtx::dt
    let dt = tick_rate.seconds_per_tick() as f32;
    dt
}

/// Seconds of near-rest motion before a body may sleep, from the authored tick count.
///
/// The half-tick offset is deliberate. The solver accumulates `dt` per step and sleeps at
/// `>=`, and repeated addition of `dt` is not bit-equal to `n * dt`, so a threshold at exactly
/// `ticks * dt` could land on either side of the comparison by an ulp; offset by half a tick, the
/// crossing tick is unambiguous. `sleep_after_ticks: 0` therefore yields a negative threshold,
/// which means the body may sleep as soon as it is at rest.
fn time_until_sleep(sleep_after_ticks: u32, tick_rate: TickRate) -> Real {
    (sleep_after_ticks as f32 - 0.5) * seconds_per_tick(tick_rate)
}

/// The solver locks for a rigid body: the body's own [`AxisLock`] OR'd with the plane's.
///
/// A plane locks the off-plane translation axis and the two **in-plane** rotation axes, leaving
/// rotation about the plane normal free — the one degree of rotational freedom a 2D game has.
fn locked_axes(lock: AxisLock, plane: Option<Plane>) -> LockedAxes {
    let translation = [
        LockedAxes::TRANSLATION_LOCKED_X,
        LockedAxes::TRANSLATION_LOCKED_Y,
        LockedAxes::TRANSLATION_LOCKED_Z,
    ];
    let rotation = [
        LockedAxes::ROTATION_LOCKED_X,
        LockedAxes::ROTATION_LOCKED_Y,
        LockedAxes::ROTATION_LOCKED_Z,
    ];
    let mut axes = LockedAxes::empty();
    for axis in 0..3 {
        if lock.translation[axis] {
            axes |= translation[axis];
        }
        if lock.rotation[axis] {
            axes |= rotation[axis];
        }
    }
    if let Some(plane) = plane {
        let [first, second] = plane.axes();
        axes |= translation[off_plane_axis(plane)];
        axes |= rotation[first] | rotation[second];
    }
    axes
}

fn rapier_rule(rule: CombineRule) -> CoefficientCombineRule {
    match rule {
        CombineRule::Average => CoefficientCombineRule::Average,
        CombineRule::Min => CoefficientCombineRule::Min,
        CombineRule::Multiply => CoefficientCombineRule::Multiply,
        CombineRule::Max => CoefficientCombineRule::Max,
    }
}

fn combine_rule(rule: CoefficientCombineRule) -> CombineRule {
    match rule {
        CoefficientCombineRule::Min => CombineRule::Min,
        CoefficientCombineRule::Multiply => CombineRule::Multiply,
        CoefficientCombineRule::Max => CombineRule::Max,
        // The two rules the authored surface does not offer never reach a built collider.
        _ => CombineRule::Average,
    }
}

/// The one world axis a plane's 2D queries ignore, as an index into `[x, y, z]`.
fn off_plane_axis(plane: Plane) -> usize {
    match plane {
        Plane::Xz => 1,
        Plane::Xy => 2,
    }
}

/// The collider builder for an authored shape, or the [`DiagnosticCode`] and reason a
/// [`ShapeRejection`] carries when the shape cannot be built.
///
/// | authored `shape` | parry shape |
/// |---|---|
/// | `sphere` | `Ball` |
/// | `box` | `Cuboid` |
/// | `capsule` | `Capsule` along `y` |
/// | `cylinder` | `Cylinder` along `y` |
/// | `convex` | `ConvexPolyhedron` over the authored points |
/// | `heightfield` | `HeightField` |
/// | `compound` | `Compound`, its parts posed by their authored translation and Euler XYZ degrees |
/// | `mesh` | `TriMesh` over the resolved document, `FIX_INTERNAL_EDGES` |
/// | `circle` | `Ball` |
/// | `aabb` | `Cuboid`, in-plane half-extents on the plane's two axes and the **first** of them
/// repeated on the off-plane axis |
///
/// The off-plane extent has to come from somewhere — the authored form has only two — and taking
/// the first in-plane half-extent keeps it a pure function of the data, with no hidden constant and
/// no dependence on which axis happens to be larger. The plane an in-plane shape is laid out on is
/// [`Plane::Xz`] when the world has none.
///
/// `convex`, `heightfield`, `compound` and `mesh` are the only shapes that can fail: every other shape's
/// fields are unconditionally buildable once they reach here (the `Collider` validator rejects a
/// non-positive extent before this point for a scene-authored collider; game code that inserts one
/// directly skips that check, same as it skips the hull, grid and part checks this function makes
/// for itself).
pub(crate) fn shape_builder(
    shape: &Shape,
    plane: Option<Plane>,
    mesh: Option<&TriangleMesh>,
) -> Result<ColliderBuilder, (DiagnosticCode, String)> {
    match shape {
        Shape::Circle { radius } | Shape::Sphere { radius } => Ok(ColliderBuilder::ball(*radius)),
        Shape::Aabb { half_extents } => {
            let [first, second] = plane.unwrap_or(Plane::Xz).axes();
            let mut extents = [half_extents[0]; 3];
            extents[first] = half_extents[0];
            extents[second] = half_extents[1];
            Ok(ColliderBuilder::cuboid(extents[0], extents[1], extents[2]))
        }
        Shape::Box { half_extents } => Ok(ColliderBuilder::cuboid(
            half_extents[0],
            half_extents[1],
            half_extents[2],
        )),
        Shape::Capsule {
            half_height,
            radius,
        } => Ok(ColliderBuilder::capsule_y(*half_height, *radius)),
        Shape::Cylinder {
            half_height,
            radius,
        } => Ok(ColliderBuilder::cylinder(*half_height, *radius)),
        Shape::Convex { points } => {
            let vectors: Vec<Vector> = points.iter().copied().map(to_vector).collect();
            ColliderBuilder::convex_hull(&vectors).ok_or_else(|| {
                (
                    code::COLLIDER_DEGENERATE_HULL,
                    format!(
                        "the solver could not build a convex hull from {} points; they are flat, \
                         collinear or coincident within its tolerance",
                        points.len()
                    ),
                )
            })
        }
        Shape::Heightfield {
            rows,
            cols,
            heights,
            scale,
        } => heightfield(*rows, *cols, heights, *scale)
            .map_err(|reason| (code::COLLIDER_HEIGHTFIELD_GRID, reason)),
        Shape::Compound { parts } => compound(parts),
        Shape::Mesh { path } => {
            let mesh = mesh.ok_or_else(|| {
                (
                    code::COLLIDER_MESH_UNAVAILABLE,
                    format!("the mesh `{path}` was not resolved"),
                )
            })?;
            let vertices = mesh.vertices.iter().copied().map(to_vector).collect();
            ColliderBuilder::trimesh_with_flags(
                vertices,
                mesh.triangles.clone(),
                TriMeshFlags::FIX_INTERNAL_EDGES,
            )
            .map_err(|error| {
                (
                    code::COLLIDER_MESH_UNAVAILABLE,
                    format!("the solver refused the mesh `{path}`: {error}"),
                )
            })
        }
    }
}

/// A compound collider over `parts`, in authored order, each posed by its translation and Euler XYZ
/// degrees (the conversion `Transform` uses). parry sums the parts' mass properties by volume, so
/// the centre of mass comes from the geometry.
fn compound(parts: &[CompoundPart]) -> Result<ColliderBuilder, (DiagnosticCode, String)> {
    if parts.is_empty() {
        return Err((
            code::COLLIDER_COMPOUND_INVALID,
            "a compound needs at least one part".to_string(),
        ));
    }
    let mut built = Vec::with_capacity(parts.len());
    for (index, part) in parts.iter().enumerate() {
        if !part
            .translation
            .iter()
            .chain(&part.rotation)
            .all(|v| v.is_finite())
        {
            return Err((
                code::COLLIDER_COMPOUND_INVALID,
                format!("compound part {index}: translation/rotation is not finite"),
            ));
        }
        let shape = part_shape(&part.shape)
            .map_err(|(code, reason)| (code, format!("compound part {index}: {reason}")))?;
        let pose = to_pose(part.translation, euler_xyz_degrees_to_quat(part.rotation));
        built.push((pose, shape));
    }
    Ok(ColliderBuilder::compound(built))
}

/// The solver shape of one compound part. `compound`, `heightfield`, `mesh`, `circle` and `aabb` are not
/// parts: parry panics on a nested composite, and a direct insert skips the validator.
fn part_shape(shape: &Shape) -> Result<SharedShape, (DiagnosticCode, String)> {
    match shape {
        Shape::Sphere { radius } => Ok(SharedShape::ball(*radius)),
        Shape::Box { half_extents } => Ok(SharedShape::cuboid(
            half_extents[0],
            half_extents[1],
            half_extents[2],
        )),
        Shape::Capsule {
            half_height,
            radius,
        } => Ok(SharedShape::capsule_y(*half_height, *radius)),
        Shape::Cylinder {
            half_height,
            radius,
        } => Ok(SharedShape::cylinder(*half_height, *radius)),
        Shape::Convex { points } => {
            let vectors: Vec<Vector> = points.iter().copied().map(to_vector).collect();
            SharedShape::convex_hull(&vectors).ok_or_else(|| {
                (
                    code::COLLIDER_DEGENERATE_HULL,
                    format!(
                        "the solver could not build a convex hull from {} points; they are flat, \
                         collinear or coincident within its tolerance",
                        points.len()
                    ),
                )
            })
        }
        Shape::Circle { .. } => Err(not_a_part("circle")),
        Shape::Aabb { .. } => Err(not_a_part("aabb")),
        Shape::Heightfield { .. } => Err(not_a_part("heightfield")),
        Shape::Compound { .. } => Err(not_a_part("compound")),
        Shape::Mesh { .. } => Err(not_a_part("mesh")),
    }
}

fn not_a_part(kind: &str) -> (DiagnosticCode, String) {
    (
        code::COLLIDER_COMPOUND_INVALID,
        format!("a compound part cannot be a `{kind}`"),
    )
}

/// A heightfield collider, or the reason it could not be built: the grid is ragged, or smaller than
/// 2×2 (parry needs at least one cell in each direction). The wording matches the `Collider`
/// validator's for the same fault.
///
/// The authored grid is row-major and parry's is column-major, with the **row** index advancing
/// along world `z` and the **column** index along world `x`.
fn heightfield(
    rows: u32,
    cols: u32,
    heights: &[f32],
    scale: [f32; 3],
) -> Result<ColliderBuilder, String> {
    let (nrows, ncols) = (rows as usize, cols as usize);
    if nrows < 2 {
        return Err(format!("`rows` must be at least 2, found {rows}"));
    }
    if ncols < 2 {
        return Err(format!("`cols` must be at least 2, found {cols}"));
    }
    let expected = nrows * ncols;
    if heights.len() != expected {
        return Err(format!(
            "found {} heights, a {rows}\u{d7}{cols} grid needs {expected}",
            heights.len()
        ));
    }
    let mut column_major = Vec::with_capacity(heights.len());
    for col in 0..ncols {
        for row in 0..nrows {
            column_major.push(heights[row * ncols + col]);
        }
    }
    Ok(ColliderBuilder::heightfield(
        Array2::new(nrows, ncols, column_major),
        to_vector(scale),
    ))
}

/// The contact a [`Started`](ContactPhase::Started) event reports for `pair`: the world-space
/// point midway between the two surfaces and the unit normal out of `co1` (towards `co2`).
///
/// Candidates are the solver contacts, each read through its manifold point's local frames and the
/// colliders' `poses` at collision detection (`[co1, co2]`), the frames the manifold was built in,
/// so neither rapier's contact recycling nor the step's own motion leaves it stale. The best one
/// has the largest impulse, then the smallest depth, then the smallest point, then the smallest
/// normal, each by `total_cmp` — an order that never falls back to the solver's own.
fn contact_point(pair: &ContactPair, poses: [Pose; 2]) -> Option<([f32; 3], [f32; 3])> {
    use std::cmp::Ordering;
    let mut best: Option<(f32, f32, [f32; 3], [f32; 3])> = None;
    for manifold in pair.solver_manifolds() {
        let f1 = manifold.subshape_pos1().prepend_to(&poses[0]);
        let f2 = manifold.subshape_pos2().prepend_to(&poses[1]);
        let normal = from_vector(f1.rotation * manifold.local_n1);
        for solver in &manifold.data.solver_contacts {
            let id = (solver.contact_id[0] & !NEW_CONTACT_BIT) as usize;
            let Some(pt) = manifold.points.get(id) else {
                continue;
            };
            let point = from_vector((f1 * pt.local_p1 + f2 * pt.local_p2) * 0.5);
            let candidate = (pt.data.impulse, pt.dist, point, normal);
            let better = best.is_none_or(|(impulse, dist, p, n)| {
                let order = candidate
                    .0
                    .total_cmp(&impulse)
                    .reverse()
                    .then(candidate.1.total_cmp(&dist))
                    .then_with(|| cmp_triple(&candidate.2, &p))
                    .then_with(|| cmp_triple(&candidate.3, &n));
                order == Ordering::Less
            });
            if better {
                best = Some(candidate);
            }
        }
    }
    best.map(|(_, _, point, normal)| (point, normal))
}

/// Whether `pair` is actually touching: it exchanged normal impulse this step, or a solver contact
/// has closed its gap at detection (`dist <= 0`). A pair inside the prediction margin with the gap
/// still open and no impulse is active but not engaged.
fn engaged(pair: &ContactPair) -> bool {
    if pair.total_impulse_magnitude() > 0.0 {
        return true;
    }
    pair.solver_manifolds().iter().any(|manifold| {
        manifold.data.solver_contacts.iter().any(|solver| {
            let id = (solver.contact_id[0] & !NEW_CONTACT_BIT) as usize;
            manifold.points.get(id).is_some_and(|pt| pt.dist <= 0.0)
        })
    })
}

/// Lexicographic `total_cmp` of two triples.
fn cmp_triple(a: &[f32; 3], b: &[f32; 3]) -> std::cmp::Ordering {
    a[0].total_cmp(&b[0])
        .then(a[1].total_cmp(&b[1]))
        .then(a[2].total_cmp(&b[2]))
}

/// A solver vector from a world-space triple.
pub(crate) fn to_vector(value: [f32; 3]) -> Vector {
    Vector::new(value[0], value[1], value[2])
}

/// A world-space triple from a solver vector.
pub(crate) fn from_vector(value: Vector) -> [f32; 3] {
    [value.x, value.y, value.z]
}

/// A world-space quaternion `[x, y, z, w]` from a solver rotation.
fn from_rotation(value: Rotation) -> [f32; 4] {
    [value.x, value.y, value.z, value.w]
}

/// A solver pose from a world-space translation and quaternion `[x, y, z, w]`.
pub(crate) fn to_pose(translation: [f32; 3], rotation: [f32; 4]) -> Pose {
    Pose::from_parts(
        to_vector(translation),
        Rotation::from_xyzw(rotation[0], rotation[1], rotation[2], rotation[3]),
    )
}

/// A collider's `user_data`, encoding `entity` so [`collect_contacts`](PhysicsWorld::collect_contacts)
/// can map a contact pair's two colliders back to entities without a second lookup table.
fn entity_bits(entity: EntityId) -> u128 {
    u128::from(entity.index()) | (u128::from(entity.generation()) << 32)
}

/// Validates a tagged blob's version and solver pin (the bytes after the magic) and returns the
/// body that follows them.
fn check_blob_tag(rest: &[u8]) -> Result<&[u8], PhysicsError> {
    let truncated = || PhysicsError::Decode("the blob's version tag is truncated".to_string());
    let (version, rest) = rest.split_first_chunk::<4>().ok_or_else(truncated)?;
    let (pin_len, rest) = rest.split_first_chunk::<2>().ok_or_else(truncated)?;
    let version = u32::from_le_bytes(*version);
    if version != PHYSICS_BLOB_VERSION {
        return Err(PhysicsError::Decode(format!(
            "the blob is layout version {version}, but this build reads version \
             {PHYSICS_BLOB_VERSION}"
        )));
    }
    let pin_len = usize::from(u16::from_le_bytes(*pin_len));
    if rest.len() < pin_len {
        return Err(truncated());
    }
    let (pin, body) = rest.split_at(pin_len);
    if pin != rapier3d::VERSION.as_bytes() {
        return Err(PhysicsError::Decode(format!(
            "the blob was written by solver pin {}, but this build is pinned to {}",
            String::from_utf8_lossy(pin),
            rapier3d::VERSION
        )));
    }
    Ok(body)
}

/// The inverse of [`entity_bits`].
pub(crate) fn entity_from_bits(bits: u128) -> EntityId {
    EntityId::new(bits as u32, (bits >> 32) as u32)
}

/// Whether `entity` is in `live`, the entity-sorted live list.
fn is_live(live: &[(EntityId, u64)], entity: EntityId) -> bool {
    live.binary_search_by_key(&entity, |&(e, _)| e).is_ok()
}

#[cfg(test)]
mod tests {
    use rapier3d::prelude::IntegrationParameters;

    use super::*;
    use crate::systems::{DEFAULT_SOLVER_ITERATIONS, DEFAULT_STABILIZATION_ITERATIONS};

    fn tagged_blob() -> Vec<u8> {
        PhysicsWorld::new(TickRate::DEFAULT).to_bytes().unwrap()
    }

    fn pin_offset() -> usize {
        10
    }

    #[test]
    fn a_blob_is_tagged_and_round_trips() {
        let bytes = tagged_blob();
        assert_eq!(&bytes[..4], b"LPB1");
        let world = PhysicsWorld::from_bytes(&bytes, TickRate::DEFAULT).unwrap();
        assert_eq!(world.to_bytes().unwrap(), bytes);
    }

    #[test]
    fn the_solver_pool_follows_the_thread_count_in_effect() {
        use ludion_core::par::with_threads;

        let built = with_threads(3, || PhysicsWorld::new(TickRate::DEFAULT));
        assert_eq!(built.rapier.physics_pipeline.num_threads(), Some(3));

        let bytes = built.to_bytes().unwrap();
        let decoded = with_threads(1, || {
            PhysicsWorld::from_bytes(&bytes, TickRate::DEFAULT).unwrap()
        });
        assert_eq!(decoded.rapier.physics_pipeline.num_threads(), Some(1));

        let mut moved = with_threads(2, || PhysicsWorld::new(TickRate::DEFAULT));
        assert_eq!(moved.rapier.physics_pipeline.num_threads(), Some(2));
        with_threads(5, || moved.step());
        assert_eq!(moved.rapier.physics_pipeline.num_threads(), Some(5));
    }

    #[test]
    fn a_blob_of_another_version_is_refused_naming_both() {
        let mut bytes = tagged_blob();
        bytes[4..8].copy_from_slice(&(PHYSICS_BLOB_VERSION + 1).to_le_bytes());
        let Err(PhysicsError::Decode(why)) = PhysicsWorld::from_bytes(&bytes, TickRate::DEFAULT)
        else {
            panic!("a wrong blob version must be refused");
        };
        assert!(why.contains(&format!("version {}", PHYSICS_BLOB_VERSION + 1)));
        assert!(why.contains(&format!("version {PHYSICS_BLOB_VERSION}")));
    }

    #[test]
    fn a_blob_of_another_pin_is_refused_naming_both() {
        let mut bytes = tagged_blob();
        let at = pin_offset();
        bytes[at] = b'9';
        let Err(PhysicsError::Decode(why)) = PhysicsWorld::from_bytes(&bytes, TickRate::DEFAULT)
        else {
            panic!("a wrong solver pin must be refused");
        };
        assert!(why.contains(rapier3d::VERSION));
        assert!(why.contains(&String::from_utf8(bytes[at..at + 6].to_vec()).unwrap()));
    }

    #[test]
    fn a_tagged_blob_is_refused_when_truncated_inside_the_tag() {
        let bytes = tagged_blob();
        assert!(matches!(
            PhysicsWorld::from_bytes(&bytes[..6], TickRate::DEFAULT),
            Err(PhysicsError::Decode(_))
        ));
    }

    #[test]
    fn an_untagged_blob_is_refused() {
        let bytes = tagged_blob();
        let untagged = &bytes[pin_offset() + rapier3d::VERSION.len()..];
        let Err(PhysicsError::Decode(why)) = PhysicsWorld::from_bytes(untagged, TickRate::DEFAULT)
        else {
            panic!("an untagged blob must be refused");
        };
        assert!(why.contains("predates the version tag"), "{why}");
    }

    #[test]
    fn a_blob_with_trailing_bytes_is_refused() {
        let mut bytes = PhysicsWorld::new(TickRate::DEFAULT).to_bytes().unwrap();
        assert!(PhysicsWorld::from_bytes(&bytes, TickRate::DEFAULT).is_ok());
        bytes.push(0);
        assert!(matches!(
            PhysicsWorld::from_bytes(&bytes, TickRate::DEFAULT),
            Err(PhysicsError::Decode(_))
        ));
    }

    #[test]
    fn the_iteration_defaults_equal_the_pinned_rapier_defaults() {
        let defaults = IntegrationParameters::default();
        assert_eq!(
            DEFAULT_SOLVER_ITERATIONS as usize,
            defaults.num_solver_iterations
        );
        assert_eq!(
            DEFAULT_STABILIZATION_ITERATIONS as usize,
            defaults.num_internal_stabilization_iterations
        );
    }

    fn layers(names: &[&str], masks: &[u32]) -> Option<CollisionLayers> {
        let names = names.iter().map(|name| name.to_string()).collect();
        Some(CollisionLayers::new(names, masks.to_vec()))
    }

    fn rigid_ball(layer: Option<&str>) -> Collider {
        Collider {
            shape: Shape::Sphere { radius: 0.5 },
            body: Body::Rigid(Default::default()),
            layer: layer.map(str::to_string),
            sensor: false,
        }
    }

    fn groups_of(world: &PhysicsWorld, entity: EntityId) -> InteractionGroups {
        let row = world.rows.get(&entity).copied().expect("a live body");
        world.rapier.colliders[world.table[row].collider].collision_groups()
    }

    fn settings_with(layers: Option<CollisionLayers>) -> PhysicsSettings {
        PhysicsSettings {
            gravity: [0.0; 3],
            sleep_after_ticks: 1,
            layers,
            ..PhysicsSettings::new(crate::systems::PhysicsSpace::Free)
        }
    }

    #[test]
    fn retuning_the_material_and_damping_keeps_the_handles_and_a_mass_edit_refuses() {
        let mut world = PhysicsWorld::new(TickRate::DEFAULT);
        let entity = EntityId::new(1, 0);
        let pose = ([0.0; 3], [0.0, 0.0, 0.0, 1.0]);
        world.create(entity, &rigid_ball(None), pose.0, pose.1, None, Ok(None));
        let before = world.table[0].clone();

        let mut edited = rigid_ball(None);
        let Body::Rigid(tunables) = &mut edited.body else {
            unreachable!()
        };
        tunables.friction = 0.05;
        tunables.restitution = 0.8;
        tunables.linear_damping = 0.3;
        assert!(world.retune_in_place(entity, &edited, None));
        let after = &world.table[0];
        assert_eq!((after.body, after.collider), (before.body, before.collider));
        assert_eq!(world.friction(entity), Some(0.05));
        assert_eq!(world.restitution(entity), Some(0.8));
        assert_eq!(world.rapier.bodies[after.body].linear_damping(), 0.3);

        let Body::Rigid(tunables) = &mut edited.body else {
            unreachable!()
        };
        tunables.mass = 4.0;
        tunables.friction = 0.5;
        assert!(!world.retune_in_place(entity, &edited, None));
        assert_eq!(world.friction(entity), Some(0.05));
    }

    #[test]
    fn an_unlayered_collider_keeps_the_solvers_default_groups() {
        let mut world = PhysicsWorld::new(TickRate::DEFAULT);
        let entity = EntityId::new(1, 0);
        world.create(
            entity,
            &rigid_ball(None),
            [0.0; 3],
            [0.0, 0.0, 0.0, 1.0],
            None,
            Ok(None),
        );
        assert_eq!(groups_of(&world, entity), InteractionGroups::all());
    }

    #[test]
    fn a_layered_collider_gets_its_bit_as_membership_and_the_unignored_layers_as_filter() {
        let mut world = PhysicsWorld::new(TickRate::DEFAULT);
        world.apply_settings(&settings_with(layers(&["default", "x"], &[0b00, 0b10])));
        let (on_x, on_default) = (EntityId::new(1, 0), EntityId::new(2, 0));
        world.create(
            on_x,
            &rigid_ball(Some("x")),
            [0.0; 3],
            [0.0, 0.0, 0.0, 1.0],
            None,
            Ok(None),
        );
        world.create(
            on_default,
            &rigid_ball(None),
            [0.0; 3],
            [0.0, 0.0, 0.0, 1.0],
            None,
            Ok(None),
        );
        assert_eq!(
            groups_of(&world, on_x),
            InteractionGroups::new(
                Group::from_bits_truncate(0b10),
                Group::from_bits_truncate(!0b10),
                InteractionTestMode::And,
            )
        );
        assert_eq!(
            groups_of(&world, on_default),
            InteractionGroups::new(
                Group::from_bits_truncate(0b01),
                Group::from_bits_truncate(!0),
                InteractionTestMode::And,
            )
        );
    }

    #[test]
    fn a_layered_collider_with_a_bad_shape_stays_rejected_across_a_table_change() {
        let mut world = PhysicsWorld::new(TickRate::DEFAULT);
        world.apply_settings(&settings_with(layers(&["default", "x"], &[0b00, 0b00])));
        let entity = EntityId::new(1, 0);
        let ragged = Collider {
            shape: Shape::Heightfield {
                rows: 2,
                cols: 2,
                heights: vec![0.0; 3],
                scale: [1.0; 3],
            },
            body: Body::new_static(),
            layer: Some("x".to_string()),
            sensor: false,
        };
        world.create(
            entity,
            &ragged,
            [0.0; 3],
            [0.0, 0.0, 0.0, 1.0],
            None,
            Ok(None),
        );
        let first = world.take_rejections();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].code, code::COLLIDER_HEIGHTFIELD_GRID);

        world.apply_settings(&settings_with(layers(&["default", "x"], &[0b00, 0b10])));
        world.create(
            entity,
            &ragged,
            [0.0; 3],
            [0.0, 0.0, 0.0, 1.0],
            None,
            Ok(None),
        );
        assert!(
            world.take_rejections().is_empty(),
            "the shape rejection is not retried"
        );

        // A layer rejection, by contrast, is retried once the table declares the layer.
        let other = EntityId::new(2, 0);
        world.create(
            other,
            &rigid_ball(Some("y")),
            [0.0; 3],
            [0.0, 0.0, 0.0, 1.0],
            None,
            Ok(None),
        );
        assert_eq!(world.take_rejections().len(), 1);
        world.apply_settings(&settings_with(layers(
            &["default", "x", "y"],
            &[0b000, 0b010, 0b000],
        )));
        world.create(
            other,
            &rigid_ball(Some("y")),
            [0.0; 3],
            [0.0, 0.0, 0.0, 1.0],
            None,
            Ok(None),
        );
        assert!(world.has_body(other));
    }

    #[test]
    #[should_panic(expected = "one ignore mask per layer name")]
    fn a_table_with_mismatched_lengths_panics() {
        CollisionLayers::new(vec!["default".to_string()], vec![0, 0]);
    }

    #[test]
    #[should_panic(expected = "at most 32 layers")]
    fn a_table_over_thirty_two_layers_panics() {
        let names = std::iter::once("default".to_string())
            .chain((1..33).map(|index| format!("l{index}")))
            .collect::<Vec<_>>();
        let masks = vec![0; names.len()];
        CollisionLayers::new(names, masks);
    }

    #[test]
    fn re_applying_an_equal_table_changes_nothing_and_wakes_nothing() {
        let table = layers(&["default", "x"], &[0b00, 0b10]);
        let mut world = PhysicsWorld::new(TickRate::DEFAULT);
        world.apply_settings(&settings_with(table.clone()));
        let entity = EntityId::new(1, 0);
        world.create(
            entity,
            &rigid_ball(Some("x")),
            [0.0; 3],
            [0.0, 0.0, 0.0, 1.0],
            None,
            Ok(None),
        );
        for _ in 0..10 {
            world.step();
        }
        assert_eq!(world.is_sleeping(entity), Some(true));

        world.apply_settings(&settings_with(table));
        assert_eq!(world.is_sleeping(entity), Some(true));

        // A different table moves the groups, which does wake it.
        world.apply_settings(&settings_with(layers(&["default", "x"], &[0b00, 0b00])));
        assert_eq!(world.is_sleeping(entity), Some(false));
        assert!(world.take_rejections().is_empty());
    }

    #[test]
    fn apply_settings_reaches_the_solver_and_the_sleep_default() {
        let mut world = PhysicsWorld::new(TickRate::DEFAULT);
        world.apply_settings(&PhysicsSettings {
            space: crate::systems::PhysicsSpace::Free,
            gravity: [1.0, 2.0, 3.0],
            solver_iterations: 1,
            stabilization_iterations: 2,
            sleep_after_ticks: 7,
            layers: None,
            surfaces: None,
        });
        assert_eq!(world.gravity(), [1.0, 2.0, 3.0]);
        assert_eq!(world.solver_iterations(), 1);
        assert_eq!(world.stabilization_iterations(), 2);
        assert_eq!(world.sleep_after_ticks(), 7);
    }

    #[test]
    fn a_plane_locks_the_off_plane_translation_and_the_in_plane_rotations() {
        let free = AxisLock::default();
        let xz = locked_axes(free, Some(Plane::Xz));
        assert!(xz.contains(LockedAxes::TRANSLATION_LOCKED_Y));
        assert!(!xz.contains(LockedAxes::TRANSLATION_LOCKED_X));
        assert!(xz.contains(LockedAxes::ROTATION_LOCKED_X | LockedAxes::ROTATION_LOCKED_Z));
        assert!(!xz.contains(LockedAxes::ROTATION_LOCKED_Y));

        let xy = locked_axes(free, Some(Plane::Xy));
        assert!(xy.contains(LockedAxes::TRANSLATION_LOCKED_Z));
        assert!(xy.contains(LockedAxes::ROTATION_LOCKED_X | LockedAxes::ROTATION_LOCKED_Y));
        assert!(!xy.contains(LockedAxes::ROTATION_LOCKED_Z));
    }

    #[test]
    fn the_bodys_own_locks_are_ored_with_the_planes() {
        let lock = AxisLock {
            translation: [true, false, false],
            rotation: [false, true, false],
        };
        let axes = locked_axes(lock, Some(Plane::Xz));
        assert!(axes.contains(LockedAxes::TRANSLATION_LOCKED_X));
        assert!(axes.contains(LockedAxes::TRANSLATION_LOCKED_Y));
        assert!(axes.contains(LockedAxes::ROTATION_LOCKED));
        assert!(locked_axes(lock, None).contains(LockedAxes::ROTATION_LOCKED_Y));
        assert!(!locked_axes(lock, None).contains(LockedAxes::TRANSLATION_LOCKED_Y));
    }

    #[test]
    fn sleep_is_authored_in_ticks_and_crosses_half_a_tick_early() {
        let dt = seconds_per_tick(TickRate::DEFAULT);
        assert_eq!(time_until_sleep(30, TickRate::DEFAULT), 29.5 * dt);
        assert!(time_until_sleep(0, TickRate::DEFAULT) < 0.0);
    }

    #[test]
    fn a_ragged_or_tiny_height_grid_builds_nothing() {
        assert!(heightfield(2, 2, &[0.0, 0.0, 0.0], [1.0; 3]).is_err());
        assert!(heightfield(1, 4, &[0.0; 4], [1.0; 3]).is_err());
        assert!(heightfield(2, 2, &[0.0; 4], [1.0; 3]).is_ok());
    }

    #[test]
    fn an_in_plane_box_repeats_its_first_half_extent_off_plane() {
        let shape = Shape::Aabb {
            half_extents: [2.0, 3.0],
        };
        for (plane, expected) in [(Plane::Xz, [2.0, 2.0, 3.0]), (Plane::Xy, [2.0, 3.0, 2.0])] {
            let built = shape_builder(&shape, Some(plane), None).expect("an aabb always builds");
            let cuboid = built
                .build()
                .shape()
                .as_cuboid()
                .expect("an aabb builds a cuboid")
                .half_extents;
            assert_eq!(from_vector(cuboid), expected, "{plane:?}");
        }
    }

    #[test]
    fn a_degenerate_hull_builds_nothing() {
        let flat = Shape::Convex {
            points: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]],
        };
        assert!(shape_builder(&flat, None, None).is_err());
    }
}

#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! `ludion-physics` — deterministic rigid-body dynamics, wrapping `rapier3d` behind the
//! `Collider`/`Velocity` author surface (`Plan/physics.md`).
//!
//! # What a game writes
//!
//! Three things, and nothing else: a [`Collider`](ludion_spatial::Collider) component carrying a
//! shape and `body: static | kinematic | rigid`, an optional
//! [`Velocity`](ludion_spatial::Velocity) component that is both how a game sets a velocity and where
//! the resolved velocity lands ([`PhysicsWorld::apply_impulse`], [`PhysicsWorld::apply_impulse_at_point`]
//! and [`PhysicsWorld::radial_impulse`] apply impulses, with the inertia staying inside this wrapper),
//! and one call to [`install`] naming the game's position component and the [`PhysicsSettings`] —
//! at minimum a [`PhysicsSpace`], with gravity and the solver tunables at their pinned defaults — it
//! simulates under.
//!
//! ```no_run
//! # use ludion_ecs::Schedule;
//! # use ludion_physics::{NoParent, install, PhysicsPose, PhysicsSpace};
//! # use schemars::JsonSchema;
//! # use serde::{Deserialize, Serialize};
//! # #[derive(Serialize, Deserialize, JsonSchema)]
//! # struct Transform { translation: [f32; 3], rotation: [f32; 4] }
//! # impl PhysicsPose for Transform {
//! #     fn translation(&self) -> [f32; 3] { self.translation }
//! #     fn set_translation(&mut self, t: [f32; 3]) { self.translation = t; }
//! #     fn rotation_quat(&self) -> [f32; 4] { self.rotation }
//! #     fn set_rotation_quat(&mut self, r: [f32; 4]) { self.rotation = r; }
//! # }
//! # let mut schedule = Schedule::new();
//! // Game systems that write velocities run first; physics then reads, steps and mirrors.
//! install::<Transform, NoParent>(&mut schedule, PhysicsSpace::Free);
//! ```
//!
//! [`install`] appends three systems in a fixed order — write, step, mirror — because that order
//! is load-bearing and getting it wrong produces no error, only a wrong simulation.
//!
//! # Space
//!
//! [`PhysicsSpace`] is the second argument because it cannot be inferred. No host needs to install
//! a [`Plane`](ludion_spatial::Plane) resource: [`PhysicsSpace::Planar`] reads
//! [`Plane::of`](ludion_spatial::Plane::of), and an absent resource reads as the default
//! `Plane::Xz`, which locks the world `y` axis — the axis gravity points down. Reading "a plane is
//! present" as "this is a 2D game" would therefore leave a 3D pile hanging in the air with nothing
//! to read that says why. [`PhysicsSpace::Free`] ignores the plane entirely and is how a 3D game
//! opts out of one it did not ask for; [`PhysicsSpace::Planar`] applies the plane's locks — the
//! off-plane translation axis and the two in-plane rotation axes — on every rigid body, which is
//! what makes a 2D game the 3D solver with axis locks rather than a second engine. A body's own
//! [`AxisLock`](ludion_spatial::AxisLock) applies in both. A bare host (`ludion render`, `ludion
//! sweep`) has no game config to build a `Plane` from, so it locks the same default axis a game
//! with no `Plane` resource would — that is expected, not a bug to work around.
//!
//! # Which colliders this crate owns
//!
//! [`claims`] is the rule, and it is the exact complement of what
//! [`spatial::rebuild`](ludion_spatial::spatial::rebuild) skips: **a 3D shape, or a `rigid` body of
//! any shape.** So an `aabb` wall that is `static`, or a `circle` that is `kinematic`, belongs to
//! `ludion-spatial`'s 2D index and never becomes a solver body — which is what keeps a twin-stick
//! game's thousands of in-plane colliders out of a solver they have no use for, even if it
//! registers this verb. A `circle` or `aabb` reaches the solver only as `rigid`, the plane-locked
//! 2D-physics case, and a physics scene's level geometry is therefore authored with 3D shapes
//! (`box`, `heightfield`, `convex`, `mesh`) rather than `aabb`. Nothing is allocated until the first tick
//! claims something: the [`PhysicsWorld`] resource does not exist before then, and the step and
//! mirror are no-ops.
//!
//! # One owner per phase
//!
//! The ECS owns authored state and the solver owns derived state, and the direction flips exactly
//! once per tick. Game code writes [`Velocity`](ludion_spatial::Velocity), and the pose of
//! kinematic bodies; the write system pushes that in; the step runs; the mirror pulls poses and
//! velocities back into components. A game edit is detected by comparing a component against the
//! exact bits the mirror last wrote, never against a predicted value — which is also what lets a
//! resting body sleep, since an untouched component is bit-equal and pushes nothing.
//!
//! # Boundary
//!
//! **This is the only crate in the workspace that names a rapier or parry type.** Every other
//! crate — `ludion-spatial` (where `Collider` and `Velocity` live as pure data), `ludion-ecs`,
//! `ludion-engine`, every game — reaches physics only through this crate's public surface, which is
//! meant to be **the whole physics API a game needs**, expressible without opening rapier's own
//! documentation. `scripts/physics-features.mjs` is the mechanical half of the rule: clippy's
//! `disallowed-types` bans a type path in *our* code but has never seen inside a dependency, and
//! cargo unifies features workspace-wide, so that script asserts over `cargo metadata` that
//! `rapier3d` resolves to exactly the pinned feature set and that `rayon` is unreachable from this
//! crate's dependency tree.
//!
//! This crate does not depend on `ludion-engine`, and so cannot name the engine's `Transform`:
//! [`PhysicsPose`] is the seam instead, with rotation as a quaternion so that the Euler convention
//! lives in one place. `ludion_core::math`'s `euler_xyz_degrees_to_quat` and
//! `quat_to_euler_xyz_degrees` are that convention — intrinsic XYZ, degrees, the same pair the
//! renderer builds its model matrices from.
//!
//! # Determinism
//!
//! `rapier3d` is pinned (`=0.35.3` today) with `default-features = false` and features `dim3 f32
//! std enhanced-determinism serde-serialize`: `enhanced-determinism` selects `IndexMap`/`IndexSet`
//! with a fixed hasher inside the solver and excludes `parallel`, so the solver never reaches for a
//! second thread. What stays ours is the fixed timestep ([`PhysicsWorld::new`] takes a
//! [`TickRate`](ludion_core::time::TickRate) and sets `dt` once), sleeping authored in ticks, and
//! above all the iteration order: the handle table is a `Vec` in creation order, it is the only
//! thing this crate iterates, and it rides in the snapshot rather than being rebuilt by walking a
//! solver set. See `ludion devdocs ludion-physics` for the full argument and the pin-upgrade
//! ritual.
//!
//! # Contacts
//!
//! [`PhysicsWorld::contact_events`] reports which entity pairs started or stopped touching on the
//! last step, canonical and sorted, with the step's summed normal-impulse magnitude, a world-space
//! point and an `a`→`b` normal on a start. See [`ContactEvent`] for the full contract.
//!
//! # Impulses
//!
//! [`PhysicsWorld::apply_impulse`] (at the centre of mass), [`PhysicsWorld::apply_impulse_at_point`]
//! (spins the body) and [`PhysicsWorld::radial_impulse`] (a [`Blast`] with a [`Falloff`]) queue a call
//! on the world resource; the write system applies the queue in call order just before the step, on
//! `rigid` bodies only, waking sleepers, with no `Velocity` component needed. Queue from a system
//! that runs before [`install`]'s — a snapshot with calls still queued is refused. Each tick's
//! blast hits are on [`PhysicsWorld::blast_hits`]; a call naming a body that cannot take an
//! impulse lands on [`PhysicsWorld::take_impulse_misses`].
//!
//! # Time scale
//!
//! [`PhysicsWorld::scale_time`] runs the frame's step at a fraction `[MIN_TIME_SCALE, 1]` of the fixed timestep
//! and [`PhysicsWorld::freeze`] skips it, each per frame and reset by the step (the
//! `GameClock::hold` idiom); scales compose by minimum, a freeze wins, and nothing is snapshotted.
//! [`PhysicsWorld::last_step_dt`] reports what the last step ran with.
//! An impact's impulse stays on its `Started` event at any scale (summed over one base tick of
//! simulated time).
//!
//! # Queries
//!
//! [`PhysicsWorld::ray_cast`], [`PhysicsWorld::shape_cast`] and [`PhysicsWorld::overlap`] read the
//! state as of the last completed step — a body created, moved, teleported or despawned this tick
//! becomes visible only after the step that follows. The two casts return every hit as a
//! [`QueryHit`], sorted `(distance, entity)` ascending; `overlap` returns every hit `EntityId`,
//! sorted ascending. Rapier's own BVH traversal order is never exposed. Each takes a [`QueryFilter`]:
//! one excluded entity plus ignored layer names, which only drops hits.
//!
//! # Joints
//!
//! A [`Joint`](ludion_spatial::Joint) component names its two bodies by `EntityId` and is mapped
//! onto a rapier impulse joint the tick both bodies exist: `anchor` and the kind's `axis` are read
//! in the first body's local frame, and the joint's zero is the bodies' relative pose at the moment
//! it is built — a mid-flight grab forms where the bodies already are. A joint whose `a` or `b` has
//! no body is left inert rather than deleted, and [`PhysicsWorld::take_missing_joints`] reports it
//! once; a `break_impulse` past its linear constraint impulse removes the rapier joint and reports a
//! [`JointBroken`] on [`PhysicsWorld::joint_events`] — the component itself is removed by the
//! mirror system in the same tick, so a joint exists iff its component does. A carrier's
//! [`JointMotor`](ludion_spatial::JointMotor) drives the joint's free axes without rebuilding it; see
//! [`PhysicsWorld::take_motor_mismatches`]. See `Plan/physics.md` for the full design and
//! `ludion devdocs ludion-physics` for the kind → locked-axes mapping.
//!
//! # Character motor
//!
//! A [`CharacterMotor`] owns one kinematic capsule body, built by the write pass from the motor's
//! `radius` and `height` (or [`CharacterMotorState::height`]): the entity's translation is the
//! character's **feet**, the body centre sits half the height above it, and the capsule never
//! rotates. The capsule's dimensions live in the motor alone; an entity with a `Collider` as well
//! keeps only the motor's (`L1295`), and an invalid motor gets no body (`L1294`).
//!
//! # What is not here
//!
//! `ludion-spatial`'s 2D index and its four queries are untouched by this crate and answer a
//! different question — what is near this point on the plane this frame — with no bodies and no
//! solver; an entity belongs to one tier or the other, never both, and [`claims`] is the one
//! predicate that decides which. Springs and multibody joints are not offered either
//! (`Plan/physics.md`).

mod contacts;
mod impulse;
mod joints;
mod meshes;
mod motor;
mod pose;
mod queries;
mod sensors;
mod systems;
mod time_scale;
mod wireframe;
mod world;

pub use contacts::{ContactEvent, ContactPhase};
pub use impulse::{Blast, BlastHit, Falloff, ImpulseMiss};
pub use joints::JointBroken;
pub use ludion_spatial::{NoParent, ParentPose};
pub use meshes::{ColliderMeshes, MeshLoader, TriangleMesh};
pub use motor::{
    CharacterMotor, CharacterMotorState, GroundHit, GroundProbe, LaneEnd, LaneFrame, LaneReport,
    MotorError, MoveConstraint, MoveReport, PlatformCarry, PlatformPose, ResizeReport, SurfaceHit,
    SurfaceVelocity, fits_character, ground_probe_reach, height_in_force, move_character,
    move_character_with, probe_character_ground, resize_character, teleport_character,
};
pub use pose::PhysicsPose;
pub use queries::{QueryFilter, QueryHit};
pub use sensors::{ExitCause, SensorEvent, SensorPhase};
pub use systems::{
    CollisionLayers, DEFAULT_SLEEP_AFTER_TICKS, DEFAULT_SOLVER_ITERATIONS,
    DEFAULT_STABILIZATION_ITERATIONS, KinematicTeleports, MAX_COLLISION_LAYERS, MIRROR,
    PhysicsSettings, PhysicsSpace, STEP, WRITE, claims, install, teleport_kinematic,
};
pub use time_scale::MIN_TIME_SCALE;
pub use wireframe::{WireBody, feature_edges};
pub use world::{
    DEFAULT_GRAVITY, MotorMismatch, PhysicsError, PhysicsWorld, ShapeKind, ShapeRejection,
    StepBreakdown,
};

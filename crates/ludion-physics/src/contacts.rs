//! [`ContactEvent`]: which collider pairs started or stopped touching on the last step.

use ludion_core::ids::EntityId;
use serde::{Deserialize, Serialize};

/// Whether a [`ContactEvent`] is the pair's first touch or its last.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContactPhase {
    /// The pair first touched: penetrating, or exchanging impulse; a pair inside the prediction
    /// margin with a gap still open has not started. The event arrives once the pair's impulse
    /// window has closed (see [`ContactEvent`]).
    Started,
    /// The pair stopped touching this step.
    Stopped,
}

/// One collider pair's contact transition on the last step `PhysicsWorld::step` ran.
///
/// Read through [`PhysicsWorld::contact_events`](crate::PhysicsWorld::contact_events). The pair is
/// canonical (`a < b`, by [`EntityId`]'s own ordering), the list is sorted ascending by `(a, b)`,
/// and each pair appears at most once per step. The list holds exactly one step's events: it is
/// cleared when the next step starts, and since the physics triple runs after every game system
/// under the scene block, a system reading it this tick sees what tick `N - 1`'s step produced.
///
/// `point` and `normal` are named like [`QueryHit`](crate::QueryHit)'s. On a start they come
/// from the pair's solver contacts, picked by the highest normal impulse, then the deepest
/// penetration, then the smallest point and the smallest normal, so the pick never depends on the
/// solver's own contact order and a start with no impulse yet (penetrating within the allowed
/// error) resolves by depth. `point` is midway between the two surfaces at the picked contact and `normal`
/// points from `a` towards `b`, unit length, both at the poses the step's collision detection saw
/// (before the step moved the bodies). Both are `[0.0; 3]` on a stop.
///
/// The pick is deterministic for a given `solver_iterations`. It ranks per-point impulses, which the
/// count redistributes, so a single-contact start (a sphere) picks the same contact at any count to
/// tolerance while a multi-point start may pick a different corner at a different count.
///
/// `impulse` is the pair's summed normal-impulse magnitude over its first base tick of simulated
/// time on a [`Started`](ContactPhase::Started) event and `0.0` on a [`Stopped`](ContactPhase::Stopped)
/// one. At full speed that is the step the pair first touched; under
/// [`scale_time`](crate::PhysicsWorld::scale_time) it is the `ceil(1 / scale)` steps from the touch,
/// and the `Started` arrives when they have run, so an impact's impulse is on its `Started` at any
/// scale. `point` and `normal` are the touch's. A pair that separates inside the window reports its
/// `Started` with what it gathered, then its `Stopped` on the next step. A pair only ever touches when at
/// least one side is `rigid` — a `static`-vs-`static` or `static`-vs-`kinematic` pair never enters
/// the solver's contact graph. A resting or sleeping contact stays touching and emits no further
/// event; it only stops when the pair actually separates, when one side despawns, or when one
/// side's `Collider` is removed while the entity stays alive — both removal paths report exactly
/// one `Stopped` for the pair on the next step, still naming the despawned or de-collided entity.
///
/// Not hashed: this is derived solver state, and resources never reach the world hash. It rides in
/// the snapshot anyway, because tick `N`'s events are read on tick `N + 1` and a snapshot can sit
/// between the two ticks.
///
/// # Example
/// A system that spawns a spark at every start; the contact is read on the tick after the step.
/// ```
/// use ludion_core::time::{FrameIndex, TickRate};
/// use ludion_ecs::{Schedule, SystemCtx, World};
/// use ludion_physics::{ContactPhase, PhysicsPose, PhysicsSpace, PhysicsWorld, install};
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
/// #[derive(Serialize, Deserialize, JsonSchema)]
/// struct Spark {
///     facing: [f32; 3],
/// }
///
/// fn sparks(world: &mut World, _ctx: &SystemCtx) {
///     let Some(physics) = world.resource::<PhysicsWorld>() else { return };
///     let starts: Vec<_> = physics
///         .contact_events()
///         .iter()
///         .filter(|event| event.phase == ContactPhase::Started)
///         .map(|event| (event.point, event.normal))
///         .collect();
///     for (point, normal) in starts {
///         let spark = world.spawn();
///         let pose = Transform { translation: point, rotation: [0.0, 0.0, 0.0, 1.0] };
///         world.insert(spark, pose).unwrap();
///         world.insert(spark, Spark { facing: normal }).unwrap();
///     }
/// }
///
/// let mut world = World::new();
/// let pose_at = |y| Transform { translation: [0.0, y, 0.0], rotation: [0.0, 0.0, 0.0, 1.0] };
/// let floor = world.spawn();
/// world.insert(floor, pose_at(-0.5)).unwrap();
/// let slab = Shape::Box { half_extents: [5.0, 0.5, 5.0] };
/// world
///     .insert(floor, Collider { shape: slab, body: Body::new_static(), layer: None })
///     .unwrap();
/// let ball = world.spawn();
/// world.insert(ball, pose_at(1.0)).unwrap();
/// let body = Body::Rigid(RigidBody { mass: 1.0, ..RigidBody::default() });
/// world
///     .insert(ball, Collider { shape: Shape::Sphere { radius: 0.5 }, body, layer: None })
///     .unwrap();
///
/// let mut schedule = Schedule::new();
/// schedule.add_system("sparks", sparks);
/// install::<Transform>(&mut schedule, PhysicsSpace::Free);
/// for frame in 0..90 {
///     schedule.run(&mut world, &SystemCtx::new(FrameIndex::new(frame), TickRate::DEFAULT));
/// }
///
/// let (spark,) = world.query::<(&Spark,)>().next().expect("the ball landed");
/// assert!(spark.facing[1] > 0.99, "`normal` points from `a` (the floor) up to `b` (the ball)");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContactEvent {
    /// The lower-ordered entity of the pair.
    pub a: EntityId,
    /// The higher-ordered entity of the pair.
    pub b: EntityId,
    /// Whether the pair started or stopped touching.
    pub phase: ContactPhase,
    /// The summed normal-impulse magnitude over the first base tick of simulated time on a start,
    /// `0.0` on a stop.
    pub impulse: f32,
    /// World-space contact point on a start, midway between the two surfaces at the picked contact;
    /// `[0.0; 3]` on a stop.
    pub point: [f32; 3],
    /// Unit world-space normal from `a` towards `b` on a start; `[0.0; 3]` on a stop.
    pub normal: [f32; 3],
}

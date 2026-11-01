//! [`SensorEvent`]: which non-sensor entities entered or left each sensor on the last step.

use ludion_core::ids::EntityId;
use serde::{Deserialize, Serialize};

use crate::world::{PhysicsWorld, entity_from_bits};

/// Why an occupant left a sensor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExitCause {
    /// Both entities and their colliders are still there and the volumes no longer overlap: one of
    /// them moved, was teleported, or was reshaped.
    Separated,
    /// The sensor or the occupant was despawned. The event still names the dead id.
    Despawned,
    /// Both entities are alive but the pair no longer qualifies: a `Collider` was removed, the
    /// sensor stopped being one (or the occupant became one), or a `layers:` edit now makes the two
    /// ignore each other.
    Removed,
}

/// Whether a [`SensorEvent`] is an entry or an exit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SensorPhase {
    /// The entity now overlaps the sensor.
    Entered,
    /// The entity no longer overlaps the sensor, for the stated cause.
    Exited {
        /// Why it left.
        cause: ExitCause,
    },
}

/// One sensor/occupant transition on the last step `PhysicsWorld::step` ran.
///
/// Read through [`PhysicsWorld::sensor_events`]. The pair is oriented `(sensor, other)`: `sensor`
/// is the entity whose `Collider` has `sensor: true` and `other` the non-sensor entity that entered
/// or left. The list is sorted ascending by `(sensor, other)`, a pair appears at most once per
/// step, and it holds exactly one step's events: it is cleared when the next step starts, and since
/// the physics triple runs after every game system under the scene block, a system reading it this
/// tick sees what tick `N - 1`'s step produced.
///
/// Overlap is judged at the poses the step produced, so a body written inside a sensor on tick `N`
/// is reported `Entered` on tick `N + 1`. Detection is discrete (no continuous collision detection):
/// keep a body's per-tick displacement under the sensor's thickness or it can tunnel through
/// unseen. Sensors never report against other sensors, and a pair the layer table ignores never
/// reports. A sleeping body stays an occupant.
///
/// An event may name a despawned id (`Exited { cause: Despawned }`): check the cause before reading
/// that entity's components.
///
/// Not hashed, but it rides the snapshot: tick `N`'s events are read on tick `N + 1` and a snapshot
/// can sit between the two ticks.
///
/// # Example
/// A kinematic body written inside a static sensor is reported on the following tick.
/// ```
/// use ludion_core::time::{FrameIndex, TickRate};
/// use ludion_ecs::{Schedule, SystemCtx, World};
/// use ludion_physics::{NoParent, PhysicsPose, PhysicsSpace, PhysicsWorld, SensorPhase, install};
/// use ludion_spatial::{Body, Collider, Shape};
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
/// let pose = Transform { translation: [0.0; 3], rotation: [0.0, 0.0, 0.0, 1.0] };
/// let zone = world.spawn();
/// world.insert(zone, pose).unwrap();
/// let volume = Shape::Box { half_extents: [1.0; 3] };
/// world
///     .insert(zone, Collider { shape: volume, body: Body::new_static(), layer: None, sensor: true })
///     .unwrap();
/// let hero = world.spawn();
/// world.insert(hero, pose).unwrap();
/// let capsule = Shape::Sphere { radius: 0.25 };
/// world
///     .insert(hero, Collider { shape: capsule, body: Body::new_kinematic(), layer: None, sensor: false })
///     .unwrap();
///
/// let mut schedule = Schedule::new();
/// install::<Transform, NoParent>(&mut schedule, PhysicsSpace::Free);
/// let ctx = |frame| SystemCtx::new(FrameIndex::new(frame), TickRate::DEFAULT);
/// schedule.run(&mut world, &ctx(0));
/// let events = world.resource::<PhysicsWorld>().unwrap().sensor_events();
/// assert_eq!((events[0].sensor, events[0].other, events[0].phase), (zone, hero, SensorPhase::Entered));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SensorEvent {
    /// The sensor entity.
    pub sensor: EntityId,
    /// The non-sensor entity that entered or left.
    pub other: EntityId,
    /// Entry or exit.
    pub phase: SensorPhase,
}

impl PhysicsWorld {
    /// The sensor transitions the last step produced, sorted ascending by `(sensor, other)`. See
    /// [`SensorEvent`]. Empty after a frozen step.
    ///
    /// # Example
    /// ```
    /// use ludion_core::time::TickRate;
    /// use ludion_physics::PhysicsWorld;
    ///
    /// let world = PhysicsWorld::new(TickRate::DEFAULT);
    /// assert!(world.sensor_events().is_empty());
    /// ```
    pub fn sensor_events(&self) -> &[SensorEvent] {
        &self.sensor_events
    }

    /// The non-sensor entities overlapping `sensor` as of the last step, ascending. Empty for an
    /// entity that is not a sensor, or unknown.
    ///
    /// # Example
    /// ```
    /// use ludion_core::ids::EntityId;
    /// use ludion_core::time::TickRate;
    /// use ludion_physics::PhysicsWorld;
    ///
    /// let world = PhysicsWorld::new(TickRate::DEFAULT);
    /// assert!(world.sensor_occupants(EntityId::new(0, 0)).is_empty());
    /// ```
    pub fn sensor_occupants(&self, sensor: EntityId) -> &[EntityId] {
        let lo = self.sensing.partition_point(|pair| pair.0 < sensor);
        let hi = self.sensing.partition_point(|pair| pair.0 <= sensor);
        &self.occupants[lo..hi]
    }

    /// Records entities the write system found gone from the ECS, so an exit for a pair that
    /// included one reports [`ExitCause::Despawned`]. Kept until a non-frozen step has judged it.
    pub(crate) fn note_despawned(&mut self, ids: &[EntityId]) {
        if ids.is_empty() {
            return;
        }
        self.despawned.extend_from_slice(ids);
        self.despawned.sort_unstable();
        self.despawned.dedup();
    }

    /// Tests every sensor against the post-step poses, then merge-diffs the sorted result against
    /// the previous baseline into `sensor_events`.
    ///
    /// Deliberately not the solver's intersection graph: its narrow phase runs before kinematic
    /// bodies move, so it would judge them at last tick's pose. Rapier's closing broad-phase update
    /// keeps the query pipeline valid at the post-step poses. The walk over the table is creation
    /// order, and the pairs are sorted before use, so rapier's own order never reaches the output.
    pub(crate) fn collect_sensors(&mut self) {
        if self.sensing.is_empty() && !self.table.iter().any(|entry| entry.source.sensor) {
            return;
        }
        let mut current: Vec<(EntityId, EntityId)> = Vec::new();
        let pipeline = self.rapier.query_pipeline();
        for entry in self.table.iter().filter(|entry| entry.source.sensor) {
            let Some(sensor) = self.rapier.colliders.get(entry.collider) else {
                continue;
            };
            for (_, candidate) in pipeline.intersect_shape(*sensor.position(), sensor.shape()) {
                let other = entity_from_bits(candidate.user_data);
                if other == entry.entity
                    || candidate.is_sensor()
                    || !sensor.collision_groups().test(candidate.collision_groups())
                {
                    continue;
                }
                current.push((entry.entity, other));
            }
        }
        current.sort_unstable();
        current.dedup();

        let mut events = Vec::new();
        let mut old = self.sensing.iter().copied().peekable();
        let mut new = current.iter().copied().peekable();
        loop {
            let take_old = match (old.peek(), new.peek()) {
                (None, None) => break,
                (Some(_), None) => true,
                (None, Some(_)) => false,
                (Some(o), Some(n)) if o == n => {
                    old.next();
                    new.next();
                    continue;
                }
                (Some(o), Some(n)) => o < n,
            };
            if take_old {
                let (sensor, other) = old.next().expect("peeked");
                let cause = self.exit_cause(sensor, other);
                events.push(SensorEvent {
                    sensor,
                    other,
                    phase: SensorPhase::Exited { cause },
                });
            } else {
                let (sensor, other) = new.next().expect("peeked");
                events.push(SensorEvent {
                    sensor,
                    other,
                    phase: SensorPhase::Entered,
                });
            }
        }
        self.sensor_events = events;
        self.sensing = current;
        self.occupants = self.sensing.iter().map(|pair| pair.1).collect();
    }

    /// Why a baseline pair is no longer overlapping, judged against the world as it is now.
    fn exit_cause(&self, sensor: EntityId, other: EntityId) -> ExitCause {
        if self.despawned.binary_search(&sensor).is_ok()
            || self.despawned.binary_search(&other).is_ok()
        {
            return ExitCause::Despawned;
        }
        let (Some(&s_row), Some(&o_row)) = (self.rows.get(&sensor), self.rows.get(&other)) else {
            return ExitCause::Removed;
        };
        let (s, o) = (&self.table[s_row], &self.table[o_row]);
        if !s.source.sensor || o.source.sensor {
            return ExitCause::Removed;
        }
        let groups = |entry: &crate::world::BodyEntry| {
            self.rapier
                .colliders
                .get(entry.collider)
                .map(|collider| collider.collision_groups())
        };
        match (groups(s), groups(o)) {
            (Some(a), Some(b)) if a.test(b) => ExitCause::Separated,
            _ => ExitCause::Removed,
        }
    }
}

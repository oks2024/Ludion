//! Impulses on a [`PhysicsWorld`]: [`apply_impulse`](PhysicsWorld::apply_impulse),
//! [`apply_impulse_at_point`](PhysicsWorld::apply_impulse_at_point) and
//! [`radial_impulse`](PhysicsWorld::radial_impulse) queue a call, and the physics write system
//! applies the queue just before the step. See the crate's `# Impulses` docs for the contract.

use ludion_core::ids::EntityId;
use ludion_core::math::sqrt;
use rapier3d::prelude::{Ball, Pose, RigidBody, RigidBodyHandle, Vector};
use serde::{Deserialize, Serialize};

use crate::queries::{QueryFilter, check_finite, unit};
use crate::world::{PhysicsError, PhysicsWorld, entity_from_bits, from_vector, to_vector};

/// How a [`Blast`]'s impulse fades with the distance `d` from its centre to a body's collider
/// surface (`0` when the centre is inside the collider).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Falloff {
    /// Every hit body gets the full impulse.
    Constant,
    /// `1 - d / radius`: full at the surface nearest the centre, nothing at the radius.
    Linear,
    /// `(min_distance / max(d, min_distance))²`: full inside `min_distance`, then an inverse-square
    /// fade. `min_distance` must be finite and greater than `0`.
    InverseSquare {
        /// The distance inside which the impulse is not amplified further.
        min_distance: f32,
    },
}

/// One radial impulse: every rigid body whose collider touches the ball `radius` around `center`
/// is pushed away from `center`, by `impulse` scaled by `falloff`.
///
/// # Example
/// ```
/// use ludion_core::time::TickRate;
/// use ludion_physics::{Blast, Falloff, PhysicsWorld, QueryFilter};
///
/// let mut world = PhysicsWorld::new(TickRate::DEFAULT);
/// let blast = Blast {
///     center: [0.0, 0.0, 0.0],
///     radius: 4.0,
///     impulse: 20.0,
///     falloff: Falloff::Linear,
///     filter: QueryFilter::new(),
/// };
/// // The first blast queued this tick is blast 0; it hits nothing in an empty world.
/// assert_eq!(world.radial_impulse(blast).unwrap(), 0);
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct Blast {
    /// World-space centre of the blast.
    pub center: [f32; 3],
    /// Radius of the ball tested against every collider, in world units. Must be finite and `> 0`.
    pub radius: f32,
    /// Impulse magnitude (`N·s`) at falloff `1.0`. Negative pulls bodies toward `center`.
    pub impulse: f32,
    /// How the impulse fades with distance.
    pub falloff: Falloff,
    /// One excluded entity and ignored layers; `static` and `kinematic` bodies are always skipped.
    pub filter: QueryFilter,
}

/// One body a blast pushed, on [`PhysicsWorld::blast_hits`].
///
/// Rides the snapshot beside the contact events; never hashed.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BlastHit {
    /// The blast's index among that tick's blasts, in call order — what
    /// [`PhysicsWorld::radial_impulse`] returned.
    pub blast: u32,
    /// The body that was pushed.
    pub entity: EntityId,
    /// The impulse vector applied (`N·s`) — zero when the falloff reached zero.
    pub impulse: [f32; 3],
    /// World-space point the impulse was applied at: the collider's closest point to the blast
    /// centre, or the centre itself when it is inside the collider.
    pub point: [f32; 3],
}

/// One at-centre or at-point impulse that named an entity with no rigid body when it was applied.
///
/// Queued here rather than reported for the `ShapeRejection` reason;
/// [`PhysicsWorld::take_impulse_misses`] hands the queue to a caller that can report it as `L3038`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImpulseMiss {
    /// The entity the impulse named.
    pub entity: EntityId,
    /// Why nothing was applied: the entity has no body, or its body is `static` or `kinematic`.
    pub reason: String,
}

/// A call waiting for the physics write system.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum QueuedImpulse {
    AtCom {
        entity: EntityId,
        impulse: [f32; 3],
    },
    AtPoint {
        entity: EntityId,
        impulse: [f32; 3],
        point: [f32; 3],
    },
    Radial(Blast),
}

/// The share of a blast's impulse a body at surface distance `d` receives.
fn falloff(falloff: Falloff, d: f32, radius: f32) -> f32 {
    match falloff {
        Falloff::Constant => 1.0,
        Falloff::Linear => (1.0 - d / radius).clamp(0.0, 1.0),
        Falloff::InverseSquare { min_distance } => {
            let ratio = min_distance / d.max(min_distance);
            ratio * ratio
        }
    }
}

/// One body a blast will push, resolved against the last step before any impulse is applied.
struct BlastTarget {
    entity: EntityId,
    body: RigidBodyHandle,
    point: Vector,
    impulse: Vector,
}

impl PhysicsWorld {
    /// Queues an impulse (`N·s`, `mass * Δvelocity`) at `entity`'s centre of mass: no spin, and the
    /// entity needs no `Velocity` component.
    ///
    /// Nothing moves until the physics write system runs, just before the step, so it takes effect
    /// on the tick it was queued in; queue it from a system that runs before the physics write
    /// system (under a scene `physics:` block every game system does). Calls apply in call order,
    /// wake a sleeping body, and move `rigid` bodies only — an entity with no body or a
    /// `static`/`kinematic` one is a dropped call reported as `L3038` by the engine, see
    /// [`take_impulse_misses`](PhysicsWorld::take_impulse_misses). `impulse` must be finite,
    /// otherwise [`PhysicsError::QueryInput`] and nothing is queued.
    ///
    /// # Example
    /// ```
    /// use ludion_core::ids::EntityId;
    /// use ludion_core::time::TickRate;
    /// use ludion_physics::PhysicsWorld;
    ///
    /// let mut world = PhysicsWorld::new(TickRate::DEFAULT);
    /// assert!(world.apply_impulse(EntityId::new(0, 0), [4.0, 0.0, 0.0]).is_ok());
    /// assert!(world.apply_impulse(EntityId::new(0, 0), [f32::NAN, 0.0, 0.0]).is_err());
    /// ```
    pub fn apply_impulse(
        &mut self,
        entity: EntityId,
        impulse: [f32; 3],
    ) -> Result<(), PhysicsError> {
        check_finite("impulse", &impulse)?;
        self.pending_impulses
            .push(QueuedImpulse::AtCom { entity, impulse });
        Ok(())
    }

    /// [`apply_impulse`](PhysicsWorld::apply_impulse) at a world-space `point`: the offset from the
    /// centre of mass also spins the body, through the solver's own inertia. `point` must be finite.
    pub fn apply_impulse_at_point(
        &mut self,
        entity: EntityId,
        impulse: [f32; 3],
        point: [f32; 3],
    ) -> Result<(), PhysicsError> {
        check_finite("impulse", &impulse)?;
        check_finite("point", &point)?;
        self.pending_impulses.push(QueuedImpulse::AtPoint {
            entity,
            impulse,
            point,
        });
        Ok(())
    }

    /// Queues a radial impulse and returns its index among this tick's blasts (`0` for the first),
    /// the [`BlastHit::blast`] value its hits carry.
    ///
    /// When the write system applies it, every `rigid` body whose collider intersects the ball
    /// `blast.radius` around `blast.center` — read from the last completed step, as the queries
    /// are — gets an impulse along the line from the centre to its collider's closest point (to its
    /// centre of mass when the centre is inside the collider), applied at that point, so an
    /// off-centre hit spins the body. A body created or rebuilt this tick was not in the last step,
    /// so it is not hit until the next tick. Bodies are visited in `EntityId` order; `static` and
    /// `kinematic` bodies are skipped silently. A body whose direction is undefined (the centre is
    /// exactly at its centre of mass) is not pushed and not reported. The hits are readable through
    /// [`blast_hits`](PhysicsWorld::blast_hits) after the step.
    ///
    /// `center` and `impulse` must be finite, `radius` finite and `> 0`, an
    /// [`InverseSquare`](Falloff::InverseSquare) `min_distance` finite and `> 0`, and every ignored
    /// layer one the scene declares — otherwise [`PhysicsError::QueryInput`] and nothing is queued.
    ///
    /// # Example
    /// ```
    /// use ludion_core::time::TickRate;
    /// use ludion_physics::{Blast, Falloff, PhysicsError, PhysicsWorld, QueryFilter};
    ///
    /// let mut world = PhysicsWorld::new(TickRate::DEFAULT);
    /// let blast = |radius| Blast {
    ///     center: [0.0; 3],
    ///     radius,
    ///     impulse: 10.0,
    ///     falloff: Falloff::Constant,
    ///     filter: QueryFilter::new(),
    /// };
    /// assert_eq!(world.radial_impulse(blast(3.0)).unwrap(), 0);
    /// assert_eq!(world.radial_impulse(blast(5.0)).unwrap(), 1);
    /// assert!(matches!(
    ///     world.radial_impulse(blast(0.0)),
    ///     Err(PhysicsError::QueryInput(_))
    /// ));
    /// ```
    pub fn radial_impulse(&mut self, blast: Blast) -> Result<u32, PhysicsError> {
        check_finite("center", &blast.center)?;
        check_finite("radius", &[blast.radius])?;
        check_finite("impulse", &[blast.impulse])?;
        if blast.radius <= 0.0 {
            return Err(PhysicsError::QueryInput(format!(
                "`radius` must be greater than zero, found {}",
                blast.radius
            )));
        }
        if let Falloff::InverseSquare { min_distance } = blast.falloff {
            check_finite("min_distance", &[min_distance])?;
            if min_distance <= 0.0 {
                return Err(PhysicsError::QueryInput(format!(
                    "`min_distance` must be greater than zero, found {min_distance}"
                )));
            }
        }
        self.query_drops(&blast.filter).map(|_| ())?;
        let index = self.blasts_queued;
        self.blasts_queued += 1;
        self.pending_impulses.push(QueuedImpulse::Radial(blast));
        Ok(index)
    }

    /// Every body the last tick's blasts pushed: grouped by blast in call order, `EntityId`
    /// ascending inside a blast. Holds exactly one tick's hits — cleared when the next write
    /// system runs — so a system reading it sees what the previous tick's blasts did. Not hashed;
    /// it rides the snapshot beside [`contact_events`](PhysicsWorld::contact_events).
    pub fn blast_hits(&self) -> &[BlastHit] {
        &self.blast_hits
    }

    /// Takes every [`ImpulseMiss`] queued since the last call — the
    /// [`take_motor_mismatches`](PhysicsWorld::take_motor_mismatches) contract, for `L3038`.
    pub fn take_impulse_misses(&mut self) -> Vec<ImpulseMiss> {
        std::mem::take(&mut self.pending_impulse_misses)
    }

    /// Applies every queued call in call order and fills [`blast_hits`](PhysicsWorld::blast_hits).
    ///
    /// Runs at the end of the physics write system, after every body is created, rebuilt and
    /// pushed, so a `Velocity` the game pushed this tick never overwrites an impulse. The hits and
    /// the blast counter reset every tick, whether or not anything was queued.
    pub(crate) fn apply_queued_impulses(&mut self) {
        self.blast_hits.clear();
        self.blasts_queued = 0;
        let queue = std::mem::take(&mut self.pending_impulses);
        let mut blast_index = 0;
        for queued in queue {
            match queued {
                QueuedImpulse::AtCom { entity, impulse } => {
                    self.apply_to_entity(entity, |body| {
                        body.apply_impulse(to_vector(impulse), true);
                    });
                }
                QueuedImpulse::AtPoint {
                    entity,
                    impulse,
                    point,
                } => {
                    self.apply_to_entity(entity, |body| {
                        body.apply_impulse_at_point(to_vector(impulse), to_vector(point), true);
                    });
                }
                QueuedImpulse::Radial(blast) => {
                    self.apply_blast(blast_index, &blast);
                    blast_index += 1;
                }
            }
        }
    }

    /// Runs `apply` on `entity`'s rigid body, or queues an [`ImpulseMiss`] saying why it cannot.
    fn apply_to_entity(&mut self, entity: EntityId, apply: impl FnOnce(&mut RigidBody)) {
        match self.rigid_body_of(entity) {
            Ok(handle) => {
                if let Some(body) = self.rapier.bodies.get_mut(handle) {
                    apply(body);
                }
            }
            Err(reason) => self.pending_impulse_misses.push(ImpulseMiss {
                entity,
                reason: reason.to_string(),
            }),
        }
    }

    /// Resolves every body `blast` reaches against the last step, then pushes them all.
    fn apply_blast(&mut self, index: u32, blast: &Blast) {
        let targets = self.blast_targets(blast);
        for target in &targets {
            self.blast_hits.push(BlastHit {
                blast: index,
                entity: target.entity,
                impulse: from_vector(target.impulse),
                point: from_vector(target.point),
            });
        }
        // Pushed in spawn-rank order, so the wake order is churn-invariant and survives a rebuild;
        // `blast_hits` above keeps its documented `EntityId` order.
        let mut order: Vec<usize> = (0..targets.len()).collect();
        order.sort_by_key(|&i| self.spawn_rank(targets[i].entity));
        for i in order {
            let target = &targets[i];
            if let Some(body) = self.rapier.bodies.get_mut(target.body) {
                body.apply_impulse_at_point(target.impulse, target.point, true);
            }
        }
    }

    /// The rigid bodies `blast` pushes (`EntityId` ascending; application order is separate), each with its closest point and impulse
    /// vector.
    fn blast_targets(&self, blast: &Blast) -> Vec<BlastTarget> {
        // A layer table that no longer declares an ignored name reaches here only if it changed
        // after the call; the blast then reaches nothing rather than ignoring the filter.
        let Ok(drops) = self.query_drops(&blast.filter) else {
            return Vec::new();
        };
        let center = to_vector(blast.center);
        let ball = Ball::new(blast.radius);
        let mut candidates: Vec<(EntityId, RigidBodyHandle, Vector, Vector)> = Vec::new();
        for (handle, collider) in self
            .rapier
            .query_pipeline()
            .intersect_shape(Pose::from_translation(center), &ball)
        {
            // Rapier maps a BVH leaf to a collider by slot alone, so a leaf of a collider retired
            // this tick can resolve to a new one that reused the slot.
            if self.fresh_colliders.contains(&handle) {
                continue;
            }
            let entity = entity_from_bits(collider.user_data);
            if drops(entity) {
                continue;
            }
            let Ok(body_handle) = self.rigid_body_of(entity) else {
                continue;
            };
            let Some(body) = self.rapier.bodies.get(body_handle) else {
                continue;
            };
            let closest = collider
                .shape()
                .project_point(collider.position(), center, true)
                .point;
            candidates.push((entity, body_handle, closest, body.center_of_mass()));
        }
        candidates.sort_unstable_by_key(|(entity, ..)| *entity);
        candidates
            .into_iter()
            .filter_map(|(entity, body, point, mass_center)| {
                let offset = point - center;
                let d = sqrt(offset.x * offset.x + offset.y * offset.y + offset.z * offset.z);
                let (direction, distance) = if d == 0.0 {
                    (unit(from_vector(mass_center - center))?, 0.0)
                } else {
                    (unit(from_vector(offset))?, d)
                };
                let magnitude = blast.impulse * falloff(blast.falloff, distance, blast.radius);
                Some(BlastTarget {
                    entity,
                    body,
                    point,
                    impulse: to_vector(direction) * magnitude,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_ignores_distance() {
        assert_eq!(falloff(Falloff::Constant, 0.0, 4.0), 1.0);
        assert_eq!(falloff(Falloff::Constant, 4.0, 4.0), 1.0);
    }

    #[test]
    fn linear_fades_to_zero_at_the_radius() {
        assert_eq!(falloff(Falloff::Linear, 0.0, 4.0), 1.0);
        assert_eq!(falloff(Falloff::Linear, 1.0, 4.0), 0.75);
        assert_eq!(falloff(Falloff::Linear, 4.0, 4.0), 0.0);
    }

    #[test]
    fn linear_never_goes_negative_past_the_radius() {
        assert_eq!(falloff(Falloff::Linear, 4.5, 4.0), 0.0);
    }

    #[test]
    fn inverse_square_clamps_inside_min_distance() {
        let fade = Falloff::InverseSquare { min_distance: 2.0 };
        assert_eq!(falloff(fade, 0.0, 10.0), 1.0);
        assert_eq!(falloff(fade, 2.0, 10.0), 1.0);
        assert_eq!(falloff(fade, 4.0, 10.0), 0.25);
        assert_eq!(falloff(fade, 8.0, 10.0), 0.0625);
    }
}

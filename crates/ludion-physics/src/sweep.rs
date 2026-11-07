//! [`PhysicsWorld::first_hit`] and [`PhysicsWorld::first_hits`]: the nearest hit of a sphere (or a
//! ray) swept along a segment, with no allocation and a filter resolved once.

use std::cmp::Ordering;

use ludion_core::ids::EntityId;
use ludion_core::math::sqrt;
use ludion_core::par;
use rapier3d::parry::query::ShapeCastOptions;
use rapier3d::parry::shape::{Ball, Shape as _};
use rapier3d::prelude::{Aabb, BoundingVolume, Ray};

use crate::queries::{QueryFilter, advance, unit};
use crate::world::{
    DEFAULT_LAYER, PhysicsError, PhysicsWorld, entity_from_bits, from_vector, to_pose, to_vector,
};

/// Probes per parallel chunk of [`PhysicsWorld::first_hits`]; fixed so the split never depends on
/// the thread count.
const SWEEP_CHUNK: usize = 256;

/// One probe for [`PhysicsWorld::first_hit`]: a sphere of `radius` (`0.0` is a ray) swept from
/// `origin` to `origin + delta`.
///
/// # Example
/// ```
/// use ludion_core::ids::EntityId;
/// use ludion_physics::Sweep;
///
/// let probe = Sweep::new([0.0; 3], [10.0, 0.0, 0.0], 0.25).excluding(EntityId::new(3, 0));
/// assert_eq!(probe.exclude, Some(EntityId::new(3, 0)));
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sweep {
    /// Where the sweep starts.
    pub origin: [f32; 3],
    /// The travel: the sweep ends at `origin + delta`.
    pub delta: [f32; 3],
    /// Sphere radius; `0.0` sweeps a ray.
    pub radius: f32,
    /// A collider this probe skips, such as the shooter's own; see
    /// [`PhysicsWorld::first_hit_skipping`] for more.
    pub exclude: Option<EntityId>,
}

impl Sweep {
    /// A probe that skips nothing.
    pub fn new(origin: [f32; 3], delta: [f32; 3], radius: f32) -> Self {
        Self {
            origin,
            delta,
            radius,
            exclude: None,
        }
    }

    /// Skips `entity`'s collider, replacing any earlier one.
    pub fn excluding(mut self, entity: EntityId) -> Self {
        self.exclude = Some(entity);
        self
    }
}

/// A [`QueryFilter`] resolved once against the world's layer table: a bitmask of ignored layers,
/// copyable and cheap to reuse for every probe.
///
/// Only valid for the layer table it was built against: rebuild it each tick (it costs nothing), or
/// after `apply_settings` changes the table.
///
/// # Example
/// ```
/// use ludion_core::time::TickRate;
/// use ludion_physics::{PhysicsWorld, QueryFilter};
///
/// let world = PhysicsWorld::new(TickRate::DEFAULT);
/// let filter = world.sweep_filter(&QueryFilter::new().ignore_layer("default")).unwrap();
/// let copy = filter;
/// assert_eq!(filter, copy);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SweepFilter {
    ignored: u32,
    include_sensors: bool,
    exclude: Option<EntityId>,
    exclude_also: Option<EntityId>,
}

/// The nearest hit of a [`Sweep`].
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct SweepHit {
    /// The entity whose collider was hit.
    pub entity: EntityId,
    /// Fraction of `delta` travelled before the hit, in `[0, 1]`; `0.0` when the probe starts
    /// touching or inside the collider.
    pub t: f32,
    /// World units travelled before the hit (`t` times the length of `delta`).
    pub distance: f32,
    /// World-space point on the hit collider's surface; at `t == 0` the nearest surface point, or
    /// `origin` when the probe's centre starts inside.
    pub point: [f32; 3],
    /// World-space unit normal at `point`, pointing out of the hit collider toward the probe. A probe
    /// that starts touching or inside reports the reversed travel direction instead. A probe that
    /// starts touching and is moving away from, or along, the surface has no hit at all.
    pub normal: [f32; 3],
}

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The best hit so far, keyed `(toi, entity)`.
struct Best {
    toi: f32,
    entity: EntityId,
    point: [f32; 3],
    normal: [f32; 3],
}

impl Best {
    fn beaten_by(&self, toi: f32, entity: EntityId) -> bool {
        toi.total_cmp(&self.toi).then(entity.cmp(&self.entity)) == Ordering::Less
    }
}

impl PhysicsWorld {
    /// Resolves `filter` against the layer table into a [`SweepFilter`]. An ignored layer name must
    /// be `default` or declared by the scene, otherwise [`PhysicsError::QueryInput`].
    ///
    /// # Example
    /// ```
    /// use ludion_core::time::TickRate;
    /// use ludion_physics::{PhysicsError, PhysicsWorld, QueryFilter};
    ///
    /// let world = PhysicsWorld::new(TickRate::DEFAULT);
    /// assert!(world.sweep_filter(&QueryFilter::new()).is_ok());
    /// let typo = QueryFilter::new().ignore_layer("dumy");
    /// assert!(matches!(world.sweep_filter(&typo), Err(PhysicsError::QueryInput(_))));
    /// ```
    pub fn sweep_filter(&self, filter: &QueryFilter) -> Result<SweepFilter, PhysicsError> {
        self.check_layer_names(&filter.ignored_layers)?;
        let mut ignored = 0u32;
        for name in &filter.ignored_layers {
            let index = match self.layer_table() {
                Some(table) => table.index_of(name),
                None => (name == DEFAULT_LAYER).then_some(0),
            };
            if let Some(index) = index {
                ignored |= 1 << index;
            }
        }
        Ok(SweepFilter {
            ignored,
            include_sensors: filter.include_sensors,
            exclude: filter.exclude,
            exclude_also: filter.exclude_also,
        })
    }

    /// The nearest hit of `sweep` by `(t, entity)`, or `None` for a miss. Allocates nothing, even
    /// at `t == 0`.
    ///
    /// Reads the state as of the last completed step, like [`ray_cast`](PhysicsWorld::ray_cast).
    /// Every collider gets one exact cast, never rapier's first-hit calls, so equal times of impact go
    /// to the lowest entity whatever the BVH order. A probe that starts touching or inside a collider
    /// hits at `t == 0` with `normal == -direction` and `point` the nearest surface point (`origin`
    /// when the centre is inside). A probe that starts touching a surface but travels away from it or
    /// along it does not hit that collider; a ray starting on a surface and leaving it likewise.
    ///
    /// An invalid probe is a miss, not an error: a non-finite origin, delta or radius, a negative
    /// radius, a zero-length delta, or a delta whose length is not finite.
    ///
    /// # Example
    /// ```
    /// use ludion_core::time::TickRate;
    /// use ludion_physics::{PhysicsWorld, QueryFilter, Sweep};
    ///
    /// let world = PhysicsWorld::new(TickRate::DEFAULT);
    /// let filter = world.sweep_filter(&QueryFilter::new()).unwrap();
    /// let probe = Sweep::new([0.0; 3], [10.0, 0.0, 0.0], 0.0);
    /// assert!(world.first_hit(&probe, &filter).is_none());
    /// ```
    pub fn first_hit(&self, sweep: &Sweep, filter: &SweepFilter) -> Option<SweepHit> {
        self.first_hit_skipping(sweep, filter, &[])
    }

    /// [`first_hit`](PhysicsWorld::first_hit) that also skips every collider whose entity is in
    /// `skip`, such as the entities a piercing projectile already passed through. The result is the
    /// next hit behind them, or `None`.
    ///
    /// # Example
    /// ```
    /// use ludion_core::ids::EntityId;
    /// use ludion_core::time::TickRate;
    /// use ludion_physics::{PhysicsWorld, QueryFilter, Sweep};
    ///
    /// let world = PhysicsWorld::new(TickRate::DEFAULT);
    /// let filter = world.sweep_filter(&QueryFilter::new()).unwrap();
    /// let probe = Sweep::new([0.0; 3], [10.0, 0.0, 0.0], 0.25);
    /// let passed = [EntityId::new(3, 0)];
    /// assert!(world.first_hit_skipping(&probe, &filter, &passed).is_none());
    /// ```
    pub fn first_hit_skipping(
        &self,
        sweep: &Sweep,
        filter: &SweepFilter,
        skip: &[EntityId],
    ) -> Option<SweepHit> {
        let Sweep {
            origin,
            delta,
            radius,
            exclude,
        } = *sweep;
        if !origin.iter().chain(&delta).all(|c| c.is_finite())
            || !radius.is_finite()
            || radius < 0.0
        {
            return None;
        }
        let length = sqrt(delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]);
        if !length.is_finite() || length == 0.0 {
            return None;
        }
        let unit_dir = unit(delta)?;
        let end = advance(origin, unit_dir, length);
        let ball = Ball::new(radius);
        let start = to_pose(origin, [0.0, 0.0, 0.0, 1.0]);
        let swept = if radius == 0.0 {
            let (a, b) = (to_vector(origin), to_vector(end));
            Aabb::new(a.min(b), a.max(b))
        } else {
            ball.compute_aabb(&start)
                .merged(&ball.compute_aabb(&to_pose(end, [0.0, 0.0, 0.0, 1.0])))
        };
        let ray = Ray::new(to_vector(origin), to_vector(unit_dir));
        let options = ShapeCastOptions::with_max_time_of_impact(length);
        let pipeline = self.rapier.query_pipeline();
        let mut best: Option<Best> = None;
        for (_, collider) in pipeline.intersect_aabb_conservative(swept) {
            let entity = entity_from_bits(collider.user_data);
            if Some(entity) == exclude
                || skip.contains(&entity)
                || Some(entity) == filter.exclude
                || Some(entity) == filter.exclude_also
                || (!filter.include_sensors && collider.is_sensor())
                || collider.collision_groups().memberships.bits() & filter.ignored != 0
            {
                continue;
            }
            let (toi, point, normal) = if radius == 0.0 {
                let Some(hit) = collider.shape().cast_ray_and_get_normal(
                    collider.position(),
                    &ray,
                    length,
                    true,
                ) else {
                    continue;
                };
                // A ray that starts on the surface and leaves it is not a hit. A surface shape reports
                // the face normal; a solid reports a zero one, so test whether a step along the ray
                // is still in the shape.
                if hit.time_of_impact == 0.0 {
                    let normal = from_vector(hit.normal);
                    let leaving = if normal == [0.0; 3] {
                        let scale = origin.iter().fold(1.0_f32, |m, c| m.max(c.abs()));
                        let step = advance(origin, unit_dir, 1e-4 * scale);
                        !collider
                            .shape()
                            .contains_point(collider.position(), to_vector(step))
                    } else {
                        dot3(unit_dir, normal) > 0.0
                    };
                    if leaving {
                        continue;
                    }
                }
                (
                    hit.time_of_impact,
                    advance(origin, unit_dir, hit.time_of_impact),
                    from_vector(hit.normal),
                )
            } else {
                let pose = collider.position();
                // parry's cast runs a heap-allocating EPA when the ball starts in contact, so
                // detect t = 0 with a closed-form projection and cast only a clear start.
                let proj = collider
                    .shape()
                    .project_point(pose, to_vector(origin), true);
                let gap = proj.point - to_vector(origin);
                let gap_len = sqrt(gap.x * gap.x + gap.y * gap.y + gap.z * gap.z);
                if proj.is_inside || gap_len <= radius {
                    // Touching but moving away from, or along, the nearest surface point is no hit.
                    if !proj.is_inside
                        && gap_len > 0.0
                        && dot3(delta, [-gap.x, -gap.y, -gap.z]) >= 0.0
                    {
                        continue;
                    }
                    let point = if proj.is_inside {
                        origin
                    } else {
                        from_vector(proj.point)
                    };
                    (0.0, point, unit_dir.map(|c| -c))
                } else {
                    let Ok(Some(hit)) = pipeline.dispatcher.cast_shapes(
                        &pose.inv_mul(&start),
                        pose.rotation.inverse() * to_vector(unit_dir),
                        collider.shape(),
                        &ball,
                        options,
                    ) else {
                        continue;
                    };
                    let hit = hit.transform1_by(pose);
                    (
                        hit.time_of_impact,
                        from_vector(hit.witness1),
                        from_vector(hit.normal1),
                    )
                }
            };
            if best.as_ref().is_none_or(|b| b.beaten_by(toi, entity)) {
                best = Some(Best {
                    toi,
                    entity,
                    point,
                    normal,
                });
            }
        }
        best.map(|b| SweepHit {
            entity: b.entity,
            t: (b.toi / length).clamp(0.0, 1.0),
            distance: b.toi,
            point: b.point,
            normal: if b.toi == 0.0 {
                unit_dir.map(|c| -c)
            } else {
                b.normal
            },
        })
    }

    /// [`first_hit`](PhysicsWorld::first_hit) for every probe in `sweeps`, written to the same index
    /// of `out`. Fans out over `ludion_core::par` in fixed chunks, so the result is the same at every
    /// thread count.
    ///
    /// # Panics
    /// Panics if `sweeps` and `out` differ in length.
    ///
    /// # Example
    /// ```
    /// use ludion_core::time::TickRate;
    /// use ludion_physics::{PhysicsWorld, QueryFilter, Sweep};
    ///
    /// let world = PhysicsWorld::new(TickRate::DEFAULT);
    /// let filter = world.sweep_filter(&QueryFilter::new()).unwrap();
    /// let probes = [
    ///     Sweep::new([0.0; 3], [10.0, 0.0, 0.0], 0.0),
    ///     Sweep::new([0.0; 3], [0.0, 5.0, 0.0], 0.5),
    /// ];
    /// let mut out = [None; 2];
    /// world.first_hits(&probes, &filter, &mut out);
    /// assert_eq!(out, [None, None]);
    /// ```
    pub fn first_hits(&self, sweeps: &[Sweep], filter: &SweepFilter, out: &mut [Option<SweepHit>]) {
        assert_eq!(
            sweeps.len(),
            out.len(),
            "first_hits: {} sweeps but {} output slots",
            sweeps.len(),
            out.len()
        );
        par::par_chunks(out, SWEEP_CHUNK, |chunk, slots| {
            for (offset, slot) in slots.iter_mut().enumerate() {
                *slot = self.first_hit(&sweeps[chunk * SWEEP_CHUNK + offset], filter);
            }
        });
    }
}

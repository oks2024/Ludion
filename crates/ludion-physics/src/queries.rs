//! [`QueryHit`] and the three read-only queries against a [`PhysicsWorld`]: ray cast, shape cast
//! and overlap. See the crate's `# Queries` docs for the freshness and ordering contract shared by
//! all three.

use ludion_core::ids::EntityId;
use ludion_core::math::sqrt;
use ludion_spatial::Shape;
use rapier3d::parry::query::ShapeCastOptions;
use rapier3d::parry::shape::Shape as ParryShape;
use rapier3d::prelude::{BoundingVolume, Ray};

use crate::sweep::SweepFilter;
use crate::world::{
    PhysicsError, PhysicsWorld, entity_from_bits, from_vector, shape_builder, to_pose, to_vector,
};

/// One hit from [`PhysicsWorld::ray_cast`] or [`PhysicsWorld::shape_cast`].
///
/// Never snapshotted: a query is a pure read over a stepped world, so a hit carries no state a
/// restore would need to reproduce.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QueryHit {
    /// The entity whose collider was hit.
    pub entity: EntityId,
    /// World units travelled along the query's (normalized) direction before the hit — `0.0` when
    /// a shape cast starts already overlapping.
    pub distance: f32,
    /// World-space point on the hit collider's surface.
    pub point: [f32; 3],
    /// World-space normal at `point`, pointing out of the hit collider. Parry reports a zero
    /// vector for a ray whose origin starts inside the shape.
    pub normal: [f32; 3],
}

/// Sorts hits `(distance, entity)` ascending — the wrapper's own tie-break, never rapier's BVH
/// traversal order. Each entity has at most one collider, so the two keys together are unique.
fn sort_hits(hits: &mut [QueryHit]) {
    hits.sort_unstable_by(|a, b| {
        a.distance
            .total_cmp(&b.distance)
            .then(a.entity.cmp(&b.entity))
    });
}

/// Normalizes `direction`, or `None` if it is zero or a component is non-finite; a direction whose
/// squared length leaves `f32`'s range is scaled by its largest component first. Inputs the plain
/// formula handles are normalized by it unchanged, so no existing result moves.
pub(crate) fn unit(direction: [f32; 3]) -> Option<[f32; 3]> {
    let length = sqrt(
        direction[0] * direction[0] + direction[1] * direction[1] + direction[2] * direction[2],
    );
    if !length.is_finite() || length == 0.0 {
        let max = direction.iter().fold(0.0f32, |m, c| m.max(c.abs()));
        if !direction.iter().all(|c| c.is_finite()) || max == 0.0 {
            return None;
        }
        let scaled = direction.map(|c| c / max);
        let length = sqrt(scaled[0] * scaled[0] + scaled[1] * scaled[1] + scaled[2] * scaled[2]);
        return Some(scaled.map(|c| c / length));
    }
    Some([
        direction[0] / length,
        direction[1] / length,
        direction[2] / length,
    ])
}

/// `point` advanced by `distance` world units along `direction` (assumed unit length).
pub(crate) fn advance(point: [f32; 3], direction: [f32; 3], distance: f32) -> [f32; 3] {
    from_vector(to_vector(point) + to_vector(direction) * distance)
}

/// `Err(QueryInput)` naming `name` if any component of `value` is non-finite.
pub(crate) fn check_finite(name: &str, value: &[f32]) -> Result<(), PhysicsError> {
    if value.iter().all(|component| component.is_finite()) {
        Ok(())
    } else {
        Err(PhysicsError::QueryInput(format!(
            "`{name}` must be finite, found {value:?}"
        )))
    }
}

/// How far `x² + y² + z² + w²` may drift from `1.0` and still count as a unit quaternion — loose
/// enough to pass an `f32` Euler→quat conversion's own drift (around `1e-7`), tight enough to catch
/// a zero, an identity-typo `[0, 0, 0, 0]`, or an otherwise unnormalized quaternion.
const UNIT_ROTATION_TOLERANCE: f32 = 1e-4;

/// `Err(QueryInput)` unless `rotation` is finite and a unit quaternion (within
/// [`UNIT_ROTATION_TOLERANCE`]). The value is never renormalized; it passes through as given.
fn check_rotation(rotation: [f32; 4]) -> Result<(), PhysicsError> {
    check_finite("rotation", &rotation)?;
    let norm_sq = rotation[0] * rotation[0]
        + rotation[1] * rotation[1]
        + rotation[2] * rotation[2]
        + rotation[3] * rotation[3];
    if (norm_sq - 1.0).abs() <= UNIT_ROTATION_TOLERANCE {
        Ok(())
    } else {
        Err(PhysicsError::QueryInput(format!(
            "`rotation` must be a unit quaternion (x² + y² + z² + w² == 1), found {rotation:?}"
        )))
    }
}

/// `Err(QueryInput)` unless `max_distance` is finite and non-negative (`0.0` is valid).
fn check_max_distance(max_distance: f32) -> Result<(), PhysicsError> {
    if max_distance.is_finite() && max_distance >= 0.0 {
        Ok(())
    } else {
        Err(PhysicsError::QueryInput(format!(
            "`max_distance` must be finite and non-negative, found {max_distance}"
        )))
    }
}

/// `Err(QueryInput)` for a `compound` or a `mesh`: a query shape is one convex-or-simple shape, and
/// a composite is not accepted (cast or overlap each part's shape instead).
fn reject_composite(shape: &Shape) -> Result<(), PhysicsError> {
    match shape {
        Shape::Compound { .. } => Err(PhysicsError::QueryInput(
            "a compound is not a query shape; cast or overlap each part's shape instead".into(),
        )),
        Shape::Mesh { .. } => Err(PhysicsError::QueryInput(
            "a mesh is not a query shape; cast or overlap a convex shape against it instead".into(),
        )),
        _ => Ok(()),
    }
}

/// The input checks of [`PhysicsWorld::ray_cast`] in their documented order, then the normalized
/// direction.
pub(crate) fn check_ray(
    origin: [f32; 3],
    direction: [f32; 3],
    max_distance: f32,
) -> Result<[f32; 3], PhysicsError> {
    check_finite("origin", &origin)?;
    check_max_distance(max_distance)?;
    check_direction(direction)
}

/// The input checks of [`PhysicsWorld::shape_cast`] in their documented order, then the normalized
/// direction.
pub(crate) fn check_cast(
    translation: [f32; 3],
    rotation: [f32; 4],
    direction: [f32; 3],
    max_distance: f32,
) -> Result<[f32; 3], PhysicsError> {
    check_finite("translation", &translation)?;
    check_rotation(rotation)?;
    check_max_distance(max_distance)?;
    check_direction(direction)
}

/// `Err(QueryInput)` naming `direction`, or the normalized direction.
fn check_direction(direction: [f32; 3]) -> Result<[f32; 3], PhysicsError> {
    unit(direction).ok_or_else(|| {
        PhysicsError::QueryInput(format!(
            "`direction` must be finite and non-zero, found {direction:?}"
        ))
    })
}

/// What a query leaves out: one entity (typically the caller's own body), any number of
/// collision layers by name, sensor colliders unless [`include_sensors`](QueryFilter::include_sensors)
/// is called, and every non-static body if [`only_fixed`](QueryFilter::only_fixed) is called.
///
/// The filter only drops hits after they are collected, so result order is unchanged. An ignored
/// name must be `default` (always valid) or a layer the scene's `physics.layers` declares;
/// otherwise the query is [`PhysicsError::QueryInput`], checked before any candidate is gathered.
/// Never snapshotted or hashed.
///
/// # Example
/// ```
/// use ludion_core::time::TickRate;
/// use ludion_physics::{PhysicsError, PhysicsWorld, QueryFilter};
/// use ludion_spatial::Shape;
///
/// let world = PhysicsWorld::new(TickRate::DEFAULT);
/// let sphere = Shape::Sphere { radius: 0.5 };
/// let at = [0.0, 0.0, 0.0];
/// let identity = [0.0, 0.0, 0.0, 1.0];
///
/// let filter = QueryFilter::new().ignore_layer("default");
/// assert!(world.overlap(&sphere, at, identity, &filter).unwrap().is_empty());
///
/// // A world with no layer table declares nothing but `default`.
/// let typo = QueryFilter::new().ignore_layer("dumy");
/// let err = world.overlap(&sphere, at, identity, &typo).unwrap_err();
/// assert!(matches!(err, PhysicsError::QueryInput(_)));
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QueryFilter {
    pub(crate) exclude: Option<EntityId>,
    pub(crate) ignored_layers: Vec<String>,
    pub(crate) include_sensors: bool,
    pub(crate) only_fixed: bool,
}

impl QueryFilter {
    /// A filter that drops only sensor colliders (see [`include_sensors`](QueryFilter::include_sensors)).
    pub fn new() -> Self {
        Self::default()
    }

    /// Drops `entity`'s hit, replacing any earlier excluded entity.
    pub fn exclude(mut self, entity: EntityId) -> Self {
        self.exclude = Some(entity);
        self
    }

    /// Drops every hit on the layer `name`. Repeatable; a repeat is harmless.
    pub fn ignore_layer(mut self, name: impl Into<String>) -> Self {
        self.ignored_layers.push(name.into());
        self
    }

    /// Keeps hits on sensor colliders, which every query drops by default.
    pub fn include_sensors(mut self) -> Self {
        self.include_sensors = true;
        self
    }

    /// Keeps hits on static (`body: static`) colliders only and drops kinematic bodies, rigid
    /// bodies and character motors.
    ///
    /// # Example
    /// ```
    /// use ludion_core::time::TickRate;
    /// use ludion_physics::{PhysicsWorld, QueryFilter};
    ///
    /// let world = PhysicsWorld::new(TickRate::DEFAULT);
    /// // `tests/queries.rs` covers a static, a kinematic and a rigid box on one ray.
    /// let filter = QueryFilter::new().only_fixed();
    /// let hits = world.ray_cast([0.0; 3], [1.0, 0.0, 0.0], 10.0, &filter).unwrap();
    /// assert!(hits.is_empty());
    /// ```
    pub fn only_fixed(mut self) -> Self {
        self.only_fixed = true;
        self
    }
}

impl PhysicsWorld {
    /// Casts a ray from `origin` along `direction` (need not be unit length; normalized here) up
    /// to `max_distance` world units, returning every hit sorted `(distance, entity)` ascending —
    /// never rapier's BVH traversal order. `filter` drops excluded entities, ignored layers, sensors
    /// and, with `only_fixed`, non-static bodies ([`QueryFilter`]).
    ///
    /// Reads the state as of the last completed step (see the crate's `# Queries` docs): a body
    /// created, moved, teleported or despawned this tick is visible only after the step that
    /// follows. `origin` must be finite, `direction` finite and non-zero, and `max_distance`
    /// finite and non-negative (`0.0` is valid — only a hit exactly at `origin` qualifies);
    /// anything else, or a layer name the scene does not declare, is [`PhysicsError::QueryInput`].
    ///
    /// # Example
    /// ```
    /// use ludion_core::time::TickRate;
    /// use ludion_physics::{PhysicsWorld, QueryFilter};
    ///
    /// let world = PhysicsWorld::new(TickRate::DEFAULT);
    /// let hits = world
    ///     .ray_cast([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], 100.0, &QueryFilter::new())
    ///     .unwrap();
    /// assert!(hits.is_empty());
    /// ```
    pub fn ray_cast(
        &self,
        origin: [f32; 3],
        direction: [f32; 3],
        max_distance: f32,
        filter: &QueryFilter,
    ) -> Result<Vec<QueryHit>, PhysicsError> {
        let unit_dir = check_ray(origin, direction, max_distance)?;
        let filter = self.sweep_filter(filter)?;
        Ok(self.ray_hits(origin, unit_dir, max_distance, &filter))
    }

    /// [`ray_cast`](PhysicsWorld::ray_cast) past the input checks, with the filter already resolved
    /// and `unit_dir` normalized.
    pub(crate) fn ray_hits(
        &self,
        origin: [f32; 3],
        unit_dir: [f32; 3],
        max_distance: f32,
        filter: &SweepFilter,
    ) -> Vec<QueryHit> {
        let ray = Ray::new(to_vector(origin), to_vector(unit_dir));
        let mut hits: Vec<QueryHit> = self
            .rapier
            .query_pipeline()
            .intersect_ray(ray, max_distance, true)
            .filter_map(|(_, collider, hit)| {
                let entity = entity_from_bits(collider.user_data);
                if filter.drops(self, entity, collider) {
                    return None;
                }
                Some(QueryHit {
                    entity,
                    distance: hit.time_of_impact,
                    point: advance(origin, unit_dir, hit.time_of_impact),
                    normal: from_vector(hit.normal),
                })
            })
            .collect();
        sort_hits(&mut hits);
        hits
    }

    /// Sweeps `shape` (authored the same way a `Collider` is, laid out on the world's plane if it
    /// has one) from `translation`/`rotation` along `direction` up to `max_distance` world units,
    /// returning every hit sorted `(distance, entity)` ascending. `filter` drops excluded entities,
    /// ignored layers, sensors and, with `only_fixed`, non-static bodies ([`QueryFilter`]).
    ///
    /// Candidates come from the swept axis-aligned bounding box of the start and end poses, each
    /// then given one exact cast — never rapier's first-hit `cast_shape`, which would pick among
    /// equal times of impact by BVH traversal order rather than ours. Reads the state as of the
    /// last completed step, same as [`ray_cast`](PhysicsWorld::ray_cast).
    ///
    /// `translation` must be finite, `rotation` finite and a unit quaternion, `direction` finite
    /// and non-zero, `max_distance` finite and non-negative, and every ignored layer name one the
    /// scene declares — otherwise [`PhysicsError::QueryInput`], as is a `compound` shape (cast each
    /// part's shape instead). A shape the solver cannot build
    /// (a degenerate `convex` hull, a malformed `heightfield` grid) is [`PhysicsError::QueryShape`],
    /// checked before any candidate is gathered so a bad shape errors even in an empty world.
    ///
    /// # Example
    /// ```
    /// use ludion_core::time::TickRate;
    /// use ludion_physics::{PhysicsWorld, QueryFilter};
    /// use ludion_spatial::Shape;
    ///
    /// let world = PhysicsWorld::new(TickRate::DEFAULT);
    /// let sphere = Shape::Sphere { radius: 0.5 };
    /// let hits = world
    ///     .shape_cast(
    ///         &sphere,
    ///         [0.0, 0.0, 0.0],
    ///         [0.0, 0.0, 0.0, 1.0],
    ///         [1.0, 0.0, 0.0],
    ///         10.0,
    ///         &QueryFilter::new(),
    ///     )
    ///     .unwrap();
    /// assert!(hits.is_empty());
    /// ```
    pub fn shape_cast(
        &self,
        shape: &Shape,
        translation: [f32; 3],
        rotation: [f32; 4],
        direction: [f32; 3],
        max_distance: f32,
        filter: &QueryFilter,
    ) -> Result<Vec<QueryHit>, PhysicsError> {
        let unit_dir = check_cast(translation, rotation, direction, max_distance)?;
        let filter = self.sweep_filter(filter)?;
        self.shape_cast_resolved(
            shape,
            translation,
            rotation,
            unit_dir,
            max_distance,
            &filter,
        )
    }

    /// [`shape_cast`](PhysicsWorld::shape_cast) past the input checks and the layer names: builds
    /// `shape` and casts it with the resolved `filter`.
    pub(crate) fn shape_cast_resolved(
        &self,
        shape: &Shape,
        translation: [f32; 3],
        rotation: [f32; 4],
        unit_dir: [f32; 3],
        max_distance: f32,
        filter: &SweepFilter,
    ) -> Result<Vec<QueryHit>, PhysicsError> {
        reject_composite(shape)?;
        let built = shape_builder(shape, self.plane, None)
            .map_err(|(code, reason)| PhysicsError::QueryShape { code, reason })?;
        Ok(self.shape_hits(
            &*built.shape,
            translation,
            rotation,
            unit_dir,
            max_distance,
            filter,
        ))
    }

    /// [`shape_cast`](PhysicsWorld::shape_cast) past the input checks, with the filter resolved,
    /// `unit_dir` normalized and `query_shape` already built (a motor builds each of its capsules
    /// once per solve).
    pub(crate) fn shape_hits(
        &self,
        query_shape: &dyn ParryShape,
        translation: [f32; 3],
        rotation: [f32; 4],
        unit_dir: [f32; 3],
        max_distance: f32,
        filter: &SweepFilter,
    ) -> Vec<QueryHit> {
        let start = to_pose(translation, rotation);
        let end = to_pose(advance(translation, unit_dir, max_distance), rotation);
        let swept = query_shape
            .compute_aabb(&start)
            .merged(&query_shape.compute_aabb(&end));
        let options = ShapeCastOptions::with_max_time_of_impact(max_distance);
        let pipeline = self.rapier.query_pipeline();
        let mut hits = Vec::new();
        for (_, collider) in pipeline.intersect_aabb_conservative(swept) {
            let entity = entity_from_bits(collider.user_data);
            if filter.drops(self, entity, collider) {
                continue;
            }
            let pose = collider.position();
            let Ok(Some(hit)) = pipeline.dispatcher.cast_shapes(
                &pose.inv_mul(&start),
                pose.rotation.inverse() * to_vector(unit_dir),
                collider.shape(),
                query_shape,
                options,
            ) else {
                continue;
            };
            let hit = hit.transform1_by(pose);
            hits.push(QueryHit {
                entity,
                distance: hit.time_of_impact,
                point: from_vector(hit.witness1),
                normal: from_vector(hit.normal1),
            });
        }
        sort_hits(&mut hits);
        hits
    }

    /// Every entity whose collider overlaps `shape` at `translation`/`rotation`, sorted by entity
    /// ascending. `filter` drops excluded entities, ignored layers, sensors and, with
    /// `only_fixed`, non-static bodies ([`QueryFilter`]).
    ///
    /// Reads the state as of the last completed step, same as
    /// [`ray_cast`](PhysicsWorld::ray_cast). `translation` must be finite and `rotation` finite and
    /// a unit quaternion, and every ignored layer name one the scene declares — otherwise
    /// [`PhysicsError::QueryInput`], as is a `compound` shape (overlap each part's shape instead) —
    /// and a shape the solver cannot build is [`PhysicsError::QueryShape`].
    ///
    /// # Example
    /// ```
    /// use ludion_core::time::TickRate;
    /// use ludion_physics::{PhysicsError, PhysicsWorld, QueryFilter};
    /// use ludion_spatial::Shape;
    ///
    /// let world = PhysicsWorld::new(TickRate::DEFAULT);
    /// let degenerate = Shape::Convex {
    ///     points: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]],
    /// };
    /// let err = world
    ///     .overlap(&degenerate, [0.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0], &QueryFilter::new())
    ///     .unwrap_err();
    /// assert!(matches!(err, PhysicsError::QueryShape { .. }));
    /// ```
    pub fn overlap(
        &self,
        shape: &Shape,
        translation: [f32; 3],
        rotation: [f32; 4],
        filter: &QueryFilter,
    ) -> Result<Vec<EntityId>, PhysicsError> {
        check_finite("translation", &translation)?;
        check_rotation(rotation)?;
        let drops = self.query_drops(filter)?;
        reject_composite(shape)?;
        let built = shape_builder(shape, self.plane, None)
            .map_err(|(code, reason)| PhysicsError::QueryShape { code, reason })?;
        let start = to_pose(translation, rotation);
        let mut entities: Vec<EntityId> = self
            .rapier
            .query_pipeline()
            .intersect_shape(start, &*built.shape)
            .filter_map(|(_, collider)| {
                let entity = entity_from_bits(collider.user_data);
                (!drops(entity)).then_some(entity)
            })
            .collect();
        entities.sort_unstable();
        Ok(entities)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(entity: u32, distance: f32) -> QueryHit {
        QueryHit {
            entity: EntityId::new(entity, 0),
            distance,
            point: [0.0; 3],
            normal: [0.0; 3],
        }
    }

    #[test]
    fn sort_hits_orders_by_distance_then_entity() {
        let mut hits = vec![hit(3, 1.0), hit(1, 1.0), hit(2, 0.5)];
        sort_hits(&mut hits);
        assert_eq!(
            hits.iter().map(|h| h.entity.index()).collect::<Vec<_>>(),
            vec![2, 1, 3]
        );
        assert_eq!(hits[1].distance, 1.0);
        assert_eq!(hits[2].distance, 1.0);
    }

    #[test]
    fn unit_rejects_zero_and_nan() {
        assert_eq!(unit([0.0, 0.0, 0.0]), None);
        assert_eq!(unit([f32::NAN, 0.0, 0.0]), None);
        assert_eq!(unit([f32::INFINITY, 0.0, 0.0]), None);
        assert!(unit([2.0, 0.0, 0.0]).is_some());
    }

    #[test]
    fn unit_normalizes_directions_whose_squared_length_leaves_f32() {
        assert_eq!(unit([1e20, 0.0, 0.0]), Some([1.0, 0.0, 0.0]));
        assert_eq!(unit([0.0, -3e38, 0.0]), Some([0.0, -1.0, 0.0]));
        assert_eq!(unit([1e-30, 0.0, 0.0]), Some([1.0, 0.0, 0.0]));
        let diagonal = unit([3e38, 3e38, 0.0]).expect("finite and non-zero");
        for c in &diagonal[..2] {
            assert!(
                (c - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6,
                "{diagonal:?}"
            );
        }
        assert_eq!(diagonal[2], 0.0);
    }

    #[test]
    fn unit_leaves_an_ordinary_direction_bit_identical() {
        assert_eq!(unit([3.0, 4.0, 0.0]), Some([0.6, 0.8, 0.0]));
    }
}

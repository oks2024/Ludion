//! The motor's capsule queries against a [`PhysicsWorld`]: the first blocking cast hit and the
//! penetration contacts that the public queries do not expose.

use ludion_core::ids::EntityId;
use ludion_spatial::Shape;
use rapier3d::parry::shape::Shape as ParryShape;
use rapier3d::prelude::{BoundingVolume, SharedShape};

use crate::motor::Gather;
use crate::queries::{QueryHit, check_cast, check_ray};
use crate::sweep::SweepFilter;
use crate::world::{PhysicsWorld, entity_from_bits, from_vector, shape_builder, to_pose};

const IDENTITY: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// How far along the cast direction a hit's normal must oppose it to count as blocking: a surface
/// the capsule only touches while moving away reports distance 0 and is skipped.
const BLOCKING: f32 = -1e-4;

/// The same test for a prop resting on the ground: its cast reports the floor it touches with a
/// normal a few thousandths off vertical, which must not read as a wall in its way.
const PROP_BLOCKING: f32 = -0.1;

/// One capsule-versus-collider contact.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Contact {
    pub entity: EntityId,
    /// Signed gap: negative is penetration depth.
    pub dist: f32,
    /// World-space normal pointing out of the obstacle.
    pub normal: [f32; 3],
    /// World-space point on the obstacle.
    pub point: [f32; 3],
}

/// The capsule a motor sweeps: its centre, the cylinder half-length and the radius.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Capsule {
    pub half_height: f32,
    pub radius: f32,
}

impl Capsule {
    /// The solver shape of this capsule, built once for a solve's many queries.
    pub(crate) fn built(self, physics: &PhysicsWorld) -> SharedShape {
        let shape = Shape::Capsule {
            half_height: self.half_height,
            radius: self.radius,
        };
        shape_builder(&shape, physics.plane, None)
            .expect("a capsule always builds")
            .shape
    }
}

impl PhysicsWorld {
    /// The first hit along `direction` (unit) within `length` whose normal opposes the motion and
    /// that `skip` does not drop, or `None`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn cast_capsule(
        &self,
        capsule: &dyn ParryShape,
        center: [f32; 3],
        direction: [f32; 3],
        length: f32,
        filter: &SweepFilter,
        gather: Option<&Gather>,
        skip: impl Fn(&QueryHit) -> bool,
    ) -> Option<QueryHit> {
        let unit_dir = check_cast(center, IDENTITY, direction, length).ok()?;
        let hits = self.shape_hits(capsule, center, IDENTITY, unit_dir, length, filter, gather);
        hits.into_iter().find(|hit| {
            let along = hit.normal[0] * direction[0]
                + hit.normal[1] * direction[1]
                + hit.normal[2] * direction[2];
            along < BLOCKING && !skip(hit)
        })
    }

    /// Every contact of the capsule at `center` within `prediction`, sorted `(dist, entity)`; a
    /// caller that needs a churn-invariant order among equal gaps re-sorts by spawn rank (the
    /// motor's `Solver::contacts` does).
    /// Composite shapes (a level mesh) report their single deepest contact.
    pub(crate) fn capsule_contacts(
        &self,
        shape: &dyn ParryShape,
        center: [f32; 3],
        prediction: f32,
        filter: &SweepFilter,
        gather: Option<&Gather>,
    ) -> Vec<Contact> {
        let at = to_pose(center, IDENTITY);
        let aabb = shape.compute_aabb(&at).loosened(prediction);
        let pipeline = self.rapier.query_pipeline();
        let mut contacts = Vec::new();
        self.visit(gather, aabb, |collider| {
            let entity = entity_from_bits(collider.user_data);
            if filter.drops(self, entity, collider) {
                return;
            }
            let pose = collider.position();
            let Ok(Some(contact)) = pipeline.dispatcher.contact(
                &pose.inv_mul(&at),
                collider.shape(),
                shape,
                prediction,
            ) else {
                return;
            };
            let normal = clean_normal(from_vector(pose.rotation * contact.normal1));
            if !contact.dist.is_finite() || !normal.iter().all(|c| c.is_finite()) {
                return;
            }
            contacts.push(Contact {
                entity,
                dist: contact.dist,
                normal,
                point: from_vector(pose * contact.point1),
            });
        });
        contacts.sort_unstable_by(|a, b| a.dist.total_cmp(&b.dist).then(a.entity.cmp(&b.entity)));
        contacts
    }

    /// How far `prop` can travel along `direction` (unit) before a non-rigid collider stops it,
    /// up to `limit`. A prop whose shape cannot be cast (a compound or a mesh) is taken as free.
    /// `filter` already excludes the motor.
    pub(crate) fn prop_free_distance(
        &self,
        prop: EntityId,
        direction: [f32; 3],
        limit: f32,
        filter: &SweepFilter,
        gather: Option<&Gather>,
    ) -> f32 {
        let Some((shape, translation, rotation)) = self.shape_and_pose(prop) else {
            return limit;
        };
        let Ok(unit_dir) = check_cast(translation, rotation, direction, limit) else {
            return limit;
        };
        let Ok(hits) = self.shape_cast_resolved(
            &shape,
            translation,
            rotation,
            unit_dir,
            limit,
            filter,
            gather,
        ) else {
            return limit;
        };
        hits.into_iter()
            .find(|hit| {
                let along = hit.normal[0] * direction[0]
                    + hit.normal[1] * direction[1]
                    + hit.normal[2] * direction[2];
                hit.entity != prop && !self.is_rigid(hit.entity) && along < PROP_BLOCKING
            })
            .map_or(limit, |hit| hit.distance)
    }

    /// The first ray hit straight down from `origin` within `length`: the walkable face under a
    /// ledge rim. A hit with no usable normal is skipped.
    pub(crate) fn probe_down(
        &self,
        origin: [f32; 3],
        length: f32,
        filter: &SweepFilter,
        gather: Option<&Gather>,
    ) -> Option<QueryHit> {
        let unit_dir = check_ray(origin, [0.0, -1.0, 0.0], length).ok()?;
        let hits = self.ray_hits(origin, unit_dir, length, filter, gather);
        hits.into_iter()
            .find(|hit| hit.normal[1] > 0.0 && hit.normal.iter().all(|c| c.is_finite()))
    }
}

/// `normal` with the solver's rounding noise flushed: a component under `1e-5` is an axis-aligned
/// surface's `1e-7` residue, and keeping it makes a push along it drift off the axis.
pub(crate) fn clean_normal(normal: [f32; 3]) -> [f32; 3] {
    let flushed = normal.map(|c| if c.abs() < 1e-5 { 0.0 } else { c });
    let length = ludion_core::math::sqrt(
        flushed[0] * flushed[0] + flushed[1] * flushed[1] + flushed[2] * flushed[2],
    );
    if length > 0.0 {
        flushed.map(|c| c / length)
    } else {
        normal
    }
}

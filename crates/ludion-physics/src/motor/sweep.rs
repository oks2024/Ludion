//! The motor's capsule queries against a [`PhysicsWorld`]: the first blocking cast hit and the
//! penetration contacts that the public queries do not expose.

use ludion_core::ids::EntityId;
use ludion_spatial::Shape;
use rapier3d::prelude::BoundingVolume;

use crate::queries::{QueryFilter, QueryHit};
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
}

/// The capsule a motor sweeps: its centre, the cylinder half-length and the radius.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Capsule {
    pub half_height: f32,
    pub radius: f32,
}

impl Capsule {
    fn shape(self) -> Shape {
        Shape::Capsule {
            half_height: self.half_height,
            radius: self.radius,
        }
    }
}

impl PhysicsWorld {
    /// The first hit along `direction` (unit) within `length` whose normal opposes the motion and
    /// that `skip` does not drop, or `None`.
    pub(crate) fn cast_capsule(
        &self,
        capsule: Capsule,
        center: [f32; 3],
        direction: [f32; 3],
        length: f32,
        filter: &QueryFilter,
        skip: impl Fn(&QueryHit) -> bool,
    ) -> Option<QueryHit> {
        let hits = self
            .shape_cast(
                &capsule.shape(),
                center,
                IDENTITY,
                direction,
                length,
                filter,
            )
            .ok()?;
        hits.into_iter().find(|hit| {
            let along = hit.normal[0] * direction[0]
                + hit.normal[1] * direction[1]
                + hit.normal[2] * direction[2];
            along < BLOCKING && !skip(hit)
        })
    }

    /// Every contact of the capsule at `center` within `prediction`, sorted `(dist, entity)`.
    /// Composite shapes (a level mesh) report their single deepest contact.
    pub(crate) fn capsule_contacts(
        &self,
        capsule: Capsule,
        center: [f32; 3],
        prediction: f32,
        filter: &QueryFilter,
    ) -> Vec<Contact> {
        let Ok(drops) = self.query_drops(filter) else {
            return Vec::new();
        };
        let Ok(built) = shape_builder(&capsule.shape(), self.plane, None) else {
            return Vec::new();
        };
        let shape = built.shape;
        let at = to_pose(center, IDENTITY);
        let aabb = shape.compute_aabb(&at).loosened(prediction);
        let pipeline = self.rapier.query_pipeline();
        let mut contacts = Vec::new();
        for (_, collider) in pipeline.intersect_aabb_conservative(aabb) {
            let entity = entity_from_bits(collider.user_data);
            if drops(entity) {
                continue;
            }
            let pose = collider.position();
            let Ok(Some(contact)) = pipeline.dispatcher.contact(
                &pose.inv_mul(&at),
                collider.shape(),
                &*shape,
                prediction,
            ) else {
                continue;
            };
            let normal = clean_normal(from_vector(pose.rotation * contact.normal1));
            if !contact.dist.is_finite() || !normal.iter().all(|c| c.is_finite()) {
                continue;
            }
            contacts.push(Contact {
                entity,
                dist: contact.dist,
                normal,
            });
        }
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
        filter: &QueryFilter,
    ) -> f32 {
        let Some((shape, translation, rotation)) = self.shape_and_pose(prop) else {
            return limit;
        };
        let Ok(hits) = self.shape_cast(&shape, translation, rotation, direction, limit, filter)
        else {
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
        filter: &QueryFilter,
    ) -> Option<QueryHit> {
        let hits = self
            .ray_cast(origin, [0.0, -1.0, 0.0], length, filter)
            .ok()?;
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

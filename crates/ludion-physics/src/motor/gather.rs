//! One BVH walk per motor solve: the colliders near the capsule, kept with their BVH leaf boxes, so
//! the solve's many casts and contact queries scan a short list instead of re-walking the tree.
//!
//! Exact by construction: a query whose bounds lie inside the gathered box applies the same leaf
//! test rapier's traversal would, to the same leaf box, over a superset of the leaves it could
//! visit; a query that leaves the box takes the BVH path unchanged. The box therefore sets speed,
//! never results.

use rapier3d::prelude::{Aabb, BoundingVolume, Collider, ColliderHandle};

use crate::sweep::SweepFilter;
use crate::world::{PhysicsWorld, entity_from_bits};

/// Every collider whose BVH leaf box meets `bounds` and that neither the query pipeline's own
/// filter nor the gathering [`SweepFilter`] drops.
pub(crate) struct Gather {
    bounds: Aabb,
    leaves: Vec<(Aabb, ColliderHandle)>,
}

impl Gather {
    /// The kept leaves with their BVH boxes.
    #[cfg(test)]
    pub(crate) fn handles(&self) -> Vec<ColliderHandle> {
        self.leaves.iter().map(|(_, handle)| *handle).collect()
    }

    /// Whether a query with bounds `q` can be answered from this gather.
    pub(crate) fn covers(&self, q: &Aabb) -> bool {
        self.bounds.contains(q)
    }
}

impl PhysicsWorld {
    /// Walks the BVH once over `bounds`, keeping what `filter` does not drop.
    pub(crate) fn gather(&self, bounds: Aabb, filter: &SweepFilter) -> Gather {
        let pipeline = self.rapier.query_pipeline();
        let mut leaves = Vec::new();
        for leaf in pipeline.bvh.leaves(|node| node.aabb().intersects(&bounds)) {
            let Some((collider, handle)) = pipeline.colliders.get_unknown_gen(leaf) else {
                continue;
            };
            if !pipeline.filter.test(pipeline.bodies, handle, collider)
                || filter.drops(self, entity_from_bits(collider.user_data), collider)
            {
                continue;
            }
            let Some(node) = pipeline.bvh.leaf_node(leaf) else {
                continue;
            };
            leaves.push((node.aabb(), handle));
        }
        Gather { bounds, leaves }
    }

    /// Runs `f` on every collider the BVH walk over `q` would yield past the pipeline's filter:
    /// from `gather` when it covers `q`, else from the BVH itself.
    pub(crate) fn visit(&self, gather: Option<&Gather>, q: Aabb, f: impl FnMut(&Collider)) {
        match gather.filter(|g| g.covers(&q)) {
            Some(gather) => self
                .gathered(gather, |leaf| leaf.intersects(&q))
                .for_each(f),
            None => self
                .rapier
                .query_pipeline()
                .intersect_aabb_conservative(q)
                .map(|(_, collider)| collider)
                .for_each(f),
        }
    }

    /// The colliders the gather holds whose leaf box passes `leaf_hits`, in gather order. The
    /// caller has checked [`Gather::covers`] for its query's bounds.
    pub(crate) fn gathered<'a>(
        &'a self,
        gather: &'a Gather,
        leaf_hits: impl Fn(&Aabb) -> bool + 'a,
    ) -> impl Iterator<Item = &'a Collider> + 'a {
        let colliders = self.rapier.query_pipeline().colliders;
        gather
            .leaves
            .iter()
            .filter(move |(aabb, _)| leaf_hits(aabb))
            .filter_map(move |(_, handle)| colliders.get(*handle))
    }
}

#[cfg(test)]
mod tests {
    use ludion_core::ids::EntityId;
    use ludion_core::math::{cos, sin, sqrt};
    use ludion_core::time::{FrameIndex, TickRate};
    use ludion_ecs::{Schedule, SystemCtx, World};
    use ludion_spatial::{Body, Collider as SpatialCollider, RigidBody, Shape, Velocity};
    use rapier3d::prelude::Aabb;
    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};

    use super::*;
    use crate::motor::sweep::Capsule;
    use crate::queries::{QueryHit, unit};
    use crate::world::{to_pose, to_vector};
    use crate::{
        CollisionLayers, NoParent, PhysicsPose, PhysicsSettings, PhysicsSpace, QueryFilter,
    };

    #[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Default)]
    struct Pose {
        translation: [f32; 3],
        rotation: [f32; 4],
    }

    impl PhysicsPose for Pose {
        fn translation(&self) -> [f32; 3] {
            self.translation
        }
        fn set_translation(&mut self, translation: [f32; 3]) {
            self.translation = translation;
        }
        fn rotation_quat(&self) -> [f32; 4] {
            self.rotation
        }
        fn set_rotation_quat(&mut self, rotation: [f32; 4]) {
            self.rotation = rotation;
        }
    }

    /// A tiny deterministic generator, so the query setups are fixed.
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> f32 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (self.0 >> 40) as f32 / (1u64 << 24) as f32
        }

        fn range(&mut self, lo: f32, hi: f32) -> f32 {
            lo + (hi - lo) * self.next()
        }
    }

    fn spawn(world: &mut World, at: [f32; 3], shape: Shape, body: Body, layer: &str) -> EntityId {
        let entity = world.spawn();
        let collider = SpatialCollider {
            shape,
            body,
            layer: (layer != "default").then(|| layer.to_string()),
            sensor: false,
        };
        world
            .insert(
                entity,
                Pose {
                    translation: at,
                    rotation: [0.0, 0.0, 0.0, 1.0],
                },
            )
            .unwrap();
        world.insert(entity, collider).unwrap();
        world.insert(entity, Velocity::default()).unwrap();
        entity
    }

    /// A crowd of kinematic capsules ignoring their own layer, packed in a disc, among a floor, a
    /// wall, a step, a rigid prop and a sensor; one step builds the BVH.
    fn crowd_world() -> (World, Vec<EntityId>) {
        let layers = CollisionLayers::new(vec!["default".into(), "crowd".into()], vec![0, 0b10]);
        let mut world = World::new();
        let mut schedule = Schedule::new();
        crate::install::<Pose, NoParent>(
            &mut schedule,
            PhysicsSettings {
                layers: Some(layers),
                ..PhysicsSettings::new(PhysicsSpace::Free)
            },
        );
        let fixed = |half: [f32; 3]| Shape::Box { half_extents: half };
        spawn(
            &mut world,
            [0.0, -0.5, 0.0],
            fixed([20.0, 0.5, 20.0]),
            Body::new_static(),
            "default",
        );
        spawn(
            &mut world,
            [4.0, 1.0, 0.0],
            fixed([0.2, 1.0, 3.0]),
            Body::new_static(),
            "default",
        );
        spawn(
            &mut world,
            [-1.0, 0.1, 2.5],
            fixed([0.6, 0.1, 0.6]),
            Body::new_static(),
            "default",
        );
        spawn(
            &mut world,
            [1.2, 0.5, -2.0],
            fixed([0.4, 0.4, 0.4]),
            Body::Rigid(RigidBody::default()),
            "default",
        );
        let sensor = spawn(
            &mut world,
            [0.0, 1.0, 0.0],
            fixed([1.0, 1.0, 1.0]),
            Body::new_static(),
            "default",
        );
        world
            .get_mut::<SpatialCollider>(sensor)
            .unwrap()
            .unwrap()
            .sensor = true;
        let mut crowd = Vec::new();
        let mut rng = Lcg(7);
        for _ in 0..60 {
            let angle = rng.range(0.0, std::f32::consts::TAU);
            let radius = 1.5 * sqrt(rng.next());
            crowd.push(spawn(
                &mut world,
                [radius * cos(angle), 0.45, radius * sin(angle)],
                Shape::Capsule {
                    half_height: 0.17,
                    radius: 0.28,
                },
                Body::new_kinematic(),
                "crowd",
            ));
        }
        schedule.run(
            &mut world,
            &SystemCtx::new(FrameIndex::new(0), TickRate::DEFAULT),
        );
        (world, crowd)
    }

    fn bits(hits: &[QueryHit]) -> Vec<(EntityId, [u32; 10])> {
        hits.iter()
            .map(|h| {
                let mut f = [0u32; 10];
                f[0] = h.distance.to_bits();
                for k in 0..3 {
                    f[1 + k] = h.point[k].to_bits();
                    f[4 + k] = h.normal[k].to_bits();
                }
                (h.entity, f)
            })
            .collect()
    }

    fn filter_for(physics: &PhysicsWorld, member: EntityId) -> SweepFilter {
        physics
            .sweep_filter(&QueryFilter::new().exclude(member).ignore_layer("crowd"))
            .unwrap()
    }

    #[test]
    fn the_gather_holds_the_conservative_set_minus_the_drops() {
        let (world, crowd) = crowd_world();
        let physics = world.resource::<PhysicsWorld>().unwrap();
        let filter = filter_for(physics, crowd[0]);
        let mut rng = Lcg(11);
        for _ in 0..40 {
            let c = [
                rng.range(-2.0, 2.0),
                rng.range(0.0, 1.5),
                rng.range(-2.0, 2.0),
            ];
            let half = rng.range(0.2, 3.0);
            let bounds = Aabb::new(
                to_vector([c[0] - half, c[1] - half, c[2] - half]),
                to_vector([c[0] + half, c[1] + half, c[2] + half]),
            );
            let gather = physics.gather(bounds, &filter);
            let pipeline = physics.rapier.query_pipeline();
            let mut want: Vec<_> = pipeline
                .intersect_aabb_conservative(bounds)
                .filter(|(_, co)| !filter.drops(physics, entity_from_bits(co.user_data), co))
                .map(|(handle, _)| handle)
                .collect();
            let mut got = gather.handles();
            want.sort_by_key(|h| h.into_raw_parts());
            got.sort_by_key(|h| h.into_raw_parts());
            assert_eq!(got, want);
        }
    }

    #[test]
    fn gathered_queries_equal_the_bvh_bitwise() {
        let (world, crowd) = crowd_world();
        let physics = world.resource::<PhysicsWorld>().unwrap();
        let capsule = Capsule {
            half_height: 0.17,
            radius: 0.28,
        };
        let shape = capsule.built(physics);
        let wide = Capsule {
            radius: 0.31,
            ..capsule
        }
        .built(physics);
        let mut rng = Lcg(99);
        let (mut covered, mut escaped, mut found) = (0, 0, 0);
        for round in 0..500 {
            let member = crowd[round % crowd.len()];
            let filter = filter_for(physics, member);
            let at = [
                rng.range(-2.5, 2.5),
                rng.range(0.2, 1.2),
                rng.range(-2.5, 2.5),
            ];
            let reach = rng.range(0.1, 2.0);
            let base = shape.compute_aabb(&to_pose(at, [0.0, 0.0, 0.0, 1.0]));
            let box_ = Aabb::new(
                base.mins - to_vector([reach; 3]),
                base.maxs + to_vector([reach; 3]),
            );
            let gather = physics.gather(box_, &filter);
            let direction = unit([
                rng.range(-1.0, 1.0),
                rng.range(-1.0, 1.0),
                rng.range(-1.0, 1.0),
            ])
            .unwrap_or([1.0, 0.0, 0.0]);
            let length = rng.range(0.0, 2.5);
            let prediction = rng.range(0.0, 0.1);
            let identity = [0.0, 0.0, 0.0, 1.0];

            let hits = |g: Option<&Gather>| {
                physics.shape_hits(&*shape, at, identity, direction, length, &filter, g)
            };
            let swept = physics.shape_hits(&*shape, at, identity, direction, length, &filter, None);
            assert_eq!(bits(&hits(Some(&gather))), bits(&swept));

            let rays = physics.ray_hits(at, direction, length, &filter, Some(&gather));
            assert_eq!(
                bits(&rays),
                bits(&physics.ray_hits(at, direction, length, &filter, None))
            );

            let contacts =
                |g: Option<&Gather>| physics.capsule_contacts(&*wide, at, prediction, &filter, g);
            let (a, b) = (contacts(Some(&gather)), contacts(None));
            assert_eq!(a.len(), b.len());
            found += a.len() + swept.len() + rays.len();
            for (x, y) in a.iter().zip(&b) {
                assert_eq!(x.entity, y.entity);
                assert_eq!(x.dist.to_bits(), y.dist.to_bits());
                for k in 0..3 {
                    assert_eq!(x.normal[k].to_bits(), y.normal[k].to_bits());
                    assert_eq!(x.point[k].to_bits(), y.point[k].to_bits());
                }
            }

            let end = [
                at[0] + direction[0] * length,
                at[1] + direction[1] * length,
                at[2] + direction[2] * length,
            ];
            let q = shape
                .compute_aabb(&to_pose(at, identity))
                .merged(&shape.compute_aabb(&to_pose(end, identity)));
            if gather.covers(&q) {
                covered += 1;
            } else {
                escaped += 1;
            }
        }
        assert!(
            covered > 50 && escaped > 50,
            "covered {covered}, escaped {escaped}"
        );
        assert!(found > 500, "only {found} hits compared");
    }
}

//! The engine-typed half of [`ludion_scene::instantiate_into`]: `ludion-scene` never names a
//! component type, so naming a part and baking a placement onto a root go through these hooks.

use ludion_core::ids::EntityId;
use ludion_core::math::{Trs, quat_to_euler_xyz_degrees};
use ludion_ecs::World;
use ludion_scene::InstanceHooks;

use crate::components::{Name, Transform};
use crate::hierarchy::level_trs;

/// The engine's [`InstanceHooks`]: names parts with [`Name`] and places roots through
/// [`Transform`]. For hosts that instantiate an expanded scene themselves.
pub struct EngineHooks;

impl InstanceHooks for EngineHooks {
    fn name(&self, world: &mut World, entity: EntityId, name: &str) {
        world
            .insert(entity, Name(name.to_owned()))
            .expect("the entity was just instantiated and is alive");
    }

    fn place(&self, world: &mut World, root: EntityId, placement: &Trs) {
        // An identity placement leaves the root as authored: no baked `Transform`, no
        // quaternion round trip through Euler angles.
        if *placement == Trs::IDENTITY {
            return;
        }
        let local = world
            .get::<Transform>(root)
            .ok()
            .flatten()
            .copied()
            .unwrap_or_default();
        let composed = placement.mul_trs(&level_trs(&local));
        // The bake's `-0.0` is noise (a saved `0.0` must reload as itself): `+ 0.0` makes it `0.0`.
        let unsigned = |v: [f32; 3]| v.map(|v| v + 0.0);
        world
            .insert(
                root,
                Transform {
                    translation: unsigned(composed.translation),
                    rotation: unsigned(quat_to_euler_xyz_degrees(composed.rotation)),
                    scale: unsigned(composed.scale),
                },
            )
            .expect("the entity was just instantiated and is alive");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::builtin_registry;
    use crate::hierarchy::world_transform;
    use ludion_core::codes::code;
    use ludion_core::math::euler_xyz_degrees_to_quat;
    use ludion_core::time::{FrameIndex, TickRate};
    use ludion_ecs::{Parent, Registry, Schedule, SystemCtx};
    use ludion_scene::{InstantiateOptions, SceneEntity, instantiate_into};
    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};
    use std::cell::RefCell;

    #[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
    struct Aim {
        #[schemars(with = "ludion_ecs::EntityRefSchema")]
        target: EntityId,
    }

    fn registry() -> Registry {
        let mut registry = builtin_registry();
        registry.register::<Aim>("Aim").expect("unique");
        registry
    }

    fn group(source: &str, registry: &Registry) -> Vec<SceneEntity> {
        ludion_scene::check(source, registry)
            .expect("the group checks")
            .entities
    }

    fn options<'a>(placement: Option<Trs>, prefix: Option<&'a str>) -> InstantiateOptions<'a> {
        InstantiateOptions {
            placement,
            name_prefix: prefix,
            hooks: &EngineHooks,
        }
    }

    const LINKED: &str = "format: 1
entities:
  - name: a
    components:
      Transform:
        translation:
          - 1.0
          - 0.0
          - 0.0
  - name: b
    components:
      Transform:
        translation:
          - 0.0
          - 1.0
          - 0.0
      Parent:
        index: 0
        generation: 0
      Aim:
        target:
          index: 0
          generation: 0
      Joint:
        a:
          index: 0
          generation: 0
        b:
          index: 1
          generation: 0
        kind:
          fixed: {}
";

    /// Every reference lands on its own copy's ids, index and generation, in a world whose free
    /// list is being reused.
    #[test]
    fn references_land_on_the_spawned_ids() {
        let registry = registry();
        let group = group(LINKED, &registry);
        let mut world = World::new();
        let spawned: Vec<_> = (0..5).map(|_| world.spawn()).collect();
        world.despawn(spawned[3]).unwrap();
        world.despawn(spawned[1]).unwrap();

        let first = instantiate_into(&mut world, &group, &registry, &options(None, None)).unwrap();
        let second = instantiate_into(&mut world, &group, &registry, &options(None, None)).unwrap();
        for ids in [&first, &second] {
            assert_eq!(world.get::<Parent>(ids[1]).unwrap().unwrap().0, ids[0]);
            assert_eq!(world.get::<Aim>(ids[1]).unwrap().unwrap().target, ids[0]);
            let joint = world.get::<ludion_spatial::Joint>(ids[1]).unwrap().unwrap();
            assert_eq!((joint.a, joint.b), (ids[0], ids[1]));
        }
        assert!(first[0].generation() >= 1);
        assert_ne!(first[0], second[0]);
    }

    /// The placement lands on the root's stored pose only; the child's stored pose is the
    /// document's, and its world pose composes through the placed root.
    #[test]
    fn placement_bakes_onto_roots_only() {
        let registry = registry();
        let group = group(LINKED, &registry);
        let placement = Trs {
            translation: [10.0, 0.0, 0.0],
            rotation: euler_xyz_degrees_to_quat([0.0, 90.0, 0.0]),
            scale: [1.0; 3],
        };
        let mut world = World::new();
        let ids = instantiate_into(
            &mut world,
            &group,
            &registry,
            &options(Some(placement), None),
        )
        .unwrap();

        let child = world.get::<Transform>(ids[1]).unwrap().copied().unwrap();
        assert_eq!(child.translation, [0.0, 1.0, 0.0]);
        let root = level_trs(&world.get::<Transform>(ids[0]).unwrap().copied().unwrap());
        let expected = placement.mul_trs(&level_trs(&Transform {
            translation: [1.0, 0.0, 0.0],
            ..Transform::default()
        }));
        for (got, want) in root.translation.iter().zip(expected.translation) {
            assert!((got - want).abs() < 1e-4, "{:?} vs {:?}", root, expected);
        }
        let world_pose = world_transform(&world, ids[1]).unwrap().trs;
        let want = placement
            .mul_trs(&level_trs(&Transform {
                translation: [1.0, 0.0, 0.0],
                ..Transform::default()
            }))
            .mul_trs(&level_trs(&child));
        for (got, want) in world_pose.translation.iter().zip(want.translation) {
            assert!((got - want).abs() < 1e-4, "{world_pose:?} vs {want:?}");
        }
    }

    /// A root without a `Transform` takes the placement itself.
    #[test]
    fn a_root_without_a_pose_takes_the_placement() {
        let registry = registry();
        let group = group("format: 1\nentities:\n  - components: {}\n", &registry);
        let placement = Trs {
            translation: [3.0, 4.0, 5.0],
            ..Trs::IDENTITY
        };
        let mut world = World::new();
        let ids = instantiate_into(
            &mut world,
            &group,
            &registry,
            &options(Some(placement), None),
        )
        .unwrap();
        let pose = world.get::<Transform>(ids[0]).unwrap().copied().unwrap();
        assert_eq!(pose.translation, [3.0, 4.0, 5.0]);
        assert_eq!(pose.scale, [1.0; 3]);
    }

    #[test]
    fn names_take_the_prefix_when_given() {
        let registry = registry();
        let group = group(LINKED, &registry);
        let mut world = World::new();
        let prefixed = instantiate_into(
            &mut world,
            &group,
            &registry,
            &options(None, Some("goblin")),
        )
        .unwrap();
        let bare = instantiate_into(&mut world, &group, &registry, &options(None, None)).unwrap();
        assert_eq!(
            world.get::<Name>(prefixed[0]).unwrap().unwrap().0,
            "goblin/a"
        );
        assert_eq!(
            world.get::<Name>(prefixed[1]).unwrap().unwrap().0,
            "goblin/b"
        );
        assert_eq!(world.get::<Name>(bare[0]).unwrap().unwrap().0, "a");
    }

    thread_local! {
        static REFUSAL: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
    }

    /// A sim system cannot spawn a presentation part: the call is refused with `L0028`, not a panic,
    /// and the world is untouched.
    #[test]
    fn a_sim_system_cannot_instantiate_a_presentation_part() {
        let registry = registry();
        let group = group(
            "format: 1\nentities:\n  - presentation: true\n    components: {}\n",
            &registry,
        );
        let mut world = World::new();
        world.spawn();
        let before = (
            world.entity_count(),
            world.allocator_state(),
            world.presentation_allocator_state(),
        );

        let mut schedule = Schedule::new();
        schedule.add_system_shared(
            "instantiate_presentation",
            std::sync::Arc::new(move |world: &mut World, _: &SystemCtx| {
                let result = instantiate_into(world, &group, &registry, &options(None, None));
                REFUSAL.with(|slot| {
                    *slot.borrow_mut() = Some(match result {
                        Ok(_) => Vec::new(),
                        Err(diagnostics) => diagnostics
                            .iter()
                            .map(|d| format!("{} {}", d.code, d.path.as_deref().unwrap_or("")))
                            .collect(),
                    });
                });
            }),
        );
        schedule.run(
            &mut world,
            &SystemCtx::new(FrameIndex::ZERO, TickRate::DEFAULT),
        );

        let refused = REFUSAL
            .with(|slot| slot.borrow_mut().take())
            .expect("the system ran");
        assert_eq!(
            refused,
            [format!(
                "{} /entities/0/presentation",
                code::PRESENTATION_BOUNDARY
            )]
        );
        assert_eq!(
            (
                world.entity_count(),
                world.allocator_state(),
                world.presentation_allocator_state()
            ),
            before
        );
    }
}

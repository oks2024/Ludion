//! Integration tests for scene → world instantiation.

mod common;

use common::{Health, Shape, Sprite, Transform, registry};
use ludion_scene::{check, instantiate, parse};

const VALID: &str = include_str!("fixtures/valid.yaml");

/// The committed gallery fixture instantiates: every scene entity exists in the world, in
/// document order, with exactly the components its document block declares.
#[test]
fn valid_fixture_instantiates_with_all_components() {
    let registry = registry();
    let scene = check(VALID, &registry).expect("valid.yaml must validate");
    let world = instantiate(&scene, &registry).expect("valid.yaml must instantiate");

    assert_eq!(world.entity_count(), scene.entities.len());
    let entities: Vec<_> = world.entities().collect();
    for (index, (scene_entity, &id)) in scene.entities.iter().zip(&entities).enumerate() {
        // A fresh world spawns slot indices 0, 1, 2… — document position is the entity index.
        assert_eq!(id.index(), u32::try_from(index).unwrap());
        for (name, _) in &scene_entity.components {
            let info = registry
                .get(name)
                .expect("checked scenes name known components");
            assert!(
                info.has(&world, id),
                "entity {index} is missing its `{name}` component"
            );
        }
    }
}

/// Component values arrive typed and intact, not just present.
#[test]
fn instantiated_component_values_round_trip() {
    let registry = registry();
    let scene = check(
        "format: 1\n\
         entities:\n  \
           - components:\n      \
               Transform:\n        \
                 x: 1.5\n        \
                 y: -2.0\n        \
                 z: 0.25\n      \
               Sprite:\n        \
                 texture: \"brick.png\"\n        \
                 layer: 3\n        \
                 visible: true\n  \
           - components:\n      \
               Shape:\n        \
                 Circle:\n          \
                   radius: 2.5\n      \
               Health: 100\n",
        &registry,
    )
    .expect("the scene is valid");
    let world = instantiate(&scene, &registry).expect("the scene must instantiate");

    let entities: Vec<_> = world.entities().collect();
    assert_eq!(entities.len(), 2);
    assert_eq!(
        world.get::<Transform>(entities[0]).unwrap(),
        Some(&Transform {
            x: 1.5,
            y: -2.0,
            z: 0.25
        })
    );
    assert_eq!(
        world.get::<Sprite>(entities[0]).unwrap(),
        Some(&Sprite {
            texture: "brick.png".into(),
            layer: 3,
            visible: true
        })
    );
    assert_eq!(
        world.get::<Shape>(entities[1]).unwrap(),
        Some(&Shape::Circle { radius: 2.5 })
    );
    assert_eq!(
        world.get::<Health>(entities[1]).unwrap(),
        Some(&Health(100))
    );
    // No cross-contamination: entity 0 has no Shape, entity 1 has no Transform.
    assert_eq!(world.get::<Shape>(entities[0]).unwrap(), None);
    assert_eq!(world.get::<Transform>(entities[1]).unwrap(), None);
}

/// An unchecked scene with bad component data fails safely: an `L0006` diagnostic per defect,
/// each carrying a JSON Pointer to the offending component, and no partial world escapes.
#[test]
fn bad_component_data_reports_pointered_diagnostics() {
    let registry = registry();
    // Parsed but deliberately *not* checked: `x` is a string, and `Helth` is a typo.
    let scene = parse(
        "format: 1\n\
         entities:\n  \
           - components: {}\n  \
           - components:\n      \
               Transform:\n        \
                 x: \"fast\"\n        \
                 y: 0.0\n        \
                 z: 0.0\n      \
               Helth: 5\n",
    )
    .expect("the document is well-formed");

    let Err(diagnostics) = instantiate(&scene, &registry) else {
        panic!("bad data must not instantiate");
    };
    let summary: Vec<(&str, &str)> = diagnostics
        .iter()
        .map(|d| (d.code.as_str(), d.path.as_deref().unwrap_or("")))
        .collect();
    assert_eq!(
        summary,
        vec![
            ("L0006", "/entities/1/components/Transform"),
            ("L0004", "/entities/1/components/Helth"),
        ]
    );
    // The messages name the component, rustc-style.
    assert!(diagnostics[0].message.contains("Transform"));
    assert!(diagnostics[1].message.contains("Helth"));
}

/// An empty scene instantiates to an empty world.
#[test]
fn empty_scene_instantiates_to_empty_world() {
    let registry = registry();
    let scene = check("format: 1\nentities: []\n", &registry).unwrap();
    let world = instantiate(&scene, &registry).unwrap();
    assert_eq!(world.entity_count(), 0);
}

/// `instantiate` never materializes a reserved component from a `components:` block — the reserved
/// type's owner populates it (the engine projects the entity `name:` field into `Name`). `check`
/// rejects such a block up front (L1016); this pins the unchecked-path skip.
#[test]
fn instantiate_skips_reserved_component_blocks() {
    let registry = registry();
    // Bypass `check` deliberately to exercise the instantiate-time guard directly.
    let scene = parse("format: 1\nentities:\n  - components:\n      Label: hero\n").unwrap();
    let world = instantiate(&scene, &registry).expect("instantiation succeeds, skipping Label");
    assert_eq!(world.entity_count(), 1);
    let entity = world.entities().next().unwrap();
    assert!(
        registry
            .get("Label")
            .unwrap()
            .serialize(&world, entity)
            .unwrap()
            .is_none(),
        "the reserved Label component must not have been inserted"
    );
}

fn parent_registry() -> ludion_ecs::Registry {
    let mut registry = registry();
    registry
        .register::<ludion_ecs::Parent>("Parent")
        .expect("Parent registers");
    registry
}

fn parent_to(index: usize) -> String {
    format!("      Parent:\n        index: {index}\n        generation: 0\n")
}

/// Presentation entities interleaved with sim ones take their own id range, leave the sim ids
/// as if they were absent, and have their `Parent` references remapped from document positions.
#[test]
fn presentation_entities_spawn_in_their_own_range_with_refs_remapped() {
    use ludion_ecs::{PRESENTATION_BASE, is_presentation};
    let registry = parent_registry();
    // Positions: 0 sim a, 1 pres p, 2 sim b (Parent -> 0), 3 pres q (Parent -> 2).
    let source = format!(
        "format: 1\nentities:\n  - components:\n      Health: 1\n  - presentation: true\n    components:\n      Health: 2\n  - components:\n      Health: 3\n{}  - presentation: true\n    components:\n      Health: 4\n{}",
        parent_to(0),
        parent_to(2)
    );
    let scene = check(&source, &registry).expect("scene validates");
    let world = instantiate(&scene, &registry).expect("scene instantiates");

    let ids = ludion_scene::instantiated_ids(&scene);
    let indices: Vec<u32> = ids.iter().map(|id| id.index()).collect();
    assert_eq!(indices, [0, PRESENTATION_BASE, 1, PRESENTATION_BASE + 1]);
    assert_eq!(
        world.entities().map(|id| id.index()).collect::<Vec<_>>(),
        [0, 1]
    );
    assert_eq!(
        world
            .presentation_entities()
            .map(|id| id.index())
            .collect::<Vec<_>>(),
        [PRESENTATION_BASE, PRESENTATION_BASE + 1]
    );
    assert!(ids.iter().filter(|id| is_presentation(**id)).count() == 2);

    let parent = |id| {
        world
            .get::<ludion_ecs::Parent>(id)
            .unwrap()
            .expect("has Parent")
            .0
            .index()
    };
    assert_eq!(parent(ids[2]), 0, "b's Parent names a");
    assert_eq!(
        parent(ids[3]),
        1,
        "q's Parent names b, not document position 2"
    );

    // The sim half is the scene without the presentation entities.
    let sim_only = format!(
        "format: 1\nentities:\n  - components:\n      Health: 1\n  - components:\n      Health: 3\n{}",
        parent_to(0)
    );
    let sim_scene = check(&sim_only, &registry).unwrap();
    let sim_world = instantiate(&sim_scene, &registry).unwrap();
    assert_eq!(world.entity_count(), sim_world.entity_count());
    assert_eq!(
        ludion_scene::instantiated_ids(&sim_scene),
        ids[..3].iter().step_by(2).copied().collect::<Vec<_>>()
    );
}

/// A sanctioned list remaps every element from a document position to the spawned id, in place of
/// the single field the walk used to find.
#[test]
fn a_sanctioned_ref_list_remaps_every_element() {
    use ludion_ecs::{EntityRefCollection, EntityRefSchema};

    #[derive(serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
    struct Squad {
        #[schemars(with = "EntityRefCollection<Vec<EntityRefSchema>>")]
        members: Vec<ludion_core::ids::EntityId>,
    }

    let mut registry = registry();
    registry
        .register::<Squad>("Squad")
        .expect("Squad registers");
    let source = "format: 1\nentities:\n  - components:\n      Health: 1\n  - components:\n      Health: 2\n  - components:\n      Squad:\n        members:\n          - index: 1\n            generation: 0\n          - index: 0\n            generation: 0\n";
    let scene = check(source, &registry).expect("scene validates");
    let world = instantiate(&scene, &registry).expect("scene instantiates");
    let ids = ludion_scene::instantiated_ids(&scene);
    let squad = world.get::<Squad>(ids[2]).unwrap().expect("has Squad");
    assert_eq!(squad.members, [ids[1], ids[0]]);
}

mod into {
    use super::*;
    use ludion_core::ids::EntityId;
    use ludion_core::math::Trs;
    use ludion_ecs::World;
    use ludion_scene::{InstanceHooks, InstantiateOptions, instantiate_into};
    use std::cell::RefCell;

    #[derive(Default)]
    struct Recorder {
        calls: RefCell<Vec<String>>,
    }

    impl InstanceHooks for Recorder {
        fn name(&self, _: &mut World, entity: EntityId, name: &str) {
            self.calls
                .borrow_mut()
                .push(format!("name {} {name}", entity.index()));
        }
        fn place(&self, _: &mut World, root: EntityId, _: &Trs) {
            self.calls
                .borrow_mut()
                .push(format!("place {}", root.index()));
        }
        fn provenance(&self, _: &mut World, entity: EntityId, part: &str, root: EntityId) {
            self.calls.borrow_mut().push(format!(
                "provenance {} {part} {}",
                entity.index(),
                root.index()
            ));
        }
    }

    fn options<'a>(
        hooks: &'a Recorder,
        prefix: Option<&'a str>,
        place: bool,
    ) -> InstantiateOptions<'a> {
        InstantiateOptions {
            placement: place.then_some(Trs {
                translation: [1.0, 0.0, 0.0],
                ..Trs::IDENTITY
            }),
            name_prefix: prefix,
            hooks,
        }
    }

    /// A group spawned into a world with a freed slot reuses it, and its references land on the
    /// new ids, generation included.
    #[test]
    fn refs_remap_to_reused_slot_ids_with_generation() {
        let registry = parent_registry();
        let mut world = World::new();
        let spawned: Vec<_> = (0..3).map(|_| world.spawn()).collect();
        world.despawn(spawned[1]).unwrap();
        let source = format!(
            "format: 1\nentities:\n  - components:\n      Health: 1\n  - components:\n      Health: 2\n{}",
            parent_to(0)
        );
        let scene = check(&source, &registry).unwrap();
        let hooks = Recorder::default();
        let ids = instantiate_into(
            &mut world,
            &scene.entities,
            &registry,
            &options(&hooks, None, false),
        )
        .unwrap();

        assert_eq!(ids[0].index(), spawned[1].index());
        assert!(ids[0].generation() >= 1);
        assert_eq!(
            world.get::<ludion_ecs::Parent>(ids[1]).unwrap().unwrap().0,
            ids[0]
        );
        assert_eq!(world.get::<common::Health>(ids[1]).unwrap().unwrap().0, 2);
    }

    /// A bad value anywhere refuses the whole group and leaves the world untouched.
    #[test]
    fn failure_leaves_the_world_untouched() {
        let registry = registry();
        let mut world = World::new();
        let spawned: Vec<_> = (0..3).map(|_| world.spawn()).collect();
        world.despawn(spawned[0]).unwrap();
        let scene = parse(
            "format: 1\nentities:\n  - components:\n      Health: 1\n  - components:\n      Health: not-a-number\n",
        )
        .unwrap();
        let (count, sim, pres) = (
            world.entity_count(),
            world.allocator_state(),
            world.presentation_allocator_state(),
        );
        let hooks = Recorder::default();
        let diagnostics = instantiate_into(
            &mut world,
            &scene.entities,
            &registry,
            &options(&hooks, None, false),
        )
        .unwrap_err();
        assert_eq!(
            diagnostics[0].path.as_deref(),
            Some("/entities/1/components/Health")
        );
        assert_eq!(world.entity_count(), count);
        assert_eq!(world.allocator_state(), sim);
        assert_eq!(world.presentation_allocator_state(), pres);
        assert!(hooks.calls.borrow().is_empty());
    }

    /// `place` reaches roots only, in document order; `name` gets the prefixed or bare name, and
    /// never fires for an unnamed part; `provenance` follows the names, once per named part, with
    /// the bare part name and the group's first id.
    #[test]
    fn hooks_see_roots_and_final_names() {
        let registry = parent_registry();
        let source = format!(
            "format: 1\nentities:\n  - name: pelvis\n    components:\n      Health: 1\n  - components:\n      Health: 2\n{}  - name: spine\n    components:\n      Health: 3\n{}  - name: loose\n    components:\n      Health: 4\n",
            parent_to(0),
            parent_to(0)
        );
        let scene = check(&source, &registry).unwrap();

        let hooks = Recorder::default();
        let mut world = World::new();
        instantiate_into(
            &mut world,
            &scene.entities,
            &registry,
            &options(&hooks, Some("goblin"), true),
        )
        .unwrap();
        assert_eq!(
            *hooks.calls.borrow(),
            [
                "place 0",
                "place 3",
                "name 0 goblin/pelvis",
                "name 2 goblin/spine",
                "name 3 goblin/loose",
                "provenance 0 pelvis 0",
                "provenance 2 spine 0",
                "provenance 3 loose 0"
            ]
        );

        let hooks = Recorder::default();
        let mut world = World::new();
        instantiate_into(
            &mut world,
            &scene.entities,
            &registry,
            &options(&hooks, None, false),
        )
        .unwrap();
        assert_eq!(
            *hooks.calls.borrow(),
            [
                "name 0 pelvis",
                "name 2 spine",
                "name 3 loose",
                "provenance 0 pelvis 0",
                "provenance 2 spine 0",
                "provenance 3 loose 0"
            ]
        );
    }
}

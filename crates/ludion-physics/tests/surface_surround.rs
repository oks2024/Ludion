//! `ContactEvent::surface_a/b` name the surface on each side of a contact event (an overlay answers for both,
//! a stop answers none, a restore carries them) and `surface_wireframe` tints every collider
//! outline by the surface a hit there reports.

mod common;

use common::{TestPose, pose_of, tick};
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::time::TickRate;
use ludion_ecs::{Schedule, World};
use ludion_physics::{
    ColliderMeshes, ContactEvent, ContactPhase, NoParent, PhysicsSettings, PhysicsSpace,
    PhysicsWorld, SurfaceId, SurfaceOverlay, SurfaceTable, SurfaceTag, TriangleMesh, install,
};
use ludion_spatial::{Body, Collider, RigidBody, Shape, Velocity};

const CONCRETE: SurfaceId = SurfaceId(0);
const GRATE: SurfaceId = SurfaceId(1);
const PUDDLE: SurfaceId = SurfaceId(2);
const WOOD: SurfaceId = SurfaceId(3);

fn settings(default: Option<SurfaceId>) -> PhysicsSettings {
    PhysicsSettings {
        surfaces: Some(SurfaceTable::new(
            vec![
                "concrete".into(),
                "metal_grate".into(),
                "puddle".into(),
                "wood".into(),
            ],
            default,
        )),
        ..PhysicsSettings::new(PhysicsSpace::Free)
    }
}

fn stepped(default: Option<SurfaceId>) -> Schedule {
    let mut schedule = Schedule::new();
    install::<TestPose, NoParent>(&mut schedule, settings(default));
    schedule
}

fn physics(world: &World) -> &PhysicsWorld {
    world.resource::<PhysicsWorld>().unwrap()
}

fn tag(name: &str) -> SurfaceTag {
    SurfaceTag {
        surface: name.to_string(),
        parts: vec![],
        cells: None,
    }
}

fn box_floor(world: &mut World, surface: Option<&str>) -> EntityId {
    let floor = world.spawn();
    world.insert(floor, TestPose::at([0.0, -0.5, 0.0])).unwrap();
    world
        .insert(
            floor,
            Collider {
                shape: Shape::Box {
                    half_extents: [20.0, 0.5, 20.0],
                },
                body: Body::new_static(),
                layer: None,
                sensor: false,
            },
        )
        .unwrap();
    if let Some(name) = surface {
        world.insert(floor, tag(name)).unwrap();
    }
    floor
}

fn ball(world: &mut World, at: [f32; 3], surface: Option<&str>) -> EntityId {
    let entity = world.spawn();
    world.insert(entity, TestPose::at(at)).unwrap();
    world
        .insert(
            entity,
            Collider {
                shape: Shape::Sphere { radius: 0.25 },
                body: Body::Rigid(RigidBody {
                    mass: 1.0,
                    restitution: 0.0,
                    ..RigidBody::default()
                }),
                layer: None,
                sensor: false,
            },
        )
        .unwrap();
    world.insert(entity, Velocity::default()).unwrap();
    if let Some(name) = surface {
        world.insert(entity, tag(name)).unwrap();
    }
    entity
}

/// Ticks until `pair` reports its `Started` event, and returns it.
fn first_start(
    world: &mut World,
    schedule: &mut Schedule,
    pair: (EntityId, EntityId),
) -> ContactEvent {
    for frame in 0..200 {
        tick(world, schedule, frame);
        if let Some(event) = physics(world)
            .contact_events()
            .iter()
            .find(|e| (e.a, e.b) == pair && e.phase == ContactPhase::Started)
        {
            return *event;
        }
    }
    panic!("the pair never started touching");
}

#[test]
fn a_ball_on_a_tagged_box_answers_floor_then_ball_and_a_stop_answers_none() {
    let mut world = World::new();
    let floor = box_floor(&mut world, Some("metal_grate"));
    let dropped = ball(&mut world, [0.0, 1.0, 0.0], Some("wood"));
    let mut schedule = stepped(None);
    let event = first_start(&mut world, &mut schedule, (floor, dropped));
    assert_eq!(
        [event.surface_a, event.surface_b],
        [Some(GRATE), Some(WOOD)]
    );

    assert!(
        pose_of(&world, dropped).translation[1] < 0.6,
        "the ball fell onto the floor"
    );
    world.despawn(dropped).unwrap();
    tick(&mut world, &mut schedule, 500);
    let stop = *physics(&world)
        .contact_events()
        .iter()
        .find(|e| e.phase == ContactPhase::Stopped)
        .expect("a despawn stops the contact");
    assert_eq!([stop.surface_a, stop.surface_b], [None, None]);
}

#[test]
fn an_untagged_pair_answers_the_default_surface() {
    let mut world = World::new();
    let floor = box_floor(&mut world, None);
    let dropped = ball(&mut world, [0.0, 1.0, 0.0], None);
    let mut schedule = stepped(Some(CONCRETE));
    let event = first_start(&mut world, &mut schedule, (floor, dropped));
    assert_eq!(
        [event.surface_a, event.surface_b],
        [Some(CONCRETE), Some(CONCRETE)]
    );
}

/// Eight quads along x over `x ∈ [0, 8]`: `metal_grate` on `x < 4`, `concrete` beyond.
fn two_tag_strip() -> TriangleMesh {
    let mut vertices = Vec::new();
    for i in 0..=8u32 {
        vertices.push([i as f32, 0.0, -2.0]);
        vertices.push([i as f32, 0.0, 2.0]);
    }
    let mut triangles = Vec::new();
    for i in 0..8u32 {
        let a = i * 2;
        triangles.push([a, a + 2, a + 1]);
        triangles.push([a + 1, a + 2, a + 3]);
    }
    let mut triangle_surfaces = vec![0u16; 8];
    triangle_surfaces.extend(vec![1u16; 8]);
    TriangleMesh {
        vertices,
        triangles,
        hash: 7,
        surfaces: vec!["metal_grate".into(), "concrete".into()],
        triangle_surfaces,
    }
}

fn mesh_world() -> (World, EntityId) {
    let mut world = World::new();
    world.insert_resource(ColliderMeshes::new(Box::new(|path| match path {
        "strip.mesh.yaml" => Ok(two_tag_strip()),
        other => Err(Diagnostic::new(
            code::ASSET_FILE_IO,
            format!("no mesh at `{other}`"),
        )),
    })));
    let floor = world.spawn();
    world.insert(floor, TestPose::at([0.0; 3])).unwrap();
    world
        .insert(
            floor,
            Collider {
                shape: Shape::Mesh {
                    path: "strip.mesh.yaml".into(),
                },
                body: Body::new_static(),
                layer: None,
                sensor: false,
            },
        )
        .unwrap();
    (world, floor)
}

#[test]
fn a_ball_on_a_two_tag_mesh_answers_the_triangles_tag() {
    for (x, expected) in [(1.5, GRATE), (6.5, CONCRETE)] {
        let (mut world, floor) = mesh_world();
        let dropped = ball(&mut world, [x, 1.0, 0.0], Some("wood"));
        let mut schedule = stepped(None);
        let event = first_start(&mut world, &mut schedule, (floor, dropped));
        assert_eq!(
            [event.surface_a, event.surface_b],
            [Some(expected), Some(WOOD)],
            "at x = {x}"
        );
    }
}

fn puddle(world: &mut World, x: f32) -> EntityId {
    let entity = world.spawn();
    world.insert(entity, TestPose::at([x, 0.0, 0.0])).unwrap();
    world
        .insert(
            entity,
            Collider {
                shape: Shape::Cylinder {
                    half_height: 0.05,
                    radius: 0.6,
                },
                body: Body::new_static(),
                layer: None,
                sensor: true,
            },
        )
        .unwrap();
    world
        .insert(
            entity,
            SurfaceOverlay {
                surface: "puddle".into(),
                priority: 10,
            },
        )
        .unwrap();
    entity
}

#[test]
fn a_ball_dropping_into_a_puddle_answers_the_overlay_on_both_sides() {
    let mut world = World::new();
    let floor = box_floor(&mut world, Some("metal_grate"));
    puddle(&mut world, 0.0);
    let dropped = ball(&mut world, [0.0, 1.0, 0.0], Some("wood"));
    let mut schedule = stepped(None);
    let event = first_start(&mut world, &mut schedule, (floor, dropped));
    assert_eq!(
        [event.surface_a, event.surface_b],
        [Some(PUDDLE), Some(PUDDLE)]
    );
}

#[test]
fn a_restore_carries_the_contact_surfaces() {
    let (mut world, floor) = mesh_world();
    let dropped = ball(&mut world, [1.5, 1.0, 0.0], Some("wood"));
    let mut schedule = stepped(None);
    let event = first_start(&mut world, &mut schedule, (floor, dropped));
    assert_eq!(
        [event.surface_a, event.surface_b],
        [Some(GRATE), Some(WOOD)]
    );

    let blob = physics(&world).to_bytes().unwrap();
    let restored = PhysicsWorld::from_bytes(&blob, TickRate::DEFAULT).unwrap();
    let carried = restored
        .contact_events()
        .iter()
        .find(|e| (e.a, e.b) == (floor, dropped) && e.phase == ContactPhase::Started)
        .expect("the start rides the blob");
    assert_eq!(
        [carried.surface_a, carried.surface_b],
        [Some(GRATE), Some(WOOD)]
    );
}

#[test]
fn a_tag_edit_after_the_step_keeps_the_emitted_surfaces() {
    let mut world = World::new();
    let floor = box_floor(&mut world, Some("metal_grate"));
    let dropped = ball(&mut world, [0.0, 1.0, 0.0], Some("wood"));
    let mut schedule = stepped(None);
    let event = first_start(&mut world, &mut schedule, (floor, dropped));
    assert_eq!(event.surface_b, Some(WOOD));

    world.insert(dropped, tag("concrete")).unwrap();
    let mut physics = world.remove_resource::<PhysicsWorld>().unwrap();
    physics.sync_surface_tags(&world);
    assert_eq!(physics.surface_of(dropped), Some(CONCRETE));
    let kept = physics
        .contact_events()
        .iter()
        .find(|e| (e.a, e.b) == (floor, dropped) && e.phase == ContactPhase::Started)
        .expect("the emitted event stays until the next step");
    assert_eq!([kept.surface_a, kept.surface_b], [Some(GRATE), Some(WOOD)]);
}

fn segments(world: &World) -> Vec<(Option<SurfaceId>, [f32; 3], [f32; 3])> {
    let mut out = Vec::new();
    physics(world).surface_wireframe(&mut |id, a, b| out.push((id, a, b)));
    out
}

#[test]
fn the_wireframe_emits_nothing_with_no_surfaces_declared() {
    let mut world = World::new();
    box_floor(&mut world, None);
    let mut schedule = Schedule::new();
    install::<TestPose, NoParent>(&mut schedule, PhysicsSpace::Free);
    tick(&mut world, &mut schedule, 0);
    assert!(physics(&world).surface_names().is_empty());
    assert!(segments(&world).is_empty());
}

#[test]
fn the_wireframe_colours_a_tagged_box_by_its_tag() {
    let mut world = World::new();
    box_floor(&mut world, Some("wood"));
    let mut schedule = stepped(None);
    tick(&mut world, &mut schedule, 0);
    let drawn = segments(&world);
    assert_eq!(drawn.len(), 12);
    assert!(drawn.iter().all(|s| s.0 == Some(WOOD)), "{drawn:?}");
}

#[test]
fn the_wireframe_gives_each_triangle_of_a_two_tag_mesh_its_own_tag() {
    let (mut world, _) = mesh_world();
    let mut schedule = stepped(None);
    tick(&mut world, &mut schedule, 0);
    tick(&mut world, &mut schedule, 1);
    let drawn = segments(&world);
    let count = |id| drawn.iter().filter(|s| s.0 == Some(id)).count();
    assert_eq!(count(GRATE), 8 * 3, "{drawn:?}");
    assert_eq!(count(CONCRETE), 8 * 3, "{drawn:?}");
    assert_eq!(drawn.len(), 16 * 3);
}

#[test]
fn the_wireframe_draws_an_overlay_in_its_own_id() {
    let mut world = World::new();
    box_floor(&mut world, Some("wood"));
    puddle(&mut world, 3.0);
    let mut schedule = stepped(None);
    tick(&mut world, &mut schedule, 0);
    let drawn = segments(&world);
    let near_puddle = |p: &[f32; 3]| (p[0] - 3.0).abs() < 1.0;
    let puddle_segments: Vec<_> = drawn.iter().filter(|s| s.0 == Some(PUDDLE)).collect();
    assert!(!puddle_segments.is_empty());
    assert!(
        puddle_segments
            .iter()
            .all(|s| near_puddle(&s.1) && near_puddle(&s.2))
    );
    assert_eq!(drawn.iter().filter(|s| s.0 == Some(WOOD)).count(), 12);
}

//! `SurfaceTag.parts`: an integer key tags one part of a `compound` collider and a hit reports the
//! part nearest its point (lowest index on ties); an overlay still wins; an out-of-range index and a
//! non-compound collider report the entity surface; an undeclared part surface is one `L1690`; a
//! parts-only edit and a restore both take effect.

mod common;

use std::sync::{Arc, Mutex};

use common::{TestPose, pose_of, tick};
use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_core::time::TickRate;
use ludion_ecs::{Schedule, SystemCtx, World};
use ludion_physics::{
    CharacterMotor, NoParent, PartSurface, PhysicsSettings, PhysicsSpace, PhysicsWorld,
    QueryFilter, ShapeRejection, SurfaceId, SurfaceOverlay, SurfacePart, SurfaceTable, SurfaceTag,
    install, move_character,
};
use ludion_spatial::{Body, Collider, CompoundPart, Shape};

const IDENTITY: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const CONCRETE: SurfaceId = SurfaceId(0);
const GRATE: SurfaceId = SurfaceId(1);
const CARPET: SurfaceId = SurfaceId(2);

/// Each step's x position and the ground surface the motor reported there.
type SurfaceLog = Arc<Mutex<Vec<(f32, Option<SurfaceId>)>>>;

fn settings() -> PhysicsSettings {
    PhysicsSettings {
        surfaces: Some(SurfaceTable::new(
            vec!["concrete".into(), "metal_grate".into(), "carpet".into()],
            None,
        )),
        ..PhysicsSettings::new(PhysicsSpace::Free)
    }
}

fn stepped() -> Schedule {
    let mut schedule = Schedule::new();
    install::<TestPose, NoParent>(&mut schedule, settings());
    schedule
}

fn run(world: &mut World, schedule: &mut Schedule, frames: std::ops::Range<u64>) {
    for frame in frames {
        tick(world, schedule, frame);
    }
}

fn physics(world: &World) -> &PhysicsWorld {
    world.resource::<PhysicsWorld>().unwrap()
}

fn rejections(world: &mut World) -> Vec<ShapeRejection> {
    world
        .resource_mut::<PhysicsWorld>()
        .unwrap()
        .take_rejections()
}

fn part(index: u32, surface: &str) -> PartSurface {
    PartSurface {
        part: SurfacePart::Index(index),
        surface: surface.to_string(),
    }
}

fn slab(x: f32) -> CompoundPart {
    CompoundPart {
        shape: Shape::Box {
            half_extents: [2.0, 0.5, 2.0],
        },
        translation: [x, 0.0, 0.0],
        rotation: [0.0; 3],
    }
}

/// A static compound floor, `x` in `[0, 8]` with the seam at `x = 4` (part 0 left, part 1 right), top
/// at `y = 0`, tagged `concrete` with `parts`.
fn compound_floor(world: &mut World, parts: Vec<PartSurface>) -> EntityId {
    let entity = world.spawn();
    world
        .insert(entity, TestPose::at([4.0, -0.5, 0.0]))
        .unwrap();
    world
        .insert(
            entity,
            Collider {
                shape: Shape::Compound {
                    parts: vec![slab(-2.0), slab(2.0)],
                },
                body: Body::new_static(),
                layer: None,
                sensor: false,
            },
        )
        .unwrap();
    world
        .insert(
            entity,
            SurfaceTag {
                surface: "concrete".to_string(),
                parts,
                cells: None,
            },
        )
        .unwrap();
    entity
}

fn down(physics: &PhysicsWorld, x: f32) -> ludion_physics::QueryHit {
    physics
        .ray_cast([x, 2.0, 0.0], [0.0, -1.0, 0.0], 5.0, &QueryFilter::new())
        .unwrap()
        .remove(0)
}

#[test]
fn a_motor_walking_across_the_parts_reports_concrete_then_grate() {
    let mut world = World::new();
    compound_floor(&mut world, vec![part(1, "metal_grate")]);
    let hero = world.spawn();
    world.insert(hero, TestPose::at([0.5, 0.0, 0.0])).unwrap();
    world
        .insert(
            hero,
            CharacterMotor {
                radius: 0.28,
                height: 0.9,
                slope_limit_deg: 45.0,
                step_height: 0.25,
                skin_width: 0.01,
                ..CharacterMotor::default()
            },
        )
        .unwrap();
    let log: SurfaceLog = Arc::default();
    let sink = Arc::clone(&log);
    let mut schedule = Schedule::new();
    schedule.add_system_shared(
        "walk",
        Arc::new(move |world: &mut World, _: &SystemCtx| {
            if let Ok(report) = move_character::<TestPose, NoParent>(world, hero, [0.1, -0.1, 0.0])
            {
                let x = pose_of(world, hero).translation[0];
                sink.lock()
                    .unwrap()
                    .push((x, report.ground.map(|g| g.surface).unwrap_or(None)));
            }
        }),
    );
    install::<TestPose, NoParent>(&mut schedule, settings());
    run(&mut world, &mut schedule, 0..70);

    let log = log.lock().unwrap();
    let mut seen = Vec::new();
    for &(x, surface) in log.iter() {
        // At the seam the capsule's footprint reaches both parts: skip, the rest must agree.
        if (x - 4.0).abs() < 0.4 {
            continue;
        }
        let want = if x < 4.0 { CONCRETE } else { GRATE };
        assert_eq!(surface, Some(want), "at x = {x}");
        if seen.last() != Some(&surface) {
            seen.push(surface);
        }
    }
    assert_eq!(seen, [Some(CONCRETE), Some(GRATE)]);
}

#[test]
fn every_query_reports_the_nearest_part() {
    let mut world = World::new();
    let ground = compound_floor(&mut world, vec![part(1, "metal_grate")]);
    let mut schedule = stepped();
    run(&mut world, &mut schedule, 0..2);
    let physics = physics(&world);
    let filter = QueryFilter::new();

    assert_eq!(down(physics, 1.5).surface, Some(CONCRETE));
    assert_eq!(down(physics, 6.5).surface, Some(GRATE));
    for (x, want) in [(1.5, CONCRETE), (6.5, GRATE)] {
        let cast = physics
            .shape_cast(
                &Shape::Sphere { radius: 0.1 },
                [x, 2.0, 0.0],
                IDENTITY,
                [0.0, -1.0, 0.0],
                5.0,
                &filter,
            )
            .unwrap();
        assert_eq!(cast[0].entity, ground);
        assert_eq!(cast[0].surface, Some(want), "shape cast at {x}");
        let closest = physics.closest_points([x, 0.3, 0.0], 1.0, &filter).unwrap();
        assert_eq!(closest[0].surface, Some(want), "closest point at {x}");
    }
    assert_eq!(
        physics.surface_of(ground),
        Some(CONCRETE),
        "contacts keep the tag"
    );
}

#[test]
fn a_hit_exactly_on_the_seam_reports_the_lowest_part() {
    let mut world = World::new();
    compound_floor(&mut world, vec![part(0, "metal_grate"), part(1, "carpet")]);
    let mut schedule = stepped();
    run(&mut world, &mut schedule, 0..2);
    assert_eq!(down(physics(&world), 4.0).surface, Some(GRATE));
}

#[test]
fn an_overlay_over_a_part_still_wins() {
    let mut world = World::new();
    compound_floor(&mut world, vec![part(1, "metal_grate")]);
    let patch = world.spawn();
    world.insert(patch, TestPose::at([6.0, 0.0, 0.0])).unwrap();
    world
        .insert(
            patch,
            Collider {
                shape: Shape::Cylinder {
                    half_height: 0.05,
                    radius: 1.0,
                },
                body: Body::new_static(),
                layer: None,
                sensor: true,
            },
        )
        .unwrap();
    world
        .insert(
            patch,
            SurfaceOverlay {
                surface: "carpet".to_string(),
                priority: 0,
            },
        )
        .unwrap();
    let mut schedule = stepped();
    run(&mut world, &mut schedule, 0..2);
    assert_eq!(down(physics(&world), 6.0).surface, Some(CARPET));
    assert_eq!(down(physics(&world), 7.5).surface, Some(GRATE));
}

#[test]
fn an_out_of_range_index_and_a_non_compound_collider_report_the_entity_surface() {
    let mut world = World::new();
    compound_floor(&mut world, vec![part(5, "metal_grate")]);
    let plain = world.spawn();
    world
        .insert(plain, TestPose::at([20.0, -0.5, 0.0]))
        .unwrap();
    world
        .insert(
            plain,
            Collider {
                shape: Shape::Box {
                    half_extents: [2.0, 0.5, 2.0],
                },
                body: Body::new_static(),
                layer: None,
                sensor: false,
            },
        )
        .unwrap();
    world
        .insert(
            plain,
            SurfaceTag {
                surface: "concrete".to_string(),
                parts: vec![part(0, "metal_grate")],
                cells: None,
            },
        )
        .unwrap();
    let mut schedule = stepped();
    run(&mut world, &mut schedule, 0..2);
    assert_eq!(down(physics(&world), 1.5).surface, Some(CONCRETE));
    assert_eq!(down(physics(&world), 6.5).surface, Some(CONCRETE));
    assert_eq!(down(physics(&world), 20.0).surface, Some(CONCRETE));
    assert!(rejections(&mut world).is_empty());
}

#[test]
fn an_undeclared_part_surface_rejects_once_and_that_part_reports_the_entity_surface() {
    let mut world = World::new();
    let ground = compound_floor(&mut world, vec![part(1, "metal_grat")]);
    let mut schedule = stepped();
    run(&mut world, &mut schedule, 0..4);
    let found = rejections(&mut world);
    assert_eq!(found.len(), 1, "one L1690 for a standing fault: {found:?}");
    assert_eq!(found[0].code, code::SURFACE_UNKNOWN);
    assert_eq!(found[0].entity, ground);
    assert!(
        found[0].reason.contains("SurfaceTag part"),
        "{}",
        found[0].reason
    );
    assert!(
        found[0].reason.contains("metal_grat"),
        "{}",
        found[0].reason
    );
    assert_eq!(down(physics(&world), 6.5).surface, Some(CONCRETE));
}

#[test]
fn a_parts_only_edit_takes_effect_from_the_next_write() {
    let mut world = World::new();
    let ground = compound_floor(&mut world, vec![part(1, "metal_grate")]);
    let mut schedule = stepped();
    run(&mut world, &mut schedule, 0..2);
    assert_eq!(down(physics(&world), 6.5).surface, Some(GRATE));

    world.get_mut::<SurfaceTag>(ground).unwrap().unwrap().parts[0].surface = "carpet".to_string();
    run(&mut world, &mut schedule, 2..3);
    assert_eq!(down(physics(&world), 6.5).surface, Some(CARPET));

    world
        .get_mut::<SurfaceTag>(ground)
        .unwrap()
        .unwrap()
        .parts
        .clear();
    run(&mut world, &mut schedule, 3..4);
    assert_eq!(down(physics(&world), 6.5).surface, Some(CONCRETE));
}

#[test]
fn a_restore_answers_parts_before_any_write() {
    let mut world = World::new();
    compound_floor(&mut world, vec![part(1, "metal_grate")]);
    let mut schedule = stepped();
    run(&mut world, &mut schedule, 0..2);
    let blob = physics(&world).to_bytes().unwrap();
    let mut restored = PhysicsWorld::from_bytes(&blob, TickRate::DEFAULT).unwrap();
    restored.sync_surface_tags(&world);
    restored.apply_settings(&settings());
    let hit = restored
        .ray_cast([6.5, 2.0, 0.0], [0.0, -1.0, 0.0], 5.0, &QueryFilter::new())
        .unwrap();
    assert_eq!(hit[0].surface, Some(GRATE));
    assert!(restored.take_rejections().is_empty());
}

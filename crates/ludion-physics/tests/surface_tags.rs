//! `SurfaceTag` and `physics.surfaces`: every hit that names a collider reports the collider's
//! surface (tag, else the scene default), the motor's ground and wall hits included; a live edit
//! applies from the next write without a rebuild; an undeclared name degrades to the default with
//! one `L1690`; a scene with no surfaces reports `None`; a restore answers before any write.

mod common;

use std::sync::{Arc, Mutex};

use common::{TestPose, pose_of, tick};
use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_core::time::TickRate;
use ludion_ecs::{Schedule, SystemCtx, World};
use ludion_physics::{
    CharacterMotor, NoParent, PhysicsSettings, PhysicsSpace, PhysicsWorld, QueryFilter,
    ShapeRejection, SurfaceId, SurfaceTable, SurfaceTag, install, move_character,
    probe_character_ground,
};
use ludion_spatial::{Body, Collider, Shape};

const IDENTITY: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const CONCRETE: SurfaceId = SurfaceId(0);
const GRATE: SurfaceId = SurfaceId(1);
const DIRT: SurfaceId = SurfaceId(2);

fn table() -> SurfaceTable {
    SurfaceTable::new(
        vec!["concrete".into(), "metal_grate".into(), "dirt".into()],
        Some(DIRT),
    )
}

fn settings(surfaces: Option<SurfaceTable>) -> PhysicsSettings {
    PhysicsSettings {
        surfaces,
        ..PhysicsSettings::new(PhysicsSpace::Free)
    }
}

fn tag(name: &str) -> SurfaceTag {
    SurfaceTag {
        surface: name.to_string(),
    }
}

/// A static box centred at `at` with `half` extents, optionally tagged.
fn spawn_box(world: &mut World, at: [f32; 3], half: [f32; 3], tagged: Option<&str>) -> EntityId {
    let entity = world.spawn();
    world.insert(entity, TestPose::at(at)).unwrap();
    world
        .insert(
            entity,
            Collider {
                shape: Shape::Box { half_extents: half },
                body: Body::new_static(),
                layer: None,
                sensor: false,
            },
        )
        .unwrap();
    if let Some(name) = tagged {
        world.insert(entity, tag(name)).unwrap();
    }
    entity
}

/// Three floor strips along +x, tops at `y = 0`: concrete `[0, 2]`, grate `[2, 4]`, untagged `[4, 6]`.
fn strips(world: &mut World) -> [EntityId; 3] {
    let half = [1.0, 0.5, 2.0];
    [
        spawn_box(world, [1.0, -0.5, 0.0], half, Some("concrete")),
        spawn_box(world, [3.0, -0.5, 0.0], half, Some("metal_grate")),
        spawn_box(world, [5.0, -0.5, 0.0], half, None),
    ]
}

fn motor(world: &mut World, at: [f32; 3]) -> EntityId {
    let hero = world.spawn();
    world.insert(hero, TestPose::at(at)).unwrap();
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
    hero
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

fn run(world: &mut World, schedule: &mut Schedule, frames: std::ops::Range<u64>) {
    for frame in frames {
        tick(world, schedule, frame);
    }
}

/// A schedule that only steps physics under `settings`.
fn stepped(settings: PhysicsSettings) -> Schedule {
    let mut schedule = Schedule::new();
    install::<TestPose, NoParent>(&mut schedule, settings);
    schedule
}

/// The surface each strip should report.
fn expected(x: f32) -> SurfaceId {
    match x {
        x if x < 2.0 => CONCRETE,
        x if x < 4.0 => GRATE,
        _ => DIRT,
    }
}

#[test]
fn a_motor_walking_the_strips_reports_each_surface() {
    let mut world = World::new();
    strips(&mut world);
    let hero = motor(&mut world, [0.5, 0.0, 0.0]);
    let log: Arc<Mutex<Vec<(f32, Option<SurfaceId>)>>> = Arc::default();
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
    install::<TestPose, NoParent>(&mut schedule, settings(Some(table())));
    run(&mut world, &mut schedule, 0..50);

    let log = log.lock().unwrap();
    let mut seen = Vec::new();
    for &(x, surface) in log.iter() {
        // Near a seam the capsule's footprint reaches both strips: skip, the rest must agree.
        if [2.0, 4.0].iter().any(|seam| (x - seam).abs() < 0.4) {
            continue;
        }
        assert_eq!(surface, Some(expected(x)), "at x = {x}");
        if seen.last() != Some(&surface) {
            seen.push(surface);
        }
    }
    assert_eq!(seen, [Some(CONCRETE), Some(GRATE), Some(DIRT)]);
}

#[test]
fn queries_probe_and_walls_agree() {
    let mut world = World::new();
    let [concrete, grate, plain] = strips(&mut world);
    let wall = spawn_box(
        &mut world,
        [8.0, 1.0, 0.0],
        [0.5, 1.0, 2.0],
        Some("concrete"),
    );
    let hero = motor(&mut world, [7.0, 0.0, 0.0]);
    let mut schedule = stepped(settings(Some(table())));
    run(&mut world, &mut schedule, 0..2);
    let physics = physics(&world);

    for (entity, x, want) in [
        (concrete, 1.0, CONCRETE),
        (grate, 3.0, GRATE),
        (plain, 5.0, DIRT),
    ] {
        let filter = QueryFilter::new();
        let ray = physics
            .ray_cast([x, 2.0, 0.0], [0.0, -1.0, 0.0], 5.0, &filter)
            .unwrap();
        assert_eq!(ray[0].entity, entity);
        assert_eq!(ray[0].surface, Some(want));
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
        assert_eq!(cast[0].surface, Some(want));
        let closest = physics.closest_points([x, 0.5, 0.0], 1.0, &filter).unwrap();
        assert_eq!(closest[0].entity, entity);
        assert_eq!(closest[0].surface, Some(want));
        assert_eq!(physics.surface_of(entity), Some(want));
    }

    let probe = probe_character_ground::<TestPose>(&world, hero, [5.0, 0.0, 0.0])
        .unwrap()
        .expect("ground under the probe");
    assert_eq!(probe.ground.surface, Some(DIRT));

    let mut world = world;
    let mut schedule = Schedule::new();
    schedule.add_system_shared(
        "push",
        Arc::new(move |world: &mut World, _: &SystemCtx| {
            let _ = move_character::<TestPose, NoParent>(world, hero, [0.3, -0.1, 0.0]);
        }),
    );
    install::<TestPose, NoParent>(&mut schedule, settings(Some(table())));
    let mut blocked = None;
    for frame in 2..40 {
        tick(&mut world, &mut schedule, frame);
        let report = move_character::<TestPose, NoParent>(&mut world, hero, [0.3, -0.1, 0.0])
            .expect("a report");
        if let Some(hit) = report.walls.first() {
            assert_eq!(hit.entity, wall);
            blocked = Some(hit.surface);
            break;
        }
    }
    assert_eq!(
        blocked,
        Some(Some(CONCRETE)),
        "the wall hit carries its tag"
    );
}

#[test]
fn a_live_edit_applies_from_the_next_write_without_a_rebuild() {
    let mut world = World::new();
    let [concrete, ..] = strips(&mut world);
    let mut schedule = stepped(settings(Some(table())));
    run(&mut world, &mut schedule, 0..2);
    let bodies = physics(&world).body_count();
    assert_eq!(physics(&world).surface_of(concrete), Some(CONCRETE));

    *world.get_mut::<SurfaceTag>(concrete).unwrap().unwrap() = tag("metal_grate");
    run(&mut world, &mut schedule, 2..3);
    assert_eq!(physics(&world).surface_of(concrete), Some(GRATE));
    assert_eq!(physics(&world).body_count(), bodies);

    world.remove::<SurfaceTag>(concrete).unwrap();
    run(&mut world, &mut schedule, 3..4);
    assert_eq!(
        physics(&world).surface_of(concrete),
        Some(DIRT),
        "untagged falls to the default"
    );
    assert_eq!(physics(&world).body_count(), bodies);
}

#[test]
fn an_undeclared_name_reports_the_default_and_rejects_once() {
    let mut world = World::new();
    let strip = spawn_box(
        &mut world,
        [0.0, -0.5, 0.0],
        [1.0, 0.5, 1.0],
        Some("metal_grat"),
    );
    let mut schedule = stepped(settings(Some(table())));
    run(&mut world, &mut schedule, 0..4);
    let found = rejections(&mut world);
    assert_eq!(found.len(), 1, "one L1690 for a standing fault: {found:?}");
    assert_eq!(found[0].code, code::SURFACE_UNKNOWN);
    assert_eq!(found[0].entity, strip);
    assert!(
        found[0].reason.contains("metal_grate"),
        "{}",
        found[0].reason
    );
    assert_eq!(physics(&world).body_count(), 1, "the body is kept");
    let hit = physics(&world)
        .ray_cast([0.0, 2.0, 0.0], [0.0, -1.0, 0.0], 5.0, &QueryFilter::new())
        .unwrap();
    assert_eq!(hit[0].surface, Some(DIRT));

    *world.get_mut::<SurfaceTag>(strip).unwrap().unwrap() = tag("metal_grate");
    run(&mut world, &mut schedule, 4..5);
    assert!(rejections(&mut world).is_empty(), "the fix reports nothing");
    assert_eq!(physics(&world).surface_of(strip), Some(GRATE));
}

#[test]
fn a_scene_without_surfaces_reports_none_everywhere() {
    let mut world = World::new();
    let [concrete, ..] = strips(&mut world);
    let hero = motor(&mut world, [0.5, 0.0, 0.0]);
    let mut schedule = stepped(settings(None));
    run(&mut world, &mut schedule, 0..2);
    let physics = physics(&world);
    assert_eq!(physics.surface_of(concrete), None);
    assert_eq!(physics.surface_id("concrete"), None);
    let hit = physics
        .ray_cast([1.0, 2.0, 0.0], [0.0, -1.0, 0.0], 5.0, &QueryFilter::new())
        .unwrap();
    assert_eq!(hit[0].surface, None);
    let probe = probe_character_ground::<TestPose>(&world, hero, [0.5, 0.0, 0.0])
        .unwrap()
        .expect("ground");
    assert_eq!(probe.ground.surface, None);
}

#[test]
fn a_restore_answers_before_any_write() {
    let mut world = World::new();
    let [concrete, ..] = strips(&mut world);
    let mut schedule = stepped(settings(Some(table())));
    run(&mut world, &mut schedule, 0..2);
    let blob = physics(&world).to_bytes().unwrap();
    let mut restored = PhysicsWorld::from_bytes(&blob, TickRate::DEFAULT).unwrap();
    restored.sync_surface_tags(&world);
    restored.apply_settings(&settings(Some(table())));
    assert_eq!(restored.surface_of(concrete), Some(CONCRETE));
    assert_eq!(restored.surface_name(CONCRETE), Some("concrete"));
    let hit = restored
        .ray_cast([1.0, 2.0, 0.0], [0.0, -1.0, 0.0], 5.0, &QueryFilter::new())
        .unwrap();
    assert_eq!(hit[0].surface, Some(CONCRETE));
    assert!(restored.take_rejections().is_empty());
}

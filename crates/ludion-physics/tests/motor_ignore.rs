//! `IgnoreCollisions` and `character_fit_ignoring`: a motor looks through one partner, sees
//! everything else, and without the component every result is bit-identical.

mod common;

use std::sync::{Arc, Mutex};

use common::{TestPose, pose_of, tick};
use ludion_core::ids::EntityId;
use ludion_ecs::{Schedule, SystemCtx, World};
use ludion_physics::{
    CharacterMotor, IgnoreCollisions, NoParent, PhysicsSpace, PhysicsWorld, character_fit,
    character_fit_ignoring, install, move_character,
};
use ludion_spatial::{Body, Collider, Shape};

const FLAT: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const FALL: [f32; 3] = [0.0, -0.1, 0.0];
const WALL_X: f32 = 5.0;

fn motor() -> CharacterMotor {
    CharacterMotor {
        radius: 0.28,
        height: 0.9,
        slope_limit_deg: 45.0,
        step_height: 0.25,
        skin_width: 0.01,
        ..CharacterMotor::default()
    }
}

fn spawn_box(world: &mut World, center: [f32; 3], half: [f32; 3]) -> EntityId {
    let entity = world.spawn();
    world
        .insert(
            entity,
            TestPose {
                translation: center,
                rotation: FLAT,
            },
        )
        .unwrap();
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
    entity
}

fn hero(world: &mut World, feet: [f32; 3]) -> EntityId {
    let entity = world.spawn();
    world.insert(entity, TestPose::at(feet)).unwrap();
    world.insert(entity, motor()).unwrap();
    entity
}

/// A floor, a wall at `WALL_X`, and two motors `gap` metres apart on x.
fn scene(gap: f32) -> (World, EntityId, EntityId, EntityId) {
    let mut world = World::new();
    spawn_box(&mut world, [0.0, -0.5, 0.0], [30.0, 0.5, 30.0]);
    let wall = spawn_box(&mut world, [WALL_X, 1.0, 0.0], [0.5, 1.0, 3.0]);
    let a = hero(&mut world, [0.0, 0.0, 0.0]);
    let b = hero(&mut world, [gap, 0.0, 0.0]);
    (world, a, b, wall)
}

fn rig(
    world: World,
    script: impl Fn(&mut World, u64) + Send + Sync + 'static,
) -> (World, Schedule) {
    let mut schedule = Schedule::new();
    schedule.add_system_shared(
        "test-script",
        Arc::new(move |world: &mut World, ctx: &SystemCtx| script(world, ctx.frame.as_u64())),
    );
    install::<TestPose, NoParent>(&mut schedule, PhysicsSpace::Free);
    (world, schedule)
}

fn run(world: &mut World, schedule: &mut Schedule, ticks: u64) {
    for frame in 0..ticks {
        tick(world, schedule, frame);
    }
}

#[test]
fn a_fit_at_the_partners_spot_reports_it_until_ignored() {
    let (world, a, b, wall) = scene(0.5);
    let (mut world, mut schedule) = rig(world, |_, _| {});
    run(&mut world, &mut schedule, 5);
    let spot = [0.5, 0.0, 0.0];
    assert_eq!(
        character_fit::<TestPose>(&world, a, None, spot, 0.0),
        Ok(Some(b))
    );
    assert_eq!(
        character_fit_ignoring::<TestPose>(&world, a, None, spot, 0.0, Some(&[b])),
        Ok(None)
    );
    assert_eq!(
        character_fit_ignoring::<TestPose>(&world, a, None, [WALL_X, 0.0, 0.0], 0.0, Some(&[b])),
        Ok(Some(wall)),
        "ignoring the partner does not blind the test to a wall"
    );

    world.insert(a, IgnoreCollisions { with: vec![b] }).unwrap();
    assert_eq!(
        character_fit::<TestPose>(&world, a, None, spot, 0.0),
        Ok(None),
        "the component's partner applies when no explicit one is given"
    );
    assert_eq!(
        character_fit_ignoring::<TestPose>(&world, a, None, spot, 0.0, Some(&[wall])),
        Ok(Some(b)),
        "an explicit ignore replaces the component's partner for this test"
    );
}

/// A's `moved` and final translation after a +x move of 0.6 toward B, 1 m away.
fn walk_toward_partner(
    ignore: Option<fn(EntityId, EntityId) -> IgnoreCollisions>,
) -> ([f32; 3], [f32; 3]) {
    let (mut world, a, b, _) = scene(1.0);
    if let Some(make) = ignore {
        world.insert(a, make(a, b)).unwrap();
    }
    let moved = Arc::new(Mutex::new([0.0; 3]));
    let sink = moved.clone();
    let (mut world, mut schedule) = rig(world, move |world, frame| {
        let delta = if frame == 8 { [0.6, 0.0, 0.0] } else { FALL };
        let Ok(report) = move_character::<TestPose, NoParent>(world, a, delta) else {
            return;
        };
        if frame == 8 {
            *sink.lock().unwrap() = report.moved;
        }
    });
    run(&mut world, &mut schedule, 10);
    let moved = *moved.lock().unwrap();
    (moved, pose_of(&world, a).translation)
}

#[test]
fn a_move_stops_at_the_partner_without_the_component_and_passes_with_it() {
    let (stopped, _) = walk_toward_partner(None);
    assert!(stopped[0] < 0.5, "the capsules collide: {stopped:?}");
    let (passed, end) = walk_toward_partner(Some(|_, b| IgnoreCollisions { with: vec![b] }));
    assert!(passed[0] > 0.59, "the partner is ignored: {passed:?}");
    assert!(end[0] > 0.59, "{end:?}");
}

#[test]
fn an_unrelated_partner_changes_no_bit_of_a_move() {
    let plain = walk_toward_partner(None);
    let unrelated = walk_toward_partner(Some(|a, _| IgnoreCollisions { with: vec![a] }));
    assert_eq!(plain.0.map(f32::to_bits), unrelated.0.map(f32::to_bits));
    assert_eq!(plain.1.map(f32::to_bits), unrelated.1.map(f32::to_bits));
}

#[test]
fn the_component_does_not_enter_the_physics_blob() {
    let blob = |with_component: bool| {
        let (mut world, a, b, _) = scene(1.0);
        if with_component {
            world.insert(a, IgnoreCollisions { with: vec![b] }).unwrap();
        }
        let (mut world, mut schedule) = rig(world, |_, _| {});
        run(&mut world, &mut schedule, 5);
        world
            .resource::<PhysicsWorld>()
            .unwrap()
            .to_bytes()
            .unwrap()
    };
    assert_eq!(blob(false), blob(true));
}

#[test]
fn reading_the_partner_records_no_query_access() {
    use std::any::TypeId;

    let (mut world, a, b, _) = scene(0.5);
    world.insert(a, IgnoreCollisions { with: vec![b] }).unwrap();
    let (mut world, mut schedule) = rig(world, |_, _| {});
    run(&mut world, &mut schedule, 5);
    world.begin_query_recording();
    world.set_recording_system("game");
    assert_eq!(
        character_fit::<TestPose>(&world, a, None, [0.5, 0.0, 0.0], 0.0),
        Ok(None)
    );
    let observed = world.take_query_recording().unwrap();
    let ignore = TypeId::of::<IgnoreCollisions>();
    assert!(
        observed
            .shapes("game")
            .all(|shape| shape.iter().all(|(id, _)| *id != ignore)),
        "a game must not have to declare IgnoreCollisions"
    );
}

#[test]
fn more_than_four_entries_are_l1759() {
    use ludion_ecs::SemanticValidate;
    let entry = serde_json::json!({ "index": 1, "generation": 0 });
    let four = serde_json::json!({ "with": [entry, entry, entry, entry] });
    assert!(IgnoreCollisions::validate_semantics(&four).is_empty());
    let five = serde_json::json!({ "with": [entry, entry, entry, entry, entry] });
    let found = IgnoreCollisions::validate_semantics(&five);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code.as_str(), "L1759");
    assert_eq!(found[0].path.as_deref(), Some("/with"));
}

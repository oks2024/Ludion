//! The lying posture: a horizontal capsule along the facing that yaws with the pose. Going prone in
//! the open and under a low box, the wall and pillar a lying turn must clear, crawling (walls, lips,
//! slopes), a yawing platform, teleport, resume and resize, the batch mover, thread-count invariance
//! and the upright regression. A game system registered before `install` drives the hero.

mod common;

use std::sync::{Arc, Mutex};

use common::{TestPose, pose_of, tick};
use ludion_core::ids::EntityId;
use ludion_core::math::{atan2, sin_cos};
use ludion_core::par::with_threads;
use ludion_ecs::{Schedule, SystemCtx, World};
use ludion_physics::{
    CharacterMotor, CharacterMotorState, MotorError, NoParent, PhysicsSpace, PhysicsWorld, Posture,
    PostureShape, QueryFilter, character_fit, install, move_character, move_characters,
    resize_character, resume_character, set_character_posture, suspend_character,
    teleport_character, turn_character,
};
use ludion_spatial::{Body, Collider, Shape};

const FLAT: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const FALL: [f32; 3] = [0.0, -0.1, 0.0];
const STEP_DEGREES: f32 = 15.0;

fn posture(shape: PostureShape) -> Posture {
    Posture {
        shape,
        step_height: None,
        slope_limit_deg: None,
        snap_distance: None,
    }
}

fn motor() -> CharacterMotor {
    let mut motor = CharacterMotor {
        radius: 0.3,
        height: 1.75,
        ..CharacterMotor::default()
    };
    motor.postures.insert(
        "prone".into(),
        Posture {
            step_height: Some(0.06),
            slope_limit_deg: Some(30.0),
            ..posture(PostureShape::Lying {
                length: 1.7,
                radius: 0.17,
                offset: 0.35,
            })
        },
    );
    motor.postures.insert(
        "crouch".into(),
        posture(PostureShape::Upright {
            height: 1.1,
            radius: None,
        }),
    );
    motor
}

fn yaw_quat(yaw: f32) -> [f32; 4] {
    let (s, c) = sin_cos(yaw / 2.0);
    [0.0, s, 0.0, c]
}

fn yaw_of(q: [f32; 4]) -> f32 {
    2.0 * atan2(q[1], q[3])
}

fn forward(yaw: f32) -> [f32; 3] {
    let (s, c) = sin_cos(yaw);
    [-s, 0.0, -c]
}

fn spawn_box_with(
    world: &mut World,
    center: [f32; 3],
    half: [f32; 3],
    rotation: [f32; 4],
    body: Body,
) -> EntityId {
    let entity = world.spawn();
    world
        .insert(
            entity,
            TestPose {
                translation: center,
                rotation,
            },
        )
        .unwrap();
    world
        .insert(
            entity,
            Collider {
                shape: Shape::Box { half_extents: half },
                body,
                layer: None,
                sensor: false,
            },
        )
        .unwrap();
    entity
}

fn spawn_box(world: &mut World, center: [f32; 3], half: [f32; 3]) -> EntityId {
    spawn_box_with(world, center, half, FLAT, Body::new_static())
}

fn floor(world: &mut World) -> EntityId {
    spawn_box(world, [0.0, -0.5, 0.0], [30.0, 0.5, 30.0])
}

/// A box over `z` in `-1.5..1.5` and all `x` near the origin, whose underside is 0.45 above the floor.
fn truck(world: &mut World, x: f32) -> EntityId {
    spawn_box(world, [x, 0.7, 0.0], [2.0, 0.25, 1.5])
}

fn hero(world: &mut World, feet: [f32; 3], yaw: f32, held: Option<&str>) -> EntityId {
    let entity = world.spawn();
    world
        .insert(
            entity,
            TestPose {
                rotation: yaw_quat(yaw),
                ..TestPose::at(feet)
            },
        )
        .unwrap();
    world.insert(entity, motor()).unwrap();
    if let Some(held) = held {
        world
            .insert(
                entity,
                CharacterMotorState {
                    posture: Some(held.into()),
                    ..CharacterMotorState::default()
                },
            )
            .unwrap();
    }
    entity
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

fn state(world: &World, entity: EntityId) -> CharacterMotorState {
    world
        .get::<CharacterMotorState>(entity)
        .unwrap()
        .cloned()
        .unwrap_or_default()
}

fn yaw(world: &World, entity: EntityId) -> f32 {
    yaw_of(pose_of(world, entity).rotation)
}

/// The nearest ray hit that is `of`, or `None`.
fn ray(world: &World, origin: [f32; 3], direction: [f32; 3], of: EntityId) -> Option<f32> {
    world
        .resource::<PhysicsWorld>()?
        .ray_cast(origin, direction, 10.0, &QueryFilter::new())
        .ok()?
        .into_iter()
        .find(|hit| hit.entity == of)
        .map(|hit| hit.distance)
}

type Shared<T> = Arc<Mutex<Vec<T>>>;

fn crawl(yaw: f32, speed: f32) -> [f32; 3] {
    let ahead = forward(yaw);
    [ahead[0] * speed, -0.1, ahead[2] * speed]
}

#[test]
fn going_prone_in_the_open_lays_the_body_along_the_facing() {
    let mut world = World::new();
    floor(&mut world);
    let hero = hero(&mut world, [0.0, 0.0, 0.0], 0.0, None);
    let log: Shared<(Option<f32>, Option<f32>)> = Arc::default();
    let sink = log.clone();
    let (mut world, mut schedule) = rig(world, move |world, frame| {
        if frame == 5 {
            let report = set_character_posture::<TestPose>(world, hero, Some("prone")).unwrap();
            assert_eq!(report.posture.as_deref(), Some("prone"));
            assert_eq!(report.blocked_by, None);
        }
        if frame == 12 {
            sink.lock().unwrap().push((
                ray(world, [0.0, 0.25, -3.0], [0.0, 0.0, 1.0], hero),
                ray(world, [-3.0, 0.8, 0.0], [1.0, 0.0, 0.0], hero),
            ));
        }
        let _ = move_character::<TestPose, NoParent>(world, hero, FALL);
    });
    run(&mut world, &mut schedule, 15);
    let (along, over) = log.lock().unwrap()[0];
    let along = along.expect("a ray at 0.25 m along the facing hits the body");
    assert!((along - 1.8).abs() < 0.05, "the tip is 1.2 ahead: {along}");
    assert_eq!(over, None, "a ray at 0.8 m passes over a body 0.34 m tall");
}

#[test]
fn a_stand_under_a_low_box_is_refused_until_the_body_has_crawled_out() {
    let mut world = World::new();
    floor(&mut world);
    let truck = truck(&mut world, 0.0);
    let hero = hero(&mut world, [0.0, 0.0, 0.0], 0.0, Some("prone"));
    let log: Shared<(u64, &'static str, Option<EntityId>)> = Arc::default();
    let sink = log.clone();
    let (mut world, mut schedule) = rig(world, move |world, frame| {
        let note = |what, blocked| sink.lock().unwrap().push((frame, what, blocked));
        if frame == 10 {
            let r = set_character_posture::<TestPose>(world, hero, None).unwrap();
            note("stand", r.blocked_by);
            let r = set_character_posture::<TestPose>(world, hero, Some("crouch")).unwrap();
            note("crouch", r.blocked_by);
        }
        if frame == 90 {
            let r = set_character_posture::<TestPose>(world, hero, None).unwrap();
            note("stand-out", r.blocked_by);
        }
        let step = if (11..90).contains(&frame) {
            crawl(0.0, 0.05)
        } else {
            FALL
        };
        let _ = move_character::<TestPose, NoParent>(world, hero, step);
    });
    run(&mut world, &mut schedule, 95);
    assert_eq!(
        *log.lock().unwrap(),
        [
            (10, "stand", Some(truck)),
            (10, "crouch", Some(truck)),
            (90, "stand-out", None)
        ]
    );
    assert_eq!(state(&world, hero).posture, None);
    assert!(
        pose_of(&world, hero).translation[2] < -2.0,
        "it crawled out"
    );
}

#[test]
fn a_lying_body_passes_under_a_box_a_standing_one_cannot() {
    let mut world = World::new();
    floor(&mut world);
    truck(&mut world, 0.0);
    let prone = hero(&mut world, [0.0, 0.0, 3.5], 0.0, Some("prone"));
    let upright = hero(&mut world, [4.0, 0.0, 3.5], 0.0, None);
    let (mut world, mut schedule) = rig(world, move |world, _| {
        let _ = move_character::<TestPose, NoParent>(world, prone, crawl(0.0, 0.05));
        let _ = move_character::<TestPose, NoParent>(world, upright, crawl(0.0, 0.05));
    });
    run(&mut world, &mut schedule, 120);
    assert!(
        pose_of(&world, prone).translation[2] < -1.6,
        "the prone body crawled under the 0.45 m clearance"
    );
    assert!(
        pose_of(&world, upright).translation[2] > -0.5
            || pose_of(&world, upright).translation[0] > 3.0,
        "the standing one is held by the box or walks beside it"
    );
}

#[test]
fn going_prone_facing_a_near_wall_is_refused_and_a_turn_away_allows_it() {
    let mut world = World::new();
    floor(&mut world);
    let wall = spawn_box(&mut world, [0.0, 1.0, -1.1], [3.0, 1.0, 0.1]);
    let hero = hero(&mut world, [0.0, 0.0, 0.0], 0.0, None);
    let log: Shared<(Option<String>, Option<EntityId>)> = Arc::default();
    let sink = log.clone();
    let (mut world, mut schedule) = rig(world, move |world, frame| {
        let prone = |world: &mut World| {
            let r = set_character_posture::<TestPose>(world, hero, Some("prone")).unwrap();
            sink.lock().unwrap().push((r.posture, r.blocked_by));
        };
        match frame {
            5 => prone(world),
            6 => {
                let turned = turn_character::<TestPose>(world, hero, core::f32::consts::PI);
                assert_eq!(turned.unwrap().blocked_by, None, "an upright turn is free");
            }
            8 => prone(world),
            _ => {}
        }
        let _ = move_character::<TestPose, NoParent>(world, hero, FALL);
    });
    run(&mut world, &mut schedule, 12);
    assert_eq!(
        *log.lock().unwrap(),
        [(None, Some(wall)), (Some("prone".to_string()), None)]
    );
}

#[test]
fn a_lying_turn_stops_at_the_last_clear_step_and_names_the_pillar() {
    let mut world = World::new();
    floor(&mut world);
    let pillar = spawn_box(&mut world, [0.85, 1.0, -0.85], [0.1, 1.0, 0.1]);
    let hero = hero(&mut world, [0.0, 0.0, 0.0], 0.0, Some("prone"));
    let log: Shared<(f32, Option<EntityId>)> = Arc::default();
    let sink = log.clone();
    let (mut world, mut schedule) = rig(world, move |world, frame| {
        if frame == 5 {
            let report =
                turn_character::<TestPose>(world, hero, -core::f32::consts::FRAC_PI_2).unwrap();
            sink.lock().unwrap().push((report.yaw, report.blocked_by));
        }
        let _ = move_character::<TestPose, NoParent>(world, hero, FALL);
    });
    run(&mut world, &mut schedule, 8);
    let (reported, blocked_by) = log.lock().unwrap()[0];
    assert_eq!(blocked_by, Some(pillar));
    assert!(
        reported < 0.0 && reported > -core::f32::consts::FRAC_PI_2,
        "stopped partway: {reported}"
    );
    let written = yaw(&world, hero);
    assert!((written - reported).abs() < 1e-5, "{written} vs {reported}");
    let feet = pose_of(&world, hero).translation;
    assert_eq!(
        character_fit::<TestPose>(&world, hero, Some("prone"), feet, reported),
        Ok(None)
    );
    let next = reported - STEP_DEGREES.to_radians();
    assert_eq!(
        character_fit::<TestPose>(&world, hero, Some("prone"), feet, next),
        Ok(Some(pillar)),
        "the next 15 degrees is the blocked one"
    );
}

#[test]
fn an_open_lying_turn_reaches_the_target() {
    let mut world = World::new();
    floor(&mut world);
    let hero = hero(&mut world, [0.0, 0.0, 0.0], 0.0, Some("prone"));
    let log: Shared<(f32, Option<EntityId>)> = Arc::default();
    let sink = log.clone();
    let (mut world, mut schedule) = rig(world, move |world, frame| {
        if frame == 5 {
            let report = turn_character::<TestPose>(world, hero, 2.5).unwrap();
            sink.lock().unwrap().push((report.yaw, report.blocked_by));
            let again = turn_character::<TestPose>(world, hero, 2.5).unwrap();
            assert_eq!(again.blocked_by, None);
        }
        let _ = move_character::<TestPose, NoParent>(world, hero, FALL);
    });
    run(&mut world, &mut schedule, 8);
    assert_eq!(log.lock().unwrap()[0], (2.5, None));
    assert!((yaw(&world, hero) - 2.5).abs() < 1e-5);
}

#[test]
fn the_turn_errors_come_in_order() {
    let mut world = World::new();
    floor(&mut world);
    let hero = hero(&mut world, [0.0, 0.0, 0.0], 0.0, Some("prone"));
    let bare = world.spawn();
    let log: Shared<String> = Arc::default();
    let sink = log.clone();
    let (mut world, mut schedule) = rig(world, move |world, frame| {
        if frame == 5 {
            let note = |r: Result<_, MotorError>| {
                sink.lock().unwrap().push(match r {
                    Ok(_) => "ok".to_string(),
                    Err(MotorError::NoMotor(_)) => "no-motor".to_string(),
                    Err(MotorError::NoPose(_)) => "no-pose".to_string(),
                    Err(MotorError::Suspended(_)) => "suspended".to_string(),
                    Err(MotorError::NonFiniteTeleport { .. }) => "non-finite".to_string(),
                    Err(other) => format!("{other:?}"),
                });
            };
            note(turn_character::<TestPose>(world, bare, 1.0));
            note(turn_character::<TestPose>(world, hero, f32::NAN));
            suspend_character(world, hero).unwrap();
            note(turn_character::<TestPose>(world, hero, 1.0));
        }
        let _ = move_character::<TestPose, NoParent>(world, hero, FALL);
    });
    run(&mut world, &mut schedule, 8);
    assert_eq!(
        *log.lock().unwrap(),
        ["no-motor", "non-finite", "suspended"]
    );
}

#[test]
fn a_crawl_along_a_wall_never_overlaps_it() {
    let mut world = World::new();
    floor(&mut world);
    spawn_box(&mut world, [2.0, 1.0, 0.0], [0.1, 1.0, 8.0]);
    let heading = -0.3;
    let hero = hero(&mut world, [0.8, 0.0, 4.0], heading, Some("prone"));
    let log: Shared<(u64, Option<EntityId>)> = Arc::default();
    let sink = log.clone();
    let (mut world, mut schedule) = rig(world, move |world, frame| {
        let _ = move_character::<TestPose, NoParent>(world, hero, crawl(heading, 0.05));
        let feet = pose_of(world, hero).translation;
        if let Ok(fit) = character_fit::<TestPose>(world, hero, Some("prone"), feet, heading) {
            sink.lock().unwrap().push((frame, fit));
        }
    });
    run(&mut world, &mut schedule, 80);
    let log = log.lock().unwrap();
    assert!(
        log.iter().all(|row| row.1.is_none()),
        "{:?}",
        log.iter().find(|r| r.1.is_some())
    );
    let feet = pose_of(&world, hero).translation;
    assert!(feet[0] < 1.9, "held off the wall: {feet:?}");
    assert!(feet[2] < 3.0, "slid along it: {feet:?}");
}

fn crawl_across(spec: impl Fn(&mut World), held: Option<&str>, ticks: u64) -> [f32; 3] {
    let mut world = World::new();
    floor(&mut world);
    spec(&mut world);
    let hero = hero(&mut world, [0.0, 0.0, 2.0], 0.0, held);
    let (mut world, mut schedule) = rig(world, move |world, _| {
        let _ = move_character::<TestPose, NoParent>(world, hero, crawl(0.0, 0.05));
    });
    run(&mut world, &mut schedule, ticks);
    pose_of(&world, hero).translation
}

#[test]
fn a_lying_motor_steps_a_low_lip_and_refuses_a_high_one() {
    let lip = |height: f32| {
        move |world: &mut World| {
            spawn_box(world, [0.0, height / 2.0, -1.5], [3.0, height / 2.0, 0.5]);
        }
    };
    let low = crawl_across(lip(0.05), Some("prone"), 90);
    assert!(low[2] < -1.8, "the 0.05 m lip is stepped: {low:?}");
    let high = crawl_across(lip(0.2), Some("prone"), 90);
    assert!(high[2] > -1.0, "the 0.2 m lip stops it: {high:?}");
    let upright = crawl_across(lip(0.2), None, 90);
    assert!(
        upright[2] < -1.8,
        "the standing motor climbs it: {upright:?}"
    );
}

#[test]
fn a_slope_above_the_posture_limit_stops_a_lying_motor_the_base_walks_it() {
    let angle = 35.0f32.to_radians();
    let (s, c) = sin_cos(angle / 2.0);
    let ramp = move |world: &mut World| {
        // Rises toward -Z; its top plane crosses the floor at z = -1.
        spawn_box_with(
            world,
            [0.0, -0.2 / ludion_core::math::cos(angle), -1.0],
            [3.0, 0.2, 6.0],
            [s, 0.0, 0.0, c],
            Body::new_static(),
        );
    };
    let base = crawl_across(ramp, None, 120);
    assert!(base[1] > 1.0, "the base motor climbs 35 degrees: {base:?}");
    let prone = crawl_across(ramp, Some("prone"), 120);
    assert!(prone[1] < 0.3, "30 degrees is its limit: {prone:?}");
}

#[test]
fn a_yawing_platform_turns_a_lying_rider_with_it() {
    let degree = 1.0f32.to_radians();
    let mut world = World::new();
    let platform = spawn_box_with(
        &mut world,
        [0.0, -0.25, 0.0],
        [3.0, 0.25, 3.0],
        FLAT,
        Body::new_kinematic(),
    );
    let hero = hero(&mut world, [0.2, 0.3, 0.0], 0.0, Some("prone"));
    let log: Shared<f32> = Arc::default();
    let sink = log.clone();
    let (mut world, mut schedule) = rig(world, move |world, frame| {
        if let Ok(Some(pose)) = world.get_mut::<TestPose>(platform) {
            pose.rotation = yaw_quat(degree * frame as f32);
        }
        let _ = move_character::<TestPose, NoParent>(world, hero, FALL);
        if matches!(frame, 30 | 90) {
            sink.lock()
                .unwrap()
                .push(yaw_of(pose_of(world, hero).rotation));
        }
    });
    run(&mut world, &mut schedule, 91);
    let log = log.lock().unwrap();
    let turned = log[1] - log[0];
    assert!(
        (turned - degree * 60.0).abs() < 0.01,
        "rider turned {turned}, platform {}",
        degree * 60.0
    );
    assert!(state(&world, hero).grounded);
}

#[test]
fn a_teleport_restores_the_base_posture_only_where_it_fits() {
    let mut world = World::new();
    floor(&mut world);
    truck(&mut world, 0.0);
    let hero = hero(&mut world, [0.0, 0.0, 0.0], 0.0, Some("prone"));
    let log: Shared<Option<String>> = Arc::default();
    let sink = log.clone();
    let (mut world, mut schedule) = rig(world, move |world, frame| {
        if frame == 5 {
            teleport_character::<TestPose>(world, hero, [0.5, 0.0, 0.0], 0.0).unwrap();
        }
        if frame == 7 {
            sink.lock().unwrap().push(state(world, hero).posture);
            teleport_character::<TestPose>(world, hero, [10.0, 0.0, 0.0], 0.0).unwrap();
        }
        if frame == 9 {
            sink.lock().unwrap().push(state(world, hero).posture);
        }
        let _ = move_character::<TestPose, NoParent>(world, hero, FALL);
    });
    run(&mut world, &mut schedule, 12);
    assert_eq!(*log.lock().unwrap(), [Some("prone".to_string()), None]);
}

#[test]
fn resume_tests_a_lying_body_at_the_yaw_and_resize_refuses_a_posture_with_no_height() {
    let mut world = World::new();
    floor(&mut world);
    spawn_box(&mut world, [0.0, 1.0, -1.1], [3.0, 1.0, 0.1]);
    let hero = hero(&mut world, [0.0, 0.0, 0.0], 0.0, Some("prone"));
    let log: Shared<String> = Arc::default();
    let sink = log.clone();
    let (mut world, mut schedule) = rig(world, move |world, frame| {
        let note = |s: String| sink.lock().unwrap().push(s);
        if frame == 5 {
            note(format!(
                "{:?}",
                resize_character::<TestPose>(world, hero, 0.5).err()
            ));
            suspend_character(world, hero).unwrap();
            let facing = resume_character::<TestPose>(world, hero, [0.0; 3], 0.0, Some("prone"));
            note(format!("facing {}", facing.unwrap().resumed));
            let away = resume_character::<TestPose>(
                world,
                hero,
                [0.0; 3],
                core::f32::consts::PI,
                Some("prone"),
            );
            note(format!("away {}", away.unwrap().resumed));
        }
        let _ = move_character::<TestPose, NoParent>(world, hero, FALL);
    });
    run(&mut world, &mut schedule, 8);
    let log = log.lock().unwrap();
    assert!(log[0].contains("InvalidHeight"), "{}", log[0]);
    assert!(log[0].contains("lying posture has no height"), "{}", log[0]);
    assert_eq!(log[1..], ["facing false", "away true"]);
    assert!((yaw(&world, hero).abs() - core::f32::consts::PI).abs() < 1e-4);
}

fn batch_run(batched: bool) -> Vec<[u32; 3]> {
    let mut world = World::new();
    floor(&mut world);
    spawn_box(&mut world, [3.0, 1.0, -3.0], [0.1, 1.0, 3.0]);
    let heroes: Vec<EntityId> = [0.0f32, 0.9, -1.4]
        .into_iter()
        .enumerate()
        .map(|(i, yaw)| hero(&mut world, [i as f32 * 2.0, 0.0, 0.0], yaw, Some("prone")))
        .collect();
    let trace: Shared<[u32; 3]> = Arc::default();
    let sink = trace.clone();
    let list = heroes.clone();
    let (mut world, mut schedule) = rig(world, move |world, _| {
        let moves: Vec<(EntityId, [f32; 3])> = list
            .iter()
            .map(|e| (*e, crawl(yaw_of(pose_of(world, *e).rotation), 0.05)))
            .collect();
        if batched {
            let mut out = vec![Err(MotorError::NoPhysicsWorld); moves.len()];
            move_characters::<TestPose, NoParent>(world, &moves, &mut out);
        } else {
            for (entity, step) in &moves {
                let _ = move_character::<TestPose, NoParent>(world, *entity, *step);
            }
        }
        for entity in &list {
            sink.lock()
                .unwrap()
                .push(pose_of(world, *entity).translation.map(f32::to_bits));
        }
    });
    run(&mut world, &mut schedule, 40);
    trace.lock().unwrap().clone()
}

#[test]
fn move_characters_equals_serial_calls_with_lying_motors() {
    assert_eq!(batch_run(true), batch_run(false));
}

type Row = ([u32; 3], u32, Option<String>, Option<EntityId>);

fn crawl_script() -> Vec<Row> {
    let mut world = World::new();
    floor(&mut world);
    truck(&mut world, 0.0);
    spawn_box(&mut world, [4.0, 1.0, -3.0], [0.1, 1.0, 2.0]);
    let hero = hero(&mut world, [0.0, 0.0, 3.0], 0.0, None);
    let trace = Shared::<Row>::default();
    let sink = trace.clone();
    let (mut world, mut schedule) = rig(world, move |world, frame| {
        let mut blocked = None;
        match frame {
            10 => {
                let _ = set_character_posture::<TestPose>(world, hero, Some("prone"));
            }
            60 => {
                blocked = turn_character::<TestPose>(world, hero, -1.2)
                    .ok()
                    .and_then(|r| r.blocked_by);
            }
            90 => {
                blocked = set_character_posture::<TestPose>(world, hero, None)
                    .ok()
                    .and_then(|r| r.blocked_by);
            }
            _ => {}
        }
        let heading = yaw_of(pose_of(world, hero).rotation);
        let step = if (11..100).contains(&frame) {
            crawl(heading, 0.05)
        } else {
            FALL
        };
        let _ = move_character::<TestPose, NoParent>(world, hero, step);
        let pose = pose_of(world, hero);
        sink.lock().unwrap().push((
            pose.translation.map(f32::to_bits),
            pose.rotation[1].to_bits(),
            state(world, hero).posture,
            blocked,
        ));
    });
    run(&mut world, &mut schedule, 120);
    trace.lock().unwrap().clone()
}

#[test]
fn a_crawl_script_is_identical_at_one_and_eight_threads() {
    let one = with_threads(1, crawl_script);
    let eight = with_threads(8, crawl_script);
    assert_eq!(one, eight);
    assert!(
        one.iter().any(|row| row.2.as_deref() == Some("prone")),
        "it crawled"
    );
}

#[test]
fn a_lying_posture_round_trips_through_json_without_a_zero_offset() {
    let shape = PostureShape::Lying {
        length: 1.7,
        radius: 0.17,
        offset: 0.0,
    };
    let json = serde_json::to_value(&shape).unwrap();
    assert!(json["lying"].get("offset").is_none(), "{json}");
    assert_eq!(serde_json::from_value::<PostureShape>(json).unwrap(), shape);
    let shifted = serde_json::json!({"lying": {"length": 1.7, "radius": 0.17, "offset": 0.35}});
    assert_eq!(
        serde_json::from_value::<PostureShape>(shifted).unwrap(),
        PostureShape::Lying {
            length: 1.7,
            radius: 0.17,
            offset: 0.35
        }
    );
}

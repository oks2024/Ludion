//! Motor postures: a crouch in the open, a stand refused under a low slab, a wider posture beside
//! a wall, the errors, a resize on top of a posture, `character_fit` and `resume_character` with a
//! posture, what a teleport restores, a per-posture `step_height`, thread-count invariance and the
//! batch mover. A game system registered before `install` drives the hero.

mod common;

use std::sync::{Arc, Mutex};

use common::{TestPose, pose_of, tick};
use ludion_core::ids::EntityId;
use ludion_core::par::with_threads;
use ludion_ecs::{Schedule, SystemCtx, World};
use ludion_physics::{
    CharacterMotor, CharacterMotorState, MotorError, NoParent, PhysicsSpace, Posture, PostureShape,
    character_fit, install, move_character, move_characters, resize_character, resume_character,
    set_character_posture, suspend_character, teleport_character,
};
use ludion_spatial::{Body, Collider, Shape};

const FLAT: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const FALL: [f32; 3] = [0.0, -0.1, 0.0];
const WALK: [f32; 3] = [0.05, -0.1, 0.0];

fn upright(height: f32, radius: Option<f32>) -> PostureShape {
    PostureShape::Upright { height, radius }
}

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
        radius: 0.28,
        height: 0.9,
        slope_limit_deg: 45.0,
        step_height: 0.25,
        ..CharacterMotor::default()
    };
    motor
        .postures
        .insert("crouch".into(), posture(upright(0.6, None)));
    motor
        .postures
        .insert("wide".into(), posture(upright(1.2, Some(0.5))));
    motor
        .postures
        .insert("bad".into(), posture(upright(0.4, None)));
    motor.postures.insert(
        "low".into(),
        Posture {
            step_height: Some(0.1),
            ..posture(upright(0.6, None))
        },
    );
    motor
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

fn floor(world: &mut World) -> EntityId {
    spawn_box(world, [0.0, -0.5, 0.0], [30.0, 0.5, 30.0])
}

/// A slab centred on `x` whose underside is 0.75 above the floor: a crouch (0.6) fits, the stand
/// (0.9) does not.
fn slab(world: &mut World, x: f32) -> EntityId {
    spawn_box(world, [x, 1.0, 0.0], [1.0, 0.25, 1.0])
}

fn hero(world: &mut World, feet: [f32; 3], held: Option<&str>) -> EntityId {
    let entity = world.spawn();
    world.insert(entity, TestPose::at(feet)).unwrap();
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

type Shared<T> = Arc<Mutex<Vec<T>>>;

#[test]
fn a_crouch_posture_shrinks_the_body_and_walking_works() {
    let mut world = World::new();
    floor(&mut world);
    let hero = hero(&mut world, [0.0, 0.3, 0.0], None);
    let log: Shared<(u64, Option<String>, f32)> = Arc::default();
    let sink = log.clone();
    let (mut world, mut schedule) = rig(world, move |world, frame| {
        if frame == 10 {
            let report = set_character_posture::<TestPose>(world, hero, Some("crouch")).unwrap();
            assert_eq!(report.posture.as_deref(), Some("crouch"));
            assert_eq!(report.blocked_by, None);
        }
        let step = if frame >= 12 { WALK } else { FALL };
        let Ok(report) = move_character::<TestPose, NoParent>(world, hero, step) else {
            return;
        };
        sink.lock()
            .unwrap()
            .push((frame, report.posture, report.height));
    });
    run(&mut world, &mut schedule, 40);
    let log = log.lock().unwrap();
    assert!(
        log.iter()
            .filter(|r| r.0 < 10)
            .all(|r| r.1.is_none() && r.2 == 0.9)
    );
    assert!(
        log.iter()
            .filter(|r| r.0 >= 11)
            .all(|r| r.1.as_deref() == Some("crouch") && r.2 == 0.6)
    );
    assert!(pose_of(&world, hero).translation[0] > 0.8, "it walked");
}

#[test]
fn standing_under_a_low_slab_is_refused_until_the_slab_is_gone() {
    let mut world = World::new();
    floor(&mut world);
    let slab = slab(&mut world, 0.0);
    let hero = hero(&mut world, [0.0, 0.0, 0.0], Some("crouch"));
    let log: Shared<(u64, Option<String>, Option<EntityId>)> = Arc::default();
    let sink = log.clone();
    let (mut world, mut schedule) = rig(world, move |world, frame| {
        if frame == 10 {
            world.despawn(slab).unwrap();
        }
        if matches!(frame, 5 | 10 | 12) {
            let r = set_character_posture::<TestPose>(world, hero, None).unwrap();
            sink.lock().unwrap().push((frame, r.posture, r.blocked_by));
        }
        let _ = move_character::<TestPose, NoParent>(world, hero, FALL);
    });
    run(&mut world, &mut schedule, 20);
    let log = log.lock().unwrap().clone();
    assert_eq!(log[0], (5, Some("crouch".into()), Some(slab)));
    assert_eq!(
        log[1],
        (10, Some("crouch".into()), Some(slab)),
        "last completed step"
    );
    assert_eq!(log[2], (12, None, None));
    assert_eq!(state(&world, hero).posture, None);
}

#[test]
fn a_wider_posture_beside_a_wall_is_refused() {
    let mut world = World::new();
    floor(&mut world);
    let wall = spawn_box(&mut world, [0.5, 1.0, 0.0], [0.15, 1.0, 1.0]);
    let hero = hero(&mut world, [0.0, 0.0, 0.0], None);
    let log: Shared<(Option<String>, Option<EntityId>)> = Arc::default();
    let sink = log.clone();
    let (mut world, mut schedule) = rig(world, move |world, frame| {
        if frame == 5 {
            let r = set_character_posture::<TestPose>(world, hero, Some("wide")).unwrap();
            sink.lock().unwrap().push((r.posture, r.blocked_by));
        }
        let _ = move_character::<TestPose, NoParent>(world, hero, FALL);
    });
    run(&mut world, &mut schedule, 10);
    assert_eq!(log.lock().unwrap()[0], (None, Some(wall)));
    assert_eq!(state(&world, hero).posture, None);
}

#[test]
fn the_error_cases_are_named() {
    let mut world = World::new();
    floor(&mut world);
    let hero = hero(&mut world, [0.0, 0.0, 0.0], None);
    let errors: Shared<String> = Arc::default();
    let sink = errors.clone();
    let (mut world, mut schedule) = rig(world, move |world, frame| {
        let note = |r: Result<_, MotorError>| {
            sink.lock().unwrap().push(match r {
                Ok(_) => "ok".to_string(),
                Err(MotorError::UnknownPosture { .. }) => "unknown".to_string(),
                Err(MotorError::Suspended(_)) => "suspended".to_string(),
                Err(MotorError::InvalidMotor { .. }) => "invalid".to_string(),
                Err(MotorError::NoMotor(_)) => "no-motor".to_string(),
                Err(other) => format!("{other:?}"),
            });
        };
        if frame == 5 {
            note(set_character_posture::<TestPose>(
                world,
                hero,
                Some("prone"),
            ));
            note(set_character_posture::<TestPose>(world, hero, Some("bad")));
            let bare = world.spawn();
            note(set_character_posture::<TestPose>(world, bare, None));
            suspend_character(world, hero).unwrap();
            note(set_character_posture::<TestPose>(
                world,
                hero,
                Some("crouch"),
            ));
        }
        let _ = move_character::<TestPose, NoParent>(world, hero, FALL);
    });
    run(&mut world, &mut schedule, 8);
    assert_eq!(
        *errors.lock().unwrap(),
        ["unknown", "invalid", "no-motor", "suspended"]
    );
}

#[test]
fn a_resize_rides_on_a_posture_and_the_next_posture_clears_it() {
    let mut world = World::new();
    floor(&mut world);
    let hero = hero(&mut world, [0.0, 0.0, 0.0], None);
    let log: Shared<(u64, Option<u32>, Option<String>, f32)> = Arc::default();
    let sink = log.clone();
    let (mut world, mut schedule) = rig(world, move |world, frame| {
        match frame {
            5 => {
                set_character_posture::<TestPose>(world, hero, Some("crouch")).unwrap();
            }
            8 => {
                let r = resize_character::<TestPose>(world, hero, 0.58).unwrap();
                assert_eq!(r.height, 0.58);
            }
            12 => {
                set_character_posture::<TestPose>(world, hero, Some("crouch")).unwrap();
            }
            _ => {}
        }
        let Ok(report) = move_character::<TestPose, NoParent>(world, hero, FALL) else {
            return;
        };
        let s = state(world, hero);
        sink.lock().unwrap().push((
            frame,
            s.height.map(f32::to_bits),
            report.posture,
            report.height,
        ));
    });
    run(&mut world, &mut schedule, 16);
    let log = log.lock().unwrap();
    let at = |frame: u64| log.iter().find(|row| row.0 == frame).cloned();
    assert_eq!(
        at(9),
        Some((9, Some(0.58f32.to_bits()), Some("crouch".into()), 0.58))
    );
    assert_eq!(at(13), Some((13, None, Some("crouch".into()), 0.6)));
}

#[test]
fn character_fit_and_resume_take_a_posture() {
    let mut world = World::new();
    floor(&mut world);
    let slab = slab(&mut world, 0.0);
    let hero = hero(&mut world, [0.0, 0.0, 0.0], Some("crouch"));
    let log: Shared<String> = Arc::default();
    let sink = log.clone();
    let (mut world, mut schedule) = rig(world, move |world, frame| {
        let feet = [0.0, 0.01, 0.0];
        if frame == 5 {
            let note = |s: String| sink.lock().unwrap().push(s);
            note(format!(
                "{:?}",
                character_fit::<TestPose>(world, hero, Some("crouch"), feet, 0.0)
            ));
            note(format!(
                "{:?}",
                character_fit::<TestPose>(world, hero, None, feet, 0.0)
            ));
            suspend_character(world, hero).unwrap();
            let blocked = resume_character::<TestPose>(world, hero, feet, 0.0, None).unwrap();
            assert_eq!(blocked.blocked_by, Some(slab));
            let unknown = resume_character::<TestPose>(world, hero, feet, 0.0, Some("prone"));
            assert!(matches!(unknown, Err(MotorError::UnknownPosture { .. })));
        }
        if frame == 8 {
            let r = resume_character::<TestPose>(world, hero, feet, 0.0, Some("crouch")).unwrap();
            assert!(r.resumed);
        }
        let _ = move_character::<TestPose, NoParent>(world, hero, FALL);
    });
    run(&mut world, &mut schedule, 12);
    assert_eq!(
        *log.lock().unwrap(),
        ["Ok(None)".to_string(), format!("Ok(Some({slab:?}))")]
    );
    let s = state(&world, hero);
    assert_eq!(
        (s.posture.as_deref(), s.suspended, s.height),
        (Some("crouch"), false, None)
    );
}

#[test]
fn a_teleport_restores_the_base_shape_only_where_it_fits() {
    let mut world = World::new();
    floor(&mut world);
    slab(&mut world, 0.0);
    slab(&mut world, 10.0);
    let hero = hero(&mut world, [0.0, 0.0, 0.0], Some("crouch"));
    let log: Shared<Option<String>> = Arc::default();
    let sink = log.clone();
    let (mut world, mut schedule) = rig(world, move |world, frame| {
        if frame == 5 {
            teleport_character::<TestPose>(world, hero, [10.0, 0.0, 0.0], 0.0).unwrap();
        }
        if frame == 7 {
            sink.lock().unwrap().push(state(world, hero).posture);
            teleport_character::<TestPose>(world, hero, [20.0, 0.0, 0.0], 0.0).unwrap();
        }
        if frame == 9 {
            sink.lock().unwrap().push(state(world, hero).posture);
        }
        let _ = move_character::<TestPose, NoParent>(world, hero, FALL);
    });
    run(&mut world, &mut schedule, 12);
    assert_eq!(*log.lock().unwrap(), [Some("crouch".to_string()), None]);
}

#[test]
fn a_posture_step_height_is_honoured() {
    let walk = |held: Option<&str>| {
        let mut world = World::new();
        floor(&mut world);
        spawn_box(&mut world, [1.0, 0.1, 0.0], [0.5, 0.1, 1.0]);
        let hero = hero(&mut world, [0.0, 0.0, 0.0], held);
        let (mut world, mut schedule) = rig(world, move |world, _| {
            let _ = move_character::<TestPose, NoParent>(world, hero, WALK);
        });
        run(&mut world, &mut schedule, 60);
        pose_of(&world, hero).translation[0]
    };
    assert!(walk(None) > 1.6, "the base climbs a 0.2 lip");
    assert!(
        walk(Some("crouch")) > 1.6,
        "a crouch inherits the step height"
    );
    assert!(
        walk(Some("low")) < 0.6,
        "the low posture's 0.1 step refuses it"
    );
}

fn determinism_run() -> Vec<([u32; 3], Option<String>)> {
    let mut world = World::new();
    floor(&mut world);
    slab(&mut world, 3.0);
    let hero = hero(&mut world, [2.0, 0.3, 0.0], None);
    let trace: Shared<([u32; 3], Option<String>)> = Arc::default();
    let sink = trace.clone();
    let (mut world, mut schedule) = rig(world, move |world, frame| {
        match frame {
            20 => {
                let _ = set_character_posture::<TestPose>(world, hero, Some("crouch"));
            }
            70 => {
                let _ = set_character_posture::<TestPose>(world, hero, None);
            }
            _ => {}
        }
        let step = if (21..60).contains(&frame) {
            WALK
        } else {
            FALL
        };
        let _ = move_character::<TestPose, NoParent>(world, hero, step);
        sink.lock().unwrap().push((
            pose_of(world, hero).translation.map(f32::to_bits),
            state(world, hero).posture,
        ));
    });
    run(&mut world, &mut schedule, 90);
    trace.lock().unwrap().clone()
}

#[test]
fn a_posture_script_is_identical_at_one_and_eight_threads() {
    let one = with_threads(1, determinism_run);
    let eight = with_threads(8, determinism_run);
    assert_eq!(one, eight);
    assert!(one.iter().any(|row| row.1.is_some()), "the crouch happened");
}

#[test]
fn a_motor_without_postures_serializes_without_the_keys() {
    let plain = serde_json::to_value(CharacterMotor::default()).unwrap();
    assert!(plain.get("postures").is_none());
    let state = serde_json::to_value(CharacterMotorState::default()).unwrap();
    assert!(state.get("posture").is_none());
    let crouched = CharacterMotorState {
        posture: Some("crouch".into()),
        ..CharacterMotorState::default()
    };
    assert_eq!(serde_json::to_value(crouched).unwrap()["posture"], "crouch");
}

fn batch_run(batched: bool) -> Vec<[u32; 3]> {
    let mut world = World::new();
    floor(&mut world);
    let heroes: Vec<EntityId> = (0..3)
        .map(|i| hero(&mut world, [i as f32 * 2.0, 0.0, 0.0], Some("crouch")))
        .collect();
    let trace: Shared<[u32; 3]> = Arc::default();
    let sink = trace.clone();
    let list = heroes.clone();
    let (mut world, mut schedule) = rig(world, move |world, _| {
        let moves: Vec<(EntityId, [f32; 3])> = list.iter().map(|e| (*e, WALK)).collect();
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
    run(&mut world, &mut schedule, 30);
    trace.lock().unwrap().clone()
}

#[test]
fn move_characters_equals_serial_calls_with_a_crouched_motor() {
    assert_eq!(batch_run(true), batch_run(false));
}

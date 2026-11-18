//! `suspend_character`, `character_fit` and `resume_character`: a parked motor has no body, takes
//! no moves, and comes back only where it fits. A game system registered before `install` drives
//! the hero; what it records is the assertion.

mod common;

use std::sync::{Arc, Mutex};

use common::{TestPose, pose_of, tick};
use ludion_core::ids::EntityId;
use ludion_core::par::with_threads;
use ludion_ecs::{Parent, Schedule, SystemCtx, World};
use ludion_physics::{
    CharacterMotor, CharacterMotorState, MotorError, MoveReport, NoParent, PhysicsSpace,
    PhysicsWorld, QueryFilter, character_fit, install, move_character, move_characters,
    resume_character, suspend_character, teleport_character,
};
use ludion_spatial::{Body, Collider, RigidBody, Shape, Velocity};

const FALL: [f32; 3] = [0.0, -0.1, 0.0];
const FLAT: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
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

fn spawn_box(world: &mut World, center: [f32; 3], half: [f32; 3], body: Body) -> EntityId {
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
                body,
                layer: None,
                sensor: false,
            },
        )
        .unwrap();
    entity
}

fn floor(world: &mut World) -> EntityId {
    spawn_box(
        world,
        [0.0, -0.5, 0.0],
        [30.0, 0.5, 30.0],
        Body::new_static(),
    )
}

/// A tall static wall across x = `WALL_X`.
fn wall(world: &mut World) -> EntityId {
    spawn_box(
        world,
        [WALL_X, 1.0, 0.0],
        [0.5, 1.0, 3.0],
        Body::new_static(),
    )
}

fn crate_box(world: &mut World, center: [f32; 3]) -> EntityId {
    let entity = spawn_box(
        world,
        center,
        [0.25, 0.25, 0.25],
        Body::Rigid(RigidBody {
            sleep_after_ticks: Some(1_000_000),
            ..RigidBody::default()
        }),
    );
    world.insert(entity, Velocity::default()).unwrap();
    entity
}

fn hero(world: &mut World, feet: [f32; 3]) -> EntityId {
    let entity = world.spawn();
    world.insert(entity, TestPose::at(feet)).unwrap();
    world.insert(entity, motor()).unwrap();
    entity
}

type Shared<T> = Arc<Mutex<Vec<T>>>;
type MoveResult = Result<MoveReport, MotorError>;
type Refusals = (Result<(), MotorError>, MoveResult, Vec<MoveResult>);

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

fn speed(world: &World, entity: EntityId) -> f32 {
    let v = world.get::<Velocity>(entity).unwrap().unwrap().linear;
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

fn bits(v: [f32; 3]) -> [u32; 3] {
    v.map(f32::to_bits)
}

#[test]
fn a_suspended_motor_has_no_body_and_a_ray_hits_the_wall_behind_it() {
    let mut world = World::new();
    floor(&mut world);
    let wall = wall(&mut world);
    let hero = hero(&mut world, [2.0, 0.0, 0.0]);
    let counts: Shared<(u64, usize)> = Arc::default();
    let sink = counts.clone();
    let (mut world, mut schedule) = rig(world, move |world, frame| {
        if frame == 20 {
            suspend_character(world, hero).unwrap();
        }
        let _ = move_character::<TestPose, NoParent>(world, hero, FALL);
        if let Some(physics) = world.resource::<PhysicsWorld>() {
            sink.lock().unwrap().push((frame, physics.body_count()));
        }
    });
    run(&mut world, &mut schedule, 25);
    let counts = counts.lock().unwrap();
    let before = counts.iter().find(|(f, _)| *f == 19).unwrap().1;
    let after = counts.iter().find(|(f, _)| *f == 24).unwrap().1;
    assert_eq!(after, before - 1, "{counts:?}");

    let physics = world.resource::<PhysicsWorld>().unwrap();
    let hits = physics
        .ray_cast([0.0, 0.5, 0.0], [1.0, 0.0, 0.0], 100.0, &QueryFilter::new())
        .unwrap();
    assert_eq!(hits.first().map(|h| h.entity), Some(wall), "{hits:?}");
}

#[test]
fn a_suspended_motor_takes_no_moves_or_teleports() {
    let mut world = World::new();
    floor(&mut world);
    let a = hero(&mut world, [0.0, 0.0, 0.0]);
    let b = hero(&mut world, [2.0, 0.0, 0.0]);
    let seen: Shared<Refusals> = Arc::default();
    let sink = seen.clone();
    let (mut world, mut schedule) = rig(world, move |world, frame| {
        if frame == 5 {
            suspend_character(world, a).unwrap();
            let teleport = teleport_character::<TestPose>(world, a, [1.0, 0.0, 0.0], 0.0);
            let single = move_character::<TestPose, NoParent>(world, a, FALL);
            let mut out = vec![Err(MotorError::NoPhysicsWorld); 2];
            move_characters::<TestPose, NoParent>(world, &[(a, FALL), (b, FALL)], &mut out);
            sink.lock().unwrap().push((teleport, single, out));
        }
    });
    run(&mut world, &mut schedule, 8);
    let seen = seen.lock().unwrap();
    let (teleport, single, batch) = &seen[0];
    assert_eq!(teleport, &Err(MotorError::Suspended(a)));
    assert_eq!(single.as_ref().unwrap_err(), &MotorError::Suspended(a));
    assert_eq!(batch[0].as_ref().unwrap_err(), &MotorError::Suspended(a));
    assert!(batch[1].is_ok(), "the live motor in the batch still moves");
}

#[test]
fn suspend_is_idempotent_and_clears_the_ground() {
    let mut world = World::new();
    floor(&mut world);
    let hero = hero(&mut world, [0.0, 0.0, 0.0]);
    let (mut world, mut schedule) = rig(world, move |world, frame| {
        if frame < 15 {
            let _ = move_character::<TestPose, NoParent>(world, hero, FALL);
        }
    });
    run(&mut world, &mut schedule, 15);
    assert!(state(&world, hero).grounded);
    suspend_character(&mut world, hero).unwrap();
    let once = state(&world, hero);
    suspend_character(&mut world, hero).unwrap();
    assert_eq!(state(&world, hero), once);
    assert!(once.suspended && !once.grounded && once.ground.is_none() && once.platform.is_none());
    let nobody = world.spawn();
    assert_eq!(
        suspend_character(&mut world, nobody),
        Err(MotorError::NoMotor(nobody))
    );
}

/// Suspends the hero at 10, tries `feet` at 20 and records the report and the pose around it.
fn resume_at(
    feet: [f32; 3],
) -> (
    World,
    EntityId,
    Option<ludion_physics::ResumeReport>,
    EntityId,
) {
    let mut world = World::new();
    floor(&mut world);
    let wall = wall(&mut world);
    let hero = hero(&mut world, [0.0, 0.0, 0.0]);
    let report: Shared<ludion_physics::ResumeReport> = Arc::default();
    let sink = report.clone();
    let (mut world, mut schedule) = rig(world, move |world, frame| {
        if frame == 10 {
            suspend_character(world, hero).unwrap();
        } else if frame == 20 {
            let r = resume_character::<TestPose>(world, hero, feet, 0.5, None).unwrap();
            sink.lock().unwrap().push(r);
        }
    });
    run(&mut world, &mut schedule, 30);
    let report = report.lock().unwrap().first().copied();
    (world, hero, report, wall)
}

#[test]
fn a_resume_into_a_wall_is_refused_and_changes_nothing() {
    let (world, hero, report, wall) = resume_at([WALL_X, 0.0, 0.0]);
    let report = report.unwrap();
    assert!(!report.resumed);
    assert_eq!(report.blocked_by, Some(wall));
    assert!(state(&world, hero).suspended);
    assert_eq!(pose_of(&world, hero).translation[0], 0.0);
}

#[test]
fn a_resume_in_the_open_brings_the_motor_back() {
    let (world, hero, report, _) = resume_at([-3.0, 0.0, 0.0]);
    let report = report.unwrap();
    assert!(report.resumed && report.blocked_by.is_none());
    assert!(!state(&world, hero).suspended);
    let pose = pose_of(&world, hero);
    assert_eq!(pose.translation[0], -3.0);
    assert!(pose.rotation[1].abs() > 0.1, "the yaw was written");
    let physics = world.resource::<PhysicsWorld>().unwrap();
    let hits = physics
        .ray_cast(
            [-3.0, 0.5, -2.0],
            [0.0, 0.0, 1.0],
            10.0,
            &QueryFilter::new(),
        )
        .unwrap();
    assert!(
        hits.iter().any(|h| h.entity == hero),
        "the capsule is back: {hits:?}"
    );
}

#[test]
fn a_resume_launches_no_sleeping_crate_beside_the_spot() {
    let new = [10.0, 0.0, 0.0];
    let mut world = World::new();
    floor(&mut world);
    let face = 0.28 + 0.01 / 2.0;
    let crates = [
        crate_box(&mut world, [new[0] + face + 0.25, 0.25, 0.0]),
        crate_box(&mut world, [new[0] - face - 0.25, 0.25, 0.0]),
    ];
    let hero = hero(&mut world, [0.0, 0.0, 0.0]);
    let peaks: Shared<[f32; 2]> = Arc::default();
    let sink = peaks.clone();
    let (mut world, mut schedule) = rig(world, move |world, frame| {
        if frame == 10 {
            suspend_character(world, hero).unwrap();
        } else if frame == 60 {
            assert!(
                resume_character::<TestPose>(world, hero, new, 0.0, None)
                    .unwrap()
                    .resumed
            );
        } else if frame > 60 && frame <= 63 {
            sink.lock().unwrap().push(crates.map(|c| speed(world, c)));
        }
    });
    run(&mut world, &mut schedule, 64);
    let peaks = peaks.lock().unwrap();
    assert_eq!(peaks.len(), 3);
    assert!(
        peaks.iter().flatten().all(|s| *s <= 1e-4),
        "a resume left crates moving: {peaks:?}"
    );
}

#[test]
fn a_resume_refuses_the_wrong_cases() {
    let mut world = World::new();
    floor(&mut world);
    let free = hero(&mut world, [0.0, 0.0, 0.0]);
    let parented = hero(&mut world, [2.0, 0.0, 0.0]);
    let root = world.spawn();
    world.insert(parented, Parent(root)).unwrap();
    suspend_character(&mut world, parented).unwrap();
    let nobody = world.spawn();
    let spot = [-3.0, 0.0, 0.0];
    assert_eq!(
        resume_character::<TestPose>(&mut world, parented, spot, 0.0, None),
        Err(MotorError::Parented(parented))
    );
    assert_eq!(
        resume_character::<TestPose>(&mut world, free, spot, 0.0, None),
        Err(MotorError::NotSuspended(free))
    );
    assert_eq!(
        resume_character::<TestPose>(&mut world, free, spot, 0.0, Some("prone")),
        Err(MotorError::UnknownPosture {
            entity: free,
            posture: "prone".into()
        })
    );
    assert_eq!(
        resume_character::<TestPose>(&mut world, nobody, spot, 0.0, None),
        Err(MotorError::NoMotor(nobody))
    );
    assert!(matches!(
        resume_character::<TestPose>(&mut world, free, [f32::NAN, 0.0, 0.0], 0.0, None),
        Err(MotorError::NonFiniteTeleport { .. })
    ));
    assert!(matches!(
        character_fit::<TestPose>(&world, free, None, [f32::NAN, 0.0, 0.0], 0.0),
        Err(MotorError::NonFiniteFeet(_))
    ));
}

#[test]
fn character_fit_agrees_with_resume() {
    let mut world = World::new();
    floor(&mut world);
    let wall = wall(&mut world);
    let hero = hero(&mut world, [0.0, 0.0, 0.0]);
    let (mut world, mut schedule) = rig(world, |_, _| {});
    run(&mut world, &mut schedule, 5);
    suspend_character(&mut world, hero).unwrap();
    let blocked = [WALL_X, 0.0, 0.0];
    let open = [-3.0, 0.0, 0.0];
    assert_eq!(
        character_fit::<TestPose>(&world, hero, None, blocked, 0.0),
        Ok(Some(wall))
    );
    assert_eq!(
        character_fit::<TestPose>(&world, hero, None, open, 0.0),
        Ok(None)
    );
    assert_eq!(
        character_fit::<TestPose>(&world, hero, Some("prone"), open, 0.0),
        Err(MotorError::UnknownPosture {
            entity: hero,
            posture: "prone".into()
        })
    );
    let refused = resume_character::<TestPose>(&mut world, hero, blocked, 0.0, None).unwrap();
    assert_eq!(refused.blocked_by, Some(wall));
    let resumed = resume_character::<TestPose>(&mut world, hero, open, 0.0, None).unwrap();
    assert!(resumed.resumed);
}

fn determinism_run() -> Vec<([u32; 3], [u32; 4], bool)> {
    let mut world = World::new();
    floor(&mut world);
    wall(&mut world);
    crate_box(&mut world, [-2.6, 0.25, 0.0]);
    let hero = hero(&mut world, [2.0, 0.3, 0.0]);
    let trace: Shared<([u32; 3], [u32; 4], bool)> = Arc::default();
    let sink = trace.clone();
    let (mut world, mut schedule) = rig(world, move |world, frame| {
        match frame {
            30 => suspend_character(world, hero).unwrap(),
            60 => {
                let _ = resume_character::<TestPose>(world, hero, [WALL_X, 0.0, 0.0], 0.3, None);
            }
            70 => {
                let _ = resume_character::<TestPose>(world, hero, [-2.0, 0.0, 0.0], 0.3, None);
            }
            _ => {}
        }
        let _ = move_character::<TestPose, NoParent>(world, hero, FALL);
        let pose = pose_of(world, hero);
        sink.lock().unwrap().push((
            bits(pose.translation),
            pose.rotation.map(f32::to_bits),
            state(world, hero).suspended,
        ));
    });
    run(&mut world, &mut schedule, 100);
    trace.lock().unwrap().clone()
}

#[test]
fn a_suspend_resume_script_is_identical_at_one_and_eight_threads() {
    let one = with_threads(1, determinism_run);
    let eight = with_threads(8, determinism_run);
    assert_eq!(one, eight);
    assert!(one.iter().any(|row| row.2) && !one.last().unwrap().2);
}

#[test]
fn a_world_that_never_suspends_serializes_no_suspended_key() {
    let json = serde_json::to_value(CharacterMotorState::default()).unwrap();
    assert!(json.get("suspended").is_none(), "{json}");
    let parked = serde_json::to_value(CharacterMotorState {
        suspended: true,
        ..CharacterMotorState::default()
    })
    .unwrap();
    assert_eq!(parked["suspended"], true);
}

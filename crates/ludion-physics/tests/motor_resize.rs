//! `resize_character`, `fits_character` and `teleport_character`: a crouch under open sky and under
//! a low slab, the errors, a teleport off a moving platform, a teleport that launches no prop,
//! the height a teleport restores, and thread-count invariance. A game system registered before
//! `install` drives the hero; what it records is the assertion.

mod common;

use std::sync::{Arc, Mutex};

use common::{TestPose, pose_of, tick};
use ludion_core::ids::EntityId;
use ludion_core::math::sin_cos;
use ludion_core::par::with_threads;
use ludion_ecs::{Schedule, SystemCtx, World};
use ludion_physics::{
    CharacterMotor, CharacterMotorState, MotorError, MoveReport, NoParent, PhysicsSpace,
    PlatformCarry, QueryFilter, ResizeReport, fits_character, install, move_character,
    resize_character, teleport_character,
};
use ludion_spatial::{Body, Collider, RigidBody, Shape, Velocity};

const RADIUS: f32 = 0.28;
const HEIGHT: f32 = 0.9;
const CROUCH: f32 = 0.6;
const SKIN: f32 = 0.01;
const FLAT: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const FALL: [f32; 3] = [0.0, -0.1, 0.0];

fn motor() -> CharacterMotor {
    CharacterMotor {
        radius: RADIUS,
        height: HEIGHT,
        slope_limit_deg: 45.0,
        step_height: 0.25,
        skin_width: SKIN,
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

/// A big static floor whose top is `y = 0`.
fn floor(world: &mut World) -> EntityId {
    spawn_box(
        world,
        [0.0, -0.5, 0.0],
        [30.0, 0.5, 30.0],
        Body::new_static(),
    )
}

/// A static slab centred on `x` whose underside is `underside` above the floor.
fn slab(world: &mut World, x: f32, underside: f32) -> EntityId {
    spawn_box(
        world,
        [x, underside + 0.25, 0.0],
        [1.0, 0.25, 1.0],
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

fn hero(world: &mut World, feet: [f32; 3], crouched: bool) -> EntityId {
    let entity = world.spawn();
    world.insert(entity, TestPose::at(feet)).unwrap();
    world.insert(entity, motor()).unwrap();
    if crouched {
        world
            .insert(
                entity,
                CharacterMotorState {
                    height: Some(CROUCH),
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

fn velocity_of(world: &World, entity: EntityId) -> Velocity {
    *world.get::<Velocity>(entity).unwrap().expect("a velocity")
}

fn speed(world: &World, entity: EntityId) -> f32 {
    let v = velocity_of(world, entity).linear;
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

fn feet_of(world: &World, entity: EntityId) -> [f32; 3] {
    pose_of(world, entity).translation
}

fn bits(v: [f32; 3]) -> [u32; 3] {
    v.map(f32::to_bits)
}

type Shared<T> = Arc<Mutex<Vec<T>>>;
type OpenRow = (u64, [f32; 3], Option<ResizeReport>, Option<f32>);

#[test]
fn an_open_resize_keeps_the_feet_and_rebuilds_the_capsule() {
    let mut world = World::new();
    floor(&mut world);
    let hero = hero(&mut world, [0.0, 0.3, 0.0], false);
    // (frame, feet at the start, resize report, move report height)
    let log: Shared<OpenRow> = Arc::default();
    let tops: Shared<(u64, f32)> = Arc::default();
    let (sink, top_sink) = (log.clone(), tops.clone());
    let (mut world, mut schedule) = rig(world, move |world, frame| {
        let feet = feet_of(world, hero);
        let target = match frame {
            30 => Some(CROUCH),
            60 => Some(HEIGHT),
            _ => None,
        };
        let resized = target.map(|h| resize_character::<TestPose>(world, hero, h).unwrap());
        if matches!(frame, 31 | 61)
            && let Some(physics) = world.resource::<ludion_physics::PhysicsWorld>()
        {
            let hits = physics
                .ray_cast(
                    [feet[0], 3.0, feet[2]],
                    [0.0, -1.0, 0.0],
                    10.0,
                    &QueryFilter::new(),
                )
                .unwrap();
            let top = hits
                .iter()
                .find(|hit| hit.entity == hero)
                .expect("the capsule");
            top_sink
                .lock()
                .unwrap()
                .push((frame, top.point[1] - feet[1]));
        }
        let displacement = if resized.is_some() { [0.0; 3] } else { FALL };
        let report = move_character::<TestPose, NoParent>(world, hero, displacement).ok();
        sink.lock()
            .unwrap()
            .push((frame, feet, resized, report.map(|r| r.height)));
    });
    run(&mut world, &mut schedule, 90);

    let log = log.lock().unwrap();
    let at = |frame: u64| log.iter().find(|row| row.0 == frame).unwrap();
    for (frame, grown) in [(30, CROUCH), (60, HEIGHT)] {
        let row = at(frame);
        let report = row.2.expect("resized");
        assert_eq!(report.blocked_by, None, "frame {frame}");
        assert_eq!(report.height, grown);
        assert_eq!(row.3, Some(grown), "the move used it");
        assert_eq!(
            bits(row.1),
            bits(at(frame + 1).1),
            "the feet of frame {frame} do not move across a zero-displacement resize tick"
        );
    }
    assert_eq!(at(29).3, Some(HEIGHT));
    assert_eq!(at(31).3, Some(CROUCH));
    assert_eq!(at(59).3, Some(CROUCH));
    assert_eq!(at(61).3, Some(HEIGHT));
    let tops = tops.lock().unwrap();
    assert_eq!(tops.len(), 2);
    for (frame, top) in tops.iter() {
        let want = if *frame == 31 { CROUCH } else { HEIGHT };
        assert!(
            (top - want).abs() <= SKIN,
            "frame {frame}: the solver capsule's crown is {top} above the feet, want {want}"
        );
    }
    let state = world.get::<CharacterMotorState>(hero).unwrap().unwrap();
    assert_eq!(state.height, None, "the authored height is stored as None");
}

#[test]
fn a_low_slab_blocks_standing_until_it_is_gone() {
    const DESPAWN: u64 = 40;
    let mut world = World::new();
    floor(&mut world);
    let slab = slab(&mut world, 0.0, 0.75);
    let hero = hero(&mut world, [0.0, 0.3, 0.0], true);
    // (frame, blocked_by, height, fits(0.9), fits(0.6))
    let log: Shared<(u64, Option<EntityId>, f32, bool, bool)> = Arc::default();
    let sink = log.clone();
    let (mut world, mut schedule) = rig(world, move |world, frame| {
        if frame == DESPAWN {
            world.despawn(slab).unwrap();
        }
        if frame < 20 {
            let _ = move_character::<TestPose, NoParent>(world, hero, FALL);
            return;
        }
        let report = resize_character::<TestPose>(world, hero, HEIGHT).unwrap();
        let tall = fits_character::<TestPose>(world, hero, HEIGHT).unwrap();
        let low = fits_character::<TestPose>(world, hero, CROUCH).unwrap();
        sink.lock()
            .unwrap()
            .push((frame, report.blocked_by, report.height, tall, low));
        let _ = move_character::<TestPose, NoParent>(world, hero, FALL);
    });
    run(&mut world, &mut schedule, 60);

    let log = log.lock().unwrap();
    for (frame, blocked, height, tall, low) in log.iter() {
        assert!(low, "frame {frame}: the crouch always fits");
        if *frame <= DESPAWN {
            assert_eq!(*blocked, Some(slab), "frame {frame}");
            assert_eq!(*height, CROUCH, "frame {frame}");
            assert!(!tall, "frame {frame}: fits mirrors the block");
        } else {
            assert_eq!(*blocked, None, "frame {frame}");
            assert_eq!(*height, HEIGHT, "frame {frame}");
            assert!(tall, "frame {frame}");
        }
    }
    let first_grown = log.iter().find(|row| row.2 == HEIGHT).unwrap().0;
    assert_eq!(first_grown, DESPAWN + 1, "one tick of lag, no more");
}

#[test]
fn invalid_heights_and_missing_parts_are_errors() {
    let mut world = World::new();
    floor(&mut world);
    let hero = hero(&mut world, [0.0, 0.3, 0.0], false);
    let bare = world.spawn();
    world.insert(bare, TestPose::at([5.0, 0.0, 0.0])).unwrap();
    let results: Shared<(&'static str, String)> = Arc::default();
    let sink = results.clone();
    let (mut world, mut schedule) = rig(world, move |world, frame| {
        if frame != 5 {
            return;
        }
        let push = |tag: &'static str, result: Result<ResizeReport, MotorError>| {
            sink.lock()
                .unwrap()
                .push((tag, format!("{:?}", result.err())));
        };
        push("small", resize_character::<TestPose>(world, hero, 0.5));
        push("nan", resize_character::<TestPose>(world, hero, f32::NAN));
        push("step", resize_character::<TestPose>(world, hero, 0.2));
        push("bare", resize_character::<TestPose>(world, bare, 0.6));
        let fits = fits_character::<TestPose>(world, bare, 0.6).err();
        sink.lock()
            .unwrap()
            .push(("fits-bare", format!("{fits:?}")));
        let teleport = teleport_character::<TestPose>(world, hero, [f32::NAN, 0.0, 0.0], 0.0).err();
        sink.lock()
            .unwrap()
            .push(("nan-teleport", format!("{teleport:?}")));
        let state = world
            .get::<CharacterMotorState>(hero)
            .ok()
            .flatten()
            .cloned();
        assert!(
            state.is_none_or(|s| s.height.is_none()),
            "a refused resize changes nothing"
        );
    });
    run(&mut world, &mut schedule, 8);

    let results = results.lock().unwrap();
    let get = |tag: &str| &results.iter().find(|(t, _)| *t == tag).unwrap().1;
    for tag in ["small", "nan", "step"] {
        assert!(get(tag).contains("InvalidHeight"), "{tag}: {}", get(tag));
    }
    assert!(get("small").contains("0.561") || get("small").contains("2"));
    assert!(get("bare").contains("NoMotor"), "{}", get("bare"));
    assert!(get("fits-bare").contains("NoMotor"));
    assert!(get("nan-teleport").contains("NonFiniteTeleport"));
}

/// A rider on a kinematic platform translating +x, a static floor 10 m away, optional boxes.
struct Teleport {
    hero: EntityId,
    reports: Shared<(u64, MoveReport)>,
}

fn teleport_rig(at: u64) -> (World, Schedule, Teleport) {
    let mut world = World::new();
    let platform = spawn_box(
        &mut world,
        [0.0, -0.1, 0.0],
        [2.0, 0.1, 2.0],
        Body::new_kinematic(),
    );
    spawn_box(
        &mut world,
        [10.0, -0.5, 0.0],
        [3.0, 0.5, 3.0],
        Body::new_static(),
    );
    let hero = hero(&mut world, [0.0, 0.3, 0.0], false);
    let reports: Shared<(u64, MoveReport)> = Arc::default();
    let sink = reports.clone();
    let (world, schedule) = rig(world, move |world, frame| {
        if frame < at
            && let Ok(Some(pose)) = world.get_mut::<TestPose>(platform)
        {
            pose.translation[0] = 0.02 * frame as f32;
        }
        if frame == at {
            teleport_character::<TestPose>(world, hero, [10.0, 0.5, 0.0], 1.0).unwrap();
            return;
        }
        if let Ok(report) = move_character::<TestPose, NoParent>(world, hero, FALL) {
            sink.lock().unwrap().push((frame, report));
        }
    });
    (world, schedule, Teleport { hero, reports })
}

#[test]
fn a_teleport_off_a_platform_clears_the_ride_and_lands_on_the_new_floor() {
    const AT: u64 = 50;
    let (mut world, mut schedule, rig) = teleport_rig(AT);
    run(&mut world, &mut schedule, 80);
    let reports = rig.reports.lock().unwrap();
    let at = |frame: u64| &reports.iter().find(|(f, _)| *f == frame).unwrap().1;
    assert!(at(AT - 1).grounded, "the rider was on the platform");
    assert!(at(AT - 1).platform.entity.is_some());
    let after = at(AT + 1);
    assert_eq!(after.platform, PlatformCarry::default());
    assert!(!after.grounded);
    assert!(after.ground.is_none_or(|ground| ground.distance > 0.1));
    assert!(reports.iter().any(|(f, r)| *f > AT + 1 && r.landed));
    let feet = feet_of(&world, rig.hero);
    assert!((feet[0] - 10.0).abs() < 1e-3, "{feet:?}");
    let q = pose_of(&world, rig.hero).rotation;
    let (s, c) = sin_cos(0.5);
    for (got, want) in q.iter().zip([0.0, s, 0.0, c]) {
        assert!(
            (got - want).abs() < 1e-6,
            "the yaw quaternion is pure: {q:?}"
        );
    }
}

/// A hero standing at the origin with a crate beside it, another where it will arrive and a third
/// on the straight path between; a move at `AT` (teleport or a plain pose write), and each crate's
/// peak speed in the three ticks after.
fn crate_launch(plain_write: bool) -> [f32; 3] {
    const AT: u64 = 60;
    let new = [10.0, 0.0, 0.0];
    let mut world = World::new();
    floor(&mut world);
    // The capsule surface is RADIUS from the feet; each crate face sits half a skin beyond it.
    let face = RADIUS + SKIN / 2.0;
    let crates = [
        crate_box(&mut world, [face + 0.25, 0.25, 0.0]),
        crate_box(&mut world, [new[0] + face + 0.25, 0.25, 0.0]),
        crate_box(&mut world, [5.0, 0.25, 0.0]),
    ];
    let hero = hero(&mut world, [0.0, 0.0, 0.0], false);
    let peaks: Shared<[f32; 3]> = Arc::default();
    let sink = peaks.clone();
    let (mut world, mut schedule) = rig(world, move |world, frame| {
        if frame == AT {
            if plain_write {
                world
                    .get_mut::<TestPose>(hero)
                    .unwrap()
                    .unwrap()
                    .translation = new;
            } else {
                teleport_character::<TestPose>(world, hero, new, 0.0).unwrap();
            }
        } else if frame > AT && frame <= AT + 3 {
            sink.lock().unwrap().push(crates.map(|c| speed(world, c)));
        }
    });
    run(&mut world, &mut schedule, 64);
    let peaks = peaks.lock().unwrap();
    assert_eq!(peaks.len(), 3);
    [0, 1, 2].map(|i| peaks.iter().map(|p| p[i]).fold(0.0, f32::max))
}

#[test]
fn a_teleport_launches_no_prop_where_a_plain_write_does() {
    let teleported = crate_launch(false);
    assert!(
        teleported.iter().all(|speed| *speed <= 1e-4),
        "a teleport left crates moving: {teleported:?} m/s"
    );
    let plain = crate_launch(true);
    assert!(
        plain.iter().any(|speed| *speed > 0.1),
        "the contrast: a plain write must push a crate: {plain:?} m/s"
    );
}

#[test]
fn a_teleport_restores_the_height_only_where_it_fits() {
    let mut world = World::new();
    floor(&mut world);
    slab(&mut world, 5.0, 0.75);
    let hero_open = hero(&mut world, [-8.0, 0.0, 0.0], true);
    let hero_low = hero(&mut world, [-4.0, 0.0, 0.0], true);
    let heights: Shared<(u64, f32, f32)> = Arc::default();
    let sink = heights.clone();
    let (mut world, mut schedule) = rig(world, move |world, frame| {
        if frame == 30 {
            teleport_character::<TestPose>(world, hero_open, [-1.0, 0.0, 0.0], 0.0).unwrap();
            teleport_character::<TestPose>(world, hero_low, [5.0, 0.0, 0.0], 0.0).unwrap();
        }
        let report = |world: &mut World, hero| {
            move_character::<TestPose, NoParent>(world, hero, FALL)
                .map(|r| r.height)
                .unwrap_or(0.0)
        };
        let open = report(world, hero_open);
        let low = report(world, hero_low);
        if frame > 30 {
            sink.lock().unwrap().push((frame, open, low));
        }
    });
    run(&mut world, &mut schedule, 40);

    let heights = heights.lock().unwrap();
    assert!(
        heights
            .iter()
            .all(|(_, open, low)| *open == HEIGHT && *low == CROUCH)
    );
    let height = |hero| {
        world
            .get::<CharacterMotorState>(hero)
            .unwrap()
            .unwrap()
            .height
    };
    assert_eq!(height(hero_open), None);
    assert_eq!(height(hero_low), Some(CROUCH));
}

fn determinism_run() -> Vec<([u32; 3], [u32; 4], Option<u32>)> {
    let mut world = World::new();
    floor(&mut world);
    slab(&mut world, 3.0, 0.75);
    crate_box(&mut world, [8.4, 0.25, 0.0]);
    let hero = hero(&mut world, [2.0, 0.3, 0.0], false);
    let trace: Shared<([u32; 3], [u32; 4], Option<u32>)> = Arc::default();
    let sink = trace.clone();
    let (mut world, mut schedule) = rig(world, move |world, frame| {
        match frame {
            30 => {
                let _ = resize_character::<TestPose>(world, hero, CROUCH);
            }
            60 => {
                let _ = teleport_character::<TestPose>(world, hero, [8.0, 1.0, 0.0], 0.3);
            }
            80 => {
                let _ = resize_character::<TestPose>(world, hero, HEIGHT);
            }
            _ => {}
        }
        let step = if (31..60).contains(&frame) {
            [0.03, -0.1, 0.0]
        } else {
            FALL
        };
        let _ = move_character::<TestPose, NoParent>(world, hero, step);
        let pose = pose_of(world, hero);
        let state = world.get::<CharacterMotorState>(hero).ok().flatten();
        sink.lock().unwrap().push((
            bits(pose.translation),
            pose.rotation.map(f32::to_bits),
            state.and_then(|s| s.height).map(f32::to_bits),
        ));
    });
    run(&mut world, &mut schedule, 100);
    trace.lock().unwrap().clone()
}

#[test]
fn a_resize_and_teleport_script_is_identical_at_one_and_eight_threads() {
    let one = with_threads(1, determinism_run);
    let eight = with_threads(8, determinism_run);
    assert_eq!(one, eight);
    assert!(one.iter().any(|row| row.2.is_some()), "the crouch happened");
}

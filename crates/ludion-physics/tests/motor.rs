//! `move_character`: landing, slopes, ledges, takeoff, ceilings, corners, props, crushes, errors,
//! constraints, the last-report readout and thread-count invariance. A game system registered
//! before `install` drives the hero with a scripted displacement per tick; the reports are the
//! assertions.

mod common;

use std::sync::{Arc, Mutex};

use common::{TestPose, pose_of, tick};
use ludion_core::ids::EntityId;
use ludion_core::math::sin_cos;
use ludion_core::par::with_threads;
use ludion_core::time::TickRate;
use ludion_ecs::{Schedule, World};
use ludion_physics::{
    CharacterMotor, CharacterMotorState, ColliderMeshes, GroundProbe, MotorError, MoveConstraint,
    MoveReport, NoParent, PhysicsSpace, PhysicsWorld, QueryFilter, TriangleMesh, install,
    move_character, move_character_with, probe_character_ground, resize_character,
};
use ludion_spatial::{Body, Collider, RigidBody, Shape, Velocity};

type Log = Arc<Mutex<Vec<(u64, Result<MoveReport, MotorError>)>>>;

const RADIUS: f32 = 0.28;
const HEIGHT: f32 = 0.9;
const SKIN: f32 = 0.01;

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

fn about_z(degrees: f32) -> [f32; 4] {
    let (s, c) = sin_cos(degrees.to_radians() / 2.0);
    [0.0, 0.0, s, c]
}

fn spawn_box(
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

const FLAT: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// A big static floor whose top is `y = 0`.
fn floor(world: &mut World) -> EntityId {
    spawn_box(
        world,
        [0.0, -0.5, 0.0],
        [30.0, 0.5, 30.0],
        FLAT,
        Body::new_static(),
    )
}

fn wall(world: &mut World, center: [f32; 3], half: [f32; 3]) -> EntityId {
    spawn_box(world, center, half, FLAT, Body::new_static())
}

fn hero(world: &mut World, feet: [f32; 3]) -> EntityId {
    let entity = world.spawn();
    world.insert(entity, TestPose::at(feet)).unwrap();
    world.insert(entity, motor()).unwrap();
    entity
}

struct ZeroX;

impl MoveConstraint for ZeroX {
    fn project(&mut self, _feet: [f32; 3], step: [f32; 3]) -> [f32; 3] {
        [0.0, step[1], step[2]]
    }
}

struct Rig {
    world: World,
    schedule: Schedule,
    hero: EntityId,
    log: Log,
    frame: u64,
}

impl Rig {
    fn new(
        build: impl FnOnce(&mut World) -> EntityId,
        script: impl Fn(u64) -> Vec<[f32; 3]> + Send + Sync + 'static,
    ) -> Rig {
        Rig::with(build, script, false)
    }

    fn with(
        build: impl FnOnce(&mut World) -> EntityId,
        script: impl Fn(u64) -> Vec<[f32; 3]> + Send + Sync + 'static,
        zero_x: bool,
    ) -> Rig {
        let mut world = World::new();
        let hero = build(&mut world);
        let log: Log = Arc::default();
        let sink = log.clone();
        let mut schedule = Schedule::new();
        schedule.add_system_shared(
            "test-motor",
            Arc::new(move |world: &mut World, ctx: &ludion_ecs::SystemCtx| {
                let frame = ctx.frame.as_u64();
                for displacement in script(frame) {
                    let result = if zero_x {
                        move_character_with::<TestPose, NoParent>(
                            world,
                            hero,
                            displacement,
                            Some(&mut ZeroX),
                        )
                    } else {
                        move_character::<TestPose, NoParent>(world, hero, displacement)
                    };
                    sink.lock().unwrap().push((frame, result));
                }
            }),
        );
        install::<TestPose, NoParent>(&mut schedule, PhysicsSpace::Free);
        Rig {
            world,
            schedule,
            hero,
            log,
            frame: 0,
        }
    }

    fn run(&mut self, ticks: u64) -> &mut Rig {
        for _ in 0..ticks {
            tick(&mut self.world, &mut self.schedule, self.frame);
            self.frame += 1;
        }
        self
    }

    /// The reports of ticks `1..`, the first having nothing to move through yet.
    fn reports(&self) -> Vec<MoveReport> {
        self.log
            .lock()
            .unwrap()
            .iter()
            .skip(1)
            .map(|(_, result)| result.clone().expect("a move"))
            .collect()
    }

    fn feet(&self) -> [f32; 3] {
        pose_of(&self.world, self.hero).translation
    }

    fn physics(&self) -> &PhysicsWorld {
        self.world.resource::<PhysicsWorld>().expect("a world")
    }
}

fn fall(_: u64) -> Vec<[f32; 3]> {
    vec![[0.0, -0.1, 0.0]]
}

#[test]
fn landing_on_flat_ground_reports_landed_once_then_grounded() {
    let mut rig = Rig::new(
        |world| {
            floor(world);
            hero(world, [0.0, 0.5, 0.0])
        },
        fall,
    );
    rig.run(20);
    let reports = rig.reports();
    let landed: Vec<usize> = reports
        .iter()
        .enumerate()
        .filter(|(_, r)| r.landed)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(landed.len(), 1, "landed exactly once: {landed:?}");
    assert!(reports[..landed[0]].iter().all(|r| !r.grounded));
    assert!(reports[landed[0]..].iter().all(|r| r.grounded));
    assert!((rig.feet()[1] - SKIN).abs() < 2e-3, "{:?}", rig.feet());
    let state = rig
        .world
        .get::<CharacterMotorState>(rig.hero)
        .unwrap()
        .expect("the motor writes its state");
    assert!(state.grounded && state.ground.is_some());
}

/// A static slab whose top surface passes through the origin, rising toward `+x` at `degrees`
/// (negative degrees descend).
fn slope(world: &mut World, degrees: f32, half_length: f32, shift: f32) -> EntityId {
    let (s, c) = sin_cos(degrees.to_radians());
    // Up the slope is (c, s); the top normal is (-s, c).
    let center = [c * shift + 0.25 * s, s * shift - 0.25 * c, 0.0];
    spawn_box(
        world,
        center,
        [half_length, 0.25, 5.0],
        about_z(degrees),
        Body::new_static(),
    )
}

#[test]
fn walking_down_a_thirty_degree_slope_never_flickers_airborne() {
    let mut rig = Rig::new(
        |world| {
            slope(world, -30.0, 5.0, 0.0);
            hero(world, [-3.0, 1.9, 0.0])
        },
        |_| vec![[0.05, -0.1, 0.0]],
    );
    rig.run(60);
    let reports = rig.reports();
    let first = reports.iter().position(|r| r.grounded).expect("lands");
    assert!(
        reports[first..].iter().all(|r| r.grounded),
        "airborne flicker on the slope"
    );
    assert!(rig.feet()[0] > -1.0, "kept walking: {:?}", rig.feet());
}

#[test]
fn a_sixty_degree_slope_is_not_climbed_and_slides_the_hero_back_down() {
    let mut rig = Rig::new(
        |world| {
            floor(world);
            slope(world, 60.0, 4.0, 4.0);
            hero(world, [0.6, 1.2, 0.0])
        },
        |_| vec![[0.05, -0.1, 0.0]],
    );
    rig.run(90);
    let reports = rig.reports();
    assert!(
        reports
            .iter()
            .any(|r| r.ground.is_some_and(|ground| !ground.walkable)),
        "the steep face is reported as non-walkable ground"
    );
    assert!(
        reports
            .iter()
            .any(|r| r.ground.is_some_and(|g| !g.walkable) && !r.grounded)
    );
    // It starts 1.2 up the face and ends on the floor: nothing climbed it.
    assert!(rig.feet()[1] < 0.2, "{:?}", rig.feet());
}

#[test]
fn walking_off_a_ledge_goes_airborne_and_is_never_snapped_down() {
    let mut rig = Rig::new(
        |world| {
            spawn_box(
                world,
                [-8.0, -0.5, 0.0],
                [10.0, 0.5, 5.0],
                FLAT,
                Body::new_static(),
            );
            spawn_box(
                world,
                [0.0, -3.5, 0.0],
                [30.0, 0.5, 30.0],
                FLAT,
                Body::new_static(),
            );
            hero(world, [0.0, 0.0, 0.0])
        },
        |_| vec![[0.05, -0.1, 0.0]],
    );
    rig.run(60);
    let reports = rig.reports();
    let off = reports
        .iter()
        .position(|r| r.grounded)
        .and_then(|first| {
            reports[first..]
                .iter()
                .position(|r| !r.grounded)
                .map(|i| i + first)
        })
        .expect("walks off the lip");
    for report in &reports[off..off + 10] {
        assert!(!report.grounded, "snapped back onto the ledge");
        assert!(report.moved[1] >= -0.1 - 1e-4, "{:?}", report.moved);
    }
}

#[test]
fn takeoff_is_not_grounded_on_the_flat_or_on_a_down_slope() {
    for on_slope in [false, true] {
        let mut rig = Rig::new(
            move |world| {
                if on_slope {
                    slope(world, -30.0, 5.0, 0.0);
                    hero(world, [-2.0, 1.17, 0.0])
                } else {
                    floor(world);
                    hero(world, [0.0, 0.0, 0.0])
                }
            },
            |frame| {
                if frame == 40 {
                    vec![[0.05, 0.15, 0.0]]
                } else {
                    vec![[0.05, -0.1, 0.0]]
                }
            },
        );
        rig.run(41);
        let reports = rig.reports();
        assert!(
            reports[38].grounded,
            "settled before the jump (slope: {on_slope})"
        );
        assert!(!reports[39].grounded, "takeoff tick (slope: {on_slope})");
    }
}

#[test]
fn rising_under_a_slab_reports_the_ceiling_and_removes_the_upward_motion() {
    let mut slab = None;
    let mut rig = Rig::new(
        |world| {
            floor(world);
            slab = Some(wall(world, [0.0, 1.45, 0.0], [3.0, 0.25, 3.0]));
            hero(world, [0.0, 0.0, 0.0])
        },
        |frame| {
            if frame < 3 {
                vec![[0.0, -0.05, 0.0]]
            } else {
                vec![[0.0, 0.1, 0.0]]
            }
        },
    );
    rig.run(14);
    let reports = rig.reports();
    let hit = reports
        .iter()
        .find_map(|r| r.ceiling)
        .expect("a ceiling hit");
    assert_eq!(Some(hit.entity), slab);
    let crown = rig.feet()[1] + HEIGHT;
    assert!(
        crown <= 1.2 + 1e-3 && crown > 1.2 - 0.03,
        "crown at {crown}"
    );
}

#[test]
fn a_ninety_degree_corner_and_a_ceiling_wedge_settle_without_jitter() {
    let mut walls = Vec::new();
    let mut corner = Rig::new(
        |world| {
            floor(world);
            walls.push(wall(world, [2.5, 1.0, 0.0], [0.5, 2.0, 6.0]));
            walls.push(wall(world, [0.0, 1.0, 2.5], [6.0, 2.0, 0.5]));
            hero(world, [1.0, 0.0, 1.0])
        },
        |_| vec![[0.1, -0.05, 0.1]],
    );
    let mut positions = Vec::new();
    for _ in 0..40 {
        corner.run(1);
        positions.push(corner.feet());
    }
    let reports = corner.reports();
    let contact = reports
        .iter()
        .position(|r| {
            walls
                .iter()
                .all(|w| r.walls.iter().any(|hit| hit.entity == *w))
        })
        .expect("both walls are reported");
    assert_settled_within_two_ticks(&positions, contact, 10);
    assert!(reports.iter().all(|r| !r.stuck));

    // A ceiling descending toward +x: its underside passes through (2, 1.2), 20 degrees down.
    let mut wedge = Rig::new(
        |world| {
            floor(world);
            let (s, c) = sin_cos(20.0f32.to_radians());
            spawn_box(
                world,
                [2.0 + 0.25 * s, 1.2 + 0.25 * c, 0.0],
                [4.0, 0.25, 5.0],
                about_z(-20.0),
                Body::new_static(),
            );
            hero(world, [0.0, 0.0, 0.0])
        },
        |_| vec![[0.1, -0.05, 0.0]],
    );
    let mut positions = Vec::new();
    for _ in 0..60 {
        wedge.run(1);
        positions.push(wedge.feet());
    }
    let contact = wedge
        .reports()
        .iter()
        .position(|r| r.ceiling.is_some())
        .expect("the wedge is touched");
    assert_settled_within_two_ticks(&positions, contact, 30);
    assert!(positions[contact + 2][0] > 1.5, "walked into the wedge");
}

/// From two ticks after the first contact the horizontal pose is fixed and the height within
/// cast rounding (10 ticks to spare); from `exact_from` on, the whole pose is bit-identical.
fn assert_settled_within_two_ticks(positions: &[[f32; 3]], contact: usize, exact_from: usize) {
    assert!(contact + 12 <= positions.len(), "contact at {contact}");
    let settled = positions[contact + 2];
    for (tick, pose) in positions.iter().enumerate().skip(contact + 2) {
        assert!(
            pose[0] == settled[0] && pose[2] == settled[2] && (pose[1] - settled[1]).abs() < 1e-5,
            "tick {tick} (contact at {contact}): {pose:?} vs {settled:?}"
        );
    }
    assert_eq!(
        positions[exact_from..],
        vec![positions[exact_from]; positions.len() - exact_from],
        "{positions:?}"
    );
}

fn box_pose(rig: &Rig, entity: EntityId) -> [f32; 3] {
    pose_of(&rig.world, entity).translation
}

#[test]
fn walking_into_a_dynamic_box_pushes_it() {
    let mut crate_ = None;
    let mut rig = Rig::new(
        |world| {
            floor(world);
            crate_ = Some(spawn_box(
                world,
                [1.0, 0.25, 0.0],
                [0.25, 0.25, 0.25],
                FLAT,
                Body::Rigid(RigidBody::default()),
            ));
            hero(world, [0.0, 0.0, 0.0])
        },
        |_| vec![[0.05, -0.05, 0.0]],
    );
    rig.run(60);
    let crate_ = crate_.unwrap();
    assert!(
        box_pose(&rig, crate_)[0] > 2.0,
        "{:?}",
        box_pose(&rig, crate_)
    );
    assert!(rig.feet()[0] > 1.5);
    assert!(rig.reports().iter().all(|r| !r.stuck));
}

#[test]
fn a_dynamic_box_launched_at_the_idle_hero_leaves_its_feet_untouched() {
    let mut rig = Rig::new(
        |world| {
            floor(world);
            let crate_ = spawn_box(
                world,
                [4.0, 0.25, 0.0],
                [0.25, 0.25, 0.25],
                FLAT,
                Body::Rigid(RigidBody::default()),
            );
            world
                .insert(
                    crate_,
                    Velocity {
                        linear: [-12.0, 0.0, 0.0],
                        angular: [0.0; 3],
                    },
                )
                .unwrap();
            hero(world, [0.0, 0.0, 0.0])
        },
        |_| vec![[0.0, -0.05, 0.0]],
    );
    rig.run(3);
    let settled = rig.feet();
    for _ in 0..60 {
        rig.run(1);
        assert_eq!(rig.feet().map(f32::to_bits), settled.map(f32::to_bits));
    }
}

#[test]
fn a_prop_pinned_against_a_wall_blocks_instead_of_being_overlapped_without_limit() {
    let mut crate_ = None;
    let mut rig = Rig::new(
        |world| {
            floor(world);
            wall(world, [3.5, 1.0, 0.0], [0.5, 2.0, 5.0]);
            crate_ = Some(spawn_box(
                world,
                [1.5, 0.25, 0.0],
                [0.25, 0.25, 0.25],
                FLAT,
                Body::Rigid(RigidBody::default()),
            ));
            hero(world, [0.0, 0.0, 0.0])
        },
        |_| vec![[0.05, -0.05, 0.0]],
    );
    let mut sunk = Vec::new();
    let mut gap = 0.0;
    for _ in 0..120 {
        rig.run(1);
        let feet = rig.feet();
        let at = box_pose(&rig, crate_.unwrap());
        assert!(feet.iter().chain(at.iter()).all(|c| c.is_finite()));
        sunk.push(at[0] + 0.25 - 3.0);
        gap = (at[0] - 0.25) - (feet[0] + RADIUS);
    }
    let worst = sunk.iter().copied().fold(f32::MIN, f32::max);
    // The crate arrives at the push speed, so the tick it meets the wall sinks it a couple of
    // centimetres (discrete collision); the solver then ejects it and it stays out.
    assert!(worst <= 0.03, "the crate sank {worst} into the wall");
    let settled = sunk[60..].iter().copied().fold(f32::MIN, f32::max);
    assert!(settled <= 0.01, "the crate stays in the wall: {settled}");
    assert!(
        settled <= sunk[60] + 1e-3,
        "the crate keeps creeping: {sunk:?}"
    );
    assert!(
        (SKIN * 0.4..SKIN * 2.0).contains(&gap),
        "the hero stops a skin from the pinned crate, not {gap}"
    );
}

#[test]
fn a_capsule_crushed_between_two_static_boxes_is_stuck_and_finite() {
    let mut rig = Rig::new(
        |world| {
            floor(world);
            wall(world, [0.7, 1.0, 0.0], [0.5, 2.0, 5.0]);
            wall(world, [-0.7, 1.0, 0.0], [0.5, 2.0, 5.0]);
            hero(world, [0.0, 0.0, 0.0])
        },
        |_| vec![[0.0, -0.05, 0.0]],
    );
    rig.run(4);
    let reports = rig.reports();
    assert!(reports.iter().all(|r| r.stuck), "{reports:?}");
    assert!(rig.feet().iter().all(|c| c.is_finite()));
}

#[test]
fn the_motor_errors_name_what_is_missing() {
    let mut rig = Rig::new(
        |world| {
            floor(world);
            hero(world, [0.0, 0.0, 0.0])
        },
        |frame| {
            if frame == 2 {
                vec![[f32::NAN, 0.0, 0.0]]
            } else {
                fall(frame)
            }
        },
    );
    rig.run(3);
    let log = rig.log.lock().unwrap().clone();
    assert_eq!(log[0].1, Err(MotorError::NoPhysicsWorld));
    assert!(log[1].1.is_ok());
    assert!(matches!(
        log[2].1,
        Err(MotorError::NonFiniteDisplacement(_))
    ));
    drop(log);

    let bare = rig.world.spawn();
    assert_eq!(
        move_character::<TestPose, NoParent>(&mut rig.world, bare, [0.0; 3]),
        Err(MotorError::NoMotor(bare))
    );
    let poseless = rig.world.spawn();
    rig.world.insert(poseless, motor()).unwrap();
    assert_eq!(
        move_character::<TestPose, NoParent>(&mut rig.world, poseless, [0.0; 3]),
        Err(MotorError::NoPose(poseless))
    );
    let broken = rig.world.spawn();
    rig.world.insert(broken, TestPose::default()).unwrap();
    rig.world
        .insert(
            broken,
            CharacterMotor {
                height: 0.1,
                ..motor()
            },
        )
        .unwrap();
    let error = move_character::<TestPose, NoParent>(&mut rig.world, broken, [0.0; 3]).unwrap_err();
    assert!(matches!(error, MotorError::InvalidMotor { .. }), "{error}");
}

#[test]
fn a_slide_constraint_keeps_x_fixed_through_a_wall_slide() {
    let mut rig = Rig::with(
        |world| {
            floor(world);
            wall(world, [0.0, 1.0, 2.5], [6.0, 2.0, 0.5]);
            hero(world, [0.0, 0.0, 0.0])
        },
        |_| vec![[0.1, -0.05, 0.1]],
        true,
    );
    rig.run(30);
    assert_eq!(rig.feet()[0], 0.0);
    assert!(rig.feet()[2] > 1.0, "{:?}", rig.feet());
}

#[test]
fn the_last_report_carries_its_frame_and_is_dropped_by_a_restore() {
    let mut rig = Rig::new(
        |world| {
            floor(world);
            hero(world, [0.0, 0.0, 0.0])
        },
        |frame| {
            if frame == 5 {
                vec![[0.0, -0.05, 0.0], [0.1, -0.05, 0.0]]
            } else {
                fall(frame)
            }
        },
    );
    rig.run(5);
    {
        let (frame, report) = rig.physics().last_move_report(rig.hero).expect("a report");
        assert_eq!(frame.as_u64(), 4);
        let (_, last) = rig.log.lock().unwrap().last().cloned().unwrap();
        assert_eq!(last.as_ref().unwrap(), report);
    }
    rig.run(1);
    let (frame, report) = rig.physics().last_move_report(rig.hero).unwrap();
    assert_eq!(frame.as_u64(), 5);
    let log = rig.log.lock().unwrap();
    let second = log.last().unwrap().1.as_ref().unwrap();
    assert_eq!(second, report, "the second call overwrote the first");
    assert!(second.moved[0] > 0.0);
    drop(log);
    let bytes = rig.physics().to_bytes().unwrap();
    let restored = PhysicsWorld::from_bytes(&bytes, TickRate::DEFAULT).unwrap();
    assert!(restored.last_move_report(rig.hero).is_none());
}

fn determinism_run() -> (Vec<String>, Vec<[f32; 3]>) {
    let mut rig = Rig::new(
        |world| {
            floor(world);
            slope(world, 25.0, 3.0, 3.0);
            wall(world, [-2.0, 0.1, 0.0], [0.5, 0.1, 6.0]);
            wall(world, [6.5, 1.0, 0.0], [0.5, 2.0, 6.0]);
            wall(world, [3.0, 1.0, 3.5], [6.0, 2.0, 0.5]);
            hero(world, [-3.5, 0.0, 0.0])
        },
        |frame| {
            if frame < 70 {
                vec![[0.06, -0.1, 0.0]]
            } else {
                vec![[0.06, -0.1, 0.06]]
            }
        },
    );
    let mut poses = Vec::new();
    for _ in 0..140 {
        rig.run(1);
        poses.push(rig.feet());
    }
    let reports = rig.reports().iter().map(|r| format!("{r:?}")).collect();
    (reports, poses)
}

#[test]
fn the_report_sequence_is_identical_at_one_and_eight_threads() {
    let one = with_threads(1, determinism_run);
    let eight = with_threads(8, determinism_run);
    assert_eq!(one.0, eight.0);
    assert_eq!(
        one.1
            .iter()
            .flatten()
            .map(|c| c.to_bits())
            .collect::<Vec<_>>(),
        eight
            .1
            .iter()
            .flatten()
            .map(|c| c.to_bits())
            .collect::<Vec<_>>()
    );
    assert!(one.0.iter().any(|r| r.contains("walls: [")));
    assert!(one.0.iter().any(|r| r.contains("stepped: Some")));
}

/// A static box spanning `x ∈ [from, to]` whose top is at `top`, wide enough to walk the length of.
fn block(world: &mut World, from: f32, to: f32, top: f32) -> EntityId {
    wall(
        world,
        [(from + to) / 2.0, top / 2.0, 0.0],
        [(to - from) / 2.0, top / 2.0, 5.0],
    )
}

fn run_east(_: u64) -> Vec<[f32; 3]> {
    vec![[0.06, -0.05, 0.0]]
}

#[test]
fn a_step_just_under_the_step_height_is_climbed_and_the_walk_continues() {
    let mut rig = Rig::new(
        |world| {
            floor(world);
            block(world, 1.5, 6.0, 0.24);
            hero(world, [0.0, 0.0, 0.0])
        },
        run_east,
    );
    let mut xs = vec![rig.feet()[0]];
    let mut ys = vec![rig.feet()[1]];
    for _ in 0..40 {
        rig.run(1);
        xs.push(rig.feet()[0]);
        ys.push(rig.feet()[1]);
    }
    let reports = rig.reports();
    assert_stepped_means_rose(&reports, &ys);
    let first = reports
        .iter()
        .position(|r| r.stepped.is_some())
        .expect("a step-up");
    // The climb tick travels no more than the commanded move plus a skin, and the hero never
    // leaves the ground afterwards.
    assert!(
        xs[first + 2] - xs[first + 1] <= 0.06 + SKIN,
        "{:?}",
        &xs[first..first + 4]
    );
    assert!(reports[first..].iter().all(|r| r.grounded));
    let last = reports.last().unwrap();
    assert!(last.walls.is_empty(), "{last:?}");
    assert!(
        (rig.feet()[1] - (0.24 + SKIN)).abs() < 2e-3,
        "{:?}",
        rig.feet()
    );
    assert!(rig.feet()[0] > 2.0, "{:?}", rig.feet());
}

#[test]
fn a_step_just_over_the_step_height_blocks() {
    let mut rig = Rig::new(
        |world| {
            floor(world);
            block(world, 1.5, 6.0, 0.26);
            hero(world, [0.0, 0.0, 0.0])
        },
        run_east,
    );
    rig.run(40);
    let reports = rig.reports();
    assert!(reports.iter().all(|r| r.stepped.is_none()));
    assert!(reports.last().unwrap().walls.len() == 1);
    assert!(rig.feet()[0] < 1.5 - RADIUS + 0.01, "{:?}", rig.feet());
}

#[test]
fn a_step_under_a_slab_without_headroom_is_refused() {
    let mut rig = Rig::new(
        |world| {
            floor(world);
            block(world, 1.5, 6.0, 0.2);
            // The slab's underside is 0.5 over the step top: less than the capsule height.
            wall(world, [3.75, 0.7 + 0.5, 0.0], [2.25, 0.5, 5.0]);
            hero(world, [0.0, 0.0, 0.0])
        },
        run_east,
    );
    rig.run(40);
    assert!(rig.reports().iter().all(|r| r.stepped.is_none()));
    assert!(rig.feet()[0] < 1.5, "{:?}", rig.feet());
}

#[test]
fn an_airborne_capsule_does_not_step_onto_a_lip() {
    let mut rig = Rig::new(
        |world| {
            floor(world);
            block(world, 1.5, 6.0, 0.1);
            hero(world, [0.0, 0.05, 0.0])
        },
        |_| vec![[0.1, 0.0, 0.0]],
    );
    rig.run(30);
    let reports = rig.reports();
    // The rounded bottom may ride up the lip's edge and land on it, but never as a step.
    assert!(reports.iter().all(|r| r.stepped.is_none()));
    assert!(
        reports[..10].iter().all(|r| !r.grounded),
        "{:?}",
        &reports[..10]
    );
}

fn strip(y: f32, x0: f32, x1: f32, quads: u32) -> TriangleMesh {
    let mut vertices = Vec::new();
    for i in 0..=quads {
        let x = x0 + (x1 - x0) * i as f32 / quads as f32;
        vertices.push([x, y, -2.0]);
        vertices.push([x, y, 2.0]);
    }
    let mut triangles = Vec::new();
    for i in 0..quads {
        let a = i * 2;
        triangles.push([a, a + 2, a + 1]);
        triangles.push([a + 1, a + 2, a + 3]);
    }
    TriangleMesh {
        vertices,
        triangles,
        hash: 1,
        ..Default::default()
    }
}

fn mesh_floor(world: &mut World, x0: f32, x1: f32, quads: u32) {
    world.insert_resource(ColliderMeshes::new(Box::new(move |_| {
        Ok(strip(0.0, x0, x1, quads))
    })));
    let entity = world.spawn();
    world.insert(entity, TestPose::at([0.0; 3])).unwrap();
    world
        .insert(
            entity,
            Collider {
                shape: Shape::Mesh {
                    path: "strip".into(),
                },
                body: Body::new_static(),
                layer: None,
                sensor: false,
            },
        )
        .unwrap();
}

/// One trimesh holding a floor, a vertical riser at `x = 1.5` and a top at height `top`.
fn mesh_stairs(world: &mut World, top: f32) {
    world.insert_resource(ColliderMeshes::new(Box::new(move |_| {
        let floor = strip(0.0, -2.0, 1.5, 1);
        let upper = strip(top, 1.5, 6.0, 1);
        let mut vertices = floor.vertices;
        let mut triangles = floor.triangles;
        let base = vertices.len() as u32;
        vertices.extend(upper.vertices);
        triangles.extend(upper.triangles.iter().map(|t| t.map(|i| i + base)));
        let base = vertices.len() as u32;
        vertices.extend([
            [1.5, 0.0, -2.0],
            [1.5, 0.0, 2.0],
            [1.5, top, -2.0],
            [1.5, top, 2.0],
        ]);
        triangles.push([base, base + 2, base + 1]);
        triangles.push([base + 1, base + 2, base + 3]);
        Ok(TriangleMesh {
            vertices,
            triangles,
            hash: 2,
            ..Default::default()
        })
    })));
    let entity = world.spawn();
    world.insert(entity, TestPose::at([0.0; 3])).unwrap();
    world
        .insert(
            entity,
            Collider {
                shape: Shape::Mesh {
                    path: "stairs".into(),
                },
                body: Body::new_static(),
                layer: None,
                sensor: false,
            },
        )
        .unwrap();
}

/// One trimesh holding a floor and two perpendicular walls, at `x = 2.5` and `z = 2.5`.
fn mesh_corner(world: &mut World) -> EntityId {
    world.insert_resource(ColliderMeshes::new(Box::new(|_| {
        let vertices = vec![
            [-3.0, 0.0, -3.0],
            [-3.0, 0.0, 3.0],
            [3.0, 0.0, -3.0],
            [3.0, 0.0, 3.0],
            [2.5, 0.0, -3.0],
            [2.5, 0.0, 3.0],
            [2.5, 2.0, -3.0],
            [2.5, 2.0, 3.0],
            [-3.0, 0.0, 2.5],
            [3.0, 0.0, 2.5],
            [-3.0, 2.0, 2.5],
            [3.0, 2.0, 2.5],
        ];
        let triangles = vec![
            [0, 1, 2],
            [2, 1, 3],
            [4, 6, 5],
            [5, 6, 7],
            [8, 9, 10],
            [10, 9, 11],
        ];
        Ok(TriangleMesh {
            vertices,
            triangles,
            hash: 3,
            ..Default::default()
        })
    })));
    let entity = world.spawn();
    world.insert(entity, TestPose::at([0.0; 3])).unwrap();
    world
        .insert(
            entity,
            Collider {
                shape: Shape::Mesh {
                    path: "corner".into(),
                },
                body: Body::new_static(),
                layer: None,
                sensor: false,
            },
        )
        .unwrap();
    entity
}

#[test]
fn an_inside_corner_of_one_mesh_reports_both_walls() {
    let mut mesh = None;
    let mut rig = Rig::new(
        |world| {
            mesh = Some(mesh_corner(world));
            hero(world, [1.0, 0.0, 1.0])
        },
        |_| vec![[0.1, -0.05, 0.1]],
    );
    let mut positions = Vec::new();
    for _ in 0..40 {
        rig.run(1);
        positions.push(rig.feet());
    }
    let reports = rig.reports();
    let contact = reports
        .iter()
        .position(|r| r.walls.len() == 2)
        .expect("a report names two walls");
    let walls = &reports[contact].walls;
    assert!(walls.iter().all(|hit| Some(hit.entity) == mesh));
    assert!(walls.iter().any(|hit| hit.normal[0] < -0.99));
    assert!(walls.iter().any(|hit| hit.normal[2] < -0.99));
    assert_settled_within_two_ticks(&positions, contact, contact + 2);
}

#[test]
fn a_mesh_step_just_under_the_step_height_is_climbed() {
    let mut rig = Rig::new(
        |world| {
            mesh_stairs(world, 0.24);
            hero(world, [0.0, 0.0, 0.0])
        },
        run_east,
    );
    let mut ys = vec![rig.feet()[1]];
    for _ in 0..40 {
        rig.run(1);
        ys.push(rig.feet()[1]);
    }
    let reports = rig.reports();
    assert!(reports.iter().any(|r| r.stepped.is_some()));
    assert_stepped_means_rose(&reports, &ys);
    assert!(reports.last().unwrap().walls.is_empty());
    assert!(
        (rig.feet()[1] - (0.24 + SKIN)).abs() < 2e-3,
        "{:?}",
        rig.feet()
    );
    assert!(rig.feet()[0] > 2.0, "{:?}", rig.feet());
}

#[test]
fn a_mesh_step_of_two_tenths_is_climbed() {
    let mut rig = Rig::new(
        |world| {
            mesh_stairs(world, 0.2);
            hero(world, [0.0, 0.0, 0.0])
        },
        run_east,
    );
    rig.run(40);
    assert!(
        (rig.feet()[1] - (0.2 + SKIN)).abs() < 2e-3,
        "{:?}",
        rig.feet()
    );
    assert!(rig.feet()[0] > 2.0, "{:?}", rig.feet());
}

#[test]
fn a_mesh_step_just_over_the_step_height_blocks() {
    let mut rig = Rig::new(
        |world| {
            mesh_stairs(world, 0.26);
            hero(world, [0.0, 0.0, 0.0])
        },
        run_east,
    );
    rig.run(40);
    let reports = rig.reports();
    assert!(reports.iter().all(|r| r.stepped.is_none()));
    assert!(reports.last().unwrap().walls.len() == 1);
    assert!(rig.feet()[0] < 1.5 - RADIUS + 0.01, "{:?}", rig.feet());
}

#[test]
fn a_mesh_step_under_a_slab_without_headroom_is_refused() {
    let mut rig = Rig::new(
        |world| {
            mesh_stairs(world, 0.2);
            wall(world, [3.75, 0.7 + 0.5, 0.0], [2.25, 0.5, 5.0]);
            hero(world, [0.0, 0.0, 0.0])
        },
        run_east,
    );
    rig.run(40);
    assert!(rig.reports().iter().all(|r| r.stepped.is_none()));
    assert!(rig.feet()[0] < 1.5, "{:?}", rig.feet());
}

/// One trimesh: a floor, a 0.1 riser at `x = 1.5`, a landing, then a 20 degree ramp from `x = 3`.
fn mesh_step_then_ramp(world: &mut World) {
    world.insert_resource(ColliderMeshes::new(Box::new(move |_| {
        let top = 3.0 + 3.0;
        let rise = 0.1 + 3.0 * ludion_core::math::tan(20f32.to_radians());
        let profile = [[-2.0, 0.0], [1.5, 0.0], [1.5, 0.1], [3.0, 0.1], [top, rise]];
        let mut vertices = Vec::new();
        for [x, y] in profile {
            vertices.push([x, y, -2.0]);
            vertices.push([x, y, 2.0]);
        }
        let mut triangles = Vec::new();
        for i in 0..profile.len() as u32 - 1 {
            let a = i * 2;
            triangles.push([a, a + 2, a + 1]);
            triangles.push([a + 1, a + 2, a + 3]);
        }
        Ok(TriangleMesh {
            vertices,
            triangles,
            hash: 3,
            ..Default::default()
        })
    })));
    let entity = world.spawn();
    world.insert(entity, TestPose::at([0.0; 3])).unwrap();
    world
        .insert(
            entity,
            Collider {
                shape: Shape::Mesh {
                    path: "step-ramp".into(),
                },
                body: Body::new_static(),
                layer: None,
                sensor: false,
            },
        )
        .unwrap();
}

#[test]
fn a_mesh_step_then_a_ramp_is_walked_to_the_top() {
    let mut rig = Rig::new(
        |world| {
            mesh_step_then_ramp(world);
            hero(world, [0.0, 0.0, 0.0])
        },
        run_east,
    );
    for _ in 0..150 {
        rig.run(1);
        if rig.feet()[0] > 5.5 {
            break;
        }
    }
    assert!(rig.feet()[0] > 5.5, "{:?}", rig.feet());
    assert!(rig.feet()[1] > 1.0, "{:?}", rig.feet());
    assert!(rig.reports().last().unwrap().grounded);
}

/// `ys[t]` is the feet height after `t` ticks and `reports[i]` is tick `i + 1`'s: a reported
/// climb must have been a real rise.
fn assert_stepped_means_rose(reports: &[MoveReport], ys: &[f32]) {
    for (i, report) in reports.iter().enumerate() {
        if let Some(c) = report.stepped {
            assert!(
                ys[i + 2] - ys[i + 1] >= c - 1e-4,
                "tick {}: stepped {c}, feet {} -> {}",
                i + 1,
                ys[i + 1],
                ys[i + 2]
            );
        }
    }
}

fn assert_seamless(rig: &mut Rig, ticks: u64) {
    let mut ys = Vec::new();
    for _ in 0..ticks {
        rig.run(1);
        ys.push(rig.feet()[1]);
    }
    let reports = rig.reports();
    for (i, report) in reports.iter().enumerate().skip(3) {
        assert!(
            report.grounded && report.walls.is_empty(),
            "tick {i}: {report:?}"
        );
        assert!(report.stepped.is_none(), "tick {i}: {report:?}");
        let dy = (ys[i + 1] - ys[i]).abs();
        assert!(dy < 1e-4, "tick {i}: dy {dy}");
    }
}

#[test]
fn running_across_twenty_coplanar_triangles_never_bumps() {
    let mut rig = Rig::new(
        |world| {
            mesh_floor(world, -1.0, 5.0, 10);
            hero(world, [0.0, 0.0, 0.0])
        },
        |_| vec![[0.05, -0.05, 0.0]],
    );
    assert_seamless(&mut rig, 80);
    assert!(rig.feet()[0] > 3.9, "{:?}", rig.feet());
}

#[test]
fn running_from_a_mesh_floor_onto_a_box_floor_never_bumps() {
    let mut rig = Rig::new(
        |world| {
            mesh_floor(world, -4.0, 0.0, 4);
            spawn_box(
                world,
                [3.0, -0.5, 0.0],
                [3.0, 0.5, 5.0],
                FLAT,
                Body::new_static(),
            );
            hero(world, [-2.0, 0.0, 0.0])
        },
        |_| vec![[0.05, -0.05, 0.0]],
    );
    assert_seamless(&mut rig, 80);
    assert!(rig.feet()[0] > 1.5, "{:?}", rig.feet());
}

// probe_character_ground

fn idle(_: u64) -> Vec<[f32; 3]> {
    vec![[0.0, -0.05, 0.0]]
}

fn probe(rig: &Rig, feet: [f32; 3]) -> Option<GroundProbe> {
    probe_character_ground::<TestPose>(&rig.world, rig.hero, feet).expect("a probe")
}

/// A static box floor spanning `x ∈ [from, to]` with its top at `y = 0`.
fn slab(world: &mut World, from: f32, to: f32) -> EntityId {
    wall(
        world,
        [(from + to) / 2.0, -0.5, 0.0],
        [(to - from) / 2.0, 0.5, 5.0],
    )
}

#[test]
fn a_probe_at_a_floor_edge_stands_inside_and_finds_nothing_past_the_radius() {
    let mut rig = Rig::new(
        |world| {
            slab(world, -20.0, 2.0);
            hero(world, [0.0, 0.0, 0.0])
        },
        idle,
    );
    rig.run(4);
    let inside = probe(&rig, [1.9, 0.0, 0.0]).expect("ground 0.1 m inside the edge");
    assert!(inside.ground.walkable);
    assert!(
        (inside.rest_y.expect("walkable") - SKIN).abs() < 2e-3,
        "{inside:?}"
    );
    assert_eq!(probe(&rig, [2.0 + RADIUS + 0.05, 0.0, 0.0]), None);
}

#[test]
fn a_probe_across_a_crack_as_wide_as_the_radius_is_grounded_like_the_walk() {
    let mut rig = Rig::new(
        |world| {
            slab(world, -20.0, 2.0);
            slab(world, 2.0 + RADIUS, 20.0);
            hero(world, [1.0, 0.0, 0.0])
        },
        run_east,
    );
    rig.run(40);
    assert!(rig.feet()[0] > 2.0 + RADIUS, "{:?}", rig.feet());
    assert!(rig.reports().iter().all(|r| r.grounded));

    let centre = 2.0 + RADIUS / 2.0;
    let found = probe(&rig, [centre, 0.0, 0.0]).expect("ground across the crack");
    assert!(found.ground.walkable && found.rest_y.is_some(), "{found:?}");
    let ray = rig
        .physics()
        .ray_cast(
            [centre, 0.25, 0.0],
            [0.0, -1.0, 0.0],
            0.5,
            &QueryFilter::new(),
        )
        .unwrap();
    assert!(
        ray.is_empty(),
        "the axis ray falls through the crack: {ray:?}"
    );
}

#[test]
fn a_probe_where_the_hero_rests_on_a_rim_matches_its_report() {
    let mut rig = Rig::new(
        |world| {
            slab(world, -20.0, 2.0);
            hero(world, [2.0 + RADIUS / 2.0, 0.3, 0.0])
        },
        fall,
    );
    rig.run(20);
    let last = rig.reports().pop().unwrap();
    let ground = last.ground.expect("a support");
    assert!(last.grounded && ground.walkable, "{last:?}");
    let found = probe(&rig, rig.feet()).expect("the rim support");
    assert!(found.ground.walkable);
    assert_eq!(found.ground.entity, ground.entity);
    assert!(
        (found.rest_y.expect("walkable") - rig.feet()[1]).abs() < 1e-3,
        "{found:?} {:?}",
        rig.feet()
    );
}

#[test]
fn a_probe_over_a_step_answers_either_side_of_the_step_height() {
    let eps = 0.01;
    for (top, walkable) in [(0.25 - eps, true), (0.25 + eps, false)] {
        let mut rig = Rig::new(
            |world| {
                floor(world);
                block(world, 1.5, 6.0, top);
                hero(world, [0.0, 0.0, 0.0])
            },
            idle,
        );
        rig.run(4);
        let found = probe(&rig, [3.0, 0.0, 0.0]);
        if walkable {
            let found = found.expect("the block under the step height");
            assert!(found.ground.walkable);
            assert!(
                (found.rest_y.unwrap() - (top + SKIN)).abs() < 2e-3,
                "{found:?}"
            );
        } else {
            assert_eq!(found, None, "a block over the step height is out of reach");
        }
    }
}

#[test]
fn a_crouched_motor_still_probes_walkable_on_the_flat() {
    let mut rig = Rig::new(
        |world| {
            floor(world);
            hero(world, [0.0, 0.0, 0.0])
        },
        idle,
    );
    rig.run(4);
    resize_character::<TestPose>(&mut rig.world, rig.hero, 0.6).unwrap();
    let found = probe(&rig, [1.0, 0.0, 0.0]).expect("ground");
    assert!(found.ground.walkable && found.rest_y.is_some(), "{found:?}");
}

#[test]
fn a_probe_changes_nothing() {
    let mut rig = Rig::new(
        |world| {
            floor(world);
            hero(world, [0.0, 0.0, 0.0])
        },
        idle,
    );
    rig.run(4);
    let pose = pose_of(&rig.world, rig.hero);
    let state = rig
        .world
        .get::<CharacterMotorState>(rig.hero)
        .unwrap()
        .cloned();
    let first = probe(&rig, [1.0, 0.0, 0.0]);
    assert_eq!(first, probe(&rig, [1.0, 0.0, 0.0]));
    assert_eq!(pose_of(&rig.world, rig.hero), pose);
    assert_eq!(
        rig.world
            .get::<CharacterMotorState>(rig.hero)
            .unwrap()
            .cloned(),
        state
    );
}

#[test]
fn the_probe_errors_name_what_is_missing() {
    let mut rig = Rig::new(
        |world| {
            floor(world);
            hero(world, [0.0, 0.0, 0.0])
        },
        idle,
    );
    rig.run(3);
    let nan = [f32::NAN, 0.0, 0.0];
    assert!(matches!(
        probe_character_ground::<TestPose>(&rig.world, rig.hero, nan),
        Err(MotorError::NonFiniteFeet(_))
    ));
    let bare = rig.world.spawn();
    assert_eq!(
        probe_character_ground::<TestPose>(&rig.world, bare, [0.0; 3]),
        Err(MotorError::NoMotor(bare))
    );
    let poseless = rig.world.spawn();
    rig.world.insert(poseless, motor()).unwrap();
    assert_eq!(
        probe_character_ground::<TestPose>(&rig.world, poseless, [0.0; 3]),
        Err(MotorError::NoPose(poseless))
    );
}

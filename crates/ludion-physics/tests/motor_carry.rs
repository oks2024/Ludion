//! Platform carry: riders on yawing, rising, descending, translating and belt supports, the
//! closing press, a carry into a wall, rigid and kinematic landings, the once-per-tick rule, the
//! gameplay hold and thread-count invariance. A platform system registered before the motor system
//! writes the platform's pose each tick; the reports are the assertions.

mod common;

use std::sync::{Arc, Mutex};

use common::{TestPose, pose_of, tick};
use ludion_core::ids::EntityId;
use ludion_core::math::{atan2, sin_cos};
use ludion_core::par::with_threads;
use ludion_core::time::GameClock;
use ludion_ecs::{Schedule, World};
use ludion_physics::{
    CharacterMotor, CharacterMotorState, MoveReport, PhysicsSpace, SurfaceVelocity, install,
    move_character,
};
use ludion_spatial::{Body, Collider, RigidBody, Shape};

type Log = Arc<Mutex<Vec<(u64, MoveReport)>>>;
type Mover = Arc<dyn Fn(u64) -> Option<([f32; 3], [f32; 4])> + Send + Sync>;

const RADIUS: f32 = 0.28;
const HEIGHT: f32 = 0.9;
const SKIN: f32 = 0.01;
const FLAT: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

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

fn yaw_quat(radians: f32) -> [f32; 4] {
    let (s, c) = sin_cos(radians / 2.0);
    [0.0, s, 0.0, c]
}

fn yaw_of(q: [f32; 4]) -> f32 {
    2.0 * atan2(q[1], q[3])
}

fn wrap(angle: f32) -> f32 {
    let tau = 2.0 * core::f32::consts::PI;
    (angle + core::f32::consts::PI).rem_euclid(tau) - core::f32::consts::PI
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

fn hero(world: &mut World, feet: [f32; 3]) -> EntityId {
    let entity = world.spawn();
    world.insert(entity, TestPose::at(feet)).unwrap();
    world.insert(entity, motor()).unwrap();
    entity
}

/// A scene's rider and the platform whose pose the mover drives (a platform that never moves has
/// no mover).
struct Cast {
    hero: EntityId,
    platform: EntityId,
}

struct Rig {
    world: World,
    schedule: Schedule,
    hero: EntityId,
    log: Log,
    frame: u64,
    clock: bool,
}

impl Rig {
    fn new(build: impl FnOnce(&mut World) -> Cast, mover: Option<Mover>) -> Rig {
        Rig::with(build, mover, 1, None)
    }

    /// `calls` motor calls per tick; `hold_at` marks that frame held on a `GameClock`.
    fn with(
        build: impl FnOnce(&mut World) -> Cast,
        mover: Option<Mover>,
        calls: usize,
        hold_at: Option<u64>,
    ) -> Rig {
        Rig::stepping(build, mover, calls, hold_at, [0.0; 3])
    }

    /// As [`with`](Rig::with), the rider also walking `step` a tick in the platform's frame as
    /// the motor sees it: the platform's pose of the previous tick, which is where it casts.
    fn stepping(
        build: impl FnOnce(&mut World) -> Cast,
        mover: Option<Mover>,
        calls: usize,
        hold_at: Option<u64>,
        step: [f32; 3],
    ) -> Rig {
        let seen = mover.clone();
        let mut world = World::new();
        let Cast { hero, platform } = build(&mut world);
        let clock = hold_at.is_some();
        if clock {
            world.insert_resource(GameClock::default());
        }
        let log: Log = Arc::default();
        let sink = log.clone();
        let mut schedule = Schedule::new();
        schedule.add_system_shared(
            "test-platform",
            Arc::new(move |world: &mut World, ctx: &ludion_ecs::SystemCtx| {
                let frame = ctx.frame.as_u64();
                if let Some((translation, rotation)) = mover.as_ref().and_then(|m| m(frame))
                    && let Ok(Some(pose)) = world.get_mut::<TestPose>(platform)
                {
                    pose.translation = translation;
                    pose.rotation = rotation;
                }
                if hold_at == Some(frame)
                    && let Some(clock) = world.resource_mut::<GameClock>()
                {
                    clock.hold();
                }
            }),
        );
        schedule.add_system_shared(
            "test-motor",
            Arc::new(move |world: &mut World, ctx: &ludion_ecs::SystemCtx| {
                let frame = ctx.frame.as_u64();
                let yaw = seen
                    .as_ref()
                    .and_then(|m| m(frame.saturating_sub(1)))
                    .map_or(0.0, |(_, rotation)| yaw_of(rotation));
                let (s, c) = sin_cos(yaw);
                let walk = [
                    step[0] * c + step[2] * s,
                    step[1],
                    -step[0] * s + step[2] * c,
                ];
                for call in 0..calls {
                    let displacement = if call == 0 {
                        [walk[0], walk[1] - 0.1, walk[2]]
                    } else {
                        [0.0; 3]
                    };
                    if let Ok(report) = move_character::<TestPose>(world, hero, displacement) {
                        sink.lock().unwrap().push((frame, report));
                    }
                }
            }),
        );
        install::<TestPose>(&mut schedule, PhysicsSpace::Free);
        Rig {
            world,
            schedule,
            hero,
            log,
            frame: 0,
            clock,
        }
    }

    fn run(&mut self, ticks: u64) -> &mut Rig {
        for _ in 0..ticks {
            if self.clock
                && let Some(clock) = self.world.resource_mut::<GameClock>()
            {
                clock.advance();
            }
            tick(&mut self.world, &mut self.schedule, self.frame);
            self.frame += 1;
        }
        self
    }

    /// Every report with the frame it ran in.
    fn reports(&self) -> Vec<(u64, MoveReport)> {
        self.log.lock().unwrap().clone()
    }

    fn feet(&self) -> [f32; 3] {
        pose_of(&self.world, self.hero).translation
    }

    /// The hero's feet and yaw after each tick, for sampling a ride.
    fn ride(&mut self, ticks: u64, mut sample: impl FnMut(u64, [f32; 3], f32)) {
        for _ in 0..ticks {
            self.run(1);
            let pose = pose_of(&self.world, self.hero);
            sample(self.frame - 1, pose.translation, yaw_of(pose.rotation));
        }
    }
}

fn mover(f: impl Fn(u64) -> ([f32; 3], [f32; 4]) + Send + Sync + 'static) -> Option<Mover> {
    Some(Arc::new(move |frame| Some(f(frame))))
}

/// A kinematic slab whose top starts at `y = 0`, half extents `[2, 0.25, 2]`.
fn slab(world: &mut World, body: Body) -> EntityId {
    spawn_box(world, [0.0, -0.25, 0.0], [2.0, 0.25, 2.0], FLAT, body)
}

fn point_in_platform_frame(feet: [f32; 3], origin: [f32; 3], yaw: f32) -> [f32; 3] {
    let (s, c) = sin_cos(yaw);
    let d = [
        feet[0] - origin[0],
        feet[1] - origin[1],
        feet[2] - origin[2],
    ];
    [d[0] * c - d[2] * s, d[1], d[0] * s + d[2] * c]
}

#[test]
fn a_rider_walking_on_a_yawing_platform_keeps_its_platform_local_step() {
    let degree = 1.0f32.to_radians();
    let step = [0.01, 0.0, 0.005];
    let mut rig = Rig::stepping(
        |world| Cast {
            platform: slab(world, Body::new_kinematic()),
            hero: hero(world, [-0.6, 0.3, -0.3]),
        },
        mover(move |f| ([0.0, -0.25, 0.0], yaw_quat(degree * f as f32))),
        1,
        None,
        step,
    );
    let (mut start, mut end) = (None, None);
    rig.ride(151, |frame, feet, _| {
        let local = point_in_platform_frame(feet, [0.0, -0.0, 0.0], degree * frame as f32);
        if frame == 30 {
            start = Some(local);
        }
        end = Some(local);
    });
    let (start, end) = (start.unwrap(), end.unwrap());
    for axis in [0, 2] {
        let walked = end[axis] - start[axis];
        let want = 120.0 * step[axis];
        assert!(
            (walked - want).abs() < 1e-3,
            "axis {axis}: walked {walked}, wanted {want}"
        );
    }
}

#[test]
fn a_rider_on_a_yawing_platform_stays_fixed_to_it() {
    let degree = 1.0f32.to_radians();
    let mut rig = Rig::new(
        |world| Cast {
            platform: slab(world, Body::new_kinematic()),
            hero: hero(world, [1.0, 0.3, 0.0]),
        },
        mover(move |f| ([0.0, -0.25, 0.0], yaw_quat(degree * f as f32))),
    );
    let mut start = None;
    let mut last = None;
    rig.ride(600, |frame, feet, yaw| {
        if frame == 30 {
            start = Some((
                point_in_platform_frame(feet, [0.0; 3], degree * frame as f32),
                yaw - degree * frame as f32,
            ));
        }
        last = Some((frame, feet, yaw));
    });
    let (frame, feet, yaw) = last.unwrap();
    let (local, offset) = start.unwrap();
    let now = point_in_platform_frame(feet, [0.0, 0.0, 0.0], degree * frame as f32);
    for axis in 0..3 {
        assert!(
            (now[axis] - local[axis]).abs() < 1e-3,
            "{axis}: {now:?} vs {local:?}"
        );
    }
    let turned = wrap(yaw - degree * frame as f32 - offset);
    assert!(
        turned.abs() < 1e-3,
        "rider yaw drifted from the platform's by {turned}"
    );
    let reports = rig.reports();
    for (frame, report) in &reports[40..] {
        assert!(report.platform.entity.is_some(), "frame {frame}");
        assert!(
            (report.platform.yaw - degree).abs() < 1e-4,
            "frame {frame}: {}",
            report.platform.yaw
        );
        assert!(report.grounded, "frame {frame}");
    }
}

/// The belt, yawed 90° so its local +X is world -Z, over a floor a metre below.
fn belt_scene(world: &mut World) -> (Cast, EntityId) {
    let belt = spawn_box(
        world,
        [0.0, -0.25, 0.0],
        [1.0, 0.25, 1.0],
        yaw_quat(core::f32::consts::FRAC_PI_2),
        Body::new_static(),
    );
    world
        .insert(
            belt,
            SurfaceVelocity {
                velocity: [1.5, 0.0, 0.0],
            },
        )
        .unwrap();
    let floor = spawn_box(
        world,
        [0.0, -1.5, 0.0],
        [30.0, 0.5, 30.0],
        FLAT,
        Body::new_static(),
    );
    (
        Cast {
            hero: hero(world, [0.0, 0.3, 0.0]),
            platform: belt,
        },
        floor,
    )
}

#[test]
fn a_conveyor_carries_a_still_rider_off_its_end() {
    let mut ids = None;
    let mut rig = Rig::new(
        |world| {
            let (cast, floor) = belt_scene(world);
            ids = Some((cast.platform, floor));
            cast
        },
        None,
    );
    let (belt, floor) = ids.unwrap();
    rig.run(160);
    let reports = rig.reports();
    let (s, c) = sin_cos(core::f32::consts::FRAC_PI_2);
    let expected = [1.5 * c / 60.0, 0.0, -1.5 * s / 60.0];
    let mut on_belt = 0;
    let mut on_floor = 0;
    let mut previous_grounded = false;
    let mut previous_entity = None;
    for (frame, report) in &reports {
        let entity = report.ground.map(|hit| hit.entity);
        if report.grounded
            && previous_grounded
            && entity == Some(belt)
            && previous_entity == Some(belt)
        {
            on_belt += 1;
            assert_eq!(report.platform.entity, Some(belt), "frame {frame}");
            for (axis, want) in expected.iter().enumerate() {
                assert!(
                    (report.platform.surface[axis] - want).abs() < 1e-5,
                    "frame {frame}: {:?}",
                    report.platform.surface
                );
                // Near the belt end the capsule tips over the rim and slides off it.
                if *frame <= 25 {
                    assert!(
                        (report.moved[axis] - report.platform.surface[axis]).abs() < 1e-5,
                        "frame {frame}: moved {:?}",
                        report.moved
                    );
                }
            }
        }
        if report.grounded && entity == Some(floor) {
            on_floor += 1;
            assert_eq!(report.moved[0], 0.0, "frame {frame}");
            assert_eq!(report.moved[2], 0.0, "frame {frame}");
        }
        previous_grounded = report.grounded;
        previous_entity = entity;
    }
    assert!(on_belt > 20, "rode the belt for {on_belt} ticks");
    assert!(on_floor > 20, "ended on the floor for {on_floor} ticks");
}

fn rising(slope: f32) -> Option<Mover> {
    mover(move |f| ([0.0, -0.25 + slope * f as f32, 0.0], FLAT))
}

fn press_scene(world: &mut World) -> Cast {
    Cast {
        platform: slab(world, Body::new_kinematic()),
        hero: hero(world, [0.0, 0.3, 0.0]),
    }
}

#[test]
fn a_rising_press_lifts_the_rider_every_tick() {
    let mut rig = Rig::new(press_scene, rising(0.02));
    let mut landed_at = None;
    rig.ride(120, |frame, feet, _| {
        if let Some(landed) = landed_at {
            if frame > landed {
                let top = 0.02 * frame as f32;
                assert!(
                    (feet[1] - top - SKIN).abs() < 1e-4,
                    "frame {frame}: feet {} top {top}",
                    feet[1]
                );
            }
        } else if feet[1] < 0.1 {
            landed_at = Some(frame);
        }
    });
    let landed = landed_at.expect("the rider lands") as usize;
    for (frame, report) in &rig.reports()[landed + 1..] {
        assert!(report.grounded, "frame {frame}");
    }
}

#[test]
fn a_descending_platform_never_flickers_the_rider_airborne() {
    let mut rig = Rig::new(press_scene, rising(-0.02));
    rig.run(120);
    let reports = rig.reports();
    let landed = reports.iter().position(|(_, r)| r.landed).expect("landed");
    for (frame, report) in &reports[landed..] {
        assert!(report.grounded, "frame {frame}");
    }
    assert!(
        (rig.feet()[1] - (-0.02 * 119.0) - SKIN).abs() < 1e-3,
        "{:?}",
        rig.feet()
    );
}

#[test]
fn a_closing_press_crushes_within_one_tick_of_the_gap_closing() {
    const START: f32 = 1.5;
    let mut rig = Rig::new(
        |world| {
            spawn_box(
                world,
                [0.0, -0.5, 0.0],
                [30.0, 0.5, 30.0],
                FLAT,
                Body::new_static(),
            );
            let press = spawn_box(
                world,
                [0.0, START + 0.25, 0.0],
                [2.0, 0.25, 2.0],
                FLAT,
                Body::new_kinematic(),
            );
            Cast {
                platform: press,
                hero: hero(world, [0.0, 0.0, 0.0]),
            }
        },
        mover(|f| ([0.0, START + 0.25 - 0.02 * f as f32, 0.0], FLAT)),
    );
    let mut min_feet = f32::INFINITY;
    rig.ride(100, |_, feet, _| {
        assert!(feet.iter().all(|c| c.is_finite()), "{feet:?}");
        min_feet = min_feet.min(feet[1]);
    });
    assert!(min_feet >= -SKIN - 1e-4, "feet sank to {min_feet}");
    let gap_closed = (0..100u64)
        .find(|f| (START - 0.02 * *f as f32) < HEIGHT)
        .expect("the gap closes");
    let stuck = rig
        .reports()
        .iter()
        .find(|(_, r)| r.stuck)
        .map(|(frame, _)| *frame)
        .expect("the rider is crushed");
    assert!(
        stuck <= gap_closed + 1,
        "stuck at {stuck}, gap closed at {gap_closed}"
    );
}

#[test]
fn a_carry_into_a_wall_stops_at_the_wall() {
    let mut wall_id = None;
    let mut rig = Rig::new(
        |world| {
            wall_id = Some(spawn_box(
                world,
                [3.25, 1.0, 0.0],
                [0.25, 2.0, 3.0],
                FLAT,
                Body::new_static(),
            ));
            Cast {
                platform: slab(world, Body::new_kinematic()),
                hero: hero(world, [1.0, 0.3, 0.0]),
            }
        },
        mover(|f| ([0.02 * f as f32, -0.25, 0.0], FLAT)),
    );
    let wall = wall_id.unwrap();
    let mut max_x = f32::NEG_INFINITY;
    rig.ride(220, |_, feet, _| max_x = max_x.max(feet[0]));
    assert!(
        max_x <= 3.0 - RADIUS + 1e-3,
        "the rider reached x = {max_x}"
    );
    assert!(
        max_x > 3.0 - RADIUS - 0.05,
        "the rider never got to the wall: {max_x}"
    );
    assert!(
        rig.reports()
            .iter()
            .any(|(_, r)| r.walls.iter().any(|hit| hit.entity == wall)),
        "a report names the wall"
    );
}

#[test]
fn landing_names_a_kinematic_enemy_and_a_rigid_crate_carries_nothing() {
    let mut enemy = None;
    let mut rig = Rig::new(
        |world| {
            let capsule = world.spawn();
            world
                .insert(capsule, TestPose::at([0.0, 0.5, 0.0]))
                .unwrap();
            world
                .insert(
                    capsule,
                    Collider {
                        shape: Shape::Capsule {
                            half_height: 0.3,
                            radius: 0.5,
                        },
                        body: Body::new_kinematic(),
                        layer: None,
                        sensor: false,
                    },
                )
                .unwrap();
            enemy = Some(capsule);
            Cast {
                platform: capsule,
                hero: hero(world, [0.0, 1.5, 0.0]),
            }
        },
        None,
    );
    rig.run(30);
    let enemy = enemy.unwrap();
    let reports = rig.reports();
    let landed = reports.iter().find(|(_, r)| r.landed).expect("landed");
    assert_eq!(landed.1.ground.unwrap().entity, enemy);

    let mut crate_id = None;
    let mut rig = Rig::new(
        |world| {
            spawn_box(
                world,
                [0.0, -0.5, 0.0],
                [30.0, 0.5, 30.0],
                FLAT,
                Body::new_static(),
            );
            let crate_ = spawn_box(
                world,
                [0.0, 0.5, 0.0],
                [1.0, 0.5, 1.0],
                FLAT,
                Body::Rigid(RigidBody {
                    mass: 50.0,
                    ..RigidBody::default()
                }),
            );
            crate_id = Some(crate_);
            Cast {
                platform: crate_,
                hero: hero(world, [0.0, 1.5, 0.0]),
            }
        },
        None,
    );
    rig.run(40);
    let crate_ = crate_id.unwrap();
    let reports = rig.reports();
    let landed = reports.iter().find(|(_, r)| r.landed).expect("landed");
    assert_eq!(landed.1.ground.unwrap().entity, crate_);
    assert!(
        reports.iter().all(|(_, r)| r.platform.entity.is_none()),
        "a rigid support is never carried"
    );
    let state = rig
        .world
        .get::<CharacterMotorState>(rig.hero)
        .unwrap()
        .unwrap();
    assert!(state.platform.is_none());
}

#[test]
fn a_second_call_in_the_tick_does_not_carry_again() {
    let build = |world: &mut World| Cast {
        platform: slab(world, Body::new_kinematic()),
        hero: hero(world, [0.0, 0.3, 0.0]),
    };
    let drift = || mover(|f| ([0.01 * f as f32, -0.25, 0.0], FLAT));
    let mut single = Rig::new(build, drift());
    single.run(60);
    let mut double = Rig::with(build, drift(), 2, None);
    double.run(60);
    let (a, b) = (single.feet(), double.feet());
    for axis in 0..3 {
        assert!((a[axis] - b[axis]).abs() < 1e-6, "{axis}: {a:?} vs {b:?}");
    }
    let reports = double.reports();
    assert!(reports.chunks(2).skip(10).all(|pair| {
        pair[0].1.platform.entity.is_some() && pair[1].1.platform == Default::default()
    }));
}

#[test]
fn a_held_gameplay_frame_carries_no_surface_velocity() {
    let mut belt = None;
    let mut rig = Rig::with(
        |world| {
            let (cast, _) = belt_scene(world);
            belt = Some(cast.platform);
            cast
        },
        None,
        1,
        Some(20),
    );
    rig.run(40);
    let belt = belt.unwrap();
    let reports = rig.reports();
    let at = |frame: u64| &reports.iter().find(|(f, _)| *f == frame).unwrap().1;
    assert_eq!(at(21).platform.entity, Some(belt));
    assert_eq!(at(21).platform.surface, [0.0; 3]);
    assert_ne!(at(20).platform.surface, [0.0; 3]);
    assert_ne!(at(22).platform.surface, [0.0; 3]);
}

fn determinism_run() -> String {
    let mut out = String::new();
    let mut yawing = Rig::new(
        |world| Cast {
            platform: slab(world, Body::new_kinematic()),
            hero: hero(world, [1.0, 0.3, 0.0]),
        },
        mover(|f| ([0.0, -0.25, 0.0], yaw_quat(0.0175 * f as f32))),
    );
    let mut belt = Rig::new(|world| belt_scene(world).0, None);
    let mut press = Rig::new(press_scene, rising(0.02));
    for rig in [&mut yawing, &mut belt, &mut press] {
        rig.run(100);
        out.push_str(&format!(
            "{:?}\n{:?}\n",
            rig.reports(),
            pose_of(&rig.world, rig.hero)
        ));
    }
    out
}

#[test]
fn carry_is_identical_at_one_and_eight_threads() {
    let one = with_threads(1, determinism_run);
    let eight = with_threads(8, determinism_run);
    assert_eq!(one, eight);
}

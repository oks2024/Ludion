//! The motor's constraint hook: a synthetic straight lane along +X (lateral axis +Z) shapes the
//! requested move, every slide remainder and the platform carry, and pulls the hero back onto
//! itself with a capped correction. With no lane, or a lane that changes nothing, the motor is
//! bit-identical.

mod common;

use std::sync::{Arc, Mutex};

use common::{TestPose, pose_of, tick};
use ludion_core::ids::EntityId;
use ludion_core::math::sin_cos;
use ludion_ecs::{Schedule, World};
use ludion_physics::{
    CharacterMotor, MoveConstraint, MoveReport, PhysicsSpace, install, move_character,
    move_character_with,
};
use ludion_spatial::{Body, Collider, Shape};

type Walk = Arc<dyn Fn(u64) -> [f32; 3] + Send + Sync>;
type Mover = Arc<dyn Fn(u64) -> [f32; 3] + Send + Sync>;
type Log = Arc<Mutex<Vec<(u64, MoveReport)>>>;

const RADIUS: f32 = 0.28;
const FLAT: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const LATERAL: [f32; 3] = [0.0, 0.0, 1.0];

/// A straight lane along +X through `origin`; its lateral axis is +Z.
struct Lane {
    origin: [f32; 3],
    weight: f32,
    cap: f32,
    zero_y: bool,
    corrected: Vec<([f32; 3], bool)>,
}

impl Lane {
    fn new(weight: f32, cap: f32) -> Lane {
        Lane {
            origin: [0.0; 3],
            weight,
            cap,
            zero_y: false,
            corrected: Vec::new(),
        }
    }
}

impl MoveConstraint for Lane {
    fn project(&mut self, _feet: [f32; 3], step: [f32; 3]) -> [f32; 3] {
        [
            step[0],
            if self.zero_y { 0.0 } else { step[1] },
            step[2] - self.weight * step[2],
        ]
    }

    fn correction(&mut self, feet: [f32; 3]) -> [f32; 3] {
        let error = (feet[2] - self.origin[2]) * LATERAL[2];
        [
            0.0,
            0.0,
            -error.signum() * error.abs().min(self.cap) * LATERAL[2],
        ]
    }

    fn corrected(&mut self, applied: [f32; 3], blocked: bool) {
        self.corrected.push((applied, blocked));
    }
}

fn motor() -> CharacterMotor {
    CharacterMotor {
        radius: RADIUS,
        height: 0.9,
        slope_limit_deg: 45.0,
        step_height: 0.25,
        skin_width: 0.01,
        ..CharacterMotor::default()
    }
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

/// A floor whose top is `y = 0`.
fn floor(world: &mut World) {
    spawn_box(
        world,
        [0.0, -0.5, 0.0],
        [20.0, 0.5, 20.0],
        FLAT,
        Body::new_static(),
    );
}

fn hero(world: &mut World, feet: [f32; 3]) -> EntityId {
    let entity = world.spawn();
    world.insert(entity, TestPose::at(feet)).unwrap();
    world.insert(entity, motor()).unwrap();
    entity
}

fn yaw_quat(radians: f32) -> [f32; 4] {
    let (s, c) = sin_cos(radians / 2.0);
    [0.0, s, 0.0, c]
}

struct Cast {
    hero: EntityId,
    /// The kinematic platform the mover drives, if any.
    platform: Option<EntityId>,
}

struct Rig {
    world: World,
    schedule: Schedule,
    hero: EntityId,
    log: Log,
    lane: Option<Arc<Mutex<Lane>>>,
    frame: u64,
}

impl Rig {
    fn new(
        build: impl FnOnce(&mut World) -> Cast,
        walk: Walk,
        lane: Option<Lane>,
        mover: Option<Mover>,
    ) -> Rig {
        let mut world = World::new();
        let Cast { hero, platform } = build(&mut world);
        let lane = lane.map(|lane| Arc::new(Mutex::new(lane)));
        let log: Log = Arc::default();
        let (sink, shared) = (log.clone(), lane.clone());
        let mut schedule = Schedule::new();
        schedule.add_system_shared(
            "test-platform",
            Arc::new(move |world: &mut World, ctx: &ludion_ecs::SystemCtx| {
                if let (Some(mover), Some(platform)) = (mover.as_ref(), platform)
                    && let Ok(Some(pose)) = world.get_mut::<TestPose>(platform)
                {
                    pose.translation = mover(ctx.frame.as_u64());
                }
            }),
        );
        schedule.add_system_shared(
            "test-motor",
            Arc::new(move |world: &mut World, ctx: &ludion_ecs::SystemCtx| {
                let frame = ctx.frame.as_u64();
                let step = walk(frame);
                let report = match shared.as_ref() {
                    Some(lane) => {
                        let mut lane = lane.lock().unwrap();
                        move_character_with::<TestPose>(world, hero, step, Some(&mut *lane))
                    }
                    None => move_character::<TestPose>(world, hero, step),
                };
                if let Ok(report) = report {
                    sink.lock().unwrap().push((frame, report));
                }
            }),
        );
        install::<TestPose>(&mut schedule, PhysicsSpace::Free);
        Rig {
            world,
            schedule,
            hero,
            log,
            lane,
            frame: 0,
        }
    }

    /// Runs `ticks` more ticks, calling `sample` with the hero's feet after each.
    fn ride(&mut self, ticks: u64, mut sample: impl FnMut(u64, [f32; 3])) {
        for _ in 0..ticks {
            tick(&mut self.world, &mut self.schedule, self.frame);
            self.frame += 1;
            sample(self.frame - 1, self.feet());
        }
    }

    fn feet(&self) -> [f32; 3] {
        pose_of(&self.world, self.hero).translation
    }

    fn reports(&self) -> Vec<(u64, MoveReport)> {
        self.log.lock().unwrap().clone()
    }

    /// Every correction the lane was asked to report: `(applied, blocked)`.
    fn corrections(&self) -> Vec<([f32; 3], bool)> {
        self.lane
            .as_ref()
            .unwrap()
            .lock()
            .unwrap()
            .corrected
            .clone()
    }
}

fn walk(f: impl Fn(u64) -> [f32; 3] + Send + Sync + 'static) -> Walk {
    Arc::new(f)
}

fn along_lane(speed: f32) -> Walk {
    walk(move |_| [speed, -0.05, 0.0])
}

/// A kinematic slab whose top starts at `y = 0`, half extents `[2, 0.25, 2]`.
fn slab(world: &mut World) -> EntityId {
    spawn_box(
        world,
        [0.0, -0.25, 0.0],
        [2.0, 0.25, 2.0],
        FLAT,
        Body::new_kinematic(),
    )
}

#[test]
fn no_constraint_and_an_inert_lane_are_bit_identical() {
    let build = |world: &mut World| {
        floor(world);
        spawn_box(
            world,
            [1.5, 0.075, 0.0],
            [0.5, 0.075, 5.0],
            FLAT,
            Body::new_static(),
        );
        spawn_box(
            world,
            [3.5, 1.0, 0.0],
            [0.5, 1.0, 5.0],
            FLAT,
            Body::new_static(),
        );
        Cast {
            platform: Some(slab(world)),
            hero: hero(world, [-1.5, 0.0, 0.0]),
        }
    };
    let slide = walk(|frame| [0.03, -0.05, if frame < 60 { 0.01 } else { 0.0 }]);
    let drift: Mover = Arc::new(|frame| [-1.2, -0.25, 0.01 * frame as f32]);
    let mut plain = Rig::new(build, slide.clone(), None, Some(drift.clone()));
    let mut inert = Rig::new(build, slide, Some(Lane::new(0.0, 0.0)), Some(drift));
    for _ in 0..220 {
        let (mut a, mut b) = ([0.0; 3], [0.0; 3]);
        plain.ride(1, |_, feet| a = feet);
        inert.ride(1, |_, feet| b = feet);
        assert_eq!(a, b, "frame {}", plain.frame);
    }
    assert_eq!(plain.reports(), inert.reports());
    assert!(plain.reports().iter().any(|(_, r)| r.stepped.is_some()));
    assert!(plain.reports().iter().any(|(_, r)| !r.walls.is_empty()));
    assert!(
        plain
            .reports()
            .iter()
            .any(|(_, r)| r.platform.entity.is_some())
    );
}

#[test]
fn a_pull_in_open_space_is_capped_and_converges() {
    let cap = 4.0 / 60.0;
    let mut rig = Rig::new(
        |world| {
            floor(world);
            Cast {
                platform: None,
                hero: hero(world, [0.0, 0.0, 1.5]),
            }
        },
        along_lane(0.03),
        Some(Lane::new(1.0, cap)),
        None,
    );
    let mut z = Vec::new();
    let mut y = Vec::new();
    rig.ride(40, |_, feet| {
        z.push(feet[2]);
        y.push(feet[1]);
    });
    for pair in z.windows(2).skip(1) {
        assert!((pair[0] - pair[1]).abs() <= cap + 1e-6, "{pair:?}");
        if pair[0].abs() > 1e-5 {
            assert!(pair[1].abs() < pair[0].abs(), "{pair:?}");
        }
    }
    assert!(z[39].abs() < 1e-6, "{}", z[39]);
    let rest = y[1];
    assert!(y.iter().skip(1).all(|v| (v - rest).abs() < 1e-4), "{y:?}");
    let corrections = rig.corrections();
    assert!(!corrections.is_empty());
    assert!(corrections.iter().all(|(_, blocked)| !blocked));
}

#[test]
fn a_pull_into_a_wall_beside_the_lane_stops_without_creeping() {
    let mut lane = Lane::new(1.0, 4.0 / 60.0);
    lane.origin = [0.0, 0.0, -1.5];
    let mut rig = Rig::new(
        |world| {
            floor(world);
            spawn_box(
                world,
                [0.0, 1.0, -0.5],
                [20.0, 1.0, 0.1],
                FLAT,
                Body::new_static(),
            );
            Cast {
                platform: None,
                hero: hero(world, [0.0, 0.0, 0.0]),
            }
        },
        along_lane(0.0),
        Some(lane),
        None,
    );
    let mut z = Vec::new();
    rig.ride(50, |_, feet| z.push(feet[2]));
    let wall_face = -0.4;
    for pair in z[39..].windows(2) {
        assert_eq!(pair[0].to_bits(), pair[1].to_bits(), "{pair:?}");
    }
    assert!(z[49] - wall_face >= RADIUS, "{}", z[49]);
    let corrections = rig.corrections();
    assert!(
        corrections
            .iter()
            .rev()
            .take(10)
            .all(|(_, blocked)| *blocked)
    );
}

#[test]
fn sliding_along_a_wall_at_an_angle_to_the_lane_keeps_the_lateral_coordinate() {
    let mut rig = Rig::new(
        |world| {
            floor(world);
            spawn_box(
                world,
                [2.0, 1.0, 0.0],
                [0.1, 1.0, 3.0],
                yaw_quat(30f32.to_radians()),
                Body::new_static(),
            );
            Cast {
                platform: None,
                hero: hero(world, [0.0, 0.0, 0.0]),
            }
        },
        walk(|_| [0.05, -0.05, 0.04]),
        Some(Lane::new(1.0, 0.0)),
        None,
    );
    rig.ride(60, |frame, feet| {
        assert!(feet[2].abs() <= 1e-6, "frame {frame}: {feet:?}")
    });
    assert!(rig.reports().iter().any(|(_, r)| !r.walls.is_empty()));
}

#[test]
fn sliding_off_a_slope_past_the_limit_follows_the_lane() {
    let (s, c) = sin_cos(30f32.to_radians());
    let a = core::f32::consts::FRAC_1_SQRT_2 * s;
    let tilt = [a, 0.0, -a, c];
    let (sin60, cos60) = sin_cos(60f32.to_radians());
    let n = [
        sin60 * core::f32::consts::FRAC_1_SQRT_2,
        cos60,
        sin60 * core::f32::consts::FRAC_1_SQRT_2,
    ];
    let mut rig = Rig::new(
        |world| {
            let center = [-0.25 * n[0], -0.25 * n[1], -0.25 * n[2]];
            spawn_box(world, center, [6.0, 0.25, 6.0], tilt, Body::new_static());
            Cast {
                platform: None,
                hero: hero(world, [0.0, 0.5, 0.0]),
            }
        },
        walk(|_| [0.0, -0.05, 0.0]),
        Some(Lane::new(1.0, 0.0)),
        None,
    );
    rig.ride(40, |frame, feet| {
        assert!(feet[2].abs() <= 1e-6, "frame {frame}: {feet:?}")
    });
    assert!(rig.feet()[0] > 0.1, "{:?}", rig.feet());
}

#[test]
fn a_step_up_on_a_lane_keeps_the_lateral_coordinate() {
    let mut rig = Rig::new(
        |world| {
            floor(world);
            spawn_box(
                world,
                [1.5, 0.075, 0.0],
                [0.5, 0.075, 5.0],
                FLAT,
                Body::new_static(),
            );
            Cast {
                platform: None,
                hero: hero(world, [0.0, 0.0, 0.0]),
            }
        },
        walk(|_| [0.03, -0.05, 0.02]),
        Some(Lane::new(1.0, 0.0)),
        None,
    );
    rig.ride(60, |frame, feet| {
        assert!(feet[2].abs() <= 1e-6, "frame {frame}: {feet:?}")
    });
    assert!(rig.reports().iter().any(|(_, r)| r.stepped.is_some()));
    assert!(rig.feet()[1] > 0.1, "{:?}", rig.feet());
}

#[test]
fn a_carry_across_the_lane_is_projected_and_along_it_is_kept() {
    let build = |world: &mut World| {
        let platform = slab(world);
        Cast {
            platform: Some(platform),
            hero: hero(world, [0.0, 0.0, 0.0]),
        }
    };
    let stand = walk(|_| [0.0, -0.1, 0.0]);

    let across: Mover = Arc::new(|frame| [0.0, -0.25, 0.01 * frame as f32]);
    let mut rig = Rig::new(
        build,
        stand.clone(),
        Some(Lane::new(1.0, 0.0)),
        Some(across),
    );
    rig.ride(60, |frame, feet| {
        assert!(feet[2].abs() <= 1e-6, "frame {frame}: {feet:?}")
    });
    let carried = rig.reports().into_iter().filter(|(frame, _)| *frame > 5);
    for (frame, report) in carried {
        assert!(
            (report.platform.translation[2] - 0.01).abs() < 1e-4,
            "frame {frame}: {report:?}"
        );
    }

    let along: Mover = Arc::new(|frame| [0.01 * frame as f32, -0.25, 0.0]);
    let mut rig = Rig::new(build, stand, Some(Lane::new(1.0, 0.0)), Some(along));
    let mut x = Vec::new();
    rig.ride(60, |_, feet| x.push(feet[0]));
    assert!((x[59] - x[19] - 0.4).abs() < 1e-3, "{}", x[59] - x[19]);
}

#[test]
fn the_lane_never_shapes_vertical_motion() {
    let jump = walk(|frame| {
        [
            0.02,
            if (5..10).contains(&frame) { 0.1 } else { -0.05 },
            0.0,
        ]
    });
    let build = |world: &mut World| {
        floor(world);
        Cast {
            platform: None,
            hero: hero(world, [0.0, 0.3, 0.0]),
        }
    };
    let mut lane = Lane::new(1.0, 0.0);
    lane.zero_y = true;
    let mut plain = Rig::new(build, jump.clone(), None, None);
    let mut shaped = Rig::new(build, jump, Some(lane), None);
    for _ in 0..40 {
        let (mut a, mut b) = ([0.0; 3], [0.0; 3]);
        plain.ride(1, |_, feet| a = feet);
        shaped.ride(1, |_, feet| b = feet);
        assert_eq!(a[1].to_bits(), b[1].to_bits(), "frame {}", plain.frame);
    }
    assert!(plain.reports().iter().any(|(_, r)| r.landed));
}

#[test]
fn a_half_weight_removes_half_the_lateral_move() {
    let mut rig = Rig::new(
        |world| Cast {
            platform: None,
            hero: hero(world, [0.0, 5.0, 0.0]),
        },
        walk(|_| [0.03, 0.0, 0.04]),
        Some(Lane::new(0.5, 0.0)),
        None,
    );
    rig.ride(10, |_, _| {});
    let moves = rig.reports().len() as f32;
    assert!(
        (rig.feet()[2] - 0.02 * moves).abs() < 1e-5,
        "{:?} after {moves}",
        rig.feet()
    );
}

//! An 11-part limp ragdoll shaped like a game's (ball joints, near-spherical torso and head) must
//! reach sleep soon after it settles: an island sleeps only when every part is still, so one part
//! that keeps rolling keeps all eleven awake and stepping.

mod common;
#[path = "common/ragdoll.rs"]
mod ragdoll;

use common::{pose_of, tick};
use ludion_core::ids::EntityId;
use ludion_ecs::World;
use ludion_physics::{CollisionLayers, PhysicsSettings, PhysicsSpace};
use ludion_spatial::{Body, Collider, JointMotor, RigidBody, Shape, Velocity};
use ragdoll::{Part, ball_axes, build_ragdoll, physics, schedule_with};

const LAYER: &str = "enemy";
/// Ticks within which every part must be asleep.
const BUDGET: u64 = 120;
const MAX_TICKS: u64 = 600;
/// The horizon over which awake parts are summed, the deterministic proxy for physics cost.
const COST_TICKS: u64 = 300;
const CURL_END: u64 = 24;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Scenario {
    Calm,
    Shot,
    Curl,
}

#[derive(Clone, Copy)]
struct Tune {
    linear_damping: f32,
    angular_damping: f32,
    sleep_threshold: Option<f32>,
    sleep_after_ticks: Option<u32>,
    sleep_angular_threshold: Option<f32>,
}

#[derive(Debug)]
struct Run {
    ticks_to_sleep: Option<u64>,
    awake_part_ticks: u64,
    /// The part still moving fastest at tick 120 and its farthest-point speed.
    slowest: (usize, f32),
    /// The hips' height when the run ended: a ragdoll asleep inside the floor is no settle.
    hips_y: f32,
}

struct Row {
    name: &'static str,
    /// Half extents `[r, r + hh, r]`.
    half: [f32; 3],
    centre: [f32; 3],
    mass: f32,
    parent: Option<usize>,
    limits: [f32; 3],
    /// The joint point in world space.
    point: [f32; 3],
}

fn row(
    name: &'static str,
    (r, hh): (f32, f32),
    centre: [f32; 3],
    mass: f32,
    parent: Option<usize>,
    limits: [f32; 3],
    point: [f32; 3],
) -> Row {
    Row {
        name,
        half: [r, r + hh, r],
        centre,
        mass,
        parent,
        limits,
        point,
    }
}

/// The game's rig in part order: hips, chest, head, arm/fore left, arm/fore right, thigh/shin left,
/// thigh/shin right.
fn rows() -> Vec<Row> {
    let mut rows = vec![
        row(
            "hips",
            (0.15, 0.01),
            [0.0, 0.65, 0.0],
            15.55,
            None,
            [0.0; 3],
            [0.0; 3],
        ),
        row(
            "chest",
            (0.15, 0.01),
            [0.0, 0.97, 0.0],
            15.55,
            Some(0),
            [20.0, 20.0, 15.0],
            [0.0, 0.81, 0.0],
        ),
        row(
            "head",
            (0.12, 0.011),
            [0.0, 1.26, 0.0],
            8.2,
            Some(1),
            [35.0, 40.0, 30.0],
            [0.0, 1.13, 0.0],
        ),
    ];
    for s in [-1.0_f32, 1.0] {
        let arm = rows.len();
        rows.push(row(
            "arm",
            (0.057, 0.0),
            [0.27 * s, 0.95, 0.0],
            1.93,
            Some(1),
            [80.0, 45.0, 80.0],
            [0.27 * s, 1.064, 0.0],
        ));
        rows.push(row(
            "fore",
            (0.051, 0.0),
            [0.27 * s, 0.722, 0.0],
            1.41,
            Some(arm),
            [90.0, 10.0, 10.0],
            [0.27 * s, 0.836, 0.0],
        ));
    }
    for s in [-1.0_f32, 1.0] {
        let thigh = rows.len();
        rows.push(row(
            "thigh",
            (0.051, 0.0),
            [0.09 * s, 0.32, 0.0],
            1.40,
            Some(0),
            [60.0, 30.0, 30.0],
            [0.09 * s, 0.46, 0.0],
        ));
        rows.push(row(
            "shin",
            (0.046, 0.0),
            [0.09 * s, 0.11, 0.0],
            1.02,
            Some(thigh),
            [90.0, 10.0, 10.0],
            [0.09 * s, 0.21, 0.0],
        ));
    }
    rows
}

/// Curl target x degrees, stiffness, damping and max force for a jointed part.
fn curl(name: &str) -> Option<(f32, f32, f32, f32)> {
    Some(match name {
        "chest" => (10.0, 46.97, 4.98, 28.13),
        "head" => (17.5, 50.07, 5.31, 21.12),
        "arm" => (40.0, 8.88, 0.94, 4.31),
        "fore" => (45.0, 5.24, 0.56, 2.83),
        "thigh" => (30.0, 5.19, 0.55, 2.80),
        "shin" => (45.0, 3.07, 0.33, 1.84),
        _ => return None,
    })
}

fn parts(rows: &[Row]) -> Vec<Part> {
    rows.iter()
        .map(|r| {
            let link = r.parent.map(|p| {
                let c = rows[p].centre;
                let local = [r.point[0] - c[0], r.point[1] - c[1], r.point[2] - c[2]];
                (p, ball_axes(r.limits), local)
            });
            (r.half, r.centre, link)
        })
        .collect()
}

/// `default` at 0, then `enemy` ignoring itself.
fn layers() -> CollisionLayers {
    CollisionLayers::new(
        vec!["default".to_string(), LAYER.to_string()],
        vec![0, 1 << 1],
    )
}

fn capsule(half: [f32; 3]) -> Shape {
    let radius = half[0].min(half[2]);
    Shape::Capsule {
        half_height: (half[1] - radius).max(0.01),
        radius,
    }
}

fn norm(a: [f32; 3]) -> f32 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

fn measure(scenario: Scenario, tune: Tune) -> Run {
    let rows = rows();
    let masses: Vec<f32> = rows.iter().map(|r| r.mass).collect();
    let velocity = if scenario == Scenario::Calm {
        [0.0; 3]
    } else {
        [4.0, 1.0, 0.0]
    };
    let (mut world, ids): (World, Vec<EntityId>) = build_ragdoll(
        &parts(&rows),
        &masses,
        |mass| {
            Body::Rigid(RigidBody {
                mass,
                linear_damping: tune.linear_damping,
                angular_damping: tune.angular_damping,
                sleep_threshold: tune.sleep_threshold,
                sleep_after_ticks: tune.sleep_after_ticks,
                sleep_angular_threshold: tune.sleep_angular_threshold,
                ..RigidBody::default()
            })
        },
        capsule,
        velocity,
        [0.0; 3],
    );
    for &id in &ids {
        world
            .get_mut::<Collider>(id)
            .expect("a live part")
            .expect("a collider")
            .layer = Some(LAYER.to_string());
    }
    if scenario == Scenario::Curl {
        for (&id, r) in ids.iter().zip(&rows) {
            if let Some((c, stiffness, damping, max_force)) = curl(r.name) {
                world
                    .insert(
                        id,
                        JointMotor {
                            target: vec![c * 0.6, 0.0, 0.0],
                            stiffness: stiffness * 0.6,
                            damping: damping * 0.6,
                            max_force: Some(max_force),
                        },
                    )
                    .expect("a part accepts a motor");
            }
        }
    }
    let mut schedule = schedule_with(PhysicsSettings {
        layers: Some(layers()),
        ..PhysicsSettings::new(PhysicsSpace::Free)
    });
    let mut run = Run {
        ticks_to_sleep: None,
        awake_part_ticks: 0,
        slowest: (0, 0.0),
        hips_y: 0.0,
    };
    for frame in 0..MAX_TICKS {
        if scenario == Scenario::Curl && frame == CURL_END {
            for &id in &ids {
                world.remove::<JointMotor>(id).expect("a live part");
            }
        }
        if scenario == Scenario::Curl && frame >= CURL_END {
            assert!(
                ids.iter()
                    .all(|&id| world.get::<JointMotor>(id).expect("a live part").is_none()),
                "a motor survived tick {CURL_END}"
            );
        }
        tick(&mut world, &mut schedule, frame);
        let awake = ids
            .iter()
            .filter(|&&id| physics(&mut world).is_sleeping(id) != Some(true))
            .count();
        if frame < COST_TICKS {
            run.awake_part_ticks += awake as u64;
        }
        if frame + 1 == BUDGET {
            for (i, (&id, r)) in ids.iter().zip(&rows).enumerate() {
                let v = world
                    .get::<Velocity>(id)
                    .expect("a live part")
                    .expect("a velocity");
                let speed = norm(v.linear) + norm(v.angular) * r.half[0];
                if speed > run.slowest.1 {
                    run.slowest = (i, speed);
                }
            }
        }
        if awake == 0 && run.ticks_to_sleep.is_none() {
            run.ticks_to_sleep = Some(frame + 1);
        }
        if frame >= COST_TICKS && run.ticks_to_sleep.is_some() {
            break;
        }
    }
    run.hips_y = pose_of(&world, ids[0]).translation[1];
    run
}

const BASELINE: Tune = Tune {
    linear_damping: 0.0,
    angular_damping: 0.0,
    sleep_threshold: None,
    sleep_after_ticks: None,
    sleep_angular_threshold: None,
};

/// A raised stillness threshold and a shorter timer; rapier's π/2 angular gate still binds on it.
const TUNED: Tune = Tune {
    sleep_threshold: Some(0.1),
    sleep_after_ticks: Some(15),
    ..BASELINE
};

/// The corpse tune: TUNED plus an angular gate raised to 2.4 rad/s, the least value at which all
/// three scenarios have plateaued.
const CORPSE: Tune = Tune {
    sleep_angular_threshold: Some(2.4),
    ..TUNED
};

#[test]
#[ignore = "prints the sweep"]
fn ragdoll_sleep_sweep() {
    println!("scenario  lin  ang   thr  after  ticks_to_sleep  awake_part_ticks  slowest@120");
    for scenario in [Scenario::Calm, Scenario::Shot, Scenario::Curl] {
        let tunes = [
            (0.0, 0.0, None, None),
            (0.0, 0.0, Some(0.1), None),
            (0.0, 0.0, Some(0.1), Some(15)),
            (0.0, 0.0, Some(0.1), Some(8)),
            (0.5, 2.0, Some(0.1), Some(15)),
            (0.5, 4.0, Some(0.1), Some(15)),
        ];
        for (linear_damping, angular_damping, sleep_threshold, sleep_after_ticks) in tunes {
            let run = measure(
                scenario,
                Tune {
                    linear_damping,
                    angular_damping,
                    sleep_threshold,
                    sleep_after_ticks,
                    sleep_angular_threshold: None,
                },
            );
            println!(
                "{:<8} {:>4} {:>4} {:>5} {:>6}  {:>14}  {:>16}  part {} {:.3}",
                format!("{scenario:?}"),
                linear_damping,
                angular_damping,
                sleep_threshold.map_or("-".into(), |t| t.to_string()),
                sleep_after_ticks.map_or("-".into(), |t| t.to_string()),
                run.ticks_to_sleep.map_or("none".into(), |t| t.to_string()),
                run.awake_part_ticks,
                run.slowest.0,
                run.slowest.1,
            );
        }
    }
}

#[test]
#[ignore = "prints the angular sweep"]
fn ragdoll_angular_sweep() {
    println!("scenario  ang_thr  ticks_to_sleep  awake_part_ticks  hips_y  slowest@120");
    for scenario in [Scenario::Calm, Scenario::Shot, Scenario::Curl] {
        for step in 16..=31 {
            let threshold = step as f32 / 10.0;
            let run = measure(
                scenario,
                Tune {
                    sleep_angular_threshold: Some(threshold),
                    ..TUNED
                },
            );
            println!(
                "{:<8} {:>7.1}  {:>14}  {:>16}  {:>6.3}  part {} {:.3}",
                format!("{scenario:?}"),
                threshold,
                run.ticks_to_sleep.map_or("none".into(), |t| t.to_string()),
                run.awake_part_ticks,
                run.hips_y,
                run.slowest.0,
                run.slowest.1,
            );
        }
    }
}

#[test]
#[ignore = "prints the grid sweep"]
fn ragdoll_grid_sweep() {
    println!("thr  after  ang   Calm  Shot  Curl  min_hips_y");
    for thr in [0.1, 0.15, 0.2, 0.3] {
        for after in [15, 8] {
            for ang in [None, Some(1.8), Some(2.0), Some(2.4)] {
                let runs = [Scenario::Calm, Scenario::Shot, Scenario::Curl].map(|scenario| {
                    measure(
                        scenario,
                        Tune {
                            sleep_threshold: Some(thr),
                            sleep_after_ticks: Some(after),
                            sleep_angular_threshold: ang,
                            ..BASELINE
                        },
                    )
                });
                let ticks = runs
                    .each_ref()
                    .map(|r| r.ticks_to_sleep.map_or("none".into(), |t| t.to_string()));
                let hips = runs.iter().map(|r| r.hips_y).fold(f32::MAX, f32::min);
                println!(
                    "{thr:<4} {after:>5}  {:<4}  {:>4}  {:>4}  {:>4}  {hips:.3}",
                    ang.map_or("-".into(), |a: f32| a.to_string()),
                    ticks[0],
                    ticks[1],
                    ticks[2]
                );
            }
        }
    }
}

/// Upper bounds on the tuned ticks-to-sleep per scenario: measured 128 / 160 / 150, with margin.
/// The rapier angular gate (|ω| < π/2 rad/s) binds on the shins without `sleep_angular_threshold`,
/// so 120 is out of reach.
#[test]
fn a_raised_sleep_threshold_and_a_short_timer_settle_a_limp_ragdoll_sooner() {
    let bounds = [
        (Scenario::Calm, 135),
        (Scenario::Shot, 170),
        (Scenario::Curl, 160),
    ];
    let baseline = measure(Scenario::Calm, BASELINE);
    assert!(
        baseline.ticks_to_sleep.is_none_or(|t| t > BUDGET),
        "the undamped ragdoll was expected to miss the {BUDGET}-tick budget: {baseline:?}"
    );
    for (scenario, bound) in bounds {
        let before = measure(scenario, BASELINE);
        let after = measure(scenario, TUNED);
        let slept = after.ticks_to_sleep.expect("the tuned ragdoll sleeps");
        assert!(
            after.hips_y > 0.0,
            "{scenario:?}: hips fell through the floor: {after:?}"
        );
        assert!(
            slept <= bound,
            "{scenario:?} slept at {slept}, over {bound}"
        );
        assert!(
            before.ticks_to_sleep.is_none_or(|b| slept < b),
            "{scenario:?}: tuned {slept} did not beat {:?}",
            before.ticks_to_sleep
        );
        assert!(
            after.awake_part_ticks < before.awake_part_ticks,
            "{scenario:?}: tuned cost {} vs {}",
            after.awake_part_ticks,
            before.awake_part_ticks
        );
    }
}

/// With the angular gate raised to 2.4 rad/s the ragdoll sleeps at 123 / 132 / 129 ticks, against
/// 128 / 160 / 150 without it. Shot and Curl plateau from 1.9 and 1.8, but at 2.3 Calm still
/// reads 126, three ticks over, so 2.4 is the least value at which all three have plateaued. Past it the
/// linear/drift gate binds, so the 120-tick `BUDGET` is out of reach by the angular gate alone; the
/// fastest combination swept, `sleep_threshold` 0.15 with `sleep_after_ticks` 8, reaches 108 / 124 /
/// 120 (`ragdoll_grid_sweep`).
#[test]
fn an_angular_threshold_settles_a_limp_ragdoll_sooner_still() {
    let bounds = [
        (Scenario::Calm, 128),
        (Scenario::Shot, 137),
        (Scenario::Curl, 134),
    ];
    for (scenario, bound) in bounds {
        let tuned = measure(scenario, TUNED);
        let corpse = measure(scenario, CORPSE);
        let slept = corpse.ticks_to_sleep.expect("the corpse tune sleeps");
        assert!(
            corpse.hips_y > 0.0,
            "{scenario:?}: hips fell through the floor: {corpse:?}"
        );
        assert!(
            slept <= bound,
            "{scenario:?} slept at {slept}, over {bound}"
        );
        assert!(
            corpse.awake_part_ticks <= tuned.awake_part_ticks,
            "{scenario:?}: corpse cost {} vs {}",
            corpse.awake_part_ticks,
            tuned.awake_part_ticks
        );
    }
}

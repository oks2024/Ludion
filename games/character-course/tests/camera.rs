//! The camera rig on the course, tick by tick: the scripted route drives the hero while the test
//! reads the camera's `CameraRig` and `Transform` each tick. The gate blends run a second, the eye
//! never jumps but at the respawn snap and a collision pull-in, the lane section is watched from the
//! lane's right, the chase section pulls the boom in at its pillar and porch and keeps the eye clear
//! of the course, the respawn is the one trauma pulse, the main lane runs straight under
//! `camera_relative`, and two runs hash identically per tick.

mod common;

use std::path::PathBuf;

use common::{FRAMES, ROUTE};
use ludion::prelude::*;
use ludion_scene::{PathMetric, PathSampler, parse_path};
use ludion_testkit::GameplayDriver;

/// The farthest the eye may move in one tick, metres. The blends and the lane's bends peak near
/// 0.48 m; a cut moves it metres.
const MAX_EYE_STEP: f32 = 0.5;
/// A blend runs one second of 60 Hz ticks, give or take the tick the zone's write lands on.
const BLEND_TICKS: usize = 60;
/// The chase gate's collision sphere, metres (`chase-enter`'s `collision.radius`).
const CHASE_RADIUS: f32 = 0.3;
/// The pillar's and the porch's x range, from the scene.
const PILLAR_X: std::ops::RangeInclusive<f32> = 55.0..=60.0;
const PORCH_X: std::ops::RangeInclusive<f32> = 59.0..=67.0;
/// The farthest the hero's feet move in one tick without a respawn, metres.
const TELEPORT: f32 = 1.0;

struct Tick {
    feet: [f32; 3],
    eye: [f32; 3],
    rig: CameraRig,
    hash: u64,
    /// Whether a sphere at the eye touches no collider but the hero's own.
    eye_clear: bool,
}

fn scenes() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scenes")
}

/// Runs the route one tick at a time, recording the hero, the camera and the world hash.
fn run() -> Vec<Tick> {
    let mut driver = GameplayDriver::new(
        character_course::game().project_root(scenes()),
        include_str!("../scenes/main.yaml"),
    );
    (0..FRAMES)
        .map(|frame| {
            for (at, key, down) in ROUTE {
                if at == frame {
                    driver.inject(
                        if down { "key_down" } else { "key_up" },
                        &format!("{key:?}"),
                    );
                }
            }
            driver.step(1);
            let engine = driver.engine_mut();
            let hash = engine.world_hash().unwrap().as_u64();
            let world = engine.world_mut();
            let feet = world
                .query::<(&character_course::Runner, &Transform)>()
                .next()
                .map(|(_, pose)| pose.translation)
                .unwrap();
            let hero = world
                .query_ids::<(&character_course::Runner,)>()
                .next()
                .map(|(id, _)| id)
                .unwrap();
            let (_, (pose, rig)) = world
                .query_ids::<(&Transform, &CameraRig)>()
                .next()
                .unwrap();
            let (eye, rig) = (pose.translation, rig.clone());
            let physics = world
                .resource::<PhysicsWorld>()
                .expect("the course has physics");
            let touching = physics
                .overlap(
                    &Shape::Sphere {
                        radius: CHASE_RADIUS,
                    },
                    eye,
                    [0.0, 0.0, 0.0, 1.0],
                    &QueryFilter::new().exclude(hero),
                )
                .expect("the overlap query runs");
            Tick {
                feet,
                eye,
                rig,
                hash,
                eye_clear: touching.is_empty(),
            }
        })
        .collect()
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3)
        .map(|i| (a[i] - b[i]) * (a[i] - b[i]))
        .sum::<f32>()
        .sqrt()
}

/// The ticks whose feet jumped: the respawns.
fn respawns(ticks: &[Tick]) -> Vec<usize> {
    (1..ticks.len())
        .filter(|&i| distance(ticks[i].feet, ticks[i - 1].feet) > TELEPORT)
        .collect()
}

/// `[first tick with a blend, first tick without one after it)` for every blend that ran to its end.
fn blends(ticks: &[Tick]) -> Vec<(usize, usize)> {
    let mut found = Vec::new();
    let mut start = None;
    for (i, tick) in ticks.iter().enumerate() {
        match (start, tick.rig.state.blend.is_some()) {
            (None, true) => start = Some(i),
            (Some(from), false) => {
                found.push((from, i));
                start = None;
            }
            _ => {}
        }
    }
    found
}

#[test]
fn each_zone_blend_runs_a_second() {
    let ticks = run();
    let blends = blends(&ticks);
    assert_eq!(
        blends.len(),
        3,
        "the lane enter, the lane exit and the chase blend: {blends:?}"
    );
    for (from, to) in &blends {
        let len = to - from;
        assert!(
            len.abs_diff(BLEND_TICKS) <= 1,
            "the blend from tick {from} ran {len} ticks"
        );
    }
}

#[test]
fn the_eye_never_jumps_but_at_the_respawn_snap_and_a_pull_in() {
    let ticks = run();
    let respawns = respawns(&ticks);
    assert_eq!(respawns.len(), 1, "one respawn: {respawns:?}");
    let mut worst: f32 = 0.0;
    let mut pulled: f32 = 0.0;
    for i in 1..ticks.len() {
        let step = distance(ticks[i].eye, ticks[i - 1].eye);
        if respawns.contains(&i) {
            assert!(
                ticks[i].rig.state.blend.is_none(),
                "the snap leaves no blend"
            );
        } else if ticks[i].rig.state.boom.is_some() || ticks[i - 1].rig.state.boom.is_some() {
            // A collision pull-in is a deliberate fast move.
            pulled = pulled.max(step);
        } else {
            worst = worst.max(step);
            assert!(step <= MAX_EYE_STEP, "tick {i}: the eye moved {step} m");
        }
    }
    eprintln!("largest eye step {worst}, largest pull-in step {pulled}");
}

#[test]
fn the_chase_section_pulls_the_boom_in_and_keeps_the_eye_clear() {
    let ticks = run();
    let first_chase = ticks
        .iter()
        .position(|t| matches!(t.rig.rig.mode, RigMode::Chase(_)))
        .expect("the chase gate switches the rig to chase");
    assert!(
        ticks[first_chase..]
            .iter()
            .all(|t| matches!(t.rig.rig.mode, RigMode::Chase(_))),
        "the rig stays chase to the end of the route"
    );
    let pulled = |x: &std::ops::RangeInclusive<f32>| {
        ticks[first_chase..]
            .iter()
            .any(|t| t.rig.state.boom.is_some() && x.contains(&t.eye[0]))
    };
    assert!(pulled(&PILLAR_X), "the pillar never pulled the boom in");
    assert!(pulled(&PORCH_X), "the porch never pulled the boom in");
    for (i, tick) in ticks.iter().enumerate().skip(first_chase) {
        assert!(
            tick.eye_clear,
            "tick {i}: the eye at {:?} is in the course",
            tick.eye
        );
    }
}

#[test]
fn the_lane_section_is_watched_from_the_lanes_right() {
    let ticks = run();
    let blends = blends(&ticks);
    let (enter_end, exit_start) = (blends[0].1, blends[1].0);
    let text = std::fs::read_to_string(scenes().join("paths/lane.path.yaml")).unwrap();
    let sampler = PathSampler::new(&parse_path(&text).unwrap());
    for (i, tick) in ticks.iter().enumerate().take(exit_start).skip(enter_end) {
        let eye = tick.eye.map(f64::from);
        let s = sampler.nearest_s_global(eye, PathMetric::Full);
        let (point, right) = (sampler.point_at(s), sampler.frame_at(s).right);
        let side: f64 = (0..3).map(|k| (eye[k] - point[k]) * right[k]).sum();
        assert!(
            side > 0.0,
            "tick {i}: the eye is {side} m to the lane's right"
        );
    }
}

#[test]
fn a_respawn_is_the_one_trauma_pulse_and_it_decays() {
    let ticks = run();
    let respawns = respawns(&ticks);
    let pulses: Vec<usize> = (1..ticks.len())
        .filter(|&i| ticks[i].rig.trauma > ticks[i - 1].rig.trauma)
        .collect();
    assert_eq!(pulses, respawns, "trauma rises on the respawn tick alone");
    assert!(ticks[pulses[0]].rig.trauma > 0.5);
    assert_eq!(ticks.last().unwrap().rig.trauma, 0.0);
}

#[test]
fn up_runs_the_main_lane_straight() {
    let ticks = run();
    for (i, tick) in ticks.iter().enumerate().take(320) {
        assert!(tick.feet[2].abs() < 0.01, "tick {i}: z = {}", tick.feet[2]);
    }
    assert!(ticks[319].feet[0] > 28.0, "x = {}", ticks[319].feet[0]);
}

#[test]
fn two_runs_hash_identically_per_tick() {
    let hashes = |ticks: Vec<Tick>| ticks.iter().map(|t| t.hash).collect::<Vec<_>>();
    assert_eq!(hashes(run()), hashes(run()));
}

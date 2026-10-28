//! The gallery `camera-rig` scene's CPU proofs: the rail camera under `common::rig_builder` snaps on
//! its first tick, blends to the side-on rig over its second, cuts back on `snap_rig`, shakes on a
//! trauma pulse without turning the posed yaw, and hashes identically run to run. CPU-only on
//! purpose: the rendered proof is `gallery.rs`'s `camera-rig-driven` golden.

mod common;

use std::path::PathBuf;

use ludion_ecs::World;
use ludion_engine::{Camera, CameraRig, Engine, Transform};

const TICKS: usize = 300;
const DT: f32 = 1.0 / 60.0;

fn gallery_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery")
}

fn build() -> Engine {
    let mut engine = common::rig_builder()
        .project_root(gallery_root())
        .build()
        .expect("the rig engine builds");
    engine
        .load_inline_scene(
            &std::fs::read_to_string(gallery_root().join("camera-rig.yaml"))
                .expect("the committed scene"),
        )
        .expect("the committed scene loads");
    engine
}

fn camera(world: &World) -> ludion_core::ids::EntityId {
    world
        .entities()
        .find(|&id| world.has::<Camera>(id))
        .expect("the scene has a camera")
}

fn rig_of(engine: &Engine) -> CameraRig {
    let world = engine.world();
    world
        .get::<CameraRig>(camera(world))
        .unwrap()
        .unwrap()
        .clone()
}

fn eye_of(engine: &Engine) -> [f32; 3] {
    let world = engine.world();
    world
        .get::<Transform>(camera(world))
        .unwrap()
        .unwrap()
        .translation
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f32>().sqrt()
}

fn advance(engine: &mut Engine, ticks: u64) {
    for _ in 0..ticks {
        engine.advance_live();
    }
}

#[test]
fn the_first_tick_snaps_without_a_blend() {
    let mut engine = build();
    assert!(rig_of(&engine).state.last_applied.is_none());
    engine.advance_live();
    let rig = rig_of(&engine);
    assert!(rig.state.last_applied.is_some());
    assert!(rig.state.blend.is_none(), "a snap starts no blend");
}

#[test]
fn the_switch_blends_for_a_second_in_small_steps() {
    let mut engine = build();
    advance(&mut engine, common::SWITCH_TICK);
    assert!(rig_of(&engine).state.blend.is_none(), "no blend before");
    let mut last = eye_of(&engine);
    let mut blending = 0;
    for tick in 0..120 {
        engine.advance_live();
        let eye = eye_of(&engine);
        let step = distance(last, eye);
        assert!(
            step < 0.6,
            "tick {tick} after the switch moved the eye {step} m"
        );
        last = eye;
        if rig_of(&engine).state.blend.is_some() {
            blending += 1;
        }
    }
    let expected = (1.0 / DT) as i32;
    assert!(
        (expected - 1..=expected + 1).contains(&blending),
        "the blend ran {blending} ticks"
    );
}

#[test]
fn the_snap_cuts_in_one_tick() {
    let mut engine = build();
    advance(&mut engine, common::SNAP_TICK);
    let before = eye_of(&engine);
    assert!(
        rig_of(&engine).state.blend.is_none(),
        "the switch blend is over"
    );
    engine.advance_live();
    assert!(
        distance(before, eye_of(&engine)) > 3.0,
        "the cut jumps from the side to behind the hero"
    );
    assert!(
        rig_of(&engine).state.blend.is_none(),
        "a cut leaves no blend"
    );
}

#[test]
fn the_shake_decays_and_never_turns_the_posed_yaw() {
    let mut engine = build();
    advance(&mut engine, common::SHAKE_TICK + 1);
    let rig = rig_of(&engine);
    assert!(rig.trauma > 0.0, "the pulse landed: {}", rig.trauma);
    let ticks = (rig.trauma / rig.shake.decay_per_second / DT).ceil() as u64 + 2;
    let mut yaw = rig.yaw_deg();
    let mut shaken = false;
    for _ in 0..ticks {
        engine.advance_live();
        let rig = rig_of(&engine);
        assert!(
            (rig.yaw_deg() - yaw).abs() < 0.5,
            "the posed yaw jumped {yaw} -> {}",
            rig.yaw_deg()
        );
        yaw = rig.yaw_deg();
        shaken |= rig.trauma > 0.0 && eye_of(&engine) != rig.state.pose.as_ref().unwrap().eye;
    }
    assert!(shaken, "the eye shook while trauma lasted");
    assert_eq!(rig_of(&engine).trauma, 0.0, "trauma decays to zero");
}

fn hashes() -> Vec<u64> {
    let mut engine = build();
    (0..TICKS)
        .map(|_| {
            engine.advance_live();
            engine.world_hash().unwrap().as_u64()
        })
        .collect()
}

#[test]
fn two_runs_of_the_camera_rig_hash_identically_per_tick() {
    assert_eq!(hashes(), hashes());
}

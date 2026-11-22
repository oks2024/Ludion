//! The gallery `camera-chase` scene's CPU proofs: a chase camera under `common::chase_builder` pulls
//! in against a pillar and a ceiling, crosses two overlapping zones, lists a `camera_pass` prop as an
//! occluder and peeks a corner, with a per-tick `WorldHash` trace that a second pinned machine must
//! reproduce (`examples/gallery/camera-chase.trace.txt`). CPU-only on purpose: the rendered proof
//! is `gallery.rs`'s `camera-chase-driven` golden.

mod common;

use std::path::PathBuf;

use ludion::prelude::{LayerSet, PhysicsWorld, QueryFilter, Shape};
use ludion_core::ids::EntityId;
use ludion_core::par;
use ludion_ecs::World;
use ludion_engine::{
    Camera, CameraRig, Engine, Fade, Name, camera_occluders, camera_relative, camera_relative_yaw,
};
use ludion_protocol::command::EngineRestoreParams;
use ludion_protocol::{Command, ResponseBody};

const TICKS: usize = common::CHASE_TICKS as usize;
const THREAD_COUNTS: [usize; 3] = [1, 2, 8];
/// A tick with the boom pulled in under the ceiling, and a tick mid-peek (the weight is between).
const PULL_IN_SNAPSHOT: usize = 175;
const PEEK_SNAPSHOT: usize = common::PEEK_START as usize + 4;
const COLLISION_RADIUS: f32 = 0.3;

fn gallery_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery")
}

fn build() -> Engine {
    let mut engine = common::chase_builder()
        .project_root(gallery_root())
        .build()
        .expect("the chase engine builds");
    engine
        .load_inline_scene(
            &std::fs::read_to_string(gallery_root().join("camera-chase.yaml"))
                .expect("the committed scene"),
        )
        .expect("the committed scene loads");
    engine
}

fn camera(world: &World) -> EntityId {
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

fn named(world: &mut World, wanted: &str) -> EntityId {
    world
        .query_ids::<(&Name,)>()
        .find(|(_, (name,))| name.0 == wanted)
        .map(|(id, _)| id)
        .unwrap_or_else(|| panic!("no entity named {wanted}"))
}

/// One `<tick:04> <hash:016x>` line per tick, from tick `from`.
fn trace_from(engine: &mut Engine, from: usize, to: usize) -> Vec<String> {
    (from..to)
        .map(|tick| {
            engine.advance_live();
            let hash = engine.world_hash().unwrap().as_u64();
            format!("{tick:04} {hash:016x}")
        })
        .collect()
}

fn run(n: usize) -> Vec<String> {
    trace_from(&mut build(), 0, n)
}

fn trace_path() -> PathBuf {
    gallery_root().join("camera-chase.trace.txt")
}

fn committed_trace() -> Option<Vec<String>> {
    let text = std::fs::read_to_string(trace_path()).ok()?;
    Some(
        text.lines()
            .filter(|line| !line.starts_with('#') && !line.is_empty())
            .map(str::to_owned)
            .collect(),
    )
}

#[test]
fn the_chase_matches_the_committed_cross_arch_trace() {
    let trace = run(TICKS);
    if ludion_core::bless::bless_requested() {
        let header = "\
# Per-tick WorldHash of examples/gallery/camera-chase.yaml, stepped under the chase_builder driver\n\
# (the hero's route, the boom's orbit, the corner peek and the occluder fade;\n\
# crates/ludion-testkit/tests/common/mod.rs).\n\
# Recorded with: LUDION_BLESS=1 cargo nextest run -p ludion-testkit --test camera_chase -E 'test(/committed_cross_arch_trace/)'\n\
# The rig's state (collided boom, zone, framing weight) and the Fade writes are in the hash. The M4\n\
# must reproduce this file byte-identically - never fabricated by copying the Windows trace.\n";
        std::fs::write(trace_path(), format!("{header}{}\n", trace.join("\n")))
            .expect("the gallery directory is committed");
        return;
    }
    let Some(committed) = committed_trace() else {
        eprintln!(
            "no committed trace yet - bless with: LUDION_BLESS=1 cargo nextest run -p ludion-testkit \
             --test camera_chase -E 'test(/committed_cross_arch_trace/)'"
        );
        return;
    };
    assert_eq!(
        committed.len(),
        TICKS,
        "the committed trace covers every tick"
    );
    for (tick, (actual, expected)) in trace.iter().zip(&committed).enumerate() {
        assert_eq!(actual, expected, "the trace diverges at tick {tick}");
    }
    let mut distinct: Vec<&String> = trace.iter().collect();
    distinct.sort_unstable();
    distinct.dedup();
    assert!(distinct.len() > TICKS / 2, "the run barely changes state");
}

#[test]
fn the_trace_is_identical_at_every_thread_count() {
    let reference = par::with_threads(THREAD_COUNTS[0], || run(TICKS));
    for threads in &THREAD_COUNTS[1..] {
        assert_eq!(
            par::with_threads(*threads, || run(TICKS)),
            reference,
            "{threads} threads"
        );
    }
}

#[test]
fn a_snapshot_mid_pull_in_and_mid_peek_rejoins_the_run() {
    let reference = run(TICKS);
    for at in [PULL_IN_SNAPSHOT, PEEK_SNAPSHOT] {
        let mut first = build();
        let head = trace_from(&mut first, 0, at);
        assert_eq!(head, reference[..at], "the head up to {at}");
        let rig = rig_of(&first);
        assert!(rig.state.boom.is_some(), "the boom is pulled in at {at}");
        if at == PEEK_SNAPSHOT {
            let weight = rig.state.framing_weight;
            assert!(0.0 < weight && weight < 1.0, "mid-peek weight {weight}");
        }
        let snapshot = match first.execute(Command::EngineSnapshot).unwrap().body {
            ResponseBody::Snapshot { snapshot } => snapshot,
            other => panic!("expected a snapshot, got {other:?}"),
        };

        let mut resumed = build();
        match resumed
            .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
            .unwrap()
            .body
        {
            ResponseBody::Restored { .. } => {}
            other => panic!("expected a restore, got {other:?}"),
        }
        assert_eq!(
            trace_from(&mut resumed, at, TICKS),
            reference[at..],
            "the run restored at {at} rejoins"
        );
    }
}

/// What one uninterrupted run observed, tick by tick.
struct Observed {
    boom: Vec<Option<f32>>,
    zones: Vec<Option<EntityId>>,
    framing: Vec<f32>,
    relative_ok: Vec<bool>,
    occluded: Vec<bool>,
    min_fade: f32,
    eye_clear: Vec<bool>,
}

fn observe() -> (Observed, EntityId, EntityId) {
    let mut engine = build();
    let hero = common::CHASE_HERO;
    let prop = {
        let world = engine.world_mut();
        named(world, "prop")
    };
    let layers = LayerSet::from(vec![common::CAMERA_PASS_LAYER.to_string()]);
    let mut seen = Observed {
        boom: Vec::new(),
        zones: Vec::new(),
        framing: Vec::new(),
        relative_ok: Vec::new(),
        occluded: Vec::new(),
        min_fade: 1.0,
        eye_clear: Vec::new(),
    };
    for _ in 0..TICKS {
        engine.advance_live();
        let rig = rig_of(&engine);
        let world = engine.world();
        let eye = rig.aim().expect("a posed rig").eye;
        let physics = world
            .resource::<PhysicsWorld>()
            .expect("the scene has physics");
        let filter = QueryFilter::new()
            .ignore_layer(common::CAMERA_PASS_LAYER)
            .exclude(hero);
        let touching = physics
            .overlap(
                &Shape::Sphere {
                    radius: COLLISION_RADIUS,
                },
                eye,
                [0.0, 0.0, 0.0, 1.0],
                &filter,
            )
            .expect("the overlap query runs");
        seen.eye_clear.push(touching.is_empty());
        seen.boom.push(rig.state.boom);
        seen.zones.push(rig.state.zone);
        seen.framing.push(rig.state.framing_weight);
        seen.relative_ok.push(
            camera_relative([0.0, 1.0], &rig) == camera_relative_yaw([0.0, 1.0], rig.state.yaw_deg),
        );
        let listed = camera_occluders(world, camera(world), &layers, 0.2).unwrap();
        seen.occluded.push(listed.contains(&prop));
        let fade = world.get::<Fade>(prop).unwrap().unwrap().fade;
        seen.min_fade = seen.min_fade.min(fade);
    }
    let (wide, inner) = {
        let world = engine.world_mut();
        (named(world, "zone-wide"), named(world, "zone-inner"))
    };
    (seen, wide, inner)
}

#[test]
fn the_chase_pulls_in_crosses_two_zones_fades_a_prop_and_peeks_a_corner() {
    let (seen, wide, inner) = observe();
    let orbit = common::ORBIT_START as usize..common::ORBIT_END as usize;
    assert!(
        seen.boom[orbit.clone()].iter().any(Option::is_some),
        "the pillar pulls the boom in somewhere in the orbit"
    );
    assert!(
        seen.boom[..orbit.start].iter().all(Option::is_none),
        "the open yard before the orbit leaves the boom free"
    );
    let golden =
        seen.boom[common::CHASE_GOLDEN_TICK as usize].expect("pulled in under the ceiling");
    assert!(golden < 4.0, "shorter than the inner zone's boom: {golden}");
    assert!(
        seen.eye_clear.iter().all(|&clear| clear),
        "the eye never overlaps geometry off the camera_pass layer: first at tick {:?}",
        seen.eye_clear.iter().position(|&clear| !clear)
    );

    let mut sequence: Vec<Option<EntityId>> = Vec::new();
    for &zone in &seen.zones {
        if sequence.last() != Some(&zone) {
            sequence.push(zone);
        }
    }
    assert_eq!(
        sequence,
        [None, Some(wide), Some(inner), Some(wide), None],
        "none, wide, inner, wide, none"
    );
    let entered = seen.zones.iter().position(Option::is_some).unwrap();
    assert!(
        entered < common::ROOM_ENTER_TICK as usize,
        "the wide zone's edge is outside the room: tick {entered}"
    );

    let start = common::PEEK_START as usize;
    assert!(seen.framing[..start].iter().all(|&w| w == 0.0));
    assert!(
        seen.framing[start + 15] > 0.9,
        "the peek is nearly applied within 0.25 s: {}",
        seen.framing[start + 15]
    );
    let full: Vec<usize> = (0..TICKS).filter(|&t| seen.framing[t] == 1.0).collect();
    assert!(!full.is_empty(), "the weight rests at exactly 1");
    assert!(
        full.iter().all(|&t| seen.relative_ok[t]),
        "camera_relative ignores the framing yaw during the peek"
    );
    assert_eq!(
        seen.framing[TICKS - 1],
        0.0,
        "the peek returns to exactly 0"
    );

    assert!(
        seen.occluded.iter().any(|&listed| listed),
        "camera_occluders lists the prop"
    );
    assert!(seen.min_fade < 1.0, "the prop faded: {}", seen.min_fade);
}

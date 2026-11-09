//! Request 0020's physics-horde scene (`examples/gallery/physics-horde.yaml`): 500 walking
//! `CharacterMotor`s, 120 skinned `SkinBone` ragdolls (24 pinned) re-thrown on a timer and 5,000
//! pool projectiles, driven by `horde_driver.rs` through the real `ludion-physics` schedule.
//!
//! Mirrors `physics_stress.rs` at this scene's shape: a committed per-tick `WorldHash` trace
//! (`examples/gallery/physics-horde.trace.txt`) for a second pinned machine to reproduce, a
//! snapshot taken mid-run that resumes into the rest of that trace, the committed replay
//! (`physics-horde.lrp`) with a size ceiling, and 1/2/8-thread identity. The driver's state is all
//! world state, so the restore and the replay need no custom hook beyond the same systems.

use std::path::PathBuf;

use ludion_core::par;
use ludion_ecs::World;
use ludion_engine::{Engine, Projectiles, SkinBone};
use ludion_physics::{CharacterMotor, PhysicsWorld};
use ludion_protocol::command::{EngineRestoreParams, SimStepParams};
use ludion_protocol::{Command, ResponseBody};
use ludion_spatial::{Body, Collider, Joint};
use serde_json::Value;

use crate::horde_driver::{MOTORS, PINNED, RAGDOLLS, SHOTS, build, gallery_root, horde_builder};

/// Ticks the trace covers; crosses the keyframe at tick 300.
const TICKS: usize = 360;
/// The tick the resume test's snapshot is taken at, a few ticks after a re-throw (tick 180).
const SNAPSHOT_AT: usize = 200;
/// Distinct hash lines the trace must hold. Measured 360 of 360 on 2026-11-09 on an unpinned laptop;
/// the floor is about 90% of it.
const MIN_DISTINCT: usize = 324;
/// The window around `SNAPSHOT_AT` (0-based tick indices) that must be live.
const LIVE_WINDOW: std::ops::Range<usize> = 190..210;
/// Distinct hashes `LIVE_WINDOW` must hold. Measured 20 of 20 on 2026-11-09 on an unpinned laptop.
const LIVE_WINDOW_MIN_DISTINCT: usize = 18;
/// The most bytes a fresh 360-tick recording may take. Measured 2852149 bytes on 2026-11-09
/// on an unpinned laptop; recorded bytes are deterministic, so the margin is headroom for intended growth.
const REPLAY_MAX_BYTES: usize = 3_570_000;

fn replay_path() -> PathBuf {
    gallery_root().join("physics-horde.lrp")
}

fn reference_path() -> PathBuf {
    gallery_root().join("physics-horde.trace.txt")
}

/// Advances `n` live ticks of `engine`, one `<tick:04> <hash:016x>` line per tick.
fn run_engine(engine: &mut Engine, n: usize) -> Vec<String> {
    (0..n)
        .map(|tick| {
            engine.advance_live();
            let hash = engine.world_hash().unwrap().as_u64();
            format!("{tick:04} {hash:016x}")
        })
        .collect()
}

fn run(n: usize) -> Vec<String> {
    run_engine(&mut build(), n)
}

/// The committed trace's lines, comment/blank-filtered.
fn committed_trace() -> Vec<String> {
    std::fs::read_to_string(reference_path())
        .expect("the committed reference trace")
        .lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .map(str::to_owned)
        .collect()
}

fn hashes_of(lines: &[String]) -> Vec<&str> {
    lines
        .iter()
        .map(|line| line.split_whitespace().nth(1).expect("a hash column"))
        .collect()
}

fn distinct(lines: &[String]) -> usize {
    let mut sorted: Vec<&String> = lines.iter().collect();
    sorted.sort_unstable();
    sorted.dedup();
    sorted.len()
}

fn snapshot_of(engine: &mut Engine) -> Value {
    match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    }
}

fn restore(engine: &mut Engine, snapshot: Value) {
    match engine
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap()
        .body
    {
        ResponseBody::Restored { .. } => {}
        other => panic!("expected a restore, got {other:?}"),
    }
}

#[test]
fn the_horde_holds_its_shape() {
    let mut engine = build();
    let world: &mut World = engine.world_mut();
    assert_eq!(world.query_ids::<(&CharacterMotor,)>().count(), MOTORS);
    let statics = world
        .query::<(&Collider,)>()
        .filter(|(collider,)| matches!(collider.body, Body::Static(_)))
        .count();
    assert!(
        statics >= 21,
        "{statics} statics: a floor, four walls, 16 pillars"
    );

    for _ in 0..2 {
        engine.advance_live();
    }
    let world = engine.world_mut();
    assert_eq!(world.query_ids::<(&SkinBone,)>().count(), RAGDOLLS * 11);
    let joints = world.query_ids::<(&Joint,)>().count();
    assert!(
        joints >= RAGDOLLS * 10 + PINNED,
        "{joints} joints: ten per ragdoll and a pin for each of the first {PINNED}"
    );
    // `horde_shots` fills the pool to `SHOTS` each tick before the step; shots that hit and spend
    // their bounces in the step have already left, so the live count sits just under capacity.
    let live = engine
        .world()
        .resource::<Projectiles>()
        .unwrap()
        .pool()
        .len();
    assert!(
        (SHOTS * 3 / 4..=SHOTS).contains(&live),
        "{live} live shots of a {SHOTS}-shot pool"
    );
    assert!(engine.world().resource::<PhysicsWorld>().is_some());
}

/// The horde's live count (500 motors, 1,320 ragdoll parts) is past the gated scale: `L4034`
/// reports once, however many ticks follow.
#[test]
fn the_horde_warns_once_past_the_gated_scale() {
    let mut engine = build();
    for _ in 0..4 {
        engine.advance_live();
    }
    let reported: Vec<_> = engine
        .world()
        .resource::<ludion_engine::Diagnostics>()
        .unwrap()
        .reported(ludion_core::codes::code::PHYSICS_SCALE_UNGATED)
        .collect();
    assert_eq!(reported, ["physics_scale"]);
}

#[test]
fn two_in_process_runs_of_the_horde_are_identical() {
    assert_eq!(
        run(TICKS),
        run(TICKS),
        "the same committed scene, stepped twice, must trace identically"
    );
}

#[test]
fn the_horde_matches_the_committed_cross_arch_trace() {
    let trace = run(TICKS);
    let path = reference_path();

    if ludion_core::bless::bless_requested() {
        let header = "\
# Per-tick WorldHash of examples/gallery/physics-horde.yaml, stepped by the horde driver\n\
# (crates/ludion-engine/tests/horde_driver.rs, physics_horde.rs): 500 CharacterMotors, 120 SkinBone\n\
# ragdolls (24 pinned) re-thrown every 30 ticks, 5000 pool projectiles.\n\
# Recorded with: LUDION_BLESS=1 cargo nextest run -p ludion-engine --test it -E 'test(/^physics_horde::/)'\n\
# The M4 and the RTX 3090 must reproduce this file byte-identically (Plan/physics.md gate 1,\n\
# cross-arch) - never fabricated by copying another machine's trace.\n";
        std::fs::write(&path, format!("{header}{}\n", trace.join("\n")))
            .expect("the gallery directory is committed");
        return;
    }

    let committed = committed_trace();
    assert_eq!(
        committed.len(),
        TICKS,
        "the committed trace covers every tick"
    );
    for (tick, (actual, expected)) in trace.iter().zip(&committed).enumerate() {
        assert_eq!(actual, expected, "the trace diverges at tick {tick}");
    }

    // The hash canonicalizes every non-finite float to one value, so an empty diff against a
    // degenerate (never-moved, or gone-non-finite) horde would prove nothing.
    assert!(
        distinct(&trace) >= MIN_DISTINCT,
        "only {} distinct lines of {TICKS}; the horde settled too early to be chaotic",
        distinct(&trace)
    );
}

/// The snapshot of a fresh run at `SNAPSHOT_AT`, restored into a *third*, freshly built engine,
/// reproduces the committed trace's hash at the boundary and every line after it.
#[test]
fn a_restored_horde_reproduces_the_committed_trace_from_the_snapshot_tick() {
    let committed = committed_trace();
    assert_eq!(
        committed.len(),
        TICKS,
        "the committed trace covers every tick"
    );

    let mut warm = build();
    let first_part = run_engine(&mut warm, SNAPSHOT_AT);
    assert_eq!(
        first_part,
        committed[..SNAPSHOT_AT],
        "a fresh run's first {SNAPSHOT_AT} ticks must match the committed trace"
    );

    let document = snapshot_of(&mut warm);
    assert!(
        matches!(document.get("physics"), Some(Value::String(_))),
        "a horde with {RAGDOLLS} ragdolls must carry solver state in the `physics` field at tick \
         {SNAPSHOT_AT}"
    );

    let mut resumed = build();
    restore(&mut resumed, document);
    assert_eq!(
        format!("{:016x}", resumed.world_hash().unwrap().as_u64()),
        committed[SNAPSHOT_AT - 1]
            .split_whitespace()
            .nth(1)
            .unwrap(),
        "the restore boundary itself matches committed tick {}, so a component fault and a solver \
         fault cannot be confused with each other",
        SNAPSHOT_AT - 1
    );

    for (offset, expected) in committed[SNAPSHOT_AT..].iter().enumerate() {
        resumed.advance_live();
        let tick = SNAPSHOT_AT + offset;
        let line = format!("{tick:04} {:016x}", resumed.world_hash().unwrap().as_u64());
        assert_eq!(&line, expected, "the resumed trace diverges at tick {tick}");
    }

    assert!(
        distinct(&committed[LIVE_WINDOW]) >= LIVE_WINDOW_MIN_DISTINCT,
        "the window {}..{} holds too few distinct hashes; the capture tick must be provably live",
        LIVE_WINDOW.start,
        LIVE_WINDOW.end
    );
}

/// A fresh in-process recording of the committed scene reproduces the committed trace, and both the
/// fresh bytes and the committed `.lrp` replay to it. The 360 frames cross the keyframe at tick 300.
/// Recorded at the default stream extent.
#[test]
fn the_committed_horde_replay_reproduces_the_committed_trace() {
    let bless = ludion_core::bless::bless_requested();
    let gallery = gallery_root();
    let committed = committed_trace();
    let committed_hashes = hashes_of(&committed);

    let mut engine = build();
    let fresh_path =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("ludion-engine-physics-horde.lrp");
    engine
        .set_recording(std::fs::File::create(&fresh_path).expect("the fresh recording is writable"))
        .expect("recording arms");
    let live = ludion_engine::drive_hashes(
        &mut engine,
        vec![Command::SimStep(SimStepParams {
            ticks: TICKS as u64,
            hash: false,
        })],
    )
    .expect("the session runs");
    engine.stop_recording().expect("recording flushes");

    // `live[0]` is the pre-step sample; the rest is one hash per tick, in trace order.
    let live_hashes: Vec<String> = live[1..]
        .iter()
        .map(|(_, hash)| format!("{:016x}", hash.as_u64()))
        .collect();
    assert_eq!(
        live_hashes, committed_hashes,
        "the recorded session's trace must match the committed reference"
    );

    let fresh_bytes = std::fs::read(&fresh_path).expect("the fresh recording reads back");
    assert!(
        fresh_bytes.len() <= REPLAY_MAX_BYTES,
        "the physics-horde recording is {} bytes, over its {REPLAY_MAX_BYTES}-byte ceiling — a \
         keyframe's physics blob grew; find why before raising it",
        fresh_bytes.len()
    );

    let replay_of = |bytes: &[u8]| -> Vec<String> {
        ludion_engine::replay_hashes_with(horde_builder(), bytes, &gallery).expect("replays")[1..]
            .iter()
            .map(|(_, hash)| format!("{:016x}", hash.as_u64()))
            .collect()
    };
    assert_eq!(
        replay_of(&fresh_bytes),
        committed_hashes,
        "the fresh recording must replay to the committed trace"
    );

    if bless {
        std::fs::write(replay_path(), &fresh_bytes).expect("adopting the fresh recording");
        return;
    }

    let committed_bytes = std::fs::read(replay_path()).expect("the committed physics-horde.lrp");
    assert_eq!(
        replay_of(&committed_bytes),
        committed_hashes,
        "the committed physics-horde.lrp no longer replays to the committed trace — after an \
         intentional change, re-record it with `LUDION_BLESS=1 cargo nextest run -p ludion-engine \
         --test it -E 'test(=physics_horde::the_committed_horde_replay_reproduces_the_committed_trace)'` \
         and commit it"
    );
}

#[test]
fn horde_hashes_are_thread_count_invariant() {
    // Each `par::with_threads` override is installed inside its own spawned thread: the override is
    // thread-local, so setting it on the calling thread would not reach the thread that ticks.
    #[allow(clippy::disallowed_methods)]
    let [one, two, eight] = std::thread::scope(|scope| {
        [1usize, 2, 8]
            .map(|n| scope.spawn(move || par::with_threads(n, || run(TICKS))))
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|payload| std::panic::resume_unwind(payload))
            })
    });
    assert_eq!(one, two, "the 1-thread and 2-thread horde traces diverge");
    assert_eq!(one, eight, "the 1-thread and 8-thread horde traces diverge");
}

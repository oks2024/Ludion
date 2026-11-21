//! The gallery `paired-takedown` scene's CPU proofs: two motor figures are fit, started and warped
//! under `common::takedown_builder`, the guard ragdolls on `quiet` (joint zeros at rest) and gets up
//! again. The run traces deterministically, matches the committed cross-arch trace, restores from a
//! snapshot taken after the switch and hashes identically at every thread count. CPU-only on purpose:
//! the rendered proof is `gallery.rs`'s `paired-takedown-*` goldens.

mod common;

use std::path::PathBuf;

use ludion_core::ids::EntityId;
use ludion_core::par;
use ludion_engine::components::{Name, Transform};
use ludion_engine::{AnimationEvents, Engine, EngineBuilder};
use ludion_protocol::command::EngineRestoreParams;
use ludion_protocol::{Command, ResponseBody};
use serde_json::Value;

/// The get-up and its fade, then a settled stand.
const TICKS: usize = common::TAKEDOWN_GETUP_TICK as usize + 60;
const THREAD_COUNTS: [usize; 3] = [1, 2, 8];

fn gallery_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery")
}

fn scene_path() -> PathBuf {
    gallery_root().join("paired-takedown.yaml")
}

fn trace_path() -> PathBuf {
    gallery_root().join("paired-takedown.trace.txt")
}

fn build_with(builder: EngineBuilder) -> Engine {
    let mut engine = builder
        .project_root(gallery_root())
        .build()
        .expect("the engine builds");
    engine
        .load_inline_scene(&std::fs::read_to_string(scene_path()).expect("the committed scene"))
        .expect("the committed scene loads");
    engine
}

fn build() -> Engine {
    build_with(common::takedown_builder())
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

fn committed_trace() -> Vec<String> {
    std::fs::read_to_string(trace_path())
        .expect("the committed reference trace")
        .lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .map(str::to_owned)
        .collect()
}

fn find(engine: &Engine, name: &str) -> EntityId {
    let world = engine.world();
    world
        .entities()
        .find(|id| {
            world
                .get_unrecorded::<Name>(*id)
                .ok()
                .flatten()
                .is_some_and(|n| n.0 == name)
        })
        .expect("a named entity")
}

fn transform(engine: &Engine, id: EntityId) -> Transform {
    engine
        .world()
        .get::<Transform>(id)
        .unwrap()
        .cloned()
        .expect("a Transform")
}

/// The heading of the entity's -Z axis, as `paired::heading` reads it (an Euler Y angle alone cannot
/// say it: a yaw of pi is stored with x and z near 180).
fn heading(rotation: [f32; 3]) -> f32 {
    let [x, y, z, w] = ludion_core::math::euler_xyz_degrees_to_quat(rotation);
    ludion_core::math::atan2(2.0 * (x * z + w * y), 1.0 - 2.0 * (x * x + y * y))
}

#[test]
fn two_in_process_runs_of_the_takedown_are_identical() {
    assert_eq!(run(TICKS), run(TICKS));
}

/// The fit passes (the driver asserts it), and after the warp the instigator stands at the authored
/// pose in the guard's frame: 0.5 m in front of it, facing it.
#[test]
fn the_warp_lands_on_the_authored_relative_pose() {
    let mut engine = build();
    for _ in 0..=common::TAKEDOWN_START + 20 {
        engine.advance_live();
    }
    let (player, guard) = (find(&engine, "player"), find(&engine, "guard"));
    let (p, g) = (transform(&engine, player), transform(&engine, guard));
    assert!(
        (p.translation[0] - g.translation[0]).abs() < 1e-4
            && (p.translation[2] - g.translation[2] - 0.5).abs() < 1e-4,
        "instigator {:?}, guard {:?}",
        p.translation,
        g.translation
    );
    let apart = (heading(p.rotation) - heading(g.rotation)).abs();
    assert!(
        (apart - std::f32::consts::PI).abs() < 1e-4,
        "the two face each other: {apart}"
    );
}

/// Pins the driven goldens' frame counts: the switch adds the 10 guard parts on its tick and nowhere
/// else before the get-up, and the get-up folds them back.
#[test]
fn the_switch_and_the_getup_land_on_their_ticks() {
    let mut engine = build();
    let before = engine.world().entity_count();
    let mut counts = Vec::new();
    for _ in 0..=common::TAKEDOWN_GETUP_TICK + 1 {
        engine.advance_live();
        counts.push(engine.world().entity_count());
    }
    let switch = common::TAKEDOWN_SWITCH_TICK as usize;
    let getup = common::TAKEDOWN_GETUP_TICK as usize;
    assert!(
        counts[..switch].iter().all(|&n| n == before),
        "no entity appears before the switch: {counts:?}"
    );
    assert_eq!(counts[switch], before + 10, "the switch adds the 10 parts");
    assert!(
        counts[switch..getup].iter().all(|&n| n == before + 10),
        "the parts stay until the get-up: {counts:?}"
    );
    assert_eq!(counts[getup], before, "the get-up folds the parts back");
}

#[test]
fn grab_and_quiet_fire_on_their_ticks() {
    let mut engine = build();
    let guard = find(&engine, "guard");
    let (mut grab, mut quiet) = (None, None);
    for tick in 0..=common::TAKEDOWN_SWITCH_TICK + 2 {
        engine.advance_live();
        let fired = |name: &str| {
            engine
                .world()
                .resource::<AnimationEvents>()
                .is_some_and(|events| {
                    events
                        .iter()
                        .any(|e| e.entity == guard && e.clip == "paired:takedown" && e.name == name)
                })
        };
        if fired("grab") {
            grab = Some(tick);
        }
        if fired("quiet") {
            quiet = Some(tick);
        }
    }
    assert_eq!(grab, Some(common::TAKEDOWN_GRAB_TICK));
    assert_eq!(quiet, Some(common::TAKEDOWN_SWITCH_TICK));
}

#[test]
fn the_takedown_matches_the_committed_cross_arch_trace() {
    let trace = run(TICKS);
    if ludion_core::bless::bless_requested() {
        let header = "\
# Per-tick WorldHash of examples/gallery/paired-takedown.yaml under common::takedown_builder\n\
# (crates/ludion-testkit/tests/paired_takedown.rs).\n\
# Recorded with: LUDION_BLESS=1 cargo nextest run -p ludion-testkit --test paired_takedown -E 'test(cross_arch)'\n\
# The M4 must reproduce this file byte-identically - never fabricated by copying the Windows trace.\n";
        std::fs::write(trace_path(), format!("{header}{}\n", trace.join("\n")))
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
}

fn snapshot_of(engine: &mut Engine) -> Value {
    match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    }
}

/// A snapshot taken 20 ticks after the switch, restored into a fresh engine, continues on the
/// uninterrupted trace, through the get-up.
#[test]
fn a_restore_after_the_switch_reproduces_the_trace() {
    let at = common::TAKEDOWN_SWITCH_TICK as usize + 20;
    let expected = run(TICKS);

    let mut warm = build();
    assert_eq!(trace_from(&mut warm, 0, at), expected[..at]);
    let snapshot = snapshot_of(&mut warm);

    let mut resumed = build();
    let response = resumed
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap();
    assert!(
        matches!(response.body, ResponseBody::Restored { .. }),
        "expected a restore, got {:?}",
        response.body
    );
    assert_eq!(
        format!("{:016x}", resumed.world_hash().unwrap().as_u64()),
        expected[at - 1].split_whitespace().nth(1).unwrap(),
        "the restore boundary matches tick {}",
        at - 1
    );
    assert_eq!(trace_from(&mut resumed, at, TICKS), expected[at..]);
}

/// The full run at 1, 2 and 8 threads, each override installed inside its own spawned thread (the
/// override is thread-local).
#[test]
fn the_takedown_hashes_identically_at_every_thread_count() {
    #[allow(clippy::disallowed_methods)]
    let [one, two, eight] = std::thread::scope(|scope| {
        THREAD_COUNTS
            .map(|n| scope.spawn(move || par::with_threads(n, || run(TICKS))))
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|payload| std::panic::resume_unwind(payload))
            })
    });
    assert_eq!(one, two, "the 1-thread and 2-thread traces diverge");
    assert_eq!(one, eight, "the 1-thread and 8-thread traces diverge");
}

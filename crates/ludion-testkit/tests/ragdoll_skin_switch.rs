//! The gallery `ragdoll-skin-switch` scene's CPU proofs: the running mannequin switches to its
//! `SkinBone` ragdoll under `common::skin_switch_builder`, and the run traces deterministically,
//! matches the committed cross-arch trace, restores from a snapshot taken after the switch and
//! hashes identically at every thread count. CPU-only on purpose: the rendered proof is
//! `gallery.rs`'s `ragdoll-skin-switch-driven` golden.

mod common;

use std::path::PathBuf;

use ludion_core::par;
use ludion_engine::{Engine, EngineBuilder};
use ludion_protocol::command::EngineRestoreParams;
use ludion_protocol::{Command, ResponseBody};
use serde_json::Value;

/// The switch at tick 40, then 200 ticks of tumble.
const TICKS: usize = 240;
const THREAD_COUNTS: [usize; 3] = [1, 2, 8];

fn gallery_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery")
}

fn scene_path() -> PathBuf {
    gallery_root().join("ragdoll-skin-switch.yaml")
}

fn trace_path() -> PathBuf {
    gallery_root().join("ragdoll-skin-switch.trace.txt")
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
    build_with(common::skin_switch_builder())
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

#[test]
fn two_in_process_runs_of_the_ragdoll_skin_switch_are_identical() {
    assert_eq!(run(TICKS), run(TICKS));
}

/// Pins the driven golden's frame count: the switch adds the 11 parts on its tick and nowhere else.
#[test]
fn the_switch_lands_on_its_tick() {
    let mut engine = build();
    let before = engine.world().entity_count();
    let mut counts = Vec::new();
    for _ in 0..=common::SWITCH_SKIN_TICK {
        engine.advance_live();
        counts.push(engine.world().entity_count());
    }
    let tick = common::SWITCH_SKIN_TICK as usize;
    assert!(
        counts[..tick].iter().all(|&n| n == before),
        "no entity appears before the switch: {counts:?}"
    );
    assert_eq!(counts[tick], before + 11, "the switch adds the 11 parts");
}

#[test]
fn the_ragdoll_skin_switch_matches_the_committed_cross_arch_trace() {
    let trace = run(TICKS);
    if ludion_core::bless::bless_requested() {
        let header = "\
# Per-tick WorldHash of examples/gallery/ragdoll-skin-switch.yaml under common::skin_switch_builder\n\
# (crates/ludion-testkit/tests/ragdoll_skin_switch.rs).\n\
# Recorded with: LUDION_BLESS=1 cargo nextest run -p ludion-testkit --test ragdoll_skin_switch -E 'test(cross_arch)'\n\
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
/// uninterrupted trace; the restored world never re-fires the switch.
#[test]
fn a_restore_after_the_switch_reproduces_the_trace() {
    let at = common::SWITCH_SKIN_TICK as usize + 20;
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
fn the_switch_hashes_identically_at_every_thread_count() {
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

/// The "no switch is unchanged" clause: an engine without the switching system traces the same
/// until the switch tick, then diverges on it.
#[test]
fn ticks_before_the_switch_match_a_scene_with_no_switch() {
    let switch = common::SWITCH_SKIN_TICK as usize;
    let with = run(switch + 1);
    let without = trace_from(&mut build_with(Engine::builder()), 0, switch + 1);
    assert_eq!(with[..switch], without[..switch]);
    assert_ne!(with[switch], without[switch], "the switch moves the hash");
}

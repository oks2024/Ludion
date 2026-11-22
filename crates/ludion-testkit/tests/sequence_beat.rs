//! The gallery `sequence-beat` scene's CPU proofs: `sequences/demo.yaml` played over the scene under
//! `common::sequence_beat_builder`, traced by `ludion_testkit::sequence::sequence_trace`. The rendered
//! proof is `gallery.rs`'s `sequence_beat_at` goldens, rendered through `ludion render --sequence`.

mod common;

use std::path::PathBuf;

use ludion_core::ids::EntityId;
use ludion_core::par;
use ludion_ecs::World;
use ludion_engine::sequences::SequenceHandle;
use ludion_engine::{Engine, Name, SequencePreview, Transform, builtin_registry};
use ludion_protocol::command::EngineRestoreParams;
use ludion_protocol::{Command, ResponseBody};
use ludion_testkit::sequence::sequence_trace;

const THREAD_COUNTS: [usize; 3] = [1, 2, 8];
/// Ticks past the beat's end: it parks 31 ticks on its dialogue line, so it completes on tick 151.
const TICKS: u64 = 160;
/// A tick inside the crate's span (20..60) and a tick inside the dialogue wait (50..80).
const MID_SPAN: u64 = 35;
const MID_WAIT: u64 = 65;

fn gallery_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery")
}

fn scene_path() -> PathBuf {
    gallery_root().join("sequence-beat.yaml")
}

/// The beat built and loaded, not yet playing.
fn load() -> Engine {
    let mut engine = common::sequence_beat_builder()
        .project_root(gallery_root())
        .build()
        .expect("the sequence-beat engine builds");
    engine
        .load_inline_scene(&std::fs::read_to_string(scene_path()).expect("the committed scene"))
        .expect("the committed scene loads");
    engine
}

/// The beat built, loaded and playing before tick 0.
fn build() -> (Engine, SequenceHandle) {
    let mut engine = load();
    let handle = common::play_beat(engine.world_mut());
    (engine, handle)
}

fn named(world: &World, wanted: &str) -> EntityId {
    world
        .entities()
        .find(|&id| {
            world
                .get::<Name>(id)
                .ok()
                .flatten()
                .is_some_and(|n| n.0 == wanted)
        })
        .unwrap_or_else(|| panic!("no entity named {wanted}"))
}

fn transform(engine: &Engine, wanted: &str) -> Transform {
    let world = engine.world();
    *world
        .get::<Transform>(named(world, wanted))
        .unwrap()
        .unwrap()
}

/// The trace the beat prints, derived from `sequences/demo.yaml` by hand. Its keys sit at `at`
/// 10, 20, 30, 40 and 50. The line said at 50 lives 30 ticks, so it is gone after tick 80's
/// `dialogue_update`; `sequence_update` runs before it in a tick, so the wait clears on tick 81 and every
/// key after it lands 31 ticks later than its `at`; the beat (length 120) completes on tick 151.
fn expected(player: EntityId) -> Vec<String> {
    [
        "0010 key camera at 10",
        "0010 key animation at 10",
        "0020 key transform at 20",
        "0030 key transform at 30",
        "0040 key audio at 40",
        "0050 key dialogue at 50",
        "0050 wait line_end at 50",
        "0081 resume at 50",
        "0091 key event at 60",
        "0091 fire beat_midpoint",
        "0101 key animation at 70",
        "0131 key camera at 100",
        "0141 key event at 110",
        "0141 fire beat_done",
        "0151 end completed",
    ]
    .iter()
    .map(|line| {
        let (tick, rest) = line.split_once(' ').unwrap();
        format!("{tick} {player} {rest}")
    })
    .collect()
}

#[test]
fn the_beat_matches_its_expected_trace() {
    let (mut engine, handle) = build();
    let trace = sequence_trace(&mut engine, TICKS);
    assert_eq!(trace, expected(handle.0));
}

#[test]
fn holds_gameplay_freezes_the_unbound_patroller() {
    let (mut engine, _) = build();
    // `path_follow` runs before `sequence_update`, so the patroller takes one step on tick 0, before the
    // hold engages; the baseline is the pose after it.
    sequence_trace(&mut engine, 1);
    let start = transform(&engine, "patroller");
    // The beat holds gameplay through its last tick (151); the clock resumes two sim frames after the
    // beat ends (`docs/game-clock.md`).
    sequence_trace(&mut engine, 151);
    assert_eq!(
        transform(&engine, "patroller"),
        start,
        "frozen from tick 1 through the beat's final tick"
    );
    sequence_trace(&mut engine, 4);
    assert_ne!(
        transform(&engine, "patroller"),
        start,
        "the patroller moves again within a few ticks of the end"
    );
}

#[test]
fn the_trace_and_hashes_are_identical_at_every_thread_count() {
    let run = || {
        let (mut engine, _) = build();
        let mut lines = Vec::new();
        for _ in 0..TICKS {
            lines.extend(sequence_trace(&mut engine, 1));
            let hash = engine.world_hash().unwrap().as_u64();
            lines.push(format!("{:04} {hash:016x}", engine.frame().as_u64()));
        }
        lines
    };
    let reference = par::with_threads(THREAD_COUNTS[0], run);
    for threads in &THREAD_COUNTS[1..] {
        assert_eq!(
            par::with_threads(*threads, run),
            reference,
            "{threads} threads"
        );
    }
}

#[test]
fn a_restore_mid_span_and_mid_wait_rejoins_the_run() {
    let (mut whole, _) = build();
    let mut reference_trace = Vec::new();
    let mut reference_hashes = Vec::new();
    for _ in 0..TICKS {
        reference_trace.extend(sequence_trace(&mut whole, 1));
        let hash = whole.world_hash().unwrap().as_u64();
        reference_hashes.push(format!("{:04} {hash:016x}", whole.frame().as_u64()));
    }
    for at in [MID_SPAN, MID_WAIT] {
        let (mut first, _) = build();
        let head = sequence_trace(&mut first, at);
        if at == MID_WAIT {
            let player = first
                .world()
                .entities()
                .find_map(|id| {
                    first
                        .world()
                        .get::<ludion_engine::sequences::SequencePlayer>(id)
                        .ok()
                        .flatten()
                        .cloned()
                })
                .expect("the beat is live");
            assert!(player.wait.is_some(), "parked on the line at tick {at}");
        }
        let snapshot = match first.execute(Command::EngineSnapshot).unwrap().body {
            ResponseBody::Snapshot { snapshot } => snapshot,
            other => panic!("expected a snapshot, got {other:?}"),
        };
        let mut resumed = load();
        match resumed
            .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
            .unwrap()
            .body
        {
            ResponseBody::Restored { .. } => {}
            other => panic!("expected a restore, got {other:?}"),
        }
        let mut tail = Vec::new();
        let mut tail_hashes = Vec::new();
        for _ in at..TICKS {
            tail.extend(sequence_trace(&mut resumed, 1));
            let hash = resumed.world_hash().unwrap().as_u64();
            tail_hashes.push(format!("{:04} {hash:016x}", resumed.frame().as_u64()));
        }
        let mut joined = head;
        joined.extend(tail);
        assert_eq!(
            joined, reference_trace,
            "the trace rejoins after a restore at {at}"
        );
        assert_eq!(
            tail_hashes,
            reference_hashes[at as usize..],
            "the hashes rejoin after a restore at {at}"
        );
    }
}

#[test]
fn the_preview_world_matches_the_driven_world_at_each_golden_tick() {
    for at in &common::SEQUENCE_GOLDEN_TICKS[..2] {
        let preview = SequencePreview {
            doc: gallery_root().join(common::SEQUENCE_DOC),
            bindings: ["camera", "hero", "crate", "drone"]
                .iter()
                .zip(["cam", "hero", "crate", "drone"])
                .map(|(name, role)| (role.to_owned(), (*name).to_owned()))
                .collect(),
            at: *at,
        };
        let run = ludion_engine::sequence_preview_world(
            &builtin_registry(),
            &scene_path(),
            &[],
            &preview,
        )
        .unwrap_or_else(|d| panic!("{d:?}"));
        let (mut driven, _) = build();
        sequence_trace(&mut driven, at + 1);
        for wanted in ["crate", "drone", "hero"] {
            let world = run.driver.world();
            let previewed = world
                .get::<Transform>(named(world, wanted))
                .unwrap()
                .copied();
            assert_eq!(
                previewed,
                driven
                    .world()
                    .get::<Transform>(named(driven.world(), wanted))
                    .unwrap()
                    .copied(),
                "{wanted} at tick {at}"
            );
        }
    }
}

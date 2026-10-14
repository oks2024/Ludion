//! `Plan/skeletal-animation.md` W3 step 5 — the gallery `ragdoll-skinned` scene: the W1 mannequin as
//! one skinned mesh entity plus 11 `SkinBone` rigid parts, joined by 10 `Joint`s, dropped onto a small
//! staircase.
//!
//! **The committed scene is hand-authored, not generated** (unlike `ragdoll_scene.rs`'s
//! `ragdoll.yaml`): a `SkinBone` part needs no `switch_to_ragdoll`-style conversion, so there is no
//! `generate_scene_text` half here and no `generated_from`. This file only proves the committed scene
//! traces, resumes and replays deterministically — the same trace/resume/thread-identity/CLI gates
//! `ragdoll_scene.rs` and `physics_stress.rs` already carry, one gallery scene over.

use std::path::PathBuf;

use ludion_engine::Engine;
use ludion_protocol::command::{EngineRestoreParams, SimStepParams};
use ludion_protocol::{Command, ResponseBody};
use serde_json::Value;

/// Ticks the trace covers. One figure over three small steps settles much sooner than
/// `ragdoll.yaml`'s six over a full staircase, so this is a fifth of that scene's 600.
const TICKS: usize = 360;
/// How many distinct hash lines the trace must hold before it counts as chaotic rather than settling
/// early and repeating one line. Measured: all 360 lines are distinct (the solver's own state keeps
/// nudging the hash even once the figure looks at rest); this floor keeps a wide margin under that.
const MIN_DISTINCT: usize = 300;
/// The tick the snapshot-format smoke captures at — inside the tumble (the pelvis is still mid-fall
/// at tick 60, well before it reaches the floor).
const SNAPSHOT_SMOKE_TICK: usize = 60;
/// The tick the resume test's snapshot is taken at; the resumed run covers `SNAPSHOT_AT..TICKS`. Falls
/// inside the tumble (measured: the figure is still rolling through the steps at tick 120).
const SNAPSHOT_AT: usize = 120;
/// The window around `SNAPSHOT_AT` (0-based tick indices) that the resume test also checks is still
/// live.
const LIVE_WINDOW: std::ops::Range<usize> = 100..140;
/// How many distinct hashes `LIVE_WINDOW` must hold for the capture tick to count as live.
const LIVE_WINDOW_MIN_DISTINCT: usize = 35;

fn gallery_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery")
}

fn scene_path() -> PathBuf {
    gallery_root().join("ragdoll-skinned.yaml")
}

fn trace_path() -> PathBuf {
    gallery_root().join("ragdoll-skinned.trace.txt")
}

fn replay_path() -> PathBuf {
    gallery_root().join("ragdoll-skinned.lrp")
}

/// A fresh, bare engine with the committed `ragdoll-skinned.yaml` loaded. Its own `physics: space:
/// free` block is the opt-in.
fn build() -> Engine {
    Engine::builder()
        .load_scene_file(&scene_path())
        .expect("the committed gallery scene loads")
}

/// Advances `n` live ticks, returning one `<tick:04> <hash:016x>` line per tick.
fn run(n: usize) -> Vec<String> {
    let mut engine = build();
    (0..n)
        .map(|tick| {
            engine.advance_live();
            let hash = engine.world_hash().unwrap().as_u64();
            format!("{tick:04} {hash:016x}")
        })
        .collect()
}

/// The committed trace's lines, comment/blank-filtered.
fn committed_trace() -> Vec<String> {
    std::fs::read_to_string(trace_path())
        .expect("the committed reference trace")
        .lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .map(str::to_owned)
        .collect()
}

#[test]
fn two_in_process_runs_of_the_ragdoll_skinned_scene_are_identical() {
    assert_eq!(
        run(TICKS),
        run(TICKS),
        "the same committed scene, stepped twice, must trace identically"
    );
}

#[test]
fn the_ragdoll_skinned_scene_matches_the_committed_cross_arch_trace() {
    let trace = run(TICKS);
    let path = trace_path();

    if ludion_core::bless::bless_requested() {
        let header = "\
# Per-tick WorldHash of examples/gallery/ragdoll-skinned.yaml, stepped under a bare engine (its own\n\
# physics: space: free block is the only opt-in) (crates/ludion-engine/tests/ragdoll_skinned_scene.rs).\n\
# Recorded with: LUDION_BLESS=1 cargo test -p ludion-engine --test ragdoll_skinned_scene\n\
# The M4 must reproduce this file byte-identically (Plan/skeletal-animation.md gate, cross-arch) -\n\
# never fabricated by copying the Windows trace.\n";
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

    let mut distinct: Vec<&String> = trace.iter().collect();
    distinct.sort_unstable();
    distinct.dedup();
    assert!(
        distinct.len() >= MIN_DISTINCT,
        "only {} distinct lines of {TICKS}; the figure settled too early to be chaotic",
        distinct.len()
    );
}

/// A session with a jointed skinned ragdoll must carry solver state in the `physics` field.
#[test]
fn the_ragdoll_skinned_scene_snapshot_carries_the_physics_field() {
    let mut engine = build();
    for _ in 0..SNAPSHOT_SMOKE_TICK {
        engine.advance_live();
    }
    let snapshot = match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    };
    assert!(
        matches!(snapshot.get("physics"), Some(Value::String(_))),
        "a scene with a jointed skinned ragdoll must carry solver state in the `physics` field at \
         tick {SNAPSHOT_SMOKE_TICK}"
    );
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

/// Gate 2 at this scene's scale, tied to the committed trace: a fresh engine run to `SNAPSHOT_AT`
/// matches the committed trace's first `SNAPSHOT_AT` lines; its snapshot, restored into a *third*,
/// freshly built engine, reproduces the committed trace's hash at the restore boundary and every line
/// after it.
#[test]
fn a_restored_ragdoll_skinned_scene_reproduces_the_committed_trace_from_the_snapshot_tick() {
    let committed = committed_trace();
    assert_eq!(
        committed.len(),
        TICKS,
        "the committed trace covers every tick"
    );

    let mut warm = build();
    let first_half: Vec<String> = (0..SNAPSHOT_AT)
        .map(|tick| {
            warm.advance_live();
            let hash = warm.world_hash().unwrap().as_u64();
            format!("{tick:04} {hash:016x}")
        })
        .collect();
    assert_eq!(
        first_half,
        committed[..SNAPSHOT_AT],
        "a fresh run's first {SNAPSHOT_AT} ticks must match the committed trace"
    );

    let document = snapshot_of(&mut warm);
    assert!(
        matches!(document.get("physics"), Some(Value::String(_))),
        "a scene with a jointed skinned ragdoll must carry solver state in the `physics` field at \
         tick {SNAPSHOT_AT}"
    );

    let mut resumed = build();
    restore(&mut resumed, document);
    assert_eq!(
        format!("{:016x}", resumed.world_hash().unwrap().as_u64()),
        committed[SNAPSHOT_AT - 1]
            .split_whitespace()
            .nth(1)
            .unwrap(),
        "the restore boundary itself matches committed tick {}",
        SNAPSHOT_AT - 1
    );

    for (offset, expected) in committed[SNAPSHOT_AT..].iter().enumerate() {
        resumed.advance_live();
        let tick = SNAPSHOT_AT + offset;
        let line = format!("{tick:04} {:016x}", resumed.world_hash().unwrap().as_u64());
        assert_eq!(&line, expected, "the resumed trace diverges at tick {tick}");
    }

    let live_window: Vec<&String> = committed[LIVE_WINDOW].iter().collect();
    let mut distinct: Vec<&&String> = live_window.iter().collect();
    distinct.sort_unstable();
    distinct.dedup();
    assert!(
        distinct.len() >= LIVE_WINDOW_MIN_DISTINCT,
        "only {} distinct hashes across ticks {}..{}; the capture tick must be provably live",
        distinct.len(),
        LIVE_WINDOW.start,
        LIVE_WINDOW.end
    );
}

/// The hashes a `<tick> <hash>` trace line list carries, in order.
fn hashes_of(lines: &[String]) -> Vec<&str> {
    lines
        .iter()
        .map(|line| line.split_whitespace().nth(1).expect("a hash column"))
        .collect()
}

/// The committed `ragdoll-skinned.lrp` replay: a fresh in-process recording of the committed scene's
/// `TICKS`-tick session reproduces the committed trace, and both the fresh bytes and the committed
/// `.lrp` bytes replay to it — the `ragdoll_scene.rs` shape, one gallery scene over. Recorded at the
/// default stream extent.
#[test]
fn the_committed_ragdoll_skinned_replay_reproduces_the_committed_trace() {
    let bless = ludion_core::bless::bless_requested();
    let gallery = gallery_root();
    let committed_scene_text =
        std::fs::read_to_string(scene_path()).expect("the committed ragdoll-skinned scene");
    let committed = committed_trace();
    let committed_hashes = hashes_of(&committed);

    let mut engine = Engine::builder()
        .project_root(&gallery)
        .build()
        .expect("the bare engine builds");
    engine
        .load_inline_scene(&committed_scene_text)
        .expect("the committed scene loads");

    let fresh_path =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("ludion-engine-ragdoll-skinned.lrp");
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
    let replay_builder = || Engine::builder().project_root(gallery.clone());
    let replayed = ludion_engine::replay_hashes_with(replay_builder(), &fresh_bytes, &gallery)
        .expect("the fresh recording replays");
    let replayed_hashes: Vec<String> = replayed[1..]
        .iter()
        .map(|(_, hash)| format!("{:016x}", hash.as_u64()))
        .collect();
    assert_eq!(
        replayed_hashes, committed_hashes,
        "the fresh recording must replay to the committed trace"
    );

    if bless {
        std::fs::write(replay_path(), &fresh_bytes).expect("adopting the fresh recording");
        return;
    }

    let committed_bytes = std::fs::read(replay_path()).expect("the committed ragdoll-skinned.lrp");
    let committed_replayed =
        ludion_engine::replay_hashes_with(replay_builder(), &committed_bytes, &gallery)
            .expect("the committed replay runs");
    let committed_replayed_hashes: Vec<String> = committed_replayed[1..]
        .iter()
        .map(|(_, hash)| format!("{:016x}", hash.as_u64()))
        .collect();
    assert_eq!(
        committed_replayed_hashes, committed_hashes,
        "the committed ragdoll-skinned.lrp no longer replays to the committed trace — after an \
         intentional change, re-record it with `LUDION_BLESS=1 cargo test -p ludion-engine --test \
         ragdoll_skinned_scene the_committed_ragdoll_skinned_replay_reproduces_the_committed_trace` \
         and commit it"
    );
}

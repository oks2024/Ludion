//! `Plan/physics.md` W2, gates 1 and 2 for the stress scene: the gallery's 512-body prop pile
//! (`examples/gallery/physics-stress.yaml`) stepped through the real `ludion-physics` schedule, not
//! the render-only path `ludion-testkit`'s gallery suite exercises.
//!
//! Mirrors `physics_gallery.rs` one scene up in scale: loads the committed YAML, whose
//! `physics: space: free` block is the only opt-in there is, under a **plain, bare engine**, steps
//! it for real, and traces the per-tick `WorldHash` against a committed reference
//! (`examples/gallery/physics-stress.trace.txt`) so a second pinned machine has something to
//! reproduce bit-exactly. It also proves gate 2 at this scale: a snapshot taken mid-trace restores
//! into a fresh engine and reproduces the rest of the committed trace exactly.
//!
//! `examples/gallery/physics-stress.lrp` is the committed replay of the same 600 ticks; its
//! keyframe at tick 300 carries the 512-body physics blob, so a fresh recording's size is gated by
//! [`REPLAY_MAX_BYTES`] — the body-count ceiling's snapshot-size watch (`dev-docs/physics-pin.md`).

use std::path::PathBuf;

use ludion_engine::Engine;
use ludion_protocol::command::{EngineRestoreParams, SimStepParams};
use ludion_protocol::{Command, ResponseBody};
use ludion_spatial::{Body, Collider};
use serde_json::Value;

/// The rigid-body count of the stress pile: the gated scale the budget
/// (`budgets/pinned-rtx3090-physics-stress.toml`) and `dev-docs/physics-pin.md` cite.
const RIGID_BODIES: usize = 512;
/// Ticks the trace covers.
const TICKS: usize = 600;
/// How many distinct hash lines the trace must hold before it counts as chaotic rather than a pile
/// that quickly sleeps and repeats one line.
const MIN_DISTINCT: usize = 540;
/// The tick the snapshot-format smoke captures at: well after the pile has started landing and
/// interlocking, so the `physics` field carries a live, non-trivial solver.
const SNAPSHOT_SMOKE_TICK: usize = 100;
/// The tick the resume test's snapshot is taken at; the resumed run covers `SNAPSHOT_AT..TICKS`.
const SNAPSHOT_AT: usize = 300;
/// The window around `SNAPSHOT_AT` (0-based tick indices) that the resume test also checks is
/// still live, so the capture tick is provably mid-simulation rather than a settled repeat.
const LIVE_WINDOW: std::ops::Range<usize> = 280..320;
/// How many distinct hashes `LIVE_WINDOW` must hold for the capture tick to count as live.
const LIVE_WINDOW_MIN_DISTINCT: usize = 35;

/// The most bytes a fresh 600-tick recording of the scene may take. Measured 2029462 bytes on
/// 2026-09-23; recorded bytes are deterministic, so the margin is headroom for intended growth, not
/// noise.
const REPLAY_MAX_BYTES: usize = 2_555_904;

fn scene_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery/physics-stress.yaml")
}

fn gallery_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery")
}

fn replay_path() -> PathBuf {
    gallery_root().join("physics-stress.lrp")
}

fn reference_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/gallery/physics-stress.trace.txt")
}

/// A fresh, bare engine with the scene loaded. The scene's own `physics: space: free` block is the
/// opt-in — `free` ignores the engine's default plane, so the pile falls on `y` rather than hanging
/// frozen under the plane's locks (see `physics_snapshot.rs`'s `game` for the same reasoning).
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
    std::fs::read_to_string(reference_path())
        .expect("the committed reference trace")
        .lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .map(str::to_owned)
        .collect()
}

#[test]
fn two_in_process_runs_of_the_stress_pile_are_identical() {
    assert_eq!(
        run(TICKS),
        run(TICKS),
        "the same committed scene, stepped twice, must trace identically"
    );
}

#[test]
fn the_stress_pile_matches_the_committed_cross_arch_trace() {
    let trace = run(TICKS);
    let path = reference_path();

    if ludion_core::bless::bless_requested() {
        let header = "\
# Per-tick WorldHash of examples/gallery/physics-stress.yaml, stepped under a bare engine (its own\n\
# physics: space: free block is the only opt-in) (crates/ludion-engine/tests/physics_stress.rs).\n\
# Recorded with: LUDION_BLESS=1 cargo nextest run -p ludion-engine --test it -E 'test(/^physics_stress::/)'\n\
# Free space: free ignores the engine's default Plane, so the pile falls on y rather than hanging\n\
# frozen under the plane's locks. The M4 must reproduce this file byte-identically\n\
# (Plan/physics.md gate 1, cross-arch) - never fabricated by copying the Windows trace.\n";
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
    // degenerate (never-moved, or gone-non-finite) pile would prove nothing; the distinct-line
    // count is what rules that out.
    let mut distinct: Vec<&String> = trace.iter().collect();
    distinct.sort_unstable();
    distinct.dedup();
    assert!(
        distinct.len() >= MIN_DISTINCT,
        "only {} distinct lines of {TICKS}; the pile settled too early to be chaotic",
        distinct.len()
    );
}

/// The other half of `physics_snapshot.rs`'s proof, against a scene loaded from disk rather than
/// authored inline: a session with 512 rigid bodies carries the solver state as the `physics`
/// field.
#[test]
fn the_stress_pile_snapshot_carries_the_physics_field() {
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
        "a pile with 512 rigid bodies must carry solver state in the `physics` field at tick \
         {SNAPSHOT_SMOKE_TICK}"
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

/// Gate 2 at this scene's scale, tied to the committed trace rather than only to a second
/// in-process run: a fresh engine run to `SNAPSHOT_AT` matches the committed trace's first
/// `SNAPSHOT_AT` lines; its snapshot, restored into a *third*, freshly built engine, reproduces the
/// committed trace's hash at the restore boundary and every line after it. The M4 must reproduce
/// this same file byte-identically, so tying the proof to the committed bytes — not just to a
/// same-run comparison — is what a second pinned machine actually needs to check against.
#[test]
fn a_restored_stress_pile_reproduces_the_committed_trace_from_tick_300() {
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
        "a pile with 512 rigid bodies must carry solver state in the `physics` field at tick \
         {SNAPSHOT_AT}"
    );

    // A third engine, built fresh from the same committed scene file, so the restore is the only
    // thing carrying the solver's state across — not a world that happens to still hold it.
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

    let live_window: Vec<&String> = committed[LIVE_WINDOW].iter().collect();
    let mut distinct: Vec<&&String> = live_window.iter().collect();
    distinct.sort_unstable();
    distinct.dedup();
    assert!(
        distinct.len() >= LIVE_WINDOW_MIN_DISTINCT,
        "only {} distinct hashes across ticks {}..{}; the capture tick must be provably live, not \
         a settled repeat",
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

#[test]
fn the_stress_pile_holds_the_budgeted_rigid_body_count() {
    let mut engine = build();
    let rigid = engine
        .world_mut()
        .query::<(&Collider,)>()
        .filter(|(collider,)| matches!(collider.body, Body::Rigid(_)))
        .count();
    assert_eq!(
        rigid, RIGID_BODIES,
        "the stress pile's rigid-body count moved; the budget, the trace, the replay and the \
         body-count ceiling doc (dev-docs/physics-pin.md) move together — re-measure them, never \
         derive them"
    );
}

/// The stress pile is the gated scale itself, so `L4034` (past the scale) stays silent.
#[test]
fn the_stress_pile_is_the_gated_scale_and_does_not_warn() {
    assert_eq!(RIGID_BODIES, ludion_physics::GATED_BODIES);
    let mut engine = build();
    for _ in 0..5 {
        engine.advance_live();
    }
    let reported: Vec<_> = engine
        .world()
        .resource::<ludion_engine::Diagnostics>()
        .unwrap()
        .reported(ludion_core::codes::code::PHYSICS_SCALE_UNGATED)
        .collect();
    assert!(reported.is_empty(), "{reported:?}");
}

/// The committed `physics-stress.lrp` replay: a fresh in-process recording of the committed scene's
/// 600-tick session reproduces the committed trace, and both the fresh bytes and the committed
/// `.lrp` bytes replay to it — the same shape as `ragdoll_scene.rs`'s replay test. The 600 frames
/// cross the keyframe at tick 300 (`KEYFRAME_INTERVAL`), which carries the physics blob. Recorded at
/// the default stream extent.
#[test]
fn the_committed_stress_replay_reproduces_the_committed_trace() {
    let bless = ludion_core::bless::bless_requested();
    let gallery = gallery_root();
    let committed_scene_text =
        std::fs::read_to_string(scene_path()).expect("the committed stress scene");
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
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("ludion-engine-physics-stress.lrp");
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
        "the physics-stress recording is {} bytes, over its {REPLAY_MAX_BYTES}-byte ceiling — a \
         keyframe's physics blob grew (Plan/physics.md §Risks \"Snapshot size at scale\"); find why \
         before raising it",
        fresh_bytes.len()
    );

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

    let committed_bytes = std::fs::read(replay_path()).expect("the committed physics-stress.lrp");
    let committed_replayed =
        ludion_engine::replay_hashes_with(replay_builder(), &committed_bytes, &gallery)
            .expect("the committed replay runs");
    let committed_replayed_hashes: Vec<String> = committed_replayed[1..]
        .iter()
        .map(|(_, hash)| format!("{:016x}", hash.as_u64()))
        .collect();
    assert_eq!(
        committed_replayed_hashes, committed_hashes,
        "the committed physics-stress.lrp no longer replays to the committed trace — after an \
         intentional change, re-record it with `LUDION_BLESS=1 cargo nextest run -p ludion-engine --test it \
         -E 'test(=physics_stress::the_committed_stress_replay_reproduces_the_committed_trace)'` and commit it"
    );
}

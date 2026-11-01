//! `Plan/physics.md` W1, gates 1 and the snapshot-format smoke: the gallery's chaotic pile
//! (`examples/gallery/physics-pile.yaml`) stepped through the real `ludion-physics` schedule, not
//! the render-only path `ludion-testkit`'s gallery suite exercises.
//!
//! The gallery's own golden test never steps the schedule (a generic scene render ticks the
//! built-in registry only), so it proves nothing about the solver — it shows the lattice exactly as
//! authored. This file is what actually simulates the scene: it loads the committed YAML, whose
//! `physics: space: free` block is the only opt-in there is, under a **plain, bare engine** — the
//! proof that a bare host steps a physics scene — steps it for real, and traces the per-tick
//! `WorldHash` against a committed reference (`examples/gallery/physics-pile.trace.txt`) so a second
//! pinned machine has something to reproduce bit-exactly.

use std::path::PathBuf;

use ludion_engine::Engine;
use ludion_protocol::{Command, ResponseBody};
use serde_json::Value;

/// Ticks the trace covers.
const TICKS: usize = 600;
/// How many distinct hash lines the trace must hold before it counts as chaotic rather than a pile
/// that quickly sleeps and repeats one line.
const MIN_DISTINCT: usize = 300;
/// The tick the snapshot-format smoke captures at: well after the pile has started landing and
/// interlocking, so the `physics` field carries a live, non-trivial solver.
const SNAPSHOT_SMOKE_TICK: usize = 100;

fn scene_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery/physics-pile.yaml")
}

fn reference_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery/physics-pile.trace.txt")
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

#[test]
fn two_in_process_runs_of_the_pile_are_identical() {
    assert_eq!(
        run(TICKS),
        run(TICKS),
        "the same committed scene, stepped twice, must trace identically"
    );
}

#[test]
fn the_pile_matches_the_committed_cross_arch_trace() {
    let trace = run(TICKS);
    let path = reference_path();

    if ludion_core::bless::bless_requested() {
        let header = "\
# Per-tick WorldHash of examples/gallery/physics-pile.yaml, stepped under a bare engine (its own\n\
# physics: space: free block is the only opt-in) (crates/ludion-engine/tests/physics_gallery.rs).\n\
# Recorded with: LUDION_BLESS=1 cargo nextest run -p ludion-engine --test it -E 'test(/^physics_gallery::/)'\n\
# Free space: free ignores the engine's default Plane, so the pile falls on y rather than hanging\n\
# frozen under the plane's locks. The M4 must reproduce this file byte-identically\n\
# (Plan/physics.md gate 1, cross-arch) - never fabricated by copying the Windows trace.\n";
        std::fs::write(&path, format!("{header}{}\n", trace.join("\n")))
            .expect("the gallery directory is committed");
        return;
    }

    let committed: Vec<String> = std::fs::read_to_string(&path)
        .expect("the committed reference trace")
        .lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .map(str::to_owned)
        .collect();
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
/// authored inline: a session with rigid bodies carries the solver state as the `physics` field.
#[test]
fn the_gallery_pile_snapshot_carries_the_physics_field() {
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
        "a pile with 64 rigid bodies must carry solver state in the `physics` field at tick {SNAPSHOT_SMOKE_TICK}"
    );
}

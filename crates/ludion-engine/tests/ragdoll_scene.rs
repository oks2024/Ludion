//! `Plan/physics.md` W3, plan 21 — the gallery `ragdoll` scene: six posed `ragdoll.assembly.yaml`
//! figures dropped onto a static staircase.
//!
//! **The committed scene is generated, never hand-edited.** [`SOURCE`] holds the *pre-switch*
//! stage — the six figures as whole, posed `kind: assembly` entities plus the static obstacles —
//! under a bare engine rooted at the gallery. [`generate_scene_text`] loads it, calls
//! [`switch_to_ragdoll`] on each figure (document positions [`FIRST_RAGDOLL_INDEX`]..+6, ascending
//! id — a freshly loaded scene assigns entity ids by document order at generation 0, the
//! `physics_joints.rs`/`ragdoll.rs` convention), despawns the now-empty root shells, and saves the
//! resulting world through the engine's own canonical writer: the committed scene **is** what the
//! switch produces. It carries `generated_from: ../../crates/ludion-engine/tests/ragdoll_scene.rs`
//! (the `generate-map` precedent, `ludion_scene::Scene::generated_from`) and is skipped by
//! `scripts/map-drift.mjs` (a `.rs` source has no `generate-map` to regenerate from — this test's own
//! byte compare is the drift gate).
//!
//! The trace/resume/thread-identity/CLI gates mirror `physics_stress.rs` (`Plan/physics.md` W2's
//! stress scene) exactly, one gallery scene over.

use std::path::PathBuf;

use ludion_core::ids::EntityId;
use ludion_core::time::{SimTime, TickRate};
use ludion_engine::{Engine, load_assembly, switch_to_ragdoll};
use ludion_protocol::command::{EngineRestoreParams, SimStepParams};
use ludion_protocol::{Command, ResponseBody};
use serde_json::Value;

/// The assembly every figure's `Mesh.shape.assembly` names, relative to the gallery.
const ASSEMBLY_REL: &str = "voxels/ragdoll.assembly.yaml";
/// [`SOURCE`]'s six figures are document positions 6..12 — the ids a fresh load assigns them at
/// generation 0.
const FIRST_RAGDOLL_INDEX: u32 = 6;
const RAGDOLL_COUNT: u32 = 6;
/// [`ludion_scene::Scene::generated_from`], resolved relative to `examples/gallery/ragdoll.yaml`.
const GENERATED_FROM: &str = "../../crates/ludion-engine/tests/ragdoll_scene.rs";

/// The pre-switch source: a static staircase (floor + three ascending steps) plus six posed
/// `ragdoll.assembly.yaml` figures, staggered in drop height, rotation and angular velocity so their
/// landings spread across the trace instead of clustering (`Plan/physics.md` W3's "staggering the
/// landings is the main lever" note) — every float dyadic.
const SOURCE: &str = "\
format: 1
name: \"ragdoll - Mesh.shape.assembly + switch_to_ragdoll: 6 posed figures over a staircase\"
physics:
  space: free
entities:
  - name: camera
    components:
      Transform:
        translation:
          - 0.0
          - 4.5
          - 13.5
        rotation:
          - -10.0
          - 0.0
          - 0.0
      Camera:
        projection:
          type: perspective
          fov_y_deg: 50.0
  - name: key-light
    components:
      Transform:
        translation:
          - 2.0
          - 14.0
          - 10.0
      PointLight:
        color:
          - 1.0
          - 0.96
          - 0.9
        intensity: 5.0
        range: 48.0
  - name: floor
    components:
      Transform:
        translation:
          - 0.0
          - -0.5
          - 0.0
        scale:
          - 24.0
          - 1.0
          - 24.0
      Mesh:
        shape: cube
        color:
          - 0.55
          - 0.57
          - 0.6
          - 1.0
      Collider:
        shape:
          box:
            half_extents:
              - 12.0
              - 0.5
              - 12.0
        body: static
  - name: step-low
    components:
      Transform:
        translation:
          - -4.5
          - 0.375
          - 0.0
        scale:
          - 2.5
          - 0.75
          - 2.5
      Mesh:
        shape: cube
        color:
          - 0.5
          - 0.5
          - 0.55
          - 1.0
      Collider:
        shape:
          box:
            half_extents:
              - 1.25
              - 0.375
              - 1.25
        body: static
  - name: step-mid
    components:
      Transform:
        translation:
          - 1.5
          - 0.75
          - 0.0
        scale:
          - 2.5
          - 1.5
          - 2.5
      Mesh:
        shape: cube
        color:
          - 0.5
          - 0.5
          - 0.55
          - 1.0
      Collider:
        shape:
          box:
            half_extents:
              - 1.25
              - 0.75
              - 1.25
        body: static
  - name: step-high
    components:
      Transform:
        translation:
          - 7.5
          - 1.125
          - 0.0
        scale:
          - 2.5
          - 2.25
          - 2.5
      Mesh:
        shape: cube
        color:
          - 0.5
          - 0.5
          - 0.55
          - 1.0
      Collider:
        shape:
          box:
            half_extents:
              - 1.25
              - 1.125
              - 1.25
        body: static
  - name: ragdoll-0
    components:
      Transform:
        translation:
          - -7.5
          - 3.0
          - 0.0
        rotation:
          - 0.0
          - 0.0
          - 0.0
        scale:
          - 0.125
          - 0.125
          - 0.125
      Mesh:
        shape:
          assembly: voxels/ragdoll.assembly.yaml
      Velocity:
        angular:
          - 1.25
          - -1.5
          - 0.75
  - name: ragdoll-1
    components:
      Transform:
        translation:
          - -4.5
          - 3.5
          - 0.0
        rotation:
          - 15.0
          - 0.0
          - 0.0
        scale:
          - 0.125
          - 0.125
          - 0.125
      Mesh:
        shape:
          assembly: voxels/ragdoll.assembly.yaml
      Velocity:
        angular:
          - -1.5
          - 1.25
          - -0.75
  - name: ragdoll-2
    components:
      Transform:
        translation:
          - -1.5
          - 4.0
          - 0.0
        rotation:
          - 0.0
          - 20.0
          - 0.0
        scale:
          - 0.125
          - 0.125
          - 0.125
      Mesh:
        shape:
          assembly: voxels/ragdoll.assembly.yaml
      Velocity:
        angular:
          - 0.75
          - 0.75
          - -1.5
  - name: ragdoll-3
    components:
      Transform:
        translation:
          - 1.5
          - 4.5
          - 0.0
        rotation:
          - 10.0
          - 0.0
          - 15.0
        scale:
          - 0.125
          - 0.125
          - 0.125
      Mesh:
        shape:
          assembly: voxels/ragdoll.assembly.yaml
      Velocity:
        angular:
          - -0.75
          - -1.25
          - 1.5
  - name: ragdoll-4
    components:
      Transform:
        translation:
          - 4.5
          - 5.0
          - 0.0
        rotation:
          - 0.0
          - 0.0
          - 25.0
        scale:
          - 0.125
          - 0.125
          - 0.125
      Mesh:
        shape:
          assembly: voxels/ragdoll.assembly.yaml
      Velocity:
        angular:
          - 1.5
          - 0.0
          - -0.75
  - name: ragdoll-5
    components:
      Transform:
        translation:
          - 7.5
          - 5.5
          - 0.0
        rotation:
          - 20.0
          - 10.0
          - 0.0
        scale:
          - 0.125
          - 0.125
          - 0.125
      Mesh:
        shape:
          assembly: voxels/ragdoll.assembly.yaml
      Velocity:
        angular:
          - -1.25
          - 0.75
          - 0.0
";

/// Ticks the trace covers.
const TICKS: usize = 600;
/// How many distinct hash lines the trace must hold before it counts as chaotic rather than settling
/// early and repeating one line.
const MIN_DISTINCT: usize = 540;
/// The tick the snapshot-format smoke captures at.
const SNAPSHOT_SMOKE_TICK: usize = 100;
/// The tick the resume test's snapshot is taken at; the resumed run covers `SNAPSHOT_AT..TICKS`.
const SNAPSHOT_AT: usize = 300;
/// The window around `SNAPSHOT_AT` (0-based tick indices) that the resume test also checks is still
/// live.
const LIVE_WINDOW: std::ops::Range<usize> = 280..320;
/// How many distinct hashes `LIVE_WINDOW` must hold for the capture tick to count as live.
const LIVE_WINDOW_MIN_DISTINCT: usize = 35;

fn gallery_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery")
}

fn scene_path() -> PathBuf {
    gallery_root().join("ragdoll.yaml")
}

fn trace_path() -> PathBuf {
    gallery_root().join("ragdoll.trace.txt")
}

fn replay_path() -> PathBuf {
    gallery_root().join("ragdoll.lrp")
}

/// Generates the canonical `ragdoll.yaml` text: loads [`SOURCE`] under a bare engine rooted at the
/// gallery, switches each of the six posed figures into a ragdoll, despawns the emptied root shells,
/// and saves the resulting world through the engine's own canonical writer, then stamps
/// `generated_from`. The scene **is** what the switch produces.
fn generate_scene_text() -> String {
    let gallery = gallery_root();
    let data = load_assembly(ludion_assets::AssetRoot::content(&gallery), ASSEMBLY_REL)
        .expect("the committed assembly loads");

    let mut engine = Engine::with_project_root(&gallery);
    engine
        .load_inline_scene(SOURCE)
        .expect("the source scene loads");

    let roots: Vec<EntityId> = (FIRST_RAGDOLL_INDEX..FIRST_RAGDOLL_INDEX + RAGDOLL_COUNT)
        .map(|index| EntityId::new(index, 0))
        .collect();
    for &root in &roots {
        switch_to_ragdoll(
            engine.world_mut(),
            root,
            &data,
            SimTime::from_ticks(0, TickRate::DEFAULT),
        )
        .expect("a valid switch");
    }
    for &root in &roots {
        engine
            .world_mut()
            .despawn(root)
            .expect("the emptied root shell despawns");
    }

    let saved = engine.scene_text().expect("the switched world saves");

    let mut scene = ludion_scene::check(&saved, engine.registry()).unwrap_or_else(|diagnostics| {
        panic!("the switched world is not a valid scene: {diagnostics:?}")
    });
    scene.generated_from = Some(GENERATED_FROM.to_string());
    ludion_scene::emit(&scene, engine.registry())
}

#[test]
fn the_committed_ragdoll_scene_matches_a_fresh_generation() {
    let generated = generate_scene_text();
    let path = scene_path();

    if ludion_core::bless::bless_requested() {
        std::fs::write(&path, &generated).expect("the gallery directory is committed");
        return;
    }

    let committed = std::fs::read_to_string(&path).expect("the committed ragdoll scene");
    assert_eq!(
        committed, generated,
        "examples/gallery/ragdoll.yaml has drifted from a fresh switch_to_ragdoll generation; \
         re-bless with `LUDION_BLESS=1 cargo nextest run -p ludion-engine --test it \
         -E 'test(=ragdoll_scene::the_committed_ragdoll_scene_matches_a_fresh_generation)'`"
    );
}

/// A fresh, bare engine with the committed `ragdoll.yaml` loaded. Its own `physics: space: free`
/// block is the opt-in.
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
fn two_in_process_runs_of_the_ragdoll_scene_are_identical() {
    assert_eq!(
        run(TICKS),
        run(TICKS),
        "the same committed scene, stepped twice, must trace identically"
    );
}

#[test]
fn the_ragdoll_scene_matches_the_committed_cross_arch_trace() {
    let trace = run(TICKS);
    let path = trace_path();

    if ludion_core::bless::bless_requested() {
        let header = "\
# Per-tick WorldHash of examples/gallery/ragdoll.yaml, stepped under a bare engine (its own\n\
# physics: space: free block is the only opt-in) (crates/ludion-engine/tests/ragdoll_scene.rs).\n\
# Recorded with: LUDION_BLESS=1 cargo nextest run -p ludion-engine --test it -E 'test(/^ragdoll_scene::/)'\n\
# The M4 must reproduce this file byte-identically (Plan/physics.md gate 1, cross-arch) - never\n\
# fabricated by copying the Windows trace.\n";
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
        "only {} distinct lines of {TICKS}; the ragdolls settled too early to be chaotic",
        distinct.len()
    );
}

/// A session with six jointed ragdolls over a staircase must carry solver state in the `physics`
/// field.
#[test]
fn the_ragdoll_scene_snapshot_carries_the_physics_field() {
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
        "a scene with jointed ragdolls must carry solver state in the `physics` field at tick \
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

/// Gate 2 at this scene's scale, tied to the committed trace: a fresh engine run to `SNAPSHOT_AT`
/// matches the committed trace's first `SNAPSHOT_AT` lines; its snapshot, restored into a *third*,
/// freshly built engine, reproduces the committed trace's hash at the restore boundary and every
/// line after it.
#[test]
fn a_restored_ragdoll_scene_reproduces_the_committed_trace_from_tick_300() {
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
        "a scene with jointed ragdolls must carry solver state in the `physics` field at tick \
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

/// The committed `ragdoll.lrp` replay: a fresh in-process recording of the committed scene's
/// 600-tick session reproduces the committed trace, and both the fresh bytes and the committed
/// `.lrp` bytes replay to it — the `recorded_stress_session_replays_to_identical_worldhash_trace`
/// shape (`games/vigil/tests/replay.rs:246`), one gallery scene over. The 600 frames cross the
/// keyframe at tick 300 (`KEYFRAME_INTERVAL`), which carries the physics blob, so this also
/// exercises blob restore through a replay. Recorded at the default stream extent.
#[test]
fn the_committed_ragdoll_replay_reproduces_the_committed_trace() {
    let bless = ludion_core::bless::bless_requested();
    let gallery = gallery_root();
    let committed_scene_text =
        std::fs::read_to_string(scene_path()).expect("the committed ragdoll scene");
    let committed = committed_trace();
    let committed_hashes = hashes_of(&committed);

    let mut engine = Engine::builder()
        .project_root(&gallery)
        .build()
        .expect("the bare engine builds");
    engine
        .load_inline_scene(&committed_scene_text)
        .expect("the committed scene loads");

    let fresh_path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("ludion-engine-ragdoll.lrp");
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

    let committed_bytes = std::fs::read(replay_path()).expect("the committed ragdoll.lrp");
    let committed_replayed =
        ludion_engine::replay_hashes_with(replay_builder(), &committed_bytes, &gallery)
            .expect("the committed replay runs");
    let committed_replayed_hashes: Vec<String> = committed_replayed[1..]
        .iter()
        .map(|(_, hash)| format!("{:016x}", hash.as_u64()))
        .collect();
    assert_eq!(
        committed_replayed_hashes, committed_hashes,
        "the committed ragdoll.lrp no longer replays to the committed trace — after an intentional \
         change, re-record it with `LUDION_BLESS=1 cargo nextest run -p ludion-engine --test it \
         -E 'test(=ragdoll_scene::the_committed_ragdoll_replay_reproduces_the_committed_trace)'` and commit it"
    );
}

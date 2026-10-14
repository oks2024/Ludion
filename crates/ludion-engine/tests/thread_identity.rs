//! `Plan/physics.md` W1 gate 3: the gallery pile's, the 512-body stress scene's, and the ragdoll
//! scene's 600-tick `WorldHash` traces are thread-count invariant at 1, 2 and 8 threads.
//! `ludion-physics`'s solver is single-threaded regardless (the `enhanced-determinism` feature
//! excludes `parallel`), so this is trivially true for the solver itself; what it actually guards is
//! that the rest of the tick — the write/step/mirror systems' place in the schedule, the ECS join
//! each one runs — stays thread-count invariant with physics in the frame, the same property
//! `games/breakout/tests/thread_identity.rs` guards for a game with no physics at all.
//!
//! `#[ignore]`d like breakout's: driving each pile three times is cheap, but this lives in the same
//! `--full`/CI lane as the rest of the thread-identity corpus rather than the default lane. Run via
//! `cargo nextest run -p ludion-engine -E 'binary(thread_identity)' --run-ignored ignored-only`.

use std::path::{Path, PathBuf};

use ludion_core::par;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{
    Engine, Name, PointLight, Transform, revert_prefab, spawn_prefab, unpack_prefab,
};

/// Thread counts the gate requires bit-identical `WorldHash` traces at.
const THREAD_COUNTS: [usize; 3] = [1, 2, 8];
/// Ticks traced — the same extent each scene's committed reference is recorded over.
const TICKS: usize = 600;

fn scene_path(file_name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/gallery")
        .join(file_name)
}

fn build(scene: &Path) -> Engine {
    Engine::builder()
        .load_scene_file(scene)
        .expect("the committed gallery scene loads")
}

/// The per-tick `WorldHash` trace of a fresh, freshly-built engine.
fn trace(scene: &Path) -> Vec<u64> {
    let mut engine = build(scene);
    (0..TICKS)
        .map(|_| {
            engine.advance_live();
            engine.world_hash().unwrap().as_u64()
        })
        .collect()
}

/// Runs `trace` at each thread count and panics with `what` on divergence.
fn assert_traces_invariant<T: PartialEq + std::fmt::Debug + Send>(
    what: &str,
    trace: impl Fn() -> T + Sync,
) {
    // Each `par::with_threads` override is installed **inside its own spawned thread's closure** —
    // the override is thread-local, so setting it on the calling thread before spawning would be a
    // no-op for the thread that actually runs the tick loop (`ludion_core::par`'s own doc).
    #[allow(clippy::disallowed_methods)]
    let [one, two, eight] = std::thread::scope(|scope| {
        THREAD_COUNTS
            .map(|n| {
                let trace = &trace;
                scope.spawn(move || par::with_threads(n, trace))
            })
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|payload| std::panic::resume_unwind(payload))
            })
    });

    assert_eq!(
        one, two,
        "the 1-thread and 2-thread traces of {what} diverge"
    );
    assert_eq!(
        one, eight,
        "the 1-thread and 8-thread traces of {what} diverge"
    );
}

/// Runs the three-thread-count comparison for one committed gallery scene, panicking with the
/// scene's file name on divergence.
fn assert_thread_count_invariant(file_name: &str) {
    let scene = scene_path(file_name);
    assert_traces_invariant(&format!("examples/gallery/{file_name}"), || trace(&scene));
}

#[test]
#[ignore = "thread-identity gate; run via check.mjs --full or CI"]
fn physics_pile_hashes_are_thread_count_invariant() {
    assert_thread_count_invariant("physics-pile.yaml");
}

#[test]
#[ignore = "thread-identity gate; run via check.mjs --full or CI"]
fn physics_stress_hashes_are_thread_count_invariant() {
    assert_thread_count_invariant("physics-stress.yaml");
}

#[test]
#[ignore = "thread-identity gate; run via check.mjs --full or CI"]
fn ragdoll_hashes_are_thread_count_invariant() {
    assert_thread_count_invariant("ragdoll.yaml");
}

#[test]
#[ignore = "thread-identity gate; run via check.mjs --full or CI"]
fn ragdoll_skinned_hashes_are_thread_count_invariant() {
    assert_thread_count_invariant("ragdoll-skinned.yaml");
}

const PREFAB_PATH: &str = "prefabs/chain.prefab.yaml";

/// Three jointed rigid parts, so every spawn exercises the remap and the joint solver.
const CHAIN_PREFAB: &str = "format: 1
kind: prefab
name: chain
entities:
  - name: top
    components:
      Transform: {}
      Collider:
        shape:
          sphere:
            radius: 0.2
        body:
          rigid:
            mass: 1.0
      Velocity: {}
  - name: mid
    components:
      Transform:
        translation:
          - 0.0
          - -0.6
          - 0.0
      Collider:
        shape:
          box:
            half_extents:
              - 0.1
              - 0.25
              - 0.1
        body:
          rigid:
            mass: 1.0
      Velocity: {}
      Joint:
        a:
          index: 0
          generation: 0
        b:
          index: 1
          generation: 0
        kind:
          ball:
            limits:
              - - -45.0
                - 45.0
              - - -45.0
                - 45.0
              - - -45.0
                - 45.0
  - name: tip
    components:
      Transform:
        translation:
          - 0.0
          - -1.2
          - 0.0
      Collider:
        shape:
          sphere:
            radius: 0.15
        body:
          rigid:
            mass: 0.5
      Velocity: {}
      Joint:
        a:
          index: 1
          generation: 0
        b:
          index: 2
          generation: 0
        kind:
          hinge:
            axis:
              - 0.0
              - 0.0
              - 1.0
            limits:
              - -60.0
              - 60.0
        break_impulse: 5.0
";

const PREFAB_SCENE: &str = "format: 1
seed: 7
physics:
  space: free
entities:
  - name: floor
    components:
      Transform:
        translation:
          - 0.0
          - -0.5
          - 0.0
      Collider:
        shape:
          box:
            half_extents:
              - 12.0
              - 0.5
              - 12.0
        body: static
";

/// The live instances' ids, oldest first.
struct Instances(Vec<ludion_core::ids::EntityId>);

/// Every 40 ticks spawns a chain at a varying placement; from the fourth on, despawns the oldest
/// instance first, so freed slots are reused by the spawn that follows.
fn churn(world: &mut World, ctx: &SystemCtx) {
    if !ctx.frame.0.is_multiple_of(40) {
        return;
    }
    if world.resource::<Instances>().is_none() {
        world.insert_resource(Instances(Vec::new()));
    }
    let live = world.resource::<Instances>().map_or(0, |i| i.0.len());
    if live >= 3 {
        let oldest = world.resource_mut::<Instances>().unwrap().0.remove(0);
        // The chain's parts are not parented, so retire every part carrying the instance's root.
        let parts: Vec<_> = world
            .entities()
            .filter(|&id| {
                matches!(world.get::<ludion_engine::PrefabInstance>(id), Ok(Some(p)) if p.root == oldest)
            })
            .collect();
        assert!(
            world.despawn_many(&parts).is_empty(),
            "the instance is alive"
        );
    }
    let step = (ctx.frame.0 / 40) as f32;
    let placement = Transform {
        translation: [
            step * 0.7 - 4.0,
            2.0 + step * 0.1,
            ludion_core::math::sin(step * 1.3),
        ],
        rotation: [0.0, step * 20.0, 0.0],
        ..Transform::default()
    };
    let ids = spawn_prefab(world, PREFAB_PATH, Some(placement), Some("chain"))
        .expect("the chain prefab spawns");
    world.resource_mut::<Instances>().unwrap().0.push(ids[0]);
}

/// The per-tick `WorldHash` trace of a fresh engine whose `churn` system spawns and despawns prefab
/// instances over the scratch content root.
fn prefab_trace(root: &Path) -> Vec<u64> {
    let mut engine = Engine::builder()
        .system("churn", churn)
        .project_root(root)
        .build()
        .expect("the engine builds");
    engine
        .load_inline_scene(PREFAB_SCENE)
        .expect("the scene loads");
    (0..TICKS)
        .map(|_| {
            engine.advance_live();
            engine.world_hash().unwrap().as_u64()
        })
        .collect()
}

#[test]
#[ignore = "thread-identity gate; run via check.mjs --full or CI"]
fn prefab_spawn_hashes_are_thread_count_invariant() {
    // Written once, before any thread spawns: the three runs only read it.
    let root = std::env::temp_dir().join(format!(
        "ludion-thread-identity-prefab-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("prefabs")).unwrap();
    std::fs::write(root.join(PREFAB_PATH), CHAIN_PREFAB).unwrap();
    assert_traces_invariant("the prefab spawn/despawn churn", || prefab_trace(&root));
    let _ = std::fs::remove_dir_all(&root);
}

/// Two `chain` instances, one shifted: the nested case.
const OUTER_PREFAB: &str = "format: 1
kind: prefab
name: outer
entities:
  - name: left
    prefab: prefabs/chain.prefab.yaml
  - name: right
    prefab: prefabs/chain.prefab.yaml
    transform:
      translation:
        - 2.0
        - 0.0
        - 0.0
";

const OUTER_PATH: &str = "prefabs/outer.prefab.yaml";

/// The floor of `PREFAB_SCENE` and two nested instances, `a` carrying an override on an inner part.
const INSTANCE_SCENE: &str = "format: 1
seed: 7
physics:
  space: free
entities:
  - name: floor
    components:
      Transform:
        translation:
          - 0.0
          - -0.5
          - 0.0
      Collider:
        shape:
          box:
            half_extents:
              - 12.0
              - 0.5
              - 12.0
        body: static
  - name: a
    prefab: prefabs/outer.prefab.yaml
    transform:
      translation:
        - -4.0
        - 3.0
        - 0.0
    overrides:
      left/top:
        PointLight:
          intensity: 3.0
  - name: b
    prefab: prefabs/outer.prefab.yaml
    transform:
      translation:
        - 4.0
        - 3.0
        - 0.0
";

fn part(world: &mut World, name: &str) -> ludion_core::ids::EntityId {
    world
        .query_ids::<(&Name,)>()
        .find(|(_, (n,))| n.0 == name)
        .map(|(id, _)| id)
        .unwrap_or_else(|| panic!("a part named {name}"))
}

/// Edits instance `a` (a gained component, then a revert) and unpacks `b`, at fixed frames.
fn edit_instances(world: &mut World, ctx: &SystemCtx) {
    match ctx.frame.0 {
        100 => {
            let id = part(world, "a/left/top");
            world.insert(id, PointLight::default()).unwrap();
        }
        200 => {
            let id = part(world, "a/left/top");
            revert_prefab(world, id, None).expect("the instance reverts");
        }
        300 => {
            let id = part(world, "b/left/top");
            unpack_prefab(world, id).expect("the instance unpacks");
        }
        _ => {}
    }
}

/// The per-tick `WorldHash` trace and the final saved scene of a fresh engine running
/// `edit_instances` over scene-loaded nested instances.
fn instance_trace(root: &Path) -> (Vec<u64>, String) {
    let mut engine = Engine::builder()
        .system("edit_instances", edit_instances)
        .project_root(root)
        .build()
        .expect("the engine builds");
    engine
        .load_inline_scene(INSTANCE_SCENE)
        .expect("the scene loads");
    let hashes = (0..TICKS)
        .map(|_| {
            engine.advance_live();
            engine.world_hash().unwrap().as_u64()
        })
        .collect();
    let saved = engine.scene_text().expect("the world saves");
    (hashes, saved)
}

#[test]
#[ignore = "thread-identity gate; run via check.mjs --full or CI"]
fn prefab_scene_hashes_are_thread_count_invariant() {
    // Written once, before any thread spawns: the three runs only read it.
    let root = std::env::temp_dir().join(format!(
        "ludion-thread-identity-prefab-scene-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("prefabs")).unwrap();
    std::fs::write(root.join(PREFAB_PATH), CHAIN_PREFAB).unwrap();
    std::fs::write(root.join(OUTER_PATH), OUTER_PREFAB).unwrap();
    assert_traces_invariant(
        "the nested prefab instances, edited, reverted and unpacked",
        || instance_trace(&root),
    );
    let _ = std::fs::remove_dir_all(&root);
}

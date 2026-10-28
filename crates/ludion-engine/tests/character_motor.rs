//! `move_character` on the engine's `Transform`: a scene-authored motor walks a floor under a game
//! system, and the `WorldHash` trace survives an `engine.snapshot` / `engine.restore` mid-run and
//! is the same at one and at eight threads.

use ludion_core::par;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{Engine, EngineBuilder, Transform, move_character};
use ludion_physics::{CharacterMotor, CharacterMotorState};
use ludion_protocol::command::EngineRestoreParams;
use ludion_protocol::{Command, ResponseBody};
use serde_json::Value;
use std::path::PathBuf;

const TICKS: usize = 120;
const SNAPSHOT_AT: usize = 50;

const SCENE: &str = "format: 1
seed: 5
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
              - 20.0
              - 0.5
              - 20.0
        body: static
  - name: wall
    components:
      Transform:
        translation:
          - 3.5
          - 1.0
          - 0.0
      Collider:
        shape:
          box:
            half_extents:
              - 0.5
              - 2.0
              - 5.0
        body: static
  - name: hero
    components:
      Transform:
        translation:
          - 0.0
          - 0.6
          - 0.0
      CharacterMotor:
        radius: 0.28
        height: 0.9
";

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-character-motor-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Walks the scene's one motor toward the wall; the first tick has no physics world yet and the
/// call simply reports it.
fn walk(world: &mut World, _ctx: &SystemCtx) {
    let hero = world
        .query_ids::<(&CharacterMotor,)>()
        .next()
        .map(|(entity, _)| entity)
        .expect("the scene has a motor");
    let _ = move_character(world, hero, [0.05, -0.1, 0.02]);
}

fn game(name: &str) -> EngineBuilder {
    Engine::builder()
        .project_root(fresh_dir(name))
        .system("walk", walk)
}

fn advance_trace(engine: &mut Engine, n: usize) -> Vec<u64> {
    (0..n)
        .map(|_| {
            engine.advance_live();
            engine.world_hash().unwrap().as_u64()
        })
        .collect()
}

fn run(name: &str) -> Vec<u64> {
    let mut engine = game(name).build().expect("boots");
    engine.load_inline_scene(SCENE).unwrap();
    advance_trace(&mut engine, TICKS)
}

#[test]
fn the_motor_walks_and_lands_under_a_game_system() {
    let mut engine = game("walks").build().expect("boots");
    engine.load_inline_scene(SCENE).unwrap();
    advance_trace(&mut engine, TICKS);
    let (grounded, ground) = engine
        .world_mut()
        .query_ids::<(&CharacterMotorState,)>()
        .next()
        .map(|(_, (state,))| (state.grounded, state.ground))
        .expect("the motor wrote its state");
    assert!(grounded && ground.is_some());
}

#[test]
fn the_trace_survives_a_snapshot_restore_mid_run() {
    let reference = run("reference");
    let distinct: std::collections::BTreeSet<u64> = reference.iter().copied().collect();
    assert!(distinct.len() > 30, "a flat trace proves nothing");

    let mut first = game("first").build().expect("boots");
    first.load_inline_scene(SCENE).unwrap();
    assert_eq!(
        advance_trace(&mut first, SNAPSHOT_AT),
        reference[..SNAPSHOT_AT]
    );
    let document = match first.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    };
    assert!(document.get("physics").and_then(Value::as_str).is_some());

    let mut resumed = game("resumed").build().expect("boots");
    resumed.load_inline_scene(SCENE).unwrap();
    match resumed
        .execute(Command::EngineRestore(EngineRestoreParams {
            snapshot: document,
        }))
        .unwrap()
        .body
    {
        ResponseBody::Restored { .. } => {}
        other => panic!("expected a restore, got {other:?}"),
    }
    assert_eq!(
        resumed.world_hash().unwrap().as_u64(),
        reference[SNAPSHOT_AT - 1]
    );
    assert_eq!(
        advance_trace(&mut resumed, TICKS - SNAPSHOT_AT),
        reference[SNAPSHOT_AT..]
    );
}

#[test]
fn the_trace_is_the_same_at_one_and_eight_threads() {
    #[allow(clippy::disallowed_methods)]
    let [one, eight] = std::thread::scope(|scope| {
        [(1, "t1"), (8, "t8")]
            .map(|(n, name)| scope.spawn(move || par::with_threads(n, || run(name))))
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|payload| std::panic::resume_unwind(payload))
            })
    });
    assert_eq!(one, eight);
}

/// The bare physics function: the same walk, nothing drawn.
fn walk_bare(world: &mut World, _ctx: &SystemCtx) {
    let hero = world
        .query_ids::<(&CharacterMotor,)>()
        .next()
        .map(|(entity, _)| entity)
        .expect("the scene has a motor");
    let _ = ludion_physics::move_character::<Transform>(world, hero, [0.05, -0.1, 0.02]);
}

#[test]
fn the_motor_debug_draw_is_hash_neutral() {
    let mut drawn = game("drawn").build().expect("boots");
    drawn.load_inline_scene(SCENE).unwrap();
    let mut bare = Engine::builder()
        .project_root(fresh_dir("bare"))
        .system("walk", walk_bare)
        .build()
        .expect("boots");
    bare.load_inline_scene(SCENE).unwrap();

    assert_eq!(
        advance_trace(&mut drawn, TICKS),
        advance_trace(&mut bare, TICKS)
    );
    assert!(drawn.debug_draw_drawn().contains(&"motor"));
    assert!(!bare.debug_draw_drawn().contains(&"motor"));
}

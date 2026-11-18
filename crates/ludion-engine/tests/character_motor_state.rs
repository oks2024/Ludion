//! `CharacterMotorState` is an engine-written component like any other: it rides the world hash and
//! the snapshot, and its `ground` reference is remapped to the live floor entity on restore.

use ludion_core::ids::EntityId;
use ludion_engine::components::Name;
use ludion_engine::{Engine, EngineBuilder};
use ludion_physics::{CharacterMotor, CharacterMotorState};
use ludion_protocol::command::EngineRestoreParams;
use ludion_protocol::{Command, ResponseBody};
use serde_json::Value;
use std::path::PathBuf;

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
              - 10.0
              - 0.5
              - 10.0
        body: static
  - name: hero
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 0.0
      CharacterMotor: {}
";

fn game(name: &str) -> EngineBuilder {
    let dir: PathBuf =
        std::env::temp_dir().join(format!("ludion-motor-state-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    Engine::builder().project_root(dir)
}

fn booted(name: &str) -> Engine {
    let mut engine = game(name).build().expect("boots");
    engine.load_inline_scene(SCENE).unwrap();
    engine
}

fn named(engine: &mut Engine, wanted: &str) -> EntityId {
    engine
        .world_mut()
        .query_ids::<(&Name,)>()
        .find(|(_, (name,))| name.0 == wanted)
        .map(|(id, _)| id)
        .unwrap_or_else(|| panic!("the scene has an entity named {wanted}"))
}

fn state_of(engine: &mut Engine, entity: EntityId) -> Option<CharacterMotorState> {
    engine
        .world_mut()
        .get::<CharacterMotorState>(entity)
        .unwrap()
        .cloned()
}

fn standing(engine: &mut Engine) {
    let (floor, hero) = (named(engine, "floor"), named(engine, "hero"));
    engine
        .world_mut()
        .insert(
            hero,
            CharacterMotorState {
                grounded: true,
                ground: Some(floor),
                ..Default::default()
            },
        )
        .unwrap();
}

fn trace(engine: &mut Engine, ticks: usize) -> Vec<u64> {
    (0..ticks)
        .map(|_| {
            engine.advance_live();
            engine.world_hash().unwrap().as_u64()
        })
        .collect()
}

#[test]
fn the_motor_state_moves_the_world_hash() {
    let mut plain = booted("plain");
    let mut grounded = booted("grounded");
    let hero = named(&mut grounded, "hero");
    assert!(
        grounded
            .world_mut()
            .get::<CharacterMotor>(hero)
            .unwrap()
            .is_some()
    );
    assert_eq!(
        plain.world_hash().unwrap(),
        grounded.world_hash().unwrap(),
        "the scene alone hashes identically"
    );
    standing(&mut grounded);
    assert_ne!(plain.world_hash().unwrap(), grounded.world_hash().unwrap());
}

#[test]
fn a_snapshot_round_trips_the_state_with_its_ground_remapped() {
    let mut original = booted("original");
    standing(&mut original);
    let reference = trace(&mut original, 20);
    let floor = named(&mut original, "floor");
    let hero = named(&mut original, "hero");
    assert_eq!(
        state_of(&mut original, hero).and_then(|state| state.ground),
        Some(floor)
    );

    let mut snapshotted = booted("snapshotted");
    standing(&mut snapshotted);
    let first = trace(&mut snapshotted, 10);
    assert_eq!(first, reference[..10]);
    let document = match snapshotted.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    };

    let mut resumed = booted("resumed");
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
    let (floor, hero) = (named(&mut resumed, "floor"), named(&mut resumed, "hero"));
    let state = state_of(&mut resumed, hero).expect("the state was restored");
    assert!(state.grounded);
    assert_eq!(state.ground, Some(floor), "ground names the live floor");
    assert_eq!(resumed.world_hash().unwrap().as_u64(), reference[9]);
    assert_eq!(trace(&mut resumed, 10), reference[10..]);
}

#[test]
fn the_state_serializes_without_its_empty_fields() {
    let value = serde_json::to_value(CharacterMotorState::default()).unwrap();
    assert_eq!(value, Value::Object(Default::default()));
}

fn bodies(engine: &Engine) -> usize {
    engine
        .world()
        .resource::<ludion_physics::PhysicsWorld>()
        .expect("physics is installed")
        .body_count()
}

#[test]
fn a_snapshot_taken_while_suspended_restores_the_parking() {
    let mut original = booted("suspended");
    original.advance_live();
    let hero = named(&mut original, "hero");
    ludion_physics::suspend_character(original.world_mut(), hero).unwrap();
    original.advance_live();
    original.advance_live();
    assert_eq!(bodies(&original), 1, "only the floor");
    let document = match original.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    };

    let mut resumed = booted("suspended-restored");
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
    let hero = named(&mut resumed, "hero");
    assert!(state_of(&mut resumed, hero).is_some_and(|state| state.suspended));
    resumed.advance_live();
    resumed.advance_live();
    assert_eq!(bodies(&resumed), 1, "still no capsule after the restore");

    let report = ludion_physics::resume_character::<ludion_engine::components::Transform>(
        resumed.world_mut(),
        hero,
        [2.0, 0.0, 0.0],
        0.0,
        None,
    )
    .unwrap();
    assert!(report.resumed);
    resumed.advance_live();
    resumed.advance_live();
    assert_eq!(bodies(&resumed), 2, "the capsule is back");
}

#[test]
fn an_undeclared_posture_falls_back_to_the_base_shape_and_reports_once() {
    let mut engine = booted("posture-fallback");
    let hero = named(&mut engine, "hero");
    engine
        .world_mut()
        .insert(
            hero,
            CharacterMotorState {
                posture: Some("ghost".into()),
                ..Default::default()
            },
        )
        .unwrap();
    for _ in 0..6 {
        engine.advance_live();
    }
    let reported: Vec<String> = engine
        .world()
        .resource::<ludion_engine::diagnostics::Diagnostics>()
        .map(|d| {
            d.reported(ludion_core::codes::code::CHARACTER_MOTOR_POSTURE_FALLBACK)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(reported.len(), 1, "{reported:?}");
}

const CRAWL_SCENE: &str = "format: 1
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
  - name: hero
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 5.0
      CharacterMotor:
        radius: 0.3
        height: 1.75
        postures:
          prone:
            shape:
              lying:
                length: 1.7
                radius: 0.17
                offset: 0.35
            step_height: 0.06
";

/// Goes prone on tick 3 and crawls from tick 4, turning toward -X halfway: every tick is a pure
/// function of the world, so a restored engine must continue it bit for bit.
fn crawl(world: &mut ludion_ecs::World, ctx: &ludion_ecs::SystemCtx) {
    let hero = world
        .query_ids::<(&CharacterMotor,)>()
        .next()
        .map(|(entity, _)| entity)
        .expect("a motor");
    let frame = ctx.frame.as_u64();
    if frame == 3 {
        let report = ludion_engine::set_character_posture(world, hero, Some("prone")).unwrap();
        assert_eq!(report.posture.as_deref(), Some("prone"));
    }
    if frame == 15 {
        let _ = ludion_engine::turn_character(world, hero, 0.6);
    }
    let step = if frame >= 4 {
        [0.0, -0.1, -0.04]
    } else {
        [0.0, -0.1, 0.0]
    };
    let _ = ludion_engine::move_character(world, hero, step);
}

fn crawler(name: &str) -> Engine {
    let mut engine = game(name).system("crawl", crawl).build().expect("boots");
    engine.load_inline_scene(CRAWL_SCENE).unwrap();
    engine
}

#[test]
fn a_snapshot_mid_crawl_continues_identically() {
    let mut original = crawler("crawl-original");
    let reference = trace(&mut original, 40);
    let hero = named(&mut original, "hero");
    assert_eq!(
        state_of(&mut original, hero).and_then(|state| state.posture),
        Some("prone".to_string())
    );

    let mut snapshotted = crawler("crawl-snapshotted");
    assert_eq!(trace(&mut snapshotted, 20), reference[..20]);
    let document = match snapshotted.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    };

    let mut resumed = crawler("crawl-resumed");
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
    let hero = named(&mut resumed, "hero");
    assert_eq!(
        state_of(&mut resumed, hero).and_then(|state| state.posture),
        Some("prone".to_string())
    );
    assert_eq!(resumed.world_hash().unwrap().as_u64(), reference[19]);
    assert_eq!(trace(&mut resumed, 20), reference[20..]);
}

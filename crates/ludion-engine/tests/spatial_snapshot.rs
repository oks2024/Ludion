//! A snapshot restore continues the spatial index exactly.
//!
//! The index a system scheduled *before* `spatial_rebuild` reads on tick N was filed during tick
//! N-1, from the poses the world held when the rebuild ran; a system after the rebuild then moved
//! things, so the snapshot's poses are not the poses the index was built from. Here the mover steps
//! into the probe on odd frames and out on even ones, so on every tick the index the reader sees
//! disagrees with an index rebuilt from the poses at the end of the previous tick — a restore that
//! rebuilt on arrival would diverge at once.

use ludion_core::ids::EntityId;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{Engine, Transform, TransformHierarchy};
use ludion_protocol::command::EngineRestoreParams;
use ludion_protocol::{Command, ResponseBody};
use ludion_spatial::{Shape, spatial};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What the reader has seen so far; a component so it reaches `WorldHash`.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
struct Seen {
    sum: u64,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
struct Mover;

const SCENE: &str = "\
format: 1
seed: 7
entities:
  - name: reader
    components:
      Seen:
        sum: 0
  - name: mover
    components:
      Mover:
      Transform:
        translation:
          - 10.0
          - 0.0
          - 0.0
      Collider:
        shape:
          circle:
            radius: 1.0
        body: kinematic
";

const PROBE: Shape = Shape::Circle { radius: 1.0 };

fn reader(world: &mut World, ctx: &SystemCtx) {
    let hits = spatial::overlaps(world, PROBE, [0.0, 0.0], None).len() as u64;
    let ids: Vec<EntityId> = world.entities().collect();
    for id in ids {
        if let Ok(Some(seen)) = world.get_mut::<Seen>(id) {
            seen.sum += hits * (ctx.frame.as_u64() + 1);
        }
    }
}

/// Steps into the probe on odd frames, out on even ones — after the rebuild.
fn mover(world: &mut World, ctx: &SystemCtx) {
    let x = if ctx.frame.as_u64() % 2 == 1 {
        0.0
    } else {
        10.0
    };
    let ids: Vec<EntityId> = world.entities().collect();
    for id in ids {
        if matches!(world.get::<Mover>(id), Ok(Some(_))) {
            world.get_mut::<Transform>(id).unwrap().unwrap().translation[0] = x;
        }
    }
}

fn engine() -> Engine {
    let mut engine = Engine::builder()
        .components(|reg| {
            reg.register::<Seen>("Seen")?;
            reg.register::<Mover>("Mover")?;
            Ok(())
        })
        .system("reader", reader)
        .system(
            "spatial_rebuild",
            spatial::rebuild::<Transform, TransformHierarchy>,
        )
        .system("mover", mover)
        .build()
        .expect("boots");
    engine.load_inline_scene(SCENE).unwrap();
    engine
}

fn hash(engine: &mut Engine) -> u64 {
    engine.world_hash().unwrap().as_u64()
}

#[test]
fn a_restore_continues_the_index_a_pre_rebuild_reader_sees() {
    let mut uninterrupted = engine();
    for _ in 0..10 {
        uninterrupted.advance_live();
    }
    let expected = hash(&mut uninterrupted);

    let mut first = engine();
    for _ in 0..5 {
        first.advance_live();
    }
    let snapshot = match first.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    };

    let mut resumed = engine();
    resumed
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap();
    for _ in 0..5 {
        resumed.advance_live();
    }
    assert_eq!(hash(&mut resumed), expected);
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
struct Poisoned;

const POISON_SCENE: &str = "\
format: 1
seed: 7
entities:
  - name: fine
    components:
      Transform:
        translation:
          - 2.0
          - 0.0
          - 0.0
      Collider:
        shape:
          circle:
            radius: 1.0
        body: static
  - name: nan
    components:
      Poisoned:
      Transform:
        translation:
          - 3.0
          - 0.0
          - 0.0
      Collider:
        shape:
          circle:
            radius: 1.0
        body: static
";

/// A game writing a NaN pose onto a collider before the rebuild.
fn poison(world: &mut World, _ctx: &SystemCtx) {
    let ids: Vec<EntityId> = world.entities().collect();
    for id in ids {
        if matches!(world.get::<Poisoned>(id), Ok(Some(_))) {
            world.get_mut::<Transform>(id).unwrap().unwrap().translation[0] = f32::NAN;
        }
    }
}

fn poisoned_engine() -> Engine {
    let mut engine = Engine::builder()
        .components(|reg| {
            reg.register::<Poisoned>("Poisoned")?;
            Ok(())
        })
        .system("poison", poison)
        .system(
            "spatial_rebuild",
            spatial::rebuild::<Transform, TransformHierarchy>,
        )
        .build()
        .expect("boots");
    engine.load_inline_scene(POISON_SCENE).unwrap();
    engine
}

#[test]
fn a_non_finite_collider_pose_refuses_the_snapshot() {
    let mut first = poisoned_engine();
    first.advance_live();
    let refusal = first
        .execute(Command::EngineSnapshot)
        .expect_err("a NaN pose cannot be stored");
    assert_eq!(refusal.code.as_str(), "L0007");
    assert!(
        refusal.message.contains("`Transform`") && refusal.message.contains("non-finite"),
        "names the component: {}",
        refusal.message
    );
}

/// A game writing the pose finite again after the rebuild filed the NaN one.
fn heal(world: &mut World, _ctx: &SystemCtx) {
    let ids: Vec<EntityId> = world.entities().collect();
    for id in ids {
        if matches!(world.get::<Poisoned>(id), Ok(Some(_))) {
            world.get_mut::<Transform>(id).unwrap().unwrap().translation[0] = 3.0;
        }
    }
}

#[test]
fn an_index_entry_filed_from_a_pose_since_healed_is_left_out_of_the_snapshot() {
    let build = || {
        let mut engine = Engine::builder()
            .components(|reg| {
                reg.register::<Poisoned>("Poisoned")?;
                Ok(())
            })
            .system("poison", poison)
            .system(
                "spatial_rebuild",
                spatial::rebuild::<Transform, TransformHierarchy>,
            )
            .system("heal", heal)
            .build()
            .expect("boots");
        engine.load_inline_scene(POISON_SCENE).unwrap();
        engine
    };
    let mut first = build();
    first.advance_live();
    let snapshot = match first
        .execute(Command::EngineSnapshot)
        .expect("the world holds only finite poses")
        .body
    {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    };
    let filed = snapshot["spatial_index"]
        .as_array()
        .expect("the snapshot stores the index as a list of entries");
    assert_eq!(filed.len(), 1, "only the finite entry is stored: {filed:?}");

    build()
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .expect("the stored index restores");
}

#[test]
fn a_world_without_a_rebuild_snapshots_no_index() {
    let mut bare = Engine::builder()
        .components(|reg| {
            reg.register::<Seen>("Seen")?;
            Ok(())
        })
        .build()
        .expect("boots");
    bare.load_inline_scene("format: 1\nseed: 7\nentities: []\n")
        .unwrap();
    let snapshot = match bare.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    };
    assert!(snapshot.get("spatial_index").is_none());
}

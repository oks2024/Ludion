//! Behavioral proof for runtime prefab spawning (`crates/ludion/examples/prefab_spawn.rs`): a system
//! spawns a jointed prefab twice and a one-part prefab bare, the ids and `Joint` references are each
//! copy's own, and the spawning system's declared queries match what it actually ran.
//!
//! The game here mirrors the example's shape with a three-part prefab.

use ludion::prelude::*;
use std::path::PathBuf;

const ARM_PATH: &str = "prefabs/arm.prefab.yaml";
const STICK_PATH: &str = "prefabs/stick.prefab.yaml";

const ARM: &str = "\
format: 1
kind: prefab
name: arm
entities:
  - name: shoulder
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
  - name: upper
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
          hinge:
            axis:
              - 0.0
              - 0.0
              - 1.0
            limits:
              - -60.0
              - 60.0
      JointMotor:
        target:
          - 10.0
        stiffness: 20.0
        damping: 4.0
  - name: hand
    components:
      Transform:
        translation:
          - 0.0
          - -1.0
          - 0.0
      Collider:
        shape:
          sphere:
            radius: 0.1
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
          fixed: {}
        break_impulse: 30.0
";

const STICK: &str = "\
format: 1
kind: prefab
name: stick
entities:
  - name: stick
    components:
      Transform:
        translation:
          - 0.0
          - 4.0
          - 0.0
      Collider:
        shape:
          box:
            half_extents:
              - 0.05
              - 0.2
              - 0.05
        body:
          rigid:
            mass: 0.4
      Velocity: {}
";

const SCENE: &str = "\
format: 1
seed: 1
physics:
  space: free
entities:
  - name: camera
    components:
      Transform:
        translation:
          - 0.0
          - 4.0
          - 12.0
      Camera: {}
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

/// The ids each spawn returned, in spawn order.
#[derive(Default)]
struct Spawned(Vec<Vec<EntityId>>);

fn launch(world: &mut World, ctx: &SystemCtx) {
    let placed = |x: f32| {
        Some(Transform {
            translation: [x, 3.0, 0.0],
            ..Transform::default()
        })
    };
    let ids = match ctx.frame.0 {
        0 => spawn_prefab(world, ARM_PATH, placed(-2.0), Some("arm-1"), false),
        10 => spawn_prefab(world, ARM_PATH, placed(2.0), Some("arm-2"), false),
        20 => spawn_prefab(world, STICK_PATH, None, None, false),
        _ => return,
    }
    .expect("the prefab spawns");
    if world.resource::<Spawned>().is_none() {
        world.insert_resource(Spawned::default());
    }
    if let Some(spawned) = world.resource_mut::<Spawned>() {
        spawned.0.push(ids);
    }
}

fn engine(tag: &str) -> (Engine, PathBuf) {
    let root =
        std::env::temp_dir().join(format!("ludion-prefab-test-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("prefabs")).unwrap();
    std::fs::write(root.join(ARM_PATH), ARM).unwrap();
    std::fs::write(root.join(STICK_PATH), STICK).unwrap();
    let mut engine = Engine::builder()
        .system("launch", launch)
        .query::<(&mut Collider,)>()
        .query::<(&mut Joint,)>()
        .query::<(&mut JointMotor,)>()
        .query::<(&mut Name,)>()
        .query::<(&mut PrefabInstance,)>()
        .query::<(&Transform,)>()
        .query::<(&mut Transform,)>()
        .query::<(&mut Velocity,)>()
        .project_root(&root)
        .build()
        .expect("the game builds");
    engine.load_inline_scene(SCENE).expect("the scene loads");
    (engine, root)
}

fn step(engine: &mut Engine) -> Result<(), ludion_core::diagnostics::Diagnostic> {
    engine
        .execute(Command::SimStep(command::SimStepParams {
            ticks: 60,
            hash: false,
        }))
        .map(|_| ())
}

#[test]
fn copies_are_remapped_named_and_rooted() {
    let (mut engine, root) = engine("remap");
    step(&mut engine).unwrap();
    let world = engine.world();
    let [first, second, stick] = &world.resource::<Spawned>().unwrap().0[..] else {
        panic!("three spawns");
    };
    assert_eq!((first.len(), second.len(), stick.len()), (3, 3, 1));
    assert!(first.iter().all(|id| !second.contains(id)), "disjoint ids");
    for (copy, prefix) in [(first, "arm-1"), (second, "arm-2")] {
        for (part, &id) in ["shoulder", "upper", "hand"].iter().zip(copy) {
            let name = world.get::<Name>(id).unwrap().unwrap();
            assert_eq!(name.0, format!("{prefix}/{part}"));
            let instance = world.get::<PrefabInstance>(id).unwrap().unwrap();
            assert_eq!(instance.root, copy[0]);
            assert_eq!(instance.part, *part);
        }
        for &id in &copy[1..] {
            let joint = world.get::<Joint>(id).unwrap().unwrap();
            assert!(copy.contains(&joint.a), "a is this copy's part");
            assert_eq!(joint.b, id);
        }
    }
    assert_eq!(world.get::<Name>(stick[0]).unwrap().unwrap().0, "stick");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn the_spawning_system_passes_the_query_drift_gate() {
    let (mut engine, root) = engine("drift");
    let report = ludion_engine::check_query_drift_with(&mut engine, step).unwrap();
    let _ = std::fs::remove_dir_all(root);
    assert!(report.is_clean(), "{report}");
    assert!(report.warnings().is_empty(), "{report}");
}

//! **A goblin launched twice, and a dynamite stick** — runtime prefab instantiation end to end: a
//! `kind: prefab` file authored once, spawned from a system at a placement, its `Joint` references
//! remapped to each copy's own parts.
//!
//! The `GOBLIN` prefab is six rigid parts joined by limited hinge and ball joints (one driven by a
//! `JointMotor`), its helmet on a breakable `fixed` joint. `launch` spawns it at frame 0 and again at
//! frame 30 as `goblin-1` and `goblin-2`, and the one-part `DYNAMITE` at frame 40 under its bare name.
//! `spawn_prefab` returns the new ids in part order; each part carries a `PrefabInstance` naming the
//! copy it belongs to.
//!
//! **A spawning system writes every component its prefab carries.** `launch` therefore declares those
//! writes with `.query` (`ludion docs prefabs` lists the set) or the query-drift gate reports
//! `L1126`.
//!
//! Everything above the `main` divider is copyable into a real game. The prefabs themselves are
//! content — `kind: prefab` files under the game's content root.
//!
//! Run it headless (prints each spawn and the final part count, self-checks the remap):
//!
//! ```text
//! cargo run -p ludion --example prefab_spawn
//! ```

use ludion::prelude::*;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

// --- The game (copyable) -------------------------------------------------------------------------

/// The prefabs, by content-root-relative path.
const GOBLIN_PATH: &str = "prefabs/goblin.prefab.yaml";
const DYNAMITE_PATH: &str = "prefabs/dynamite.prefab.yaml";

/// One spawn, as `launch` saw it.
struct Spawn {
    name: Option<&'static str>,
    ids: Vec<EntityId>,
    placement: Option<Transform>,
    /// The first part's world translation right after the spawn.
    first_at: [f32; 3],
}

/// Every spawn, in order.
#[derive(Default)]
struct Spawns(Vec<Spawn>);

/// Spawns `path` and records what came back. A refused prefab is a content bug: report its
/// diagnostics and spawn nothing.
fn spawn(world: &mut World, path: &str, placement: Option<Transform>, name: Option<&'static str>) {
    let ids = match spawn_prefab(world, path, placement, name) {
        Ok(ids) => ids,
        Err(diagnostics) => {
            for diagnostic in diagnostics {
                eprintln!("{diagnostic}");
            }
            return;
        }
    };
    let first_at = world_transform(world, ids[0])
        .expect("the part is alive")
        .trs
        .translation;
    if world.resource::<Spawns>().is_none() {
        world.insert_resource(Spawns::default());
    }
    if let Some(spawns) = world.resource_mut::<Spawns>() {
        spawns.0.push(Spawn {
            name,
            ids,
            placement,
            first_at,
        });
    }
}

/// Launches a goblin at frame 0, a second one at frame 30 and a dynamite stick at frame 40.
fn launch(world: &mut World, ctx: &SystemCtx) {
    let at = |translation: [f32; 3], yaw: f32| Transform {
        translation,
        rotation: [0.0, yaw, 0.0],
        ..Transform::default()
    };
    match ctx.frame.0 {
        0 => spawn(
            world,
            GOBLIN_PATH,
            Some(at([-3.0, 1.0, 0.0], 30.0)),
            Some("goblin-1"),
        ),
        30 => spawn(
            world,
            GOBLIN_PATH,
            Some(at([3.0, 1.0, 0.0], -30.0)),
            Some("goblin-2"),
        ),
        40 => spawn(world, DYNAMITE_PATH, None, None),
        _ => {}
    }
}

/// The game: one system. A real game's `main` is `ludion::run(game())`.
pub fn game() -> EngineBuilder {
    Engine::builder()
        .system("launch", launch)
        .query::<(&mut Collider,)>()
        .query::<(&mut Joint,)>()
        .query::<(&mut JointMotor,)>()
        .query::<(&mut Name,)>()
        .query::<(&mut PrefabInstance,)>()
        .query::<(&Transform,)>()
        .query::<(&mut Transform,)>()
        .query::<(&mut Velocity,)>()
}

/// Six rigid parts. `Joint.a`/`b` are positions in this list (0 pelvis, 1 torso, 2 head, 3 arm-l,
/// 4 arm-r, 5 helmet); `spawn_prefab` rewrites them to the copy's own ids.
const GOBLIN: &str = "\
format: 1
kind: prefab
name: goblin
entities:
  - name: pelvis
    components:
      Transform: {}
      Collider:
        shape:
          box:
            half_extents:
              - 0.2
              - 0.15
              - 0.15
        body:
          rigid:
            mass: 2.0
      Velocity: {}
  - name: torso
    components:
      Transform:
        translation:
          - 0.0
          - 0.45
          - 0.0
      Collider:
        shape:
          box:
            half_extents:
              - 0.25
              - 0.3
              - 0.15
        body:
          rigid:
            mass: 3.0
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
              - 1.0
              - 0.0
              - 0.0
            limits:
              - -20.0
              - 45.0
        anchor:
          - 0.0
          - 0.15
          - 0.0
      JointMotor:
        target:
          - 10.0
        stiffness: 40.0
        damping: 6.0
        max_force: 20.0
  - name: head
    components:
      Transform:
        translation:
          - 0.0
          - 0.95
          - 0.0
      Collider:
        shape:
          sphere:
            radius: 0.18
        body:
          rigid:
            mass: 1.0
      Velocity: {}
      Joint:
        a:
          index: 1
          generation: 0
        b:
          index: 2
          generation: 0
        kind:
          ball:
            limits:
              - - -30.0
                - 30.0
              - - -45.0
                - 45.0
              - - -30.0
                - 30.0
        anchor:
          - 0.0
          - 0.3
          - 0.0
  - name: arm-l
    components:
      Transform:
        translation:
          - -0.45
          - 0.55
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
            mass: 0.8
      Velocity: {}
      Joint:
        a:
          index: 1
          generation: 0
        b:
          index: 3
          generation: 0
        kind:
          ball:
            limits:
              - - -90.0
                - 90.0
              - - -45.0
                - 45.0
              - - -90.0
                - 90.0
        anchor:
          - -0.25
          - 0.1
          - 0.0
  - name: arm-r
    components:
      Transform:
        translation:
          - 0.45
          - 0.55
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
            mass: 0.8
      Velocity: {}
      Joint:
        a:
          index: 1
          generation: 0
        b:
          index: 4
          generation: 0
        kind:
          ball:
            limits:
              - - -90.0
                - 90.0
              - - -45.0
                - 45.0
              - - -90.0
                - 90.0
        anchor:
          - 0.25
          - 0.1
          - 0.0
  - name: helmet
    components:
      Transform:
        translation:
          - 0.0
          - 1.1
          - 0.0
      Collider:
        shape:
          sphere:
            radius: 0.22
        body:
          rigid:
            mass: 0.5
      Velocity: {}
      Joint:
        a:
          index: 2
          generation: 0
        b:
          index: 5
          generation: 0
        kind:
          fixed: {}
        anchor:
          - 0.0
          - 0.15
          - 0.0
        break_impulse: 40.0
";

/// One named part and one rigid body, authored where it falls from.
const DYNAMITE: &str = "\
format: 1
kind: prefab
name: dynamite
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

/// A camera, a static floor and free-space physics with the default gravity.
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

// --- Headless demo `main` ------------------------------------------------------------------------

/// Runs 60 ticks, prints each spawn, and checks the copies are independent and correctly named.
/// Deterministic, no window.
fn main() -> ExitCode {
    let root = demo_content_root();
    if let Err(error) = write_prefabs(&root) {
        eprintln!("could not write the demo prefabs: {error}");
        return ExitCode::FAILURE;
    }
    let mut engine = match game().project_root(&root).build() {
        Ok(engine) => engine,
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(diagnostics) = engine.load_inline_scene(SCENE) {
        for diagnostic in diagnostics {
            eprintln!("{diagnostic}");
        }
        return ExitCode::FAILURE;
    }
    engine
        .execute(Command::SimStep(command::SimStepParams {
            ticks: 60,
            hash: false,
        }))
        .expect("the sim steps");

    let world = engine.world();
    let spawns = world.resource::<Spawns>().expect("launch ran");
    for spawn in &spawns.0 {
        println!(
            "spawned {:<9} {} part(s), first at ({:.2}, {:.2}, {:.2})",
            spawn.name.unwrap_or("(bare)"),
            spawn.ids.len(),
            spawn.first_at[0],
            spawn.first_at[1],
            spawn.first_at[2]
        );
    }
    let ok = check(world, spawns);
    let _ = std::fs::remove_dir_all(&root);
    if !ok {
        eprintln!("the prefab copies were not independent, named and placed as authored");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// Whether every invariant of the three spawns holds in `world`.
fn check(world: &World, spawns: &Spawns) -> bool {
    let [first, second, stick] = &spawns.0[..] else {
        return false;
    };
    let named = |spawn: &Spawn, prefix: &str| {
        ["pelvis", "torso", "head", "arm-l", "arm-r", "helmet"]
            .iter()
            .zip(&spawn.ids)
            .all(|(part, &id)| {
                matches!(world.get::<Name>(id), Ok(Some(n)) if n.0 == format!("{prefix}/{part}"))
            })
    };
    let own_joints = |spawn: &Spawn| {
        spawn.ids[1..].iter().all(|&id| {
            matches!(world.get::<Joint>(id), Ok(Some(j))
                if spawn.ids.contains(&j.a) && j.b == id)
        })
    };
    let rooted = |spawn: &Spawn| {
        spawn.ids.iter().all(
            |&id| matches!(world.get::<PrefabInstance>(id), Ok(Some(p)) if p.root == spawn.ids[0]),
        )
    };
    let at_placement = |spawn: &Spawn| {
        spawn.placement.as_ref().is_some_and(|p| {
            spawn
                .first_at
                .iter()
                .zip(p.translation)
                .all(|(a, b)| (a - b).abs() < 1e-4)
        })
    };
    let disjoint = first.ids.iter().all(|id| !second.ids.contains(id));
    first.ids.len() == 6
        && second.ids.len() == 6
        && disjoint
        && named(first, "goblin-1")
        && named(second, "goblin-2")
        && own_joints(first)
        && own_joints(second)
        && rooted(first)
        && rooted(second)
        && at_placement(first)
        && at_placement(second)
        && stick.ids.len() == 1
        && matches!(world.get::<Name>(stick.ids[0]), Ok(Some(n)) if n.0 == "stick")
        && matches!(world.get::<PrefabInstance>(stick.ids[0]), Ok(Some(p))
            if p.root == stick.ids[0] && p.part == "stick" && p.source == DYNAMITE_PATH)
}

/// A fresh, process-unique temp directory for the demo's prefabs (removed first, so a re-run starts
/// clean).
fn demo_content_root() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-prefab-example-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Writes the two prefabs under `root/prefabs/`.
fn write_prefabs(root: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(root.join("prefabs"))?;
    std::fs::write(root.join(GOBLIN_PATH), GOBLIN)?;
    std::fs::write(root.join(DYNAMITE_PATH), DYNAMITE)
}

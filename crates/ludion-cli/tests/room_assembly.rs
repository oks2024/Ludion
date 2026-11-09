//! A generated room prefab, spawned and torn down from a sim system under nav: the same `WorldHash`
//! trace at any thread count, and a recording that replays after the prefab, map and kit are deleted.
//! The kit's `box` colliders are 3D statics, so nav picks the room up the tick after the spawn.

use std::path::{Path, PathBuf};
use std::process::Command as ProcCommand;
use std::sync::{Arc, Mutex};

use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_core::par;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::diagnostics::Diagnostics;
use ludion_engine::{
    Engine, EngineBuilder, Nav, NavAgent, NavConfig, NavSteering, PrefabInstance, Transform,
    TransformHierarchy, despawn_instance, nav_steer, nav_update, replay_hashes_with, spawn_prefab,
};
use ludion_protocol::Command;
use ludion_protocol::command::SimStepParams;
use ludion_protocol::replay::{ReplayReader, ReplayRecord};
use ludion_spatial::spatial3d;

pub(crate) const KIT: &str = "format: 1
kind: map-kit
name: k
tiles:
  \".\":
    - name: floor
      shape: cube
      scale:
        - 2.0
        - 2.0
        - 2.0
      y: -1.0
      collider:
        box:
          - 1.0
          - 1.0
          - 1.0
  \"#\":
    - name: floor
      shape: cube
      scale:
        - 2.0
        - 2.0
        - 2.0
      y: -1.0
      collider:
        box:
          - 1.0
          - 1.0
          - 1.0
    - name: wall
      shape: cube
      scale:
        - 2.0
        - 2.0
        - 2.0
      y: 1.0
      collider:
        box:
          - 1.0
          - 1.0
          - 1.0
";

/// A 6 x 6 room whose wall at column 2 spans rows 0-3, leaving a door gap in rows 4-5.
const MAP: &str = "format: 1
kind: map
name: room
kit: kit.kit.yaml
cell_size: 2.0
origin:
  - 0.0
  - 0.0
rows:
  - \"..#...\"
  - \"..#...\"
  - \"..#...\"
  - \"..#...\"
  - \"......\"
  - \"......\"
";

pub(crate) const PREFAB: &str = "rooms/room.prefab.yaml";
const SPAWN: u64 = 10;
const TEARDOWN: u64 = 300;
const RESPAWN: u64 = 320;
const PLACEMENT: [f32; 3] = [12.0, 0.0, 0.0];
/// Room cell (row 1, col 4) at the placement.
const T: [f32; 3] = [21.0, 0.0, 3.0];
const WALKERS: usize = 20;

pub(crate) fn nav_config() -> NavConfig {
    NavConfig {
        nx: 24,
        nz: 12,
        y_min: -2.0,
        y_max: 8.0,
        ..NavConfig::default()
    }
}

fn seq(v: [f32; 3], indent: usize) -> String {
    v.iter()
        .map(|c| format!("{:indent$}- {c:?}\n", ""))
        .collect()
}

fn scene() -> String {
    let mut out = String::from(
        "format: 1\nseed: 1\nphysics:\n  space: free\nentities:\n  - name: ground\n    components:\n      \
         Transform:\n        translation:\n",
    );
    out += &seq([6.0, -0.5, 6.0], 10);
    out += "      Collider:\n        shape:\n          box:\n            half_extents:\n";
    out += &seq([6.0, 0.5, 6.0], 14);
    out += "        body: static\n";
    for n in 0..WALKERS {
        let at = [2.0 + 1.2 * (n % 5) as f32, 0.0, 1.0 + 1.0 * (n / 5) as f32];
        out +=
            &format!("  - name: a{n}\n    components:\n      Transform:\n        translation:\n");
        out += &seq(at, 10);
        out += "      SpatialVolume:\n        layers: 2\n      NavAgent:\n";
        out += "        speed: 6.0\n        neighbor_layers: 2\n        target:\n";
        out += &seq(T, 10);
    }
    out
}

/// A scratch project holding the kit, the map and the generated room prefab.
pub(crate) fn room_root(tag: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../../target/tmp/{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("rooms")).unwrap();
    std::fs::write(dir.join("kit.kit.yaml"), KIT).unwrap();
    std::fs::write(dir.join("room.map.yaml"), MAP).unwrap();
    generate(&dir, "room.map.yaml", PREFAB);
    dir
}

pub(crate) fn generate(root: &Path, map: &str, out: &str) {
    let output = ProcCommand::new(env!("CARGO_BIN_EXE_ludion"))
        .args([
            "generate-map",
            root.join(map).to_str().unwrap(),
            "--out",
            root.join(out).to_str().unwrap(),
            "--prefab",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "generate-map: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The ids each spawn returned, in spawn order.
#[derive(Default)]
pub(crate) struct Spawned(pub(crate) Vec<Vec<EntityId>>);

fn spawn(world: &mut World) {
    let placement = Transform {
        translation: PLACEMENT,
        ..Transform::default()
    };
    // A refused spawn (a replay whose record is missing) is a no-op, not a panic.
    if let Ok(ids) = spawn_prefab(world, PREFAB, Some(placement), Some("r"), false) {
        let mut spawned = world.remove_resource::<Spawned>().unwrap_or_default();
        spawned.0.push(ids);
        world.insert_resource(spawned);
    }
}

fn spawner(world: &mut World, ctx: &SystemCtx) {
    match ctx.frame.as_u64() {
        SPAWN | RESPAWN => spawn(world),
        TEARDOWN => {
            if let Some(root) = world.resource::<Spawned>().map(|s| s.0[0][0]) {
                despawn_instance(world, root).expect("the room's root is alive");
            }
        }
        _ => {}
    }
}

/// Applies each agent's steering, `y` from the grid node under it.
fn mover(world: &mut World, ctx: &SystemCtx) {
    let moves: Vec<_> = world
        .query_ids::<(&Transform, &NavSteering)>()
        .map(|(id, (t, s))| (id, t.translation, s.velocity))
        .collect();
    for (id, at, v) in moves {
        let mut next = [at[0] + v[0] * ctx.dt, at[1], at[2] + v[2] * ctx.dt];
        if let Some(grid) = world.resource::<Nav>().and_then(Nav::grid)
            && let Some(node) = grid.node_at([next[0], at[1] + 0.6, next[2]])
        {
            next[1] = grid.node_pos(node)[1];
        }
        world.get_mut::<Transform>(id).unwrap().unwrap().translation = next;
    }
}

/// The spawner runs before `nav_update`, so nav sees a spawned room the same tick.
pub(crate) fn game(root: &Path) -> EngineBuilder {
    Engine::builder()
        .project_root(root)
        .nav(nav_config())
        .system("spawner", spawner)
        .system(
            "spatial3d_rebuild",
            spatial3d::rebuild::<Transform, TransformHierarchy>,
        )
        .system("nav_update", nav_update)
        .system("nav_steer", nav_steer)
        .system("mover", mover)
        .query::<(&PrefabInstance,)>()
}

fn booted(root: &Path) -> Engine {
    let mut engine = game(root).build().expect("boots");
    engine.load_inline_scene(&scene()).unwrap();
    engine
}

pub(crate) fn hash_trace(root: &Path, ticks: usize) -> Vec<u64> {
    let mut engine = booted(root);
    (0..ticks)
        .map(|_| {
            engine.advance_live();
            engine.world_hash().unwrap().as_u64()
        })
        .collect()
}

fn spawned(engine: &Engine) -> Vec<Vec<EntityId>> {
    engine
        .world()
        .resource::<Spawned>()
        .map(|s| s.0.clone())
        .unwrap_or_default()
}

fn assert_no_query_drift(engine: &Engine) {
    let diagnostics = engine.world().resource::<Diagnostics>().unwrap();
    assert_eq!(
        diagnostics.reported(code::QUERY_DRIFT_UNDECLARED).count(),
        0,
        "L1126 query drift"
    );
}

#[test]
fn a_room_spawn_teardown_respawn_hashes_the_same_at_one_and_eight_threads() {
    let root = room_root("room_assembly_threads");
    let one = par::with_threads(1, || hash_trace(&root, 340));
    let again = par::with_threads(1, || hash_trace(&root, 340));
    let eight = par::with_threads(8, || hash_trace(&root, 340));
    assert_eq!(one, again, "two 1-thread runs");
    assert_eq!(one, eight, "1 and 8 threads");

    // The tick that runs the spawner is the one whose frame is `SPAWN`: its hash differs from the last.
    let spawn_tick = SPAWN as usize;
    assert!(
        (spawn_tick - 1..=spawn_tick + 1).any(|i| one[i] != one[i - 1]),
        "the spawn moves the trace"
    );

    let mut engine = par::with_threads(1, || booted(&root));
    par::with_threads(1, || {
        for _ in 0..340 {
            engine.advance_live();
        }
    });
    let spawns = spawned(&engine);
    assert_eq!(spawns.len(), 2);
    assert_eq!(spawns[0].len(), spawns[1].len(), "same part count");
    // The free list reuses each index with a bumped generation.
    let indices = |ids: &[EntityId]| {
        let mut v: Vec<_> = ids.iter().map(|id| id.index).collect();
        v.sort_unstable();
        v
    };
    assert_eq!(
        indices(&spawns[0]),
        indices(&spawns[1]),
        "the respawn reuses the freed indices"
    );
    let live: Vec<_> = engine
        .world_mut()
        .query_ids::<(&PrefabInstance,)>()
        .filter(|(_, (i,))| i.root == spawns[1][0])
        .map(|(id, _)| id)
        .collect();
    assert_eq!(live.len(), spawns[1].len(), "exactly the respawn is live");
    assert_no_query_drift(&engine);
}

/// The floor map: the node under every nav cell centre, as its world position.
fn floor_map(engine: &Engine) -> Vec<Option<[f32; 3]>> {
    let grid = engine
        .world()
        .resource::<Nav>()
        .and_then(Nav::grid)
        .expect("the grid is built");
    let cfg = nav_config();
    let mut out = Vec::new();
    for z in 0..cfg.nz {
        for x in 0..cfg.nx {
            let p = [
                (x as f32 + 0.5) * cfg.cell_size,
                0.0,
                (z as f32 + 0.5) * cfg.cell_size,
            ];
            out.push(grid.node_at(p).map(|n| grid.node_pos(n)));
        }
    }
    out
}

fn node_at_t(engine: &Engine) -> Option<[f32; 3]> {
    let grid = engine
        .world()
        .resource::<Nav>()
        .and_then(Nav::grid)
        .expect("the grid is built");
    grid.node_at([T[0], 0.0, T[2]]).map(|n| grid.node_pos(n))
}

fn tick_to(engine: &mut Engine, frame: u64) {
    while engine.frame().as_u64() < frame {
        engine.advance_live();
    }
}

#[test]
fn walkers_path_into_a_spawned_room_and_not_into_a_torn_down_one() {
    let root = room_root("room_assembly_nav");
    let mut engine = booted(&root);

    // The spawner sees frame `SPAWN` on tick SPAWN + 1.
    tick_to(&mut engine, SPAWN + 1);
    assert_eq!(spawned(&engine).len(), 1, "the room spawned on this tick");
    let before = floor_map(&engine);
    assert!(
        node_at_t(&engine).is_none(),
        "the floor is not in the grid on the spawn tick"
    );

    tick_to(&mut engine, SPAWN + 2);
    let at = node_at_t(&engine).expect("the floor is in the grid the tick after the spawn");
    assert!(at[1].abs() < 0.1, "floor top at y = 0: {at:?}");

    tick_to(&mut engine, TEARDOWN);
    let near = engine
        .world_mut()
        .query_ids::<(&Transform, &NavAgent)>()
        .filter(|(_, (t, _))| {
            let d = [t.translation[0] - T[0], t.translation[2] - T[2]];
            (d[0] * d[0] + d[1] * d[1]).sqrt() < 1.5
        })
        .count();
    assert!(
        near >= 16,
        "{near} of {WALKERS} walkers reached the target through the door"
    );

    tick_to(&mut engine, TEARDOWN + 2);
    assert!(
        node_at_t(&engine).is_none(),
        "the torn-down floor leaves the grid"
    );
    assert_eq!(
        floor_map(&engine),
        before,
        "the grid is back to its pre-spawn floor"
    );
    assert_no_query_drift(&engine);
}

#[derive(Clone, Default)]
struct SharedSink(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for SharedSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_room_spawn_replays_after_its_prefab_map_and_kit_are_deleted() {
    let root = room_root("room_assembly_replay");
    let prefab_bytes = std::fs::read_to_string(root.join(PREFAB)).unwrap();

    let sink = SharedSink::default();
    let mut engine = booted(&root);
    let mut live = vec![(engine.frame(), engine.world_hash().unwrap())];
    engine.set_recording(sink.clone()).unwrap();
    while engine.frame().as_u64() < TEARDOWN + 30 {
        engine
            .execute(Command::SimStep(SimStepParams {
                ticks: 1,
                hash: false,
            }))
            .unwrap();
        live.push((engine.frame(), engine.world_hash().unwrap()));
    }
    engine.stop_recording().unwrap();
    let bytes = sink.0.lock().unwrap().clone();

    let texts: Vec<String> = ReplayReader::new(&bytes[..])
        .unwrap()
        .records()
        .unwrap()
        .into_iter()
        .filter_map(|record| match record {
            ReplayRecord::PrefabText { document, .. } => Some(document),
            _ => None,
        })
        .collect();
    assert!(
        texts.contains(&prefab_bytes),
        "the recording carries the prefab text inline"
    );

    for gone in [PREFAB, "room.map.yaml", "kit.kit.yaml"] {
        std::fs::remove_file(root.join(gone)).unwrap();
    }
    let replayed = replay_hashes_with(game(&root), &bytes, &root).unwrap();
    assert_eq!(replayed, live);
}

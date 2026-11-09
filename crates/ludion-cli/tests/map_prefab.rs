//! A map compiled with `--prefab` spawns, through `spawn_prefab`, the same entities the scene
//! compile holds — and a mirrored one spawns clean under a rotated placement.

use std::path::{Path, PathBuf};
use std::process::{Command as ProcCommand, Output};

use ludion_core::codes::code;
use ludion_engine::diagnostics::Diagnostics;
use ludion_engine::{Engine, PrefabInstance, Transform, spawn_prefab};

const KIT: &str = "format: 1
kind: map-kit
name: k
tiles:
  \".\":
    - name: floor
      shape: cube
      y: 0.0
      merge: true
  \"#\":
    - name: wall
      shape: cube
      y: 1.0
      collider:
        aabb:
          - 1.0
          - 1.0
    - name: gem
      voxel: voxels/gem.voxel.yaml
      y: 2.0
kinds:
  gate:
    - name: gate
      shape: cube
";

const MAP: &str = "format: 1
kind: map
kit: kit.kit.yaml
cell_size: 2.0
origin:
  - -3.0
  - 1.0
rows:
  - \"..#\"
  - \"...\"
markers:
  - name: door
    kind: gate
    x: -2.0
    z: 2.0
    rotation: 30.0
";

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../../target/tmp/{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("voxels")).unwrap();
    std::fs::create_dir_all(dir.join("rooms")).unwrap();
    std::fs::write(dir.join("kit.kit.yaml"), KIT).unwrap();
    std::fs::write(dir.join("room.map.yaml"), MAP).unwrap();
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/good_voxel.voxel.yaml"),
        dir.join("voxels/gem.voxel.yaml"),
    )
    .unwrap();
    dir
}

fn ludion(args: &[&str]) {
    let output: Output = ProcCommand::new(env!("CARGO_BIN_EXE_ludion"))
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn text(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn empty_engine(root: &Path) -> Engine {
    let mut engine = Engine::builder().project_root(root).build().unwrap();
    engine
        .load_inline_scene("format: 1\nentities: []\n")
        .unwrap();
    engine
}

#[test]
fn spawned_prefab_equals_the_generated_scene() {
    let root = scratch("map_prefab_equals_scene");
    let (map, scene, prefab) = (
        root.join("room.map.yaml"),
        root.join("room.yaml"),
        root.join("rooms/room.prefab.yaml"),
    );
    ludion(&["generate-map", text(&map), "--out", text(&scene)]);
    ludion(&[
        "generate-map",
        text(&map),
        "--out",
        text(&prefab),
        "--prefab",
    ]);

    let a = Engine::builder().load_scene_file(&scene).unwrap();
    let mut b = empty_engine(&root);
    // The empty scene may hold entities of its own: compare by offset, not raw id.
    let before = b.world().entities().count();
    let ids = spawn_prefab(b.world_mut(), "rooms/room.prefab.yaml", None, None, false).unwrap();
    assert_eq!(ids.len(), b.world().entities().count() - before);

    let a_ids: Vec<_> = a.world().entities().collect();
    assert_eq!(a_ids.len(), ids.len());
    let registry = a.registry();
    for (&x, &y) in a_ids.iter().zip(&ids) {
        for info in registry.iter().filter(|i| i.name() != "PrefabInstance") {
            let (left, right) = (
                info.serialize(a.world(), x).unwrap(),
                info.serialize(b.world(), y).unwrap(),
            );
            assert_eq!(left, right, "{} differs on {x} vs {y}", info.name());
        }
        let instance = b.world().get::<PrefabInstance>(y).unwrap().unwrap();
        assert_eq!(instance.root, ids[0]);
    }
}

#[test]
fn mirrored_prefab_spawns_without_l1290() {
    let root = scratch("map_prefab_mirrored");
    let (map, prefab) = (root.join("room.map.yaml"), root.join("rooms/m.prefab.yaml"));
    ludion(&[
        "generate-map",
        text(&map),
        "--out",
        text(&prefab),
        "--mirror",
        "x",
        "--prefab",
    ]);
    let mut engine = empty_engine(&root);
    let placement = Transform {
        translation: [5.0, 0.0, 5.0],
        rotation: [0.0, 90.0, 0.0],
        ..Transform::default()
    };
    let ids = spawn_prefab(
        engine.world_mut(),
        "rooms/m.prefab.yaml",
        Some(placement),
        None,
        false,
    )
    .unwrap();
    engine.advance_live();
    for &id in &ids {
        let transform = engine.world().get::<Transform>(id).unwrap().unwrap();
        assert!(transform.scale.iter().all(|&s| s > 0.0), "{transform:?}");
    }
    let diagnostics = engine.world().resource::<Diagnostics>().unwrap();
    assert_eq!(
        diagnostics.reported(code::COLLIDER_SCALE_IGNORED).count(),
        0
    );
}

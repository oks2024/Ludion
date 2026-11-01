//! A scene's `preload:` readies the prefabs its systems will spawn before the world is committed:
//! the first spawn reads no file (the files are deleted to prove it), a bad entry fails the load
//! with a located diagnostic, the hash is untouched, a district switch warms too, `scene.save` keeps
//! the list, and a recording replays. The ignored probe prices the first spawn of a heavy skinned
//! prefab with and without it.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ludion_core::codes::code;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{
    Engine, EngineBuilder, SceneSwitch, Transform, replay_hashes_with, spawn_prefab,
};
use ludion_protocol::command::{SceneSaveParams, SimStepParams};
use ludion_protocol::{Command, ResponseBody};
use ludion_scene::{JointRest, MeshDocument, MeshJoint, MeshSkin};

const PREFAB_PATH: &str = "prefabs/rig.prefab.yaml";
const MESH_PATH: &str = "meshes/rig.mesh.yaml";

fn prefab_text(bone: &str) -> String {
    format!(
        "format: 1
kind: prefab
name: rig
entities:
  - name: skin
    components:
      Mesh:
        shape:
          asset: \"../{MESH_PATH}\"
  - name: part
    components:
      SkinBone:
        skin:
          index: 0
          generation: 0
        bone: {bone}
"
    )
}

fn scene(preload: &[&str]) -> String {
    let mut text = String::from("format: 1\nseed: 3\n");
    if !preload.is_empty() {
        text.push_str("preload:\n");
        for path in preload {
            text.push_str(&format!("  - {path}\n"));
        }
    }
    text.push_str("entities:\n  - name: anchor\n    components:\n      Transform: {}\n");
    text
}

fn joint(name: &str, parent: Option<u32>, translation: [f64; 3]) -> MeshJoint {
    let mut inverse_bind = [0.0; 16];
    for i in 0..4 {
        inverse_bind[i * 5] = 1.0;
    }
    MeshJoint {
        name: name.to_string(),
        parent,
        rest: JointRest {
            translation,
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
        },
        inverse_bind,
    }
}

fn write_rig(dir: &Path) {
    let doc = MeshDocument {
        positions: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
        skin: Some(MeshSkin {
            joints: vec![
                joint("root", None, [0.0, 0.0, 0.0]),
                joint("child", Some(0), [0.0, 1.0, 0.0]),
            ],
            weights: vec![1.0, 0.0, 0.0, 0.0, 0.5, 0.5, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            bone_indices: vec![0, 0, 0, 0, 0, 1, 0, 0, 1, 0, 0, 0],
        }),
        indices: vec![0, 1, 2],
        ..MeshDocument::default()
    };
    std::fs::write(dir.join(MESH_PATH), ludion_scene::emit_mesh(&doc)).unwrap();
}

fn fixture_root(tag: &str, bone: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-prefab-preload-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("meshes")).unwrap();
    std::fs::create_dir_all(dir.join("prefabs")).unwrap();
    write_rig(&dir);
    std::fs::write(dir.join(PREFAB_PATH), prefab_text(bone)).unwrap();
    dir
}

fn engine(root: &Path) -> Engine {
    Engine::with_project_root(root)
}

fn bone_translation(world: &World, ids: &[ludion_core::ids::EntityId]) -> [f32; 3] {
    world
        .get::<Transform>(ids[1])
        .unwrap()
        .expect("the part carries a pose")
        .translation
}

#[test]
fn a_preloaded_first_spawn_reads_no_file() {
    let root = fixture_root("warm", "child");
    let mut warm = engine(&root);
    warm.load_inline_scene(&scene(&[PREFAB_PATH])).unwrap();
    std::fs::remove_file(root.join(PREFAB_PATH)).unwrap();
    std::fs::remove_file(root.join(MESH_PATH)).unwrap();
    let ids = spawn_prefab(warm.world_mut(), PREFAB_PATH, None, None, false)
        .expect("a preloaded prefab spawns with its files gone");
    assert_eq!(bone_translation(warm.world(), &ids), [0.0, 1.0, 0.0]);

    std::fs::write(root.join(PREFAB_PATH), prefab_text("child")).unwrap();
    write_rig(&root);
    let mut cold = engine(&root);
    cold.load_inline_scene(&scene(&[])).unwrap();
    std::fs::remove_file(root.join(PREFAB_PATH)).unwrap();
    std::fs::remove_file(root.join(MESH_PATH)).unwrap();
    let refused = spawn_prefab(cold.world_mut(), PREFAB_PATH, None, None, false).unwrap_err();
    assert!(
        refused.iter().any(|d| d.code == code::PREFAB_UNRESOLVED),
        "{refused:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_missing_preload_path_fails_the_load_and_leaves_the_world() {
    let root = fixture_root("missing", "child");
    let mut engine = engine(&root);
    engine.load_inline_scene(&scene(&[])).unwrap();
    let before = engine.world_hash().unwrap();
    let errors = engine
        .load_inline_scene(&scene(&["prefabs/nope.prefab.yaml"]))
        .unwrap_err();
    let found = errors
        .iter()
        .find(|d| d.code == code::PREFAB_UNRESOLVED)
        .unwrap_or_else(|| panic!("{errors:?}"));
    assert_eq!(found.path.as_deref(), Some("/preload/0"));
    assert!(
        found.message.contains("prefabs/nope.prefab.yaml"),
        "{}",
        found.message
    );
    assert_eq!(
        engine.world_hash().unwrap(),
        before,
        "the previous world stands"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_invalid_preloaded_prefab_fails_the_load_with_its_own_code() {
    let root = fixture_root("invalid", "no-such-bone");
    let mut engine = engine(&root);
    let errors = engine
        .load_inline_scene(&scene(&[PREFAB_PATH]))
        .unwrap_err();
    assert!(
        errors
            .iter()
            .any(|d| d.code == code::SKIN_BONE_UNKNOWN_BONE),
        "{errors:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn preload_does_not_move_the_hash() {
    let root = fixture_root("hash", "child");
    let (mut with, mut without) = (engine(&root), engine(&root));
    with.load_inline_scene(&scene(&[PREFAB_PATH])).unwrap();
    without.load_inline_scene(&scene(&[])).unwrap();
    assert_eq!(with.world_hash().unwrap(), without.world_hash().unwrap());
    let _ = std::fs::remove_dir_all(&root);
}

fn switcher(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() == 1
        && let Some(switch) = world.resource_mut::<SceneSwitch>()
    {
        switch.request(
            "district.yaml".to_string(),
            "anchor".to_string(),
            Vec::new(),
        );
    }
}

#[test]
fn a_district_switch_warms_its_preloads() {
    let root = fixture_root("switch", "child");
    std::fs::write(root.join("district.yaml"), scene(&[PREFAB_PATH])).unwrap();
    let mut engine = Engine::builder()
        .project_root(&root)
        .system("switcher", switcher)
        .build()
        .unwrap();
    engine.load_inline_scene(&scene(&[])).unwrap();
    engine
        .execute(Command::SimStep(SimStepParams {
            ticks: 3,
            hash: false,
        }))
        .unwrap();
    std::fs::remove_file(root.join(PREFAB_PATH)).unwrap();
    std::fs::remove_file(root.join(MESH_PATH)).unwrap();
    let ids = spawn_prefab(engine.world_mut(), PREFAB_PATH, None, None, false)
        .expect("the district's preload warmed the library");
    assert_eq!(bone_translation(engine.world(), &ids), [0.0, 1.0, 0.0]);

    // A bad entry refuses the switch and the session runs on.
    let bad = fixture_root("switch-bad", "child");
    std::fs::write(
        bad.join("district.yaml"),
        scene(&["prefabs/nope.prefab.yaml"]),
    )
    .unwrap();
    let mut engine = Engine::builder()
        .project_root(&bad)
        .system("switcher", switcher)
        .build()
        .unwrap();
    engine.load_inline_scene(&scene(&[])).unwrap();
    engine
        .execute(Command::SimStep(SimStepParams {
            ticks: 3,
            hash: false,
        }))
        .unwrap();
    assert_eq!(
        engine.frame().as_u64(),
        3,
        "the refused switch leaves the sim running"
    );
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&bad);
}

#[test]
fn scene_save_keeps_the_preload_list() {
    let root = fixture_root("save", "child");
    let mut engine = engine(&root);
    engine.load_inline_scene(&scene(&[PREFAB_PATH])).unwrap();
    match engine
        .execute(Command::SceneSave(SceneSaveParams {
            path: "out.yaml".into(),
        }))
        .unwrap()
        .body
    {
        ResponseBody::SceneSaved { .. } => {}
        other => panic!("unexpected save body {other:?}"),
    }
    let saved = std::fs::read_to_string(root.join("out.yaml")).unwrap();
    assert!(
        saved.contains(&format!("preload:\n  - {PREFAB_PATH}\n")),
        "{saved}"
    );
    let _ = std::fs::remove_dir_all(&root);
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

fn spawner(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() == 2 {
        let _ = spawn_prefab(world, PREFAB_PATH, None, Some("a"), false);
    }
}

fn game(root: &Path) -> EngineBuilder {
    Engine::builder()
        .system("spawner", spawner)
        .project_root(root)
}

#[test]
fn a_preloading_scene_replays_from_a_deleted_prefab_file() {
    let root = fixture_root("replay", "child");
    let sink = SharedSink::default();
    let mut engine = game(&root).build().unwrap();
    engine.load_inline_scene(&scene(&[PREFAB_PATH])).unwrap();
    let mut live = vec![(engine.frame(), engine.world_hash().unwrap())];
    engine.set_recording(sink.clone()).unwrap();
    while engine.frame().as_u64() < 5 {
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
    std::fs::remove_file(root.join(PREFAB_PATH)).unwrap();
    assert_eq!(
        replay_hashes_with(game(&root), &bytes, &root).unwrap(),
        live
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// First and second `spawn_prefab` of a heavy skinned prefab, with and without `preload:`. Run with
/// `cargo test --release -p ludion-engine --test prefab_preload -- --ignored --nocapture`.
#[test]
#[ignore = "timing probe"]
#[allow(clippy::disallowed_methods)] // wall clock: a host-side timing probe, not sim
fn first_spawn_cost_with_and_without_preload() {
    let root = fixture_root("probe", "b0");
    let vertices = 30_000usize;
    let joints = 60usize;
    let doc = MeshDocument {
        positions: (0..vertices * 3)
            .map(|i| f64::from((i % 977) as u32) * 0.001)
            .collect(),
        normals: Some(
            (0..vertices * 3)
                .map(|i| f64::from((i % 3) as u32))
                .collect(),
        ),
        uvs: Some(
            (0..vertices * 2)
                .map(|i| f64::from((i % 101) as u32) * 0.01)
                .collect(),
        ),
        tangents: Some(
            (0..vertices * 4)
                .map(|i| f64::from((i % 4 == 3) as u32))
                .collect(),
        ),
        skin: Some(MeshSkin {
            joints: (0..joints)
                .map(|j| {
                    joint(
                        &format!("b{j}"),
                        j.checked_sub(1).map(|p| p as u32),
                        [0.0, 0.1, 0.0],
                    )
                })
                .collect(),
            weights: (0..vertices * 4)
                .map(|i| if i % 4 == 0 { 1.0 } else { 0.0 })
                .collect(),
            bone_indices: (0..vertices * 4)
                .map(|i| {
                    if i % 4 == 0 {
                        ((i / 4) % joints) as u32
                    } else {
                        0
                    }
                })
                .collect(),
        }),
        indices: (0..(vertices as u32 - 2))
            .flat_map(|i| [i, i + 1, i + 2])
            .collect(),
        ..MeshDocument::default()
    };
    let text = ludion_scene::emit_mesh(&doc);
    std::fs::write(root.join(MESH_PATH), &text).unwrap();
    let mut prefab = prefab_text("b0");
    for j in 1..10 {
        prefab.push_str(&format!(
            "  - name: part{j}\n    components:\n      SkinBone:\n        skin:\n          index: 0\n          generation: 0\n        bone: b{}\n",
            j * 5
        ));
    }
    std::fs::write(root.join(PREFAB_PATH), prefab).unwrap();
    println!("mesh text: {:.2} MB", text.len() as f64 / 1e6);

    for preload in [false, true] {
        let mut engine = engine(&root);
        let start = std::time::Instant::now();
        engine
            .load_inline_scene(&scene(if preload { &[PREFAB_PATH] } else { &[] }))
            .unwrap();
        let load_ms = start.elapsed().as_secs_f64() * 1e3;
        let timed = |engine: &mut Engine| {
            let start = std::time::Instant::now();
            spawn_prefab(engine.world_mut(), PREFAB_PATH, None, None, false).expect("spawns");
            start.elapsed().as_secs_f64() * 1e3
        };
        let first = timed(&mut engine);
        let second = timed(&mut engine);
        println!(
            "preload={preload}: load {load_ms:.2} ms, first spawn {first:.2} ms, second spawn {second:.2} ms"
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}

//! A rendered `Parent` hierarchy draws the world poses it composes to: the same scene with every
//! child's `Transform` replaced by its composed world pose (and its `Parent` dropped) is the
//! flattened twin, and the two must render alike. Also writes the 10k-entity perf scenes.

use std::path::{Path, PathBuf};
use std::process::{Command as ProcCommand, Output};

use ludion_scene::{Scene, SceneEntity};
use serde_json::{Value, json};

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    if let Err(e) = std::fs::remove_dir_all(&dir) {
        assert!(
            e.kind() == std::io::ErrorKind::NotFound,
            "stale scratch dir {dir:?} could not be removed: {e}"
        );
    }
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
fn run_ludion(args: &[&str]) -> Output {
    ProcCommand::new(env!("CARGO_BIN_EXE_ludion"))
        .args(args)
        .output()
        .expect("failed to run the ludion binary")
}
fn is_no_adapter(output: &Output) -> bool {
    ludion_core::require_gpu::skip_no_adapter(
        String::from_utf8_lossy(&output.stderr).contains("error[L2003]"),
    )
}

fn transform(translation: [f32; 3], rotation: [f32; 3], scale: f32) -> (&'static str, Value) {
    (
        "Transform",
        json!({ "translation": translation, "rotation": rotation, "scale": [scale, scale, scale] }),
    )
}

fn parent(index: usize) -> (&'static str, Value) {
    ("Parent", json!({ "index": index, "generation": 0 }))
}

fn entity(name: &str, components: Vec<(&'static str, Value)>) -> SceneEntity {
    SceneEntity {
        name: Some(name.to_string()),
        presentation: false,
        components: components
            .into_iter()
            .map(|(name, value)| (name.to_string(), value))
            .collect(),
        instance: None,
    }
}

fn scene(entities: Vec<SceneEntity>) -> Scene {
    Scene {
        name: Some("hierarchy".to_string()),
        seed: Some(0),
        entities,
        ..Scene::default()
    }
}

fn emit(scene: &Scene) -> String {
    ludion_scene::emit(scene, &ludion_engine::builtin_registry())
}

/// The scene with every parented entity's `Transform` replaced by its composed world pose and its
/// `Parent` dropped.
fn flatten(tree: &Scene) -> Scene {
    let registry = ludion_engine::builtin_registry();
    let checked = ludion_scene::check(&emit(tree), &registry).expect("the tree scene checks");
    let world = ludion_scene::instantiate(&checked, &registry).expect("the tree instantiates");
    let ids = ludion_scene::instantiated_ids(&checked);
    let mut flat = checked.clone();
    for (index, entity) in flat.entities.iter_mut().enumerate() {
        if !entity.components.iter().any(|(name, _)| name == "Parent") {
            continue;
        }
        let pose = ludion_engine::world_pose_transform(&world, ids[index])
            .expect("a parented entity has a Transform");
        entity.components.retain(|(name, _)| name != "Parent");
        for (name, value) in &mut entity.components {
            if name == "Transform" {
                *value = json!({
                    "translation": pose.translation,
                    "rotation": pose.rotation,
                    "scale": pose.scale,
                });
            }
        }
    }
    flat
}

fn without_parents(tree: &Scene) -> Scene {
    let mut scene = tree.clone();
    for entity in &mut scene.entities {
        entity.components.retain(|(name, _)| name != "Parent");
    }
    scene
}

fn hierarchy_scene() -> Scene {
    let mesh = |shape: &str, color: [f32; 4]| {
        (
            "Mesh",
            json!({ "shape": shape, "color": color, "roughness": 0.6 }),
        )
    };
    scene(vec![
        // 0: a rotated rig carrying the camera.
        entity(
            "rig",
            vec![transform([0.0, 3.0, 9.0], [-12.0, 8.0, 0.0], 1.0)],
        ),
        entity(
            "camera",
            vec![
                transform([0.5, 0.0, 0.0], [0.0, 0.0, 0.0], 1.0),
                ("Camera", json!({})),
                parent(0),
            ],
        ),
        entity(
            "sun",
            vec![
                transform([0.0, 8.0, 0.0], [0.0, 0.0, 0.0], 1.0),
                ("DirectionalLight", json!({ "intensity": 1.2 })),
            ],
        ),
        // 3: a lamp rig and the point light under it.
        entity(
            "lamp-rig",
            vec![transform([-2.0, 3.0, 0.0], [0.0, 30.0, 0.0], 1.0)],
        ),
        entity(
            "lamp",
            vec![
                transform([1.0, 0.5, 1.0], [0.0, 0.0, 0.0], 1.0),
                ("PointLight", json!({ "intensity": 4.0, "range": 14.0 })),
                parent(3),
            ],
        ),
        entity(
            "ground",
            vec![
                transform([0.0, 0.0, 0.0], [0.0, 0.0, 0.0], 14.0),
                mesh("plane", [0.5, 0.5, 0.5, 1.0]),
            ],
        ),
        // 6..8: a three-deep chain with a rotation and a uniform scale at every level.
        entity(
            "a",
            vec![
                transform([0.0, 0.8, 0.0], [0.0, 20.0, 0.0], 1.5),
                mesh("cube", [0.8, 0.3, 0.2, 1.0]),
            ],
        ),
        entity(
            "b",
            vec![
                transform([1.2, 0.8, 0.0], [0.0, 30.0, 15.0], 0.6),
                mesh("sphere", [0.2, 0.6, 0.9, 1.0]),
                parent(6),
            ],
        ),
        entity(
            "c",
            vec![
                transform([1.5, 0.0, 0.0], [10.0, 0.0, 40.0], 0.5),
                mesh("cube", [0.9, 0.8, 0.2, 1.0]),
                parent(7),
            ],
        ),
        // 9, 10: a non-billboard sprite under a turned pivot.
        entity(
            "pivot",
            vec![transform([-3.0, 1.0, 0.0], [0.0, 40.0, 0.0], 1.0)],
        ),
        entity(
            "sprite",
            vec![
                transform([0.0, 1.0, 0.0], [0.0, 0.0, 30.0], 1.2),
                (
                    "Sprite",
                    json!({ "size": [1.5, 1.0], "color": [1.0, 0.2, 0.2, 1.0] }),
                ),
                parent(9),
            ],
        ),
        // 11, 12: a ground marker under a turned pivot.
        entity(
            "marker-rig",
            vec![transform([3.0, 0.0, -1.0], [0.0, 25.0, 0.0], 1.0)],
        ),
        entity(
            "marker",
            vec![
                transform([0.0, 0.0, 1.0], [0.0, 15.0, 0.0], 1.5),
                (
                    "GroundMarker",
                    json!({ "shape": { "type": "segment", "length": 2.0, "width": 0.6 } }),
                ),
                parent(11),
            ],
        ),
    ])
}

fn render(scene: &Scene, name: &str) -> Result<Vec<u8>, Output> {
    let dir = scratch(name);
    let path = dir.join("scene.yaml");
    std::fs::write(&path, emit(scene)).unwrap();
    let out = dir.join("out.png");
    let output = run_ludion(&[
        "render",
        "--scene",
        path.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--width",
        "480",
        "--height",
        "270",
    ]);
    if output.status.success() {
        Ok(std::fs::read(&out).unwrap())
    } else {
        Err(output)
    }
}

#[test]
fn needs_gpu_a_hierarchy_renders_like_its_flattened_twin() {
    let tree = hierarchy_scene();
    let tree_png = match render(&tree, "hierarchy-tree") {
        Ok(png) => png,
        Err(output) if is_no_adapter(&output) => {
            eprintln!("SKIPPED a_hierarchy_renders_like_its_flattened_twin: no GPU adapter");
            return;
        }
        Err(output) => panic!("render failed: {}", String::from_utf8_lossy(&output.stderr)),
    };
    let twin_png = render(&flatten(&tree), "hierarchy-twin")
        .unwrap_or_else(|o| panic!("render failed: {}", String::from_utf8_lossy(&o.stderr)));
    let dropped_png = render(&without_parents(&tree), "hierarchy-dropped")
        .unwrap_or_else(|o| panic!("render failed: {}", String::from_utf8_lossy(&o.stderr)));

    let positive =
        ludion_testkit::compare_png_bytes(&twin_png, &tree_png, 0.999).expect("both PNGs decode");
    eprintln!("twin vs tree: {}", positive.summary);
    assert!(
        positive.pass,
        "the tree must match its flattened twin at 0.999: {}",
        positive.summary
    );
    let negative = ludion_testkit::compare_png_bytes(&twin_png, &dropped_png, 0.999)
        .expect("both PNGs decode");
    eprintln!("twin vs parents-dropped: {}", negative.summary);
    assert!(
        !negative.pass && negative.damaged_tiles.len() >= 32,
        "dropping the Parent entries must change the picture by more than a faint drift: {}",
        negative.summary
    );
}

/// 2,500 chains root→a→b→c of cubes (10,000 entities, depth 4) and the same world flattened, for
/// `ludion profile` (see `Plan/transform-hierarchy-and-prefabs.md` §W1 gates).
#[test]
#[ignore = "writes the perf scenes the hierarchy profile runs on"]
fn write_perf_scenes() {
    let cube = |tone: f32| {
        (
            "Mesh",
            json!({ "shape": "cube", "color": [tone, 0.5, 0.4, 1.0] }),
        )
    };
    let mut entities = vec![
        entity(
            "camera",
            vec![
                transform([50.0, 70.0, 130.0], [-25.0, 0.0, 0.0], 1.0),
                ("Camera", json!({})),
            ],
        ),
        entity(
            "sun",
            vec![
                transform([0.0, 8.0, 0.0], [0.0, 0.0, 0.0], 1.0),
                ("DirectionalLight", json!({})),
            ],
        ),
    ];
    for chain in 0..2500usize {
        let (x, z) = ((chain % 50) as f32 * 2.0, (chain / 50) as f32 * 2.0);
        let root = entities.len();
        entities.push(entity(
            "root",
            vec![transform([x, 0.0, -z], [0.0, 10.0, 0.0], 0.8), cube(0.3)],
        ));
        for level in 1..4usize {
            entities.push(entity(
                "link",
                vec![
                    transform([0.0, 0.5, 0.2], [5.0, 12.0, 8.0], 0.9),
                    cube(0.3 + 0.1 * level as f32),
                    parent(root + level - 1),
                ],
            ));
        }
    }
    let tree = scene(entities);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("perf");
    std::fs::create_dir_all(&dir).unwrap();
    let write = |path: &Path, scene: &Scene| {
        std::fs::write(path, emit(scene)).unwrap();
        println!("{}", path.display());
    };
    write(&dir.join("tree.yaml"), &tree);
    write(&dir.join("flat.yaml"), &flatten(&tree));
}

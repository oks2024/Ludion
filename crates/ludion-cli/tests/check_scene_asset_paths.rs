//! `ludion check-scene` on the files a scene's meshes and colliders name: a missing document is
//! `L1101` at the entity naming it, in the scene or in a prefab it instances (`Clockout 0033`).

use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::Value;

fn tree(tag: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("ludion-check-assets-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (name, text) in files {
        std::fs::write(dir.join(name), text).unwrap();
    }
    dir
}

fn run(scene: &std::path::Path, json: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ludion"));
    command.arg("check-scene").arg(scene);
    if json {
        command.arg("--json");
    }
    command.output().expect("failed to run the ludion binary")
}

/// The `L1101` of a failed `--json` check.
fn l1101(output: &Output) -> Value {
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: Value = serde_json::from_str(stdout.trim()).expect("one JSON object");
    value["diagnostics"]
        .as_array()
        .and_then(|all| all.iter().find(|d| d["code"] == "L1101"))
        .unwrap_or_else(|| panic!("no L1101 in {value}"))
        .clone()
}

#[test]
fn a_missing_mesh_asset_is_l1101_at_the_shape() {
    let dir = tree(
        "mesh",
        &[(
            "scene.yaml",
            "format: 1\nentities:\n  - name: rock\n    components:\n      Mesh:\n        shape:\n          asset: gone.mesh.yaml\n",
        )],
    );
    let diagnostic = l1101(&run(&dir.join("scene.yaml"), true));
    assert_eq!(diagnostic["code"], "L1101");
    assert_eq!(
        diagnostic["path"],
        "/entities/0/components/Mesh/shape/asset"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_missing_collider_mesh_is_l1101_at_its_path() {
    let dir = tree(
        "collider",
        &[(
            "scene.yaml",
            "format: 1\nentities:\n  - name: floor\n    components:\n      Collider:\n        shape:\n          mesh:\n            path: gone.mesh.yaml\n        body: static\n",
        )],
    );
    let diagnostic = l1101(&run(&dir.join("scene.yaml"), true));
    assert_eq!(diagnostic["code"], "L1101");
    assert_eq!(
        diagnostic["path"],
        "/entities/0/components/Collider/shape/mesh/path"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_missing_mesh_inside_an_instanced_prefab_is_located_back() {
    let dir = tree(
        "prefab",
        &[
            (
                "rock.prefab.yaml",
                "format: 1\nkind: prefab\nentities:\n  - name: rock\n    components:\n      Mesh:\n        shape:\n          asset: gone.mesh.yaml\n",
            ),
            (
                "scene.yaml",
                "format: 1\nentities:\n  - name: rock-1\n    prefab: rock.prefab.yaml\n",
            ),
        ],
    );
    let diagnostic = l1101(&run(&dir.join("scene.yaml"), true));
    assert_eq!(diagnostic["code"], "L1101");
    assert_eq!(diagnostic["path"], "/entities/0", "{diagnostic}");
    let message = diagnostic["message"].as_str().unwrap();
    assert!(
        message.contains("rock.prefab.yaml") && message.contains("components/Mesh/shape/asset"),
        "{message}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_scene_naming_a_real_mesh_is_unchanged() {
    let good = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/good_mesh.mesh.yaml"),
    )
    .unwrap();
    let dir = tree(
        "clean",
        &[
            ("good.mesh.yaml", &good),
            (
                "scene.yaml",
                "format: 1\nentities:\n  - name: rock\n    components:\n      Mesh:\n        shape:\n          asset: good.mesh.yaml\n",
            ),
        ],
    );
    let scene = dir.join("scene.yaml");
    let output = run(&scene, false);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        format!("{}: OK (1 entity, 1 component)", scene.display())
    );
    let _ = std::fs::remove_dir_all(&dir);
}

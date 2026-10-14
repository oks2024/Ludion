//! Integration tests for `ludion check-scene` on `kind: prefab` documents
//! (`Plan/transform-hierarchy-and-prefabs.md` W2): a prefab checks clean, a singleton part is
//! `L1229`, and the hierarchy body rules hold inside a prefab.

use std::path::PathBuf;
use std::process::Output;

use std::process::Command;

const BOX: &str = "      Collider:
        shape:
          box:
            half_extents:
              - 0.5
              - 0.5
              - 0.5
        body:
          ";

fn scratch(file: &str, contents: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-check-prefab-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(file);
    std::fs::write(&path, contents).unwrap();
    path
}

fn run_check_scene(path: &std::path::Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ludion"))
        .arg("check-scene")
        .arg(path)
        .output()
        .expect("failed to run the ludion binary")
}

#[test]
fn a_two_part_prefab_with_a_joint_checks_clean() {
    let source = format!(
        "format: 1\nkind: prefab\nname: door\nentities:\n  - name: frame\n    components:\n      \
         Transform: {{}}\n  - name: leaf\n    components:\n      Transform: {{}}\n{BOX}rigid: {{}}\n      \
         Joint:\n        a:\n          index: 0\n          generation: 0\n        b:\n          \
         index: 1\n          generation: 0\n        kind:\n          ball: {{}}\n"
    );
    let path = scratch("door.prefab.yaml", &source);
    let output = run_check_scene(&path);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("OK (prefab door, 2 parts, 4 components)"),
        "stdout was: {stdout}"
    );
}

#[test]
fn a_camera_part_is_l1229() {
    let path = scratch(
        "cam.prefab.yaml",
        "format: 1\nkind: prefab\nentities:\n  - name: eye\n    components:\n      Camera: {}\n",
    );
    let output = run_check_scene(&path);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("L1229"), "stderr was: {stderr}");
}

#[test]
fn a_rigid_collider_under_a_parent_is_l1221() {
    let source = format!(
        "format: 1\nkind: prefab\nentities:\n  - name: frame\n    components: {{}}\n  - name: leaf\n    \
         components:\n      Parent:\n        index: 0\n        generation: 0\n{BOX}rigid: {{}}\n"
    );
    let path = scratch("rigid.prefab.yaml", &source);
    let output = run_check_scene(&path);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("L1221"), "stderr was: {stderr}");
}

/// Writes `files` (name, text) into a fresh per-test directory and returns its path.
fn tree(tag: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("ludion-check-prefab-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (file, text) in files {
        std::fs::write(dir.join(file), text).unwrap();
    }
    dir
}

const LAMP: &str = "format: 1\nkind: prefab\nname: lamp\nentities:\n  - name: post\n    components:\n      Transform: {}\n  - name: bulb\n    components:\n      Transform: {}\n";

#[test]
fn an_instance_scene_checks_clean_with_the_expanded_count() {
    let dir = tree(
        "instances-ok",
        &[
            ("lamp.prefab.yaml", LAMP),
            (
                "scene.yaml",
                "format: 1\nentities:\n  - name: lamp-1\n    prefab: lamp.prefab.yaml\n  - name: lamp-2\n    prefab: lamp.prefab.yaml\n",
            ),
        ],
    );
    let output = run_check_scene(&dir.join("scene.yaml"));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("OK (4 entities, 4 components)"), "{stdout}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_instance_loop_is_l1232_naming_the_nested_file() {
    let dir = tree(
        "instances-cycle",
        &[
            (
                "a.prefab.yaml",
                "format: 1\nkind: prefab\nentities:\n  - name: b\n    prefab: b.prefab.yaml\n",
            ),
            (
                "b.prefab.yaml",
                "format: 1\nkind: prefab\nentities:\n  - name: a\n    prefab: a.prefab.yaml\n",
            ),
            (
                "scene.yaml",
                "format: 1\nentities:\n  - name: a-1\n    prefab: a.prefab.yaml\n",
            ),
        ],
    );
    let output = run_check_scene(&dir.join("scene.yaml"));
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("L1232"), "{stderr}");
    assert!(stderr.contains("b.prefab.yaml"), "{stderr}");
    assert!(stderr.contains("/entities/0/prefab"), "{stderr}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_bad_override_path_is_l1233_with_the_nearest_name() {
    let dir = tree(
        "instances-override",
        &[
            ("lamp.prefab.yaml", LAMP),
            (
                "scene.yaml",
                "format: 1\nentities:\n  - name: spacer\n    components: {}\n  - name: lamp-1\n    prefab: lamp.prefab.yaml\n    overrides:\n      blub:\n        Transform: {}\n",
            ),
        ],
    );
    let output = run_check_scene(&dir.join("scene.yaml"));
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("L1233"), "{stderr}");
    assert!(stderr.contains("/entities/1/overrides/blub"), "{stderr}");
    assert!(stderr.contains("`bulb`"), "{stderr}");
    let _ = std::fs::remove_dir_all(&dir);
}

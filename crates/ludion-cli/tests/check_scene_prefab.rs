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

/// A copy of the node-clip import's hinge prefab, under a scratch directory unique to `tag`.
fn hinge_copy(tag: &str) -> PathBuf {
    let source =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/import-node-clips/expected");
    let dir = std::env::temp_dir().join(format!("ludion-check-hinge-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for file in [
        "model.prefab.yaml",
        "clips/open.anim.yaml",
        "clips/snap.anim.yaml",
        "meshes/base.mesh.yaml",
        "meshes/lid.mesh.yaml",
        "meshes/latch.mesh.yaml",
    ] {
        let to = dir.join(file);
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::copy(source.join(file), to).unwrap();
    }
    dir
}

#[test]
fn a_prefab_with_clips_lists_them() {
    let dir = hinge_copy("lists");
    let output = run_check_scene(&dir.join("model.prefab.yaml"));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("3 parts"), "{stdout}");
    assert!(stdout.contains(", 2 clips)"), "{stdout}");
    assert!(
        stdout.contains("  clip open: clips/open.anim.yaml, duration"),
        "{stdout}"
    );
    assert!(
        stdout.contains("  clip snap: clips/snap.anim.yaml"),
        "{stdout}"
    );

    let json = Command::new(env!("CARGO_BIN_EXE_ludion"))
        .args(["check-scene", "--json"])
        .arg(dir.join("model.prefab.yaml"))
        .output()
        .unwrap();
    let report: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(report["clips"][1]["name"], "snap");
    assert_eq!(report["clips"][1]["looped"], false);
}

#[test]
fn a_clip_track_naming_no_part_is_l1314_with_the_nearest_part() {
    let dir = hinge_copy("bad-target");
    let clip = dir.join("clips/snap.anim.yaml");
    let text = std::fs::read_to_string(&clip).unwrap();
    assert!(text.contains("\n  latch:\n"));
    std::fs::write(&clip, text.replace("\n  latch:\n", "\n  latchh:\n")).unwrap();
    let output = run_check_scene(&dir.join("model.prefab.yaml"));
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("L1314"), "{stderr}");
    assert!(
        stderr.contains("animates `latchh`, which is not a part of this prefab"),
        "{stderr}"
    );
    assert!(stderr.contains("nearest part is `latch`"), "{stderr}");
}

#[test]
fn a_scene_placing_a_prefab_with_a_bad_clip_track_is_l1314() {
    let dir = hinge_copy("scene-bad-target");
    let clip = dir.join("clips/snap.anim.yaml");
    let text = std::fs::read_to_string(&clip).unwrap();
    std::fs::write(&clip, text.replace("\n  latch:\n", "\n  latchh:\n")).unwrap();
    let scene = dir.join("scene.yaml");
    std::fs::write(
        &scene,
        "format: 1\nname: s\nentities:\n  - name: hinge-1\n    prefab: model.prefab.yaml\n",
    )
    .unwrap();
    let output = run_check_scene(&scene);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("L1314"), "{stderr}");
    assert!(stderr.contains("in `model.prefab.yaml`"), "{stderr}");
    assert!(stderr.contains("nearest part is `latch`"), "{stderr}");

    std::fs::write(&clip, text).unwrap();
    let output = run_check_scene(&scene);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn arm_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery/prefabs/arm")
}

#[test]
fn a_prefab_with_masks_and_blends_lists_them() {
    let path = arm_dir().join("arm.prefab.yaml");
    let output = run_check_scene(&path);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("  mask forearm: lower (descendants)"),
        "{stdout}"
    );
    assert!(
        stdout.contains("  blend sway (1D): swing@0, reach@1"),
        "{stdout}"
    );

    let json = Command::new(env!("CARGO_BIN_EXE_ludion"))
        .args(["check-scene", "--json"])
        .arg(&path)
        .output()
        .unwrap();
    let report: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(
        report["masks"],
        serde_json::json!([
            { "name": "forearm", "joints": ["lower"], "descendants": true, "weights": [] }
        ])
    );
    assert_eq!(report["blends"][0]["name"], "sway");
    assert_eq!(report["blends"][0]["dimension"], 1);
    assert_eq!(report["blends"][0]["points"][1]["clip"], "reach");
}

#[test]
fn a_mask_naming_no_part_is_l1674_with_the_nearest_part() {
    let text = std::fs::read_to_string(arm_dir().join("arm.prefab.yaml"))
        .unwrap()
        .replace("\r\n", "\n")
        .replace("      - lower\n", "      - lowr\n");
    let path = scratch("bad-mask.prefab.yaml", &text);
    let output = run_check_scene(&path);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("L1674"), "{stderr}");
    assert!(stderr.contains("nearest part is `lower`"), "{stderr}");
}

/// A blend is checked even when the prefab binds no clips at all.
#[test]
fn a_blend_with_no_bound_clips_is_l1674() {
    let path = scratch(
        "unbound.prefab.yaml",
        "format: 1\nkind: prefab\nblends:\n  sway:\n    points:\n      - clip: swing\n        at: 0.0\n      \
         - clip: reach\n        at: 1.0\nentities:\n  - name: a\n    components: {}\n",
    );
    let output = run_check_scene(&path);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("L1674"), "{stderr}");
}

#[test]
fn an_unclipped_prefab_is_unchanged_and_json_lists_no_clips() {
    let path = scratch(
        "plain.prefab.yaml",
        "format: 1\nkind: prefab\nname: plain\nentities:\n  - name: a\n    components:\n      Transform: {}\n",
    );
    let output = run_check_scene(&path);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("OK (prefab plain, 1 part, 1 component)"),
        "{stdout}"
    );
    let json = Command::new(env!("CARGO_BIN_EXE_ludion"))
        .args(["check-scene", "--json"])
        .arg(&path)
        .output()
        .unwrap();
    let report: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(report["clips"], serde_json::json!([]));
    assert!(report.get("masks").is_none() && report.get("blends").is_none());
    assert!(
        !stdout.contains("mask") && !stdout.contains("blend"),
        "{stdout}"
    );
}

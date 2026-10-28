//! Integration tests for `ludion check-scene`, driven against the built `ludion` binary.

use std::path::Path;
use std::process::{Command, Output};

fn run_check_scene(file: &str) -> Output {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(file);
    Command::new(env!("CARGO_BIN_EXE_ludion"))
        .arg("check-scene")
        .arg(fixture)
        .output()
        .expect("failed to run the ludion binary")
}

/// A valid scene checks clean: zero exit and a one-line summary with the counts.
#[test]
fn good_scene_succeeds() {
    let output = run_check_scene("good.yaml");
    assert!(
        output.status.success(),
        "expected success, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("OK"), "stdout was: {stdout}");
    assert!(stdout.contains("2 entities"), "stdout was: {stdout}");
    assert!(stdout.contains("1 component)"), "stdout was: {stdout}");
}

/// A broken scene exits non-zero and prints, per defect: the L-code, the JSON Pointer to the
/// offending node, and a concrete suggestion. This is the Phase 1 exit criterion for the CLI.
#[test]
fn broken_scene_prints_code_pointer_and_suggestion() {
    let output = run_check_scene("broken.yaml");
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Defect 1: component name typo → unknown component with a nearest-match suggestion.
    assert!(stderr.contains("error[L1013]"), "stderr was: {stderr}");
    assert!(
        stderr.contains("/entities/0/components/Parnet"),
        "stderr was: {stderr}"
    );
    assert!(
        stderr.contains("nearest match is `Parent`"),
        "stderr was: {stderr}"
    );

    // Defect 2: negative entity index → schema range violation at the exact field.
    assert!(stderr.contains("error[L1014]"), "stderr was: {stderr}");
    assert!(
        stderr.contains("/entities/1/components/Parent/index"),
        "stderr was: {stderr}"
    );

    // The summary counts both.
    assert!(stderr.contains("2 errors found"), "stderr was: {stderr}");
}

/// A missing file is an L1015 diagnostic, not a panic.
#[test]
fn missing_file_is_l1015() {
    let output = Command::new(env!("CARGO_BIN_EXE_ludion"))
        .args(["check-scene", "does-not-exist.yaml"])
        .output()
        .expect("failed to run the ludion binary");
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L1015]"), "stderr was: {stderr}");
    assert!(
        stderr.contains("does-not-exist.yaml"),
        "stderr was: {stderr}"
    );
}

/// Every L-code check-scene can print has a working `ludion explain` entry — the two commands
/// are two halves of one loop (see error → explain error).
#[test]
fn scene_codes_are_explainable() {
    for code in ["L1013", "L1014", "L1015"] {
        let output = Command::new(env!("CARGO_BIN_EXE_ludion"))
            .args(["explain", code])
            .output()
            .expect("failed to run the ludion binary");
        assert!(
            output.status.success(),
            "`ludion explain {code}` failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn run_check_scene_args(file: &str, args: &[&str]) -> Output {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(file);
    Command::new(env!("CARGO_BIN_EXE_ludion"))
        .arg("check-scene")
        .arg(fixture)
        .args(args)
        .output()
        .expect("failed to run the ludion binary")
}

/// `--bounds` on a scene prints the resolved symmetric layer matrix before the `OK` line: `sensor`
/// ignores `default`, so `default` lists `sensor` too, and `dummy` ignores itself.
#[test]
fn bounds_prints_the_resolved_layer_matrix() {
    let output = run_check_scene_args("layers.yaml", &["--bounds"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines[..4],
        [
            "layers:",
            "  default: ignores sensor",
            "  dummy: ignores dummy",
            "  sensor: ignores default",
        ],
        "stdout was: {stdout}"
    );
    assert!(lines[4].contains("OK"), "stdout was: {stdout}");
}

/// `--bounds --json` is the summary object with the layer matrix added.
#[test]
fn bounds_json_prints_one_object() {
    let output = run_check_scene_args("layers.yaml", &["--bounds", "--json"]);
    assert!(output.status.success());
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout is one JSON object");
    assert_eq!(value["ok"], true);
    assert_eq!(value["kind"], "scene");
    assert_eq!(
        value["layers"],
        serde_json::json!([
            { "name": "default", "ignores": ["sensor"] },
            { "name": "dummy", "ignores": ["dummy"] },
            { "name": "sensor", "ignores": ["default"] },
        ])
    );
}

/// A scene that declares no layers says so instead of printing a matrix.
#[test]
fn bounds_on_an_unlayered_scene_says_none_declared() {
    let output = run_check_scene_args("layers-unlayered.yaml", &["--bounds"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.starts_with("layers: none declared"),
        "stdout was: {stdout}"
    );
}

/// A collider naming an undeclared layer is `L1186` with the pointer and the nearest match.
#[test]
fn an_undeclared_layer_is_l1186() {
    let output = run_check_scene_args("layers-unknown.yaml", &[]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("L1186"), "stderr was: {stderr}");
    assert!(
        stderr.contains("/entities/0/components/Collider/layer"),
        "stderr was: {stderr}"
    );
    assert!(
        stderr.contains("nearest match is `dummy`"),
        "stderr was: {stderr}"
    );
}

/// A layer on a 2D collider the solver does not claim is `L1187`.
#[test]
fn a_layer_on_an_unclaimed_collider_is_l1187() {
    let output = run_check_scene_args("layers-unclaimed.yaml", &[]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("L1187"), "stderr was: {stderr}");
    assert!(
        stderr.contains("/entities/0/components/Collider/layer"),
        "stderr was: {stderr}"
    );
}

/// A sensor on a rigid body is `L1291`.
#[test]
fn a_rigid_sensor_is_l1291() {
    let output = run_check_scene_args("sensor-rigid.yaml", &[]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("L1291"), "stderr was: {stderr}");
    assert!(
        stderr.contains("/entities/0/components/Collider/sensor"),
        "stderr was: {stderr}"
    );
}

/// A sensor on a heightfield (or mesh) is `L1292`.
#[test]
fn a_heightfield_sensor_is_l1292() {
    let output = run_check_scene_args("sensor-heightfield.yaml", &[]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("L1292"), "stderr was: {stderr}");
    assert!(
        stderr.contains("/entities/0/components/Collider/sensor"),
        "stderr was: {stderr}"
    );
}

/// A sensor on a 2D collider the solver does not claim is `L1293`.
#[test]
fn a_sensor_on_an_unclaimed_collider_is_l1293() {
    let output = run_check_scene_args("sensor-unclaimed.yaml", &[]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("L1293"), "stderr was: {stderr}");
    assert!(
        stderr.contains("/entities/0/components/Collider/sensor"),
        "stderr was: {stderr}"
    );
}

/// A `SkinBone` naming a bone that is not a joint of its skin's mesh document is `L1195`, with the
/// pointer at `bone` and the nearest joint name.
#[test]
fn a_skin_bone_unknown_bone_is_l1195() {
    let output = run_check_scene_args("skin-bone-unknown-bone.yaml", &[]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("L1195"), "stderr was: {stderr}");
    assert!(
        stderr.contains("/entities/1/components/SkinBone/bone"),
        "stderr was: {stderr}"
    );
    assert!(
        stderr.contains("nearest match is `child`"),
        "stderr was: {stderr}"
    );
}

/// `--json` alone prints the scene's summary object: `layers.yaml` has one entity and one component.
#[test]
fn json_alone_prints_the_scene_summary() {
    let output = run_check_scene_args("layers.yaml", &["--json"]);
    assert!(output.status.success());
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout is one JSON object");
    assert_eq!(value["kind"], "scene");
    assert_eq!(value["ok"], true);
    assert_eq!(value["entities"], 1);
    assert_eq!(value["components"], 1);
}

/// A document another verb validates is refused with `L0031`, pointed at `/kind`, and the help names
/// that verb.
#[test]
fn a_kind_checked_elsewhere_is_l0031() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("check_scene_elsewhere");
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    let file = dir.join("town.yaml");
    std::fs::write(&file, "kind: map\n").expect("write the map document");
    let output = Command::new(env!("CARGO_BIN_EXE_ludion"))
        .arg("check-scene")
        .arg(&file)
        .output()
        .expect("failed to run the ludion binary");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.starts_with("error[L0031]: "), "stderr was: {stderr}");
    assert!(stderr.contains("--> /kind"), "stderr was: {stderr}");
    assert!(stderr.contains("generate-map"), "stderr was: {stderr}");
}

/// A rigid body under a `Parent` is `L1221`, pointed at its `body`, naming the parent.
#[test]
fn a_rigid_child_is_l1221() {
    let output = run_check_scene_args("hierarchy-rigid-child.yaml", &[]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("L1221"), "stderr was: {stderr}");
    assert!(
        stderr.contains("/entities/1/components/Collider/body"),
        "stderr was: {stderr}"
    );
    assert!(stderr.contains("`platform`"), "stderr was: {stderr}");
}

//! Integration tests for `ludion check-scene --placement`: floating, sunk and overlapping props as
//! warnings, grouped by prefab instance or hierarchy root, never changing the exit code.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A unit cube centred on the origin.
const CUBE_MESH: &str = "format: 1
kind: mesh
positions:
  - -0.5
  - -0.5
  - -0.5
  - 0.5
  - -0.5
  - -0.5
  - 0.5
  - 0.5
  - -0.5
  - -0.5
  - 0.5
  - -0.5
  - -0.5
  - -0.5
  - 0.5
  - 0.5
  - -0.5
  - 0.5
  - 0.5
  - 0.5
  - 0.5
  - -0.5
  - 0.5
  - 0.5
indices:
  - 0
  - 2
  - 1
  - 0
  - 3
  - 2
  - 4
  - 5
  - 6
  - 4
  - 6
  - 7
  - 0
  - 1
  - 5
  - 0
  - 5
  - 4
  - 2
  - 3
  - 7
  - 2
  - 7
  - 6
  - 1
  - 2
  - 6
  - 1
  - 6
  - 5
  - 0
  - 4
  - 7
  - 0
  - 7
  - 3
";

const MESH: &str = "      Mesh:
        shape:
          asset: cube.mesh.yaml
";

const COLLIDER: &str = "      Collider:
        shape:
          aabb:
            half_extents:
              - 0.5
              - 0.5
";

/// Writes `files` into a fresh per-test directory (with the cube mesh) and returns it.
fn tree(tag: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-check-placement-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("cube.mesh.yaml"), CUBE_MESH).unwrap();
    for (file, text) in files {
        std::fs::write(dir.join(file), text).unwrap();
    }
    dir
}

/// A scene entity with a cube mesh at `x, y, z`, plus any extra component blocks.
fn prop(name: &str, [x, y, z]: [f32; 3], extra: &str) -> String {
    format!(
        "  - name: {name}\n    components:\n      Transform:\n        translation:\n          - {x:?}\n          - {y:?}\n          - {z:?}\n{MESH}{extra}"
    )
}

fn scene(entities: &[String]) -> String {
    format!("format: 1\nentities:\n{}", entities.concat())
}

fn check(scene_path: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ludion"))
        .arg("check-scene")
        .arg(scene_path)
        .args(args)
        .output()
        .expect("failed to run the ludion binary")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn placed(tag: &str, entities: &[String], args: &[&str]) -> Output {
    let dir = tree(tag, &[("scene.yaml", &scene(entities))]);
    let output = check(&dir.join("scene.yaml"), args);
    let _ = std::fs::remove_dir_all(&dir);
    output
}

#[test]
fn a_supported_stack_is_clean_with_a_summary() {
    let output = placed(
        "clean",
        &[
            prop("base", [0.0, 0.5, 0.0], ""),
            prop("hung", [0.0, 1.5, 0.0], ""),
        ],
        &["--placement"],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(!stderr(&output).contains("L124"), "{}", stderr(&output));
    assert!(
        stdout(&output).contains(
            "placement: 2 groups checked, 0 unmeasured, 0 warnings (ground y 0, tolerance 0.05 m, sink 0.1 m)"
        ),
        "{}",
        stdout(&output)
    );
}

#[test]
fn a_floating_box_is_l1240_at_its_transform() {
    let output = placed(
        "floating",
        &[prop("barrel-3", [0.0, 1.5, 0.0], "")],
        &["--placement"],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    let text = stderr(&output);
    assert!(text.contains("warning[L1240]"), "{text}");
    assert!(text.contains("`barrel-3` floats 1.00 m"), "{text}");
    assert!(text.contains("/entities/0/components/Transform"), "{text}");
}

#[test]
fn a_sunk_box_is_l1241() {
    let output = placed(
        "sunk",
        &[prop("wall-2", [0.0, -0.5, 0.0], "")],
        &["--placement"],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("warning[L1241]"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn overlapping_static_colliders_are_one_l1242_until_they_only_touch() {
    let overlapping = placed(
        "overlap",
        &[
            prop("stall-1", [0.0, 0.5, 0.0], COLLIDER),
            prop("house-4", [0.5, 0.5, 0.0], COLLIDER),
        ],
        &["--placement"],
    );
    assert!(overlapping.status.success(), "{}", stderr(&overlapping));
    let text = stderr(&overlapping);
    assert_eq!(text.matches("warning[L1242]").count(), 1, "{text}");
    assert!(
        text.contains("`stall-1`'s collider overlaps `house-4`'s"),
        "{text}"
    );

    let touching = placed(
        "touching",
        &[
            prop("stall-1", [0.0, 0.5, 0.0], COLLIDER),
            prop("house-4", [0.98, 0.5, 0.0], COLLIDER),
        ],
        &["--placement"],
    );
    assert!(
        !stderr(&touching).contains("L1242"),
        "{}",
        stderr(&touching)
    );
}

const CRATE: &str = "format: 1
kind: prefab
name: crate
entities:
  - name: lid
    components:
      Transform: {}
      Mesh:
        shape:
          asset: cube.mesh.yaml
      Collider:
        shape:
          aabb:
            half_extents:
              - 0.5
              - 0.5
  - name: body
    components:
      Transform: {}
      Mesh:
        shape:
          asset: cube.mesh.yaml
      Collider:
        shape:
          aabb:
            half_extents:
              - 0.5
              - 0.5
";

fn instance(name: &str, y: f32) -> String {
    format!(
        "  - name: {name}\n    prefab: crate.prefab.yaml\n    transform:\n      translation:\n        - 0.0\n        - {y:?}\n        - 0.0\n"
    )
}

#[test]
fn parts_of_one_instance_never_overlap_and_a_floating_instance_names_the_entry() {
    let dir = tree(
        "instances",
        &[
            ("crate.prefab.yaml", CRATE),
            ("grounded.yaml", &scene(&[instance("crate-1", 0.5)])),
            ("floating.yaml", &scene(&[instance("crate-1", 3.0)])),
        ],
    );
    let grounded = check(&dir.join("grounded.yaml"), &["--placement"]);
    assert!(grounded.status.success(), "{}", stderr(&grounded));
    assert!(!stderr(&grounded).contains("L124"), "{}", stderr(&grounded));
    assert!(
        stdout(&grounded).contains("1 groups checked, 0 unmeasured"),
        "{}",
        stdout(&grounded)
    );

    let floating = check(&dir.join("floating.yaml"), &["--placement"]);
    assert!(floating.status.success(), "{}", stderr(&floating));
    let text = stderr(&floating);
    assert!(text.contains("warning[L1240]"), "{text}");
    assert!(text.contains("`crate-1` floats"), "{text}");
    assert!(text.contains("--> /entities/0\n"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_presentation_child_is_placed_through_its_parents_world_pose() {
    let root = format!(
        "  - name: post\n    components:\n      Transform:\n        translation:\n          - 0.0\n          - 1.0\n          - 0.0\n{COLLIDER}"
    );
    let child = format!(
        "  - name: post-visual\n    presentation: true\n    components:\n      Transform:\n        translation:\n          - 0.0\n          - -0.5\n          - 0.0\n      Parent:\n        index: 0\n        generation: 0\n{MESH}"
    );
    let output = placed("presentation", &[root, child], &["--placement"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stdout(&output).contains("1 groups checked, 0 unmeasured"),
        "{}",
        stdout(&output)
    );
    assert!(!stderr(&output).contains("L124"), "{}", stderr(&output));
}

/// A one-row, two-cell grid of unit cubes at height `y`, the second cell turned a quarter.
fn grid(name: &str, y: f32) -> String {
    format!(
        "  - name: {name}\n    presentation: true\n    components:\n      Transform:\n        translation:\n          - 0.0\n          - {y:?}\n          - 0.0\n      MeshGrid:\n        legend:\n          a:\n            shape:\n              asset: cube.mesh.yaml\n        rows:\n          - \"aa\"\n        orient:\n          - \"01\"\n"
    )
}

#[test]
fn a_raised_mesh_grid_floats_and_a_grounded_one_is_clean() {
    let raised = placed("grid-raised", &[grid("plaza", 1.0)], &["--placement"]);
    assert!(raised.status.success(), "{}", stderr(&raised));
    let text = stderr(&raised);
    assert!(text.contains("warning[L1240]"), "{text}");
    assert!(text.contains("`plaza` floats 0.50 m"), "{text}");

    let grounded = placed("grid-grounded", &[grid("plaza", 0.5)], &["--placement"]);
    assert!(grounded.status.success(), "{}", stderr(&grounded));
    assert!(!stderr(&grounded).contains("L124"), "{}", stderr(&grounded));
    assert!(
        stdout(&grounded).contains("1 groups checked, 0 unmeasured"),
        "{}",
        stdout(&grounded)
    );
}

#[test]
fn tolerance_silences_a_small_float() {
    let entities = [prop("lamp", [0.0, 0.9, 0.0], "")];
    let default = placed("tolerance-default", &entities, &["--placement"]);
    assert!(stderr(&default).contains("L1240"), "{}", stderr(&default));
    let loose = placed(
        "tolerance-loose",
        &entities,
        &["--placement", "--tolerance", "0.5"],
    );
    assert!(!stderr(&loose).contains("L124"), "{}", stderr(&loose));
}

#[test]
fn without_the_flag_nothing_is_linted_and_the_tunables_need_it() {
    let entities = [prop("barrel-3", [0.0, 1.5, 0.0], "")];
    let plain = placed("off", &entities, &[]);
    assert!(plain.status.success(), "{}", stderr(&plain));
    assert!(!stderr(&plain).contains("L1240"), "{}", stderr(&plain));
    assert!(!stdout(&plain).contains("placement:"), "{}", stdout(&plain));

    let orphan = placed("orphan-flag", &entities, &["--sink", "0.2"]);
    assert!(!orphan.status.success());
    assert!(
        stderr(&orphan).contains("--placement"),
        "{}",
        stderr(&orphan)
    );
}

#[test]
fn placement_on_a_mesh_document_is_a_usage_error() {
    let dir = tree("mesh-doc", &[]);
    let output = check(&dir.join("cube.mesh.yaml"), &["--placement"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("--placement applies only to scenes; this document is a mesh"),
        "{}",
        stderr(&output)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn bounds_json_keeps_stdout_pure_json() {
    let output = placed(
        "json",
        &[prop("barrel-3", [0.0, 1.5, 0.0], "")],
        &["--bounds", "--json", "--placement"],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    serde_json::from_str::<serde_json::Value>(&stdout(&output))
        .unwrap_or_else(|e| panic!("stdout is not JSON ({e}): {}", stdout(&output)));
    assert!(stderr(&output).contains("L1240"), "{}", stderr(&output));
}

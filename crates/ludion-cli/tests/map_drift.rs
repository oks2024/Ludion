//! `ludion map-drift`: the map half of the drift gate, run over scratch projects that hold real tiny
//! maps compiled by the real `generate-map`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const ROOM: &str = "format: 1
kind: map
name: room
kit: kit.yaml
cell_size: 2.0
origin:
  - 0.0
  - 0.0
rows:
  - \"####\"
  - \"#..#\"
  - \"#..#\"
  - \"####\"
markers:
  - name: spawn
    kind: torch
    x: 5.0
    z: 5.0
";

/// The room with a gap in its right wall: the walker can leave.
const LEAKY: &str = "format: 1
kind: map
name: leaky
kit: kit.yaml
cell_size: 2.0
origin:
  - 0.0
  - 0.0
rows:
  - \"####\"
  - \"#...\"
  - \"#..#\"
  - \"####\"
markers:
  - name: spawn
    kind: torch
    x: 3.0
    z: 3.0
";

/// A sealed room with two `L1029` warnings: a `perch-` marker and another, both inside the well's
/// collider.
const PERCH: &str = "format: 1
kind: map
name: perch
kit: kit.yaml
cell_size: 2.0
origin:
  - 0.0
  - 0.0
rows:
  - \"####\"
  - \"#..#\"
  - \"#..#\"
  - \"####\"
markers:
  - name: spawn
    kind: torch
    x: 5.0
    z: 5.0
  - name: obstacle
    kind: well
    x: 3.0
    z: 3.0
  - name: perch-a
    kind: torch
    x: 3.0
    z: 3.0
  - name: buried
    kind: torch
    x: 3.0
    z: 3.0
";

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// A scratch project: `scenes/` holds the fixture kit and each `(file, text)` given.
fn scratch(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../../target/tmp/{name}"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("scenes/rooms")).unwrap();
    std::fs::copy(fixtures().join("kit.yaml"), root.join("scenes/kit.yaml")).unwrap();
    for (file, text) in files {
        std::fs::write(root.join("scenes").join(file), text).unwrap();
    }
    root
}

fn ludion(cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ludion"))
        .current_dir(cwd)
        .args(args)
        .output()
        .unwrap()
}

fn generate(root: &Path, args: &[&str]) {
    let mut all = vec!["generate-map"];
    all.extend_from_slice(args);
    let output = ludion(root, &all);
    assert!(
        output.status.success(),
        "{all:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The clean project: a scene, a prefab and a mirrored prefab, all from one room map.
fn clean_project(name: &str) -> PathBuf {
    let root = scratch(name, &[("room.map.yaml", ROOM)]);
    generate(&root, &["scenes/room.map.yaml"]);
    generate(
        &root,
        &[
            "scenes/room.map.yaml",
            "--out",
            "scenes/rooms/room.prefab.yaml",
            "--prefab",
        ],
    );
    generate(
        &root,
        &[
            "scenes/room.map.yaml",
            "--out",
            "scenes/rooms/room-mx.prefab.yaml",
            "--mirror",
            "x",
            "--prefab",
        ],
    );
    root
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn a_clean_scene_prefab_and_mirrored_prefab_pass() {
    let root = clean_project("map-drift-clean");
    let output = ludion(&root, &["map-drift"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stdout(&output).contains("1 map(s), 3 output(s) checked, none drifted"),
        "{}",
        stdout(&output)
    );

    // `--root` from elsewhere, and a positional scan dir.
    let elsewhere = ludion(
        &std::env::temp_dir(),
        &["map-drift", "--root", root.to_str().unwrap()],
    );
    assert!(elsewhere.status.success(), "{}", stderr(&elsewhere));
    let positional = ludion(&root, &["map-drift", "scenes/rooms"]);
    assert!(!positional.status.success(), "no map under scenes/rooms");
    assert!(
        stderr(&positional).contains("[L1041]"),
        "{}",
        stderr(&positional)
    );
}

#[test]
fn an_edited_prefab_drifts_with_the_command_that_fixes_it() {
    let root = clean_project("map-drift-edited");
    let mx = root.join("scenes/rooms/room-mx.prefab.yaml");
    let mut text = std::fs::read_to_string(&mx).unwrap();
    text.push_str("# hand edit\n");
    std::fs::write(&mx, text).unwrap();

    let output = ludion(&root, &["map-drift"]);
    assert!(!output.status.success());
    let err = stderr(&output);
    assert!(err.contains("[L1039]"), "{err}");
    for part in [
        "ludion generate-map scenes/room.map.yaml",
        "--out scenes/rooms/room-mx.prefab.yaml",
        "--mirror x",
        "--prefab",
    ] {
        assert!(err.contains(part), "missing `{part}` in:\n{err}");
    }

    generate(
        &root,
        &[
            "scenes/room.map.yaml",
            "--out",
            "scenes/rooms/room-mx.prefab.yaml",
            "--mirror",
            "x",
            "--prefab",
        ],
    );
    let output = ludion(&root, &["map-drift"]);
    assert!(output.status.success(), "{}", stderr(&output));
}

#[test]
fn a_prefab_only_map_needs_no_default_scene_but_an_orphan_map_does() {
    let root = clean_project("map-drift-prefab-only");
    std::fs::remove_file(root.join("scenes/room.yaml")).unwrap();
    let output = ludion(&root, &["map-drift"]);
    assert!(output.status.success(), "{}", stderr(&output));

    std::fs::remove_file(root.join("scenes/rooms/room.prefab.yaml")).unwrap();
    std::fs::remove_file(root.join("scenes/rooms/room-mx.prefab.yaml")).unwrap();
    let output = ludion(&root, &["map-drift"]);
    assert!(!output.status.success());
    let err = stderr(&output);
    assert!(
        err.contains("[L1041]") && err.contains("scenes/room.yaml"),
        "{err}"
    );
}

#[test]
fn a_moved_copy_and_a_bad_recorded_flag_are_faults() {
    let root = clean_project("map-drift-moved");
    std::fs::copy(root.join("scenes/room.yaml"), root.join("scenes/copy.yaml")).unwrap();
    let output = ludion(&root, &["map-drift"]);
    assert!(!output.status.success());
    let err = stderr(&output);
    assert!(
        err.contains("[L1041]") && err.contains("moved or orphaned copy"),
        "{err}"
    );
    assert!(err.contains("scenes/copy.yaml"), "{err}");

    std::fs::remove_file(root.join("scenes/copy.yaml")).unwrap();
    std::fs::write(
        root.join("scenes/bad.yaml"),
        "format: 1\ngenerated_from: room.map.yaml --bogus 1\nentities: []\n",
    )
    .unwrap();
    let output = ludion(&root, &["map-drift"]);
    assert!(!output.status.success());
    let err = stderr(&output);
    assert!(err.contains("[L1041]") && err.contains("--bogus"), "{err}");
}

#[test]
fn a_leaking_map_fails_with_the_leak_attached() {
    let root = scratch("map-drift-leak", &[("leaky.map.yaml", LEAKY)]);
    generate(&root, &["scenes/leaky.map.yaml"]);
    let output = ludion(&root, &["map-drift"]);
    assert!(!output.status.success());
    let err = stderr(&output);
    assert!(err.contains("[L1040]") && err.contains("[L1041]"), "{err}");
}

#[test]
fn deny_warnings_and_allow_follow_the_perch_case() {
    let root = scratch("map-drift-perch", &[("perch.map.yaml", PERCH)]);
    generate(&root, &["scenes/perch.map.yaml"]);

    let plain = ludion(&root, &["map-drift"]);
    assert!(plain.status.success(), "{}", stderr(&plain));

    let denied = ludion(&root, &["map-drift", "--deny-warnings"]);
    assert!(!denied.status.success());
    let err = stderr(&denied);
    assert!(
        err.contains("denied warning L1029") && err.contains("perch-a") && err.contains("buried"),
        "{err}"
    );

    let allow = "L1029:marker `perch-";
    let partly = ludion(&root, &["map-drift", "--deny-warnings", "--allow", allow]);
    assert!(!partly.status.success());
    let err = stderr(&partly);
    assert!(err.contains("buried"), "{err}");
    assert!(
        !err.contains("perch-a"),
        "the allowed warning must not print:\n{err}"
    );

    let map = root.join("scenes/perch.map.yaml");
    let text = std::fs::read_to_string(&map).unwrap();
    let kept = text.split("  - name: buried").next().unwrap();
    std::fs::write(&map, kept).unwrap();
    generate(&root, &["scenes/perch.map.yaml"]);
    let clean = ludion(&root, &["map-drift", "--deny-warnings", "--allow", allow]);
    assert!(clean.status.success(), "{}", stderr(&clean));
}

#[test]
fn a_bad_allow_is_a_usage_error() {
    let root = clean_project("map-drift-allow-usage");
    let output = ludion(&root, &["map-drift", "--deny-warnings", "--allow", "nope"]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("--allow"), "{}", stderr(&output));
}

#[test]
fn json_carries_ok_and_the_files() {
    let root = clean_project("map-drift-json");
    let output = ludion(&root, &["map-drift", "--json"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let value: serde_json::Value = serde_json::from_str(stdout(&output).trim()).unwrap();
    assert_eq!(value["ok"], true);
    assert_eq!(value["maps"], 1);
    assert!(value["files"].as_array().unwrap().is_empty());

    std::fs::remove_file(root.join("scenes/room.yaml")).unwrap();
    std::fs::remove_file(root.join("scenes/rooms/room.prefab.yaml")).unwrap();
    std::fs::remove_file(root.join("scenes/rooms/room-mx.prefab.yaml")).unwrap();
    let output = ludion(&root, &["map-drift", "--json"]);
    assert!(!output.status.success());
    let value: serde_json::Value = serde_json::from_str(stdout(&output).trim()).unwrap();
    assert_eq!(value["ok"], false);
    let files = value["files"].as_array().unwrap();
    assert!(
        files
            .iter()
            .any(|f| f["file"] == "scenes/room.yaml" && f["diagnostics"][0]["code"] == "L1041"),
        "{value}"
    );
}

#[test]
fn explain_knows_the_fault_code() {
    let output = ludion(&std::env::temp_dir(), &["explain", "L1041"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("map-drift"), "{}", stdout(&output));
}

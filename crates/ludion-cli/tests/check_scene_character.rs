//! Integration tests for `ludion check-scene` on `kind: assembly` documents
//! (`Plan/animation-clips.md`): the gallery mannequin checks clean, and a bad overlay is `L1138`, a retired `kind: character` is `L1246`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn run_check_scene(path: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ludion"))
        .arg("check-scene")
        .arg(path)
        .output()
        .expect("failed to run the ludion binary")
}

fn gallery() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery")
}

#[test]
fn the_gallery_character_checks_clean() {
    let output = run_check_scene(&gallery().join("characters/mannequin.character.yaml"));
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("OK (character mannequin"),
        "stdout was: {stdout}"
    );
    assert!(stdout.contains("4 clips"), "stdout was: {stdout}");
    assert!(
        stdout.contains("clip run: ../clips/run.anim.yaml, duration 0.8, speed 1.2, events 2"),
        "stdout was: {stdout}"
    );
    let after_run = stdout
        .split("  clip run:")
        .nth(1)
        .expect("the run clip is listed");
    let run_block = after_run.split("  clip ").next().unwrap();
    assert!(
        run_block.contains("    contact lower_leg_l: height min "),
        "stdout was: {stdout}"
    );
}

/// A skinned assembly lists its `$root` clips in mesh units and its rooted blends, in text and as
/// the `--json` `root` object; one without `$root` has neither.
#[test]
fn a_skinned_assembly_lists_its_root_motion() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/skinned_root/body.assembly.yaml");
    let json = |path: &Path| -> serde_json::Value {
        let output = Command::new(env!("CARGO_BIN_EXE_ludion"))
            .args(["check-scene", "--json"])
            .arg(path)
            .output()
            .expect("failed to run the ludion binary");
        serde_json::from_slice(&output.stdout).expect("one JSON object")
    };

    let output = run_check_scene(&fixture);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let rows = "  root lunge: travel x 0 z -1.5 units, yaw 45°, 1 s one-shot
  root walk: travel x 0 z -0.75 units, yaw 0°, 0.5 s loop
  root blend stride: 1 of 2 points carry $root (walk)
";
    assert!(stdout.contains(rows), "stdout was: {stdout}");
    assert_eq!(
        json(&fixture)["root"],
        serde_json::json!({
            "clips": [
                { "name": "lunge", "travel": [0.0, -1.5], "yaw": 45.0, "duration": 1.0,
                  "loop": false },
                { "name": "walk", "travel": [0.0, -0.75], "yaw": 0.0, "duration": 0.5,
                  "loop": true }
            ],
            "blends": [{ "name": "stride", "points": ["walk"] }]
        })
    );

    let plain = gallery().join("characters/mannequin.character.yaml");
    let stdout = String::from_utf8_lossy(&run_check_scene(&plain).stdout).into_owned();
    assert!(!stdout.contains("  root "), "{stdout}");
    let value = json(&plain);
    assert!(value.get("root").is_none(), "{value}");
}

/// An overlay event past its clip's duration is a data error at the character.
#[test]
fn a_bad_overlay_is_l1138() {
    let dir = std::env::temp_dir().join(format!("ludion-check-character-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("characters")).unwrap();
    std::fs::create_dir_all(dir.join("meshes")).unwrap();
    std::fs::create_dir_all(dir.join("clips")).unwrap();
    for file in ["meshes/skin-mannequin.mesh.yaml", "clips/run.anim.yaml"] {
        std::fs::copy(gallery().join(file), dir.join(file)).unwrap();
    }
    // A hand-authored clip with its own events makes the overlay redundant.
    let run = std::fs::read_to_string(dir.join("clips/run.anim.yaml")).unwrap();
    let own: String = run
        .lines()
        .filter(|line| !line.starts_with("generated_from"))
        .map(|line| format!("{line}\n"))
        .collect::<String>()
        .replace(
            "loop: true\n",
            "loop: true\nevents:\n  - t: 0.2\n    name: own\n",
        );
    std::fs::write(dir.join("clips/run.anim.yaml"), own).unwrap();
    let character = dir.join("characters/hero.character.yaml");
    std::fs::write(
        &character,
        "format: 1\nkind: assembly\nmesh: ../meshes/skin-mannequin.mesh.yaml\nclips:\n  run:\n    clip: ../clips/run.anim.yaml\n    events:\n      - t: 0.1\n        name: step\n",
    )
    .unwrap();

    let output = run_check_scene(&character);
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("L1138"), "stderr was: {stderr}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A clip far past the duration ceiling is refused at the document, not sampled for hours.
#[test]
fn a_huge_clip_duration_is_l1142() {
    let dir = std::env::temp_dir().join(format!(
        "ludion-check-character-huge-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("characters")).unwrap();
    std::fs::create_dir_all(dir.join("meshes")).unwrap();
    std::fs::create_dir_all(dir.join("clips")).unwrap();
    for file in ["meshes/skin-mannequin.mesh.yaml", "clips/run.anim.yaml"] {
        std::fs::copy(gallery().join(file), dir.join(file)).unwrap();
    }
    let run = std::fs::read_to_string(dir.join("clips/run.anim.yaml")).unwrap();
    assert!(run.contains("duration: 0.8"));
    std::fs::write(
        dir.join("clips/run.anim.yaml"),
        run.replace("duration: 0.8", "duration: 1000000000.0"),
    )
    .unwrap();
    let character = dir.join("characters/hero.character.yaml");
    std::fs::write(
        &character,
        "format: 1\nkind: assembly\nmesh: ../meshes/skin-mannequin.mesh.yaml\nclips:\n  run:\n    clip: ../clips/run.anim.yaml\n",
    )
    .unwrap();

    let output = run_check_scene(&character);
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("L1142"), "stderr was: {stderr}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The retired `kind: character` is `L1246`, which names the one-line fix.
#[test]
fn a_kind_character_document_is_l1246() {
    let dir = std::env::temp_dir().join(format!(
        "ludion-check-character-retired-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let character = dir.join("hero.character.yaml");
    std::fs::write(
        &character,
        "format: 1\nkind: character\nmesh: ../meshes/skin-mannequin.mesh.yaml\n",
    )
    .unwrap();

    let output = run_check_scene(&character);
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("L1246"), "stderr was: {stderr}");
    assert!(stderr.contains("kind: assembly"), "stderr was: {stderr}");
    let _ = std::fs::remove_dir_all(&dir);
}

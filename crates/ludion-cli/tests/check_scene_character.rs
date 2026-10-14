//! Integration tests for `ludion check-scene` on `kind: character` documents
//! (`Plan/animation-clips.md`): the gallery mannequin checks clean, and a bad overlay is `L1200`.

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

/// An overlay event past its clip's duration is a data error at the character.
#[test]
fn a_bad_overlay_is_l1200() {
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
        "format: 1\nkind: character\nmesh: ../meshes/skin-mannequin.mesh.yaml\nclips:\n  run:\n    clip: ../clips/run.anim.yaml\n    events:\n      - t: 0.1\n        name: step\n",
    )
    .unwrap();

    let output = run_check_scene(&character);
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("L1200"), "stderr was: {stderr}");
    let _ = std::fs::remove_dir_all(&dir);
}

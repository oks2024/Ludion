//! `ludion check-scene --json` on voxel and assembly documents: the summary object the editor's
//! model readout parses. Driven against the built `ludion` binary; every expected value is read off
//! the fixture's own source.

use std::path::Path;
use std::process::{Command, Output};

use serde_json::{Value, json};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ludion"))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .arg("check-scene")
        .args(args)
        .output()
        .expect("failed to run the ludion binary")
}

/// Stdout must be exactly one JSON line; returns it parsed.
fn one_line(output: &Output) -> Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.lines().count(), 1, "stdout was: {stdout}");
    serde_json::from_str(&stdout).expect("stdout is one JSON object")
}

const VOXEL: &str = "tests/fixtures/good_voxel.voxel.yaml";
const ASSEMBLY: &str = "tests/fixtures/assembly/good.assembly.yaml";

/// `good_voxel.voxel.yaml`: size [2,2,1]; slices `AB` / `.A` hold 3 filled cells in one component;
/// named `gem`, no scale. The 14/28/56 meshed counts are the ones `check_scene_voxel.rs` pins.
#[test]
fn voxel_summary_object() {
    let output = run(&[VOXEL, "--json"]);
    assert!(output.status.success());
    assert_eq!(
        one_line(&output),
        json!({
            "path": VOXEL,
            "kind": "voxel",
            "ok": true,
            "name": "gem",
            "size": [2, 2, 1],
            "filled": 3,
            "components": 1,
            "meshed": {"quads": 14, "triangles": 28, "vertices": 56},
            "scale": null,
            "generated_from": null,
            "budget_exempt": false,
            "diagnostics": [],
        })
    );
}

/// `good.assembly.yaml`: size [4,3,2]; parts `body` (root, at origin) and `arm` (child of `body`,
/// at (3,1,1), mirror x); one clip `walk`. The arm sits apart from the body, so the composed grid
/// has 2 components and raises `L1129`, which stays a warning.
#[test]
fn assembly_summary_object_carries_the_warning() {
    let output = run(&[ASSEMBLY, "--json"]);
    assert!(output.status.success());
    let value = one_line(&output);
    assert_eq!(value["kind"], "assembly");
    assert_eq!(value["ok"], true);
    assert_eq!(value["name"], "fixture");
    assert_eq!(value["size"], json!([4, 3, 2]));
    assert_eq!(value["components"], 2);
    let parts = value["parts"].as_array().unwrap();
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0]["name"], "body");
    assert_eq!(parts[0]["file"], "parts/body.part.yaml");
    assert_eq!(parts[0]["parent"], Value::Null);
    assert_eq!(parts[0]["mirror_x"], false);
    assert_eq!(parts[1]["name"], "arm");
    assert_eq!(parts[1]["parent"], "body");
    assert_eq!(parts[1]["place"], json!([3, 1, 1]));
    assert_eq!(parts[1]["mirror_x"], true);
    assert_eq!(
        value["clips"],
        json!([{"name": "walk", "file": "anims/walk.anim.yaml"}])
    );
    let diagnostics = value["diagnostics"].as_array().unwrap();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0]["code"], "L1129");
    assert_eq!(diagnostics[0]["severity"], "warning");
}

/// `broken_voxel.voxel.yaml` is refused with `L1114` (two layers short, an unknown glyph); the
/// failure object carries the structured diagnostics and no kind fields.
#[test]
fn coded_failure_prints_the_failure_object() {
    let output = run(&["tests/fixtures/broken_voxel.voxel.yaml", "--json"]);
    assert_eq!(output.status.code(), Some(1));
    let value = one_line(&output);
    assert_eq!(value["ok"], false);
    assert_eq!(value["kind"], "voxel");
    assert!(value.get("size").is_none());
    let first = &value["diagnostics"][0];
    assert_eq!(first["code"], "L1114");
    assert!(first.get("severity").is_none(), "an error omits severity");
    assert!(first["path"].as_str().unwrap().starts_with("/slices"));
}

#[test]
fn json_with_a_text_view_is_a_usage_error() {
    let output = run(&[VOXEL, "--json", "--slice", "0"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("text views"));
}

/// `--json --bounds` stays legal only on a scene or a mesh; its bare shapes are pinned elsewhere.
#[test]
fn json_bounds_on_another_kind_is_still_refused() {
    let output = run(&[
        "tests/fixtures/sprite/good.sprite.yaml",
        "--json",
        "--bounds",
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--json applies only"));
}

/// One fixture per kind, `--json` alone: the kind, `ok`, and one count read off the fixture's source.
#[test]
fn every_kind_prints_its_summary_object() {
    let cases: [(&str, &str, &str, Value); 12] = [
        // good.yaml: entities `root` and `child`.
        ("tests/fixtures/good.yaml", "scene", "entities", json!(2)),
        // One triangle: three positions' worth of vertices.
        (
            "tests/fixtures/good_mesh.mesh.yaml",
            "mesh",
            "vertices",
            json!(3),
        ),
        // Slices `AB` / `.A`: three filled cells.
        (
            "tests/fixtures/good_part.part.yaml",
            "part",
            "filled",
            json!(3),
        ),
        // One rotation track of three keys.
        (
            "tests/fixtures/good_animation.anim.yaml",
            "animation",
            "duration",
            json!(0.6),
        ),
        // Frames `idle` and `spent`, palette glyphs `#` and `o`.
        (
            "tests/fixtures/sprite/good.sprite.yaml",
            "sprite",
            "palette_glyphs",
            json!(2),
        ),
        (
            "tests/fixtures/sprite/shared.palette.yaml",
            "palette",
            "glyphs",
            json!(2),
        ),
        // One flipbook, one input.
        (
            "tests/fixtures/sprite/good.atlas-source.yaml",
            "atlas-source",
            "flipbooks",
            json!(1),
        ),
        // Regions `bolt/idle` and `bolt/spent`.
        (
            "tests/fixtures/sprite/ship.atlas.yaml",
            "atlas",
            "regions",
            json!(2),
        ),
        // `charset.chars: "AVI"`.
        (
            "tests/fixtures/font/small.font-source.yaml",
            "font-source",
            "chars",
            json!(3),
        ),
        // The gallery face's `atlas:` block.
        (
            "../../examples/gallery/fonts/inter.font.yaml",
            "font",
            "atlas",
            json!([720, 1028]),
        ),
        // Entities `root` and `arm`: Transform; Parent, Transform, Collider.
        (
            "tests/fixtures/prefabs/bar.prefab.yaml",
            "prefab",
            "components",
            json!(4),
        ),
        // The gallery mannequin binds idle, jump, run and spin.
        (
            "../../examples/gallery/characters/mannequin.character.yaml",
            "character",
            "name",
            json!("mannequin"),
        ),
    ];
    for (path, kind, key, expected) in cases {
        let output = run(&[path, "--json"]);
        assert!(output.status.success(), "{path}: {output:?}");
        let value = one_line(&output);
        assert_eq!(value["path"], path);
        assert_eq!(value["kind"], kind, "{path}");
        assert_eq!(value["ok"], true, "{path}");
        assert_eq!(value[key], expected, "{path}");
        assert_eq!(value["diagnostics"], json!([]), "{path}");
    }
}

#[test]
fn the_character_summary_lists_its_clips() {
    let output = run(&[
        "../../examples/gallery/characters/mannequin.character.yaml",
        "--json",
    ]);
    let clips = one_line(&output)["clips"].clone();
    let names: Vec<&str> = clips
        .as_array()
        .unwrap()
        .iter()
        .map(|clip| clip["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["idle", "jump", "run", "spin"]);
    assert_eq!(clips[2]["speed"], 1.2);
    assert_eq!(clips[2]["events"], 2);
    assert!(clips[2]["contacts"].is_array());
}

#[test]
fn sprite_frames_carry_their_pivots() {
    let output = run(&["tests/fixtures/sprite/good.sprite.yaml", "--json"]);
    assert_eq!(
        one_line(&output)["frames"],
        json!([{"name": "idle", "pivot": [2.0, 2.0]}, {"name": "spent", "pivot": [2.0, 3.0]}])
    );
}

/// A kind with a checker but no summary of its own prints the envelope and nothing else.
#[test]
fn a_bare_checked_kind_prints_the_envelope_only() {
    let output = run(&["tests/fixtures/gen/idol.gen.yaml", "--json"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        one_line(&output),
        json!({
            "path": "tests/fixtures/gen/idol.gen.yaml",
            "kind": "generation",
            "ok": true,
            "diagnostics": [],
        })
    );
}

/// A map is checked by `map-check`: `L0031`, pointed at `/kind`.
#[test]
fn a_kind_checked_elsewhere_prints_l0031() {
    let output = run(&["tests/fixtures/district.map.yaml", "--json"]);
    assert_eq!(output.status.code(), Some(1));
    let value = one_line(&output);
    assert_eq!(value["ok"], false);
    assert_eq!(value["diagnostics"][0]["code"], "L0031");
    assert_eq!(value["diagnostics"][0]["path"], "/kind");
}

#[test]
fn an_unreadable_file_prints_a_null_kind() {
    let output = run(&["tests/fixtures/no-such-file.yaml", "--json"]);
    assert_eq!(output.status.code(), Some(1));
    let value = one_line(&output);
    assert_eq!(value["kind"], Value::Null);
    assert_eq!(value["ok"], false);
    assert_eq!(value["diagnostics"][0]["code"], "L1015");
}

#[test]
fn an_unrecognized_kind_prints_its_diagnostic() {
    let output = run(&["tests/fixtures/unknown_kind.yaml", "--json"]);
    assert_eq!(output.status.code(), Some(1));
    let value = one_line(&output);
    assert_eq!(value["ok"], false);
    assert_eq!(value["kind"], "tilemap");
    assert_eq!(value["diagnostics"][0]["code"], "L1119");
    assert_eq!(value["diagnostics"][0]["path"], "/kind");
}

/// A malformed `fonts.yaml` is a fault of the content root, reported against that file; the scene
/// summary is still the failure object.
#[test]
fn a_font_table_fault_prints_ok_false() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("check_scene_json_fonts");
    std::fs::create_dir_all(&dir).expect("scratch dir");
    std::fs::write(
        dir.join("fonts.yaml"),
        "format: 1
kind: fonts
fonts: 3
",
    )
    .unwrap();
    let scene = dir.join("scene.yaml");
    std::fs::write(
        &scene,
        "format: 1
entities:
  - name: label
    components:
      UiText:
        text: HI
",
    )
    .unwrap();
    let output = run(&[scene.to_str().unwrap(), "--json"]);
    assert_eq!(output.status.code(), Some(1));
    let value = one_line(&output);
    assert_eq!(value["ok"], false);
    assert_eq!(value["diagnostics"][0]["code"], "L1253");
}

/// Checks a scene with one `AudioEmitter` naming `sound` in a scratch root holding `bad.wav`, which
/// is not a WAV; returns the `--json` summary.
fn audio_emitter_summary(name: &str, sound: &str) -> Value {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    std::fs::write(dir.join("bad.wav"), b"not a wav").unwrap();
    let scene = dir.join("scene.yaml");
    std::fs::write(
        &scene,
        format!(
            "format: 1\nentities:\n  - name: lamp\n    components:\n      AudioEmitter:\n        \
             sound: {sound}\n"
        ),
    )
    .unwrap();
    let output = run(&[scene.to_str().unwrap(), "--json"]);
    assert_eq!(output.status.code(), Some(1));
    one_line(&output)
}

#[test]
fn a_missing_emitter_sound_prints_l1101() {
    let value = audio_emitter_summary("check_scene_json_sound_missing", "gone.wav");
    assert_eq!(value["ok"], false);
    assert_eq!(value["diagnostics"][0]["code"], "L1101");
    assert!(
        value["diagnostics"][0]["path"]
            .as_str()
            .is_some_and(|p| p.ends_with("/components/AudioEmitter/sound")),
        "{value}"
    );
}

#[test]
fn a_non_wav_emitter_sound_prints_l1104() {
    let value = audio_emitter_summary("check_scene_json_sound_bad", "bad.wav");
    assert_eq!(value["ok"], false);
    assert_eq!(value["diagnostics"][0]["code"], "L1104");
}

#[test]
fn placement_adds_a_field_and_no_text_line() {
    let output = run(&["tests/fixtures/good.yaml", "--json", "--placement"]);
    assert!(output.status.success());
    let value = one_line(&output);
    assert_eq!(value["placement"], json!({"groups": 0, "unmeasured": 0}));
}

/// The sample the editor's readout test parses is this binary's own output: a drift on either side
/// fails here or there.
#[test]
fn editor_samples_match_the_cli() {
    for (args, sample) in [
        (VOXEL, "check-scene-voxel.json"),
        (ASSEMBLY, "check-scene-assembly.json"),
    ] {
        let output = run(&[args, "--json"]);
        let sample_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../editor/app/src/editor/voxel/__fixtures__")
            .join(sample);
        let expected: Value = serde_json::from_str(
            &std::fs::read_to_string(&sample_path).expect("the editor sample exists"),
        )
        .expect("the sample is JSON");
        assert_eq!(
            one_line(&output),
            expected,
            "the CLI's --json shape moved: update the sample and readout.ts together"
        );
    }
}

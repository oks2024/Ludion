//! Integration tests for `ludion texture build | check | vendor`, driven against the built binary.
//! CPU only: no verb opens a device.
//!
//! The texgen crate pins the output bytes; these cover the CLI surface — the committed example is
//! up to date, `check` catches a touched, stray or missing file, the validators report in text and
//! `--json` and a rules file moves their bounds, and `vendor` copies without overwriting.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

fn example() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/textures/tiles.texture.yaml")
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("texture-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run_ludion(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ludion"))
        .args(args)
        .output()
        .expect("failed to run the ludion binary")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).expect("--json emits one JSON object")
}

fn text(path: &Path) -> &str {
    path.to_str().unwrap()
}

/// Copies the example recipe into a fresh directory and builds it at 64 texels.
fn built(name: &str) -> (PathBuf, PathBuf) {
    let dir = scratch(name);
    let recipe = dir.join("tiles.texture.yaml");
    std::fs::copy(example(), &recipe).unwrap();
    let output = run_ludion(&["texture", "build", text(&recipe), "--size", "64"]);
    assert!(output.status.success(), "build failed: {}", stderr(&output));
    (recipe, dir.join("tiles"))
}

#[test]
fn the_committed_example_is_up_to_date() {
    let output = run_ludion(&["texture", "check", text(&example())]);
    assert!(output.status.success(), "{}", stderr(&output));
}

#[test]
fn a_fresh_build_checks_clean_and_each_kind_of_drift_is_l1286() {
    let (recipe, out) = built("drift");
    let check = |extra: &[&str]| {
        let mut args = vec!["texture", "check", text(&recipe)];
        args.extend_from_slice(extra);
        run_ludion(&args)
    };
    assert!(check(&[]).status.success());

    let height = out.join("height.png");
    let mut bytes = std::fs::read(&height).unwrap();
    let last = bytes.len() / 2;
    bytes[last] ^= 0xff;
    std::fs::write(&height, &bytes).unwrap();
    let output = check(&[]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("L1286") && stderr(&output).contains("height.png"));

    let (recipe, out) = built("stray");
    std::fs::write(out.join("extra.png"), b"x").unwrap();
    let output = run_ludion(&["texture", "check", text(&recipe)]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("L1286") && stderr(&output).contains("extra.png"));

    let (recipe, out) = built("never");
    std::fs::remove_file(out.join("generated.json")).unwrap();
    let output = run_ludion(&["texture", "check", text(&recipe)]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("L1286") && stderr(&output).contains("never built"));
}

#[test]
fn json_reports_steps_maps_and_validators() {
    let dir = scratch("json");
    let recipe = dir.join("tiles.texture.yaml");
    std::fs::copy(example(), &recipe).unwrap();
    let output = run_ludion(&["texture", "build", text(&recipe), "--size", "64", "--json"]);
    assert!(output.status.success());
    let report = json(&output);
    assert_eq!(report["ok"], true);
    assert_eq!(report["size"], 64);
    assert!(!report["steps"].as_array().unwrap().is_empty());
    assert!(report["maps"]["height"]["path"].is_string());
    assert_eq!(report["validators"].as_array().unwrap().len(), 9);
    assert!(report["written"].as_array().unwrap().len() >= 6);

    let output = run_ludion(&["texture", "check", text(&recipe), "--json"]);
    let report = json(&output);
    assert_eq!(report["ok"], true);
    assert_eq!(report["drift"].as_array().unwrap().len(), 0);
}

fn bench40() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/texture/bench40.texture.yaml")
}

#[test]
fn preview_without_render_writes_the_sheets() {
    let dir = scratch("preview");
    let recipe = dir.join("tiles.texture.yaml");
    std::fs::copy(example(), &recipe).unwrap();
    let output = run_ludion(&[
        "texture",
        "build",
        text(&recipe),
        "--size",
        "64",
        "--preview",
        "--no-render",
        "--json",
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    let report = json(&output);
    let feedback = &report["feedback"];
    let default_dir = dir.join(".ludion/texture/tiles");
    assert_eq!(
        feedback["dir"],
        default_dir.to_str().unwrap().replace('\\', "/")
    );
    assert!(feedback["preview"].is_null());
    assert_eq!(feedback["preview_skipped"], "--no-render");
    for file in [
        "maps.png",
        "steps.png",
        "tiling_base_color.png",
        "tiling_height.png",
    ] {
        assert!(default_dir.join(file).is_file(), "{file}");
    }
    assert!(feedback["tiling"]["height"].is_string());
    assert!(feedback["ms"]["sheets"].is_number());
    assert!(!default_dir.join("preview.png").exists());
    let output = run_ludion(&["texture", "check", text(&recipe)]);
    assert!(output.status.success(), "{}", stderr(&output));
}

#[test]
fn preview_flags_need_preview_and_out_overrides_the_folder() {
    let (recipe, _) = built("preview-flags");
    assert!(
        !run_ludion(&["texture", "build", text(&recipe), "--no-render"])
            .status
            .success()
    );
    let out = recipe.parent().unwrap().join("elsewhere");
    let output = run_ludion(&[
        "texture",
        "build",
        text(&recipe),
        "--size",
        "64",
        "--preview",
        "--no-render",
        "--preview-out",
        text(&out),
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(out.join("maps.png").is_file());
}

#[test]
fn the_forty_step_fixture_builds_with_feedback() {
    let dir = scratch("bench40");
    let recipe = dir.join("bench40.texture.yaml");
    std::fs::copy(bench40(), &recipe).unwrap();
    let output = run_ludion(&[
        "texture",
        "build",
        text(&recipe),
        "--size",
        "64",
        "--preview",
        "--no-render",
        "--json",
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    let report = json(&output);
    assert!(report["steps"].as_array().unwrap().len() >= 35);
    assert!(dir.join(".ludion/texture/bench40/steps.png").is_file());
}

#[test]
fn needs_gpu_preview_renders_the_three_shapes() {
    let dir = scratch("preview-render");
    let recipe = dir.join("tiles.texture.yaml");
    std::fs::copy(example(), &recipe).unwrap();
    let output = run_ludion(&[
        "texture",
        "build",
        text(&recipe),
        "--size",
        "64",
        "--preview",
        "--json",
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    let report = json(&output);
    if report["feedback"]["preview_skipped"].is_string()
        && ludion_core::require_gpu::skip_no_adapter(true)
    {
        return;
    }
    let preview = dir.join(".ludion/texture/tiles/preview.png");
    let (w, h, rgba) = ludion_assets::decode_png(
        &std::fs::read(&preview).expect("preview.png"),
        "preview.png",
    )
    .unwrap();
    assert!(w > 0 && h > 0);
    let first = &rgba[..4];
    assert!(rgba.chunks(4).any(|p| p != first), "preview.png is uniform");
}

#[test]
fn rules_move_bounds_and_levels_and_unknown_names_are_l1289() {
    let (recipe, _) = built("rules");
    let dir = recipe.parent().unwrap();
    let strict = dir.join("strict.toml");
    std::fs::write(&strict, "seam_height = { max = 0.0, level = \"error\" }\n").unwrap();
    let output = run_ludion(&["texture", "check", text(&recipe), "--rules", text(&strict)]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("L1288"), "{}", stderr(&output));

    let relaxed = dir.join("relaxed.toml");
    std::fs::write(
        &relaxed,
        "seam_height = { max = 0.0, level = \"error\" }\n\
         [textures.\"tiles.texture.yaml\"]\n\
         seam_height = { max = 100.0 }\n",
    )
    .unwrap();
    let output = run_ludion(&["texture", "check", text(&recipe), "--rules", text(&relaxed)]);
    assert!(output.status.success(), "{}", stderr(&output));

    let warn = dir.join("warn.toml");
    std::fs::write(&warn, "seam_height = { max = 0.0 }\n").unwrap();
    let output = run_ludion(&["texture", "check", text(&recipe), "--rules", text(&warn)]);
    assert!(output.status.success());
    assert!(stderr(&output).contains("L1287"));

    let unknown = dir.join("unknown.toml");
    std::fs::write(&unknown, "seam_heigth = { max = 1.0 }\n").unwrap();
    let output = run_ludion(&["texture", "check", text(&recipe), "--rules", text(&unknown)]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("L1289") && stderr(&output).contains("seam_height"));
}

#[test]
fn vendor_writes_the_embedded_recipe_and_refuses_a_second_run() {
    let dir = scratch("vendor");
    let dest = dir.join("cobbles.texture.yaml");
    let output = run_ludion(&["texture", "vendor", "std:cobbles", text(&dest)]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        std::fs::read_to_string(&dest).unwrap(),
        ludion_texgen::stdlib::get("cobbles").unwrap()
    );
    assert!(
        !run_ludion(&["texture", "vendor", "cobbles", text(&dest)])
            .status
            .success()
    );

    let output = run_ludion(&["texture", "vendor", "std:cobles", text(&dir.join("x.yaml"))]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("L1285") && stderr(&output).contains("std:cobbles"));
}

#[test]
fn check_scene_points_a_texture_recipe_at_texture_check() {
    let output = run_ludion(&["check-scene", text(&example()), "--json"]);
    assert!(!output.status.success());
    let report = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(report.contains("L0031"), "{report}");
    assert!(report.contains("ludion texture check"), "{report}");
}

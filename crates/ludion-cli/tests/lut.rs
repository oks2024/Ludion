//! Integration tests for `ludion lut`, driven against the built binary: the four verbs'
//! round trips, their refusals, and the `--json` contract. Scratch under `CARGO_TARGET_TMPDIR`.

use std::path::{Path, PathBuf};
use std::process::{Command as ProcCommand, Output};

use ludion_assets::cube::{CubeLut, identity, parse_cube};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lut/swatch.png")
}

/// A stable (not per-process) scratch dir, cleared each run — same rationale as `tests/map.rs`.
fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    if let Err(e) = std::fs::remove_dir_all(&dir) {
        assert!(
            e.kind() == std::io::ErrorKind::NotFound,
            "stale scratch dir {dir:?} could not be removed: {e}"
        );
    }
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run_ludion(args: &[&str]) -> Output {
    ProcCommand::new(env!("CARGO_BIN_EXE_ludion"))
        .args(args)
        .output()
        .expect("failed to run the ludion binary")
}

fn s(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn read_lut(path: &Path) -> CubeLut {
    let text = std::fs::read_to_string(path).unwrap();
    parse_cube(&text, "t.cube", 0).unwrap()
}

fn scene_with(post: &str) -> String {
    format!(
        "format: 1\nname: lut test\nentities:\n  - name: grade\n    components:\n      PostProcess:\n{post}"
    )
}

fn one_json(output: &Output) -> serde_json::Value {
    let out = stdout(output);
    assert_eq!(out.trim_end().lines().count(), 1, "one stdout line: {out}");
    serde_json::from_str(out.trim_end()).expect("stdout is one JSON value")
}

/// A `.cube` that swaps red and blue.
fn swap_cube(path: &Path) {
    let mut lut = identity(17);
    for row in &mut lut.rgb {
        row.swap(0, 2);
    }
    std::fs::write(path, lut.to_text()).unwrap();
}

#[test]
fn identity_writes_a_33_cube_that_reparses() {
    let dir = scratch("lut-identity");
    let out = dir.join("id.cube");
    let output = run_ludion(&["lut", "identity", "--out", s(&out)]);
    assert!(output.status.success(), "{}", stderr(&output));
    let lut = read_lut(&out);
    assert_eq!((lut.size, lut.rgb.len()), (33, 33 * 33 * 33));
    assert_eq!(lut.rgb, identity(33).rgb);
}

#[test]
fn a_size_outside_17_33_65_is_l2391() {
    let dir = scratch("lut-size");
    let out = dir.join("x.cube");
    let output = run_ludion(&["lut", "identity", "--size", "20", "--out", s(&out)]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("L2391"), "{}", stderr(&output));
    assert!(!out.exists());
}

#[test]
fn baking_a_default_post_process_is_the_identity() {
    let dir = scratch("lut-bake-identity");
    let scene = dir.join("s.yaml");
    std::fs::write(&scene, scene_with("        exposure: 1.3\n")).unwrap();
    let out = dir.join("b.cube");
    let output = run_ludion(&["lut", "bake", "--scene", s(&scene), "--out", s(&out)]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(read_lut(&out).rgb, identity(33).rgb);
}

#[test]
fn baking_zero_saturation_gives_equal_channels() {
    let dir = scratch("lut-bake-grey");
    let scene = dir.join("s.yaml");
    std::fs::write(&scene, scene_with("        saturation: 0.0\n")).unwrap();
    let out = dir.join("b.cube");
    let output = run_ludion(&[
        "lut",
        "bake",
        "--scene",
        s(&scene),
        "--out",
        s(&out),
        "--size",
        "17",
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    let lut = read_lut(&out);
    assert_eq!(lut.size, 17);
    for row in &lut.rgb {
        assert_eq!(row[0], row[1]);
        assert_eq!(row[1], row[2]);
    }
    assert!(stderr(&output).contains("reset"), "{}", stderr(&output));
}

#[test]
fn applying_the_identity_leaves_the_image_byte_identical() {
    let dir = scratch("lut-apply-identity");
    let lut = dir.join("id.cube");
    let out = dir.join("out.png");
    assert!(
        run_ludion(&["lut", "identity", "--out", s(&lut)])
            .status
            .success()
    );
    let output = run_ludion(&[
        "lut",
        "apply",
        "--lut",
        s(&lut),
        "--image",
        s(&fixture()),
        "--out",
        s(&out),
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    let (a, b) = (
        ludion_assets::load_png(
            ludion_assets::AssetRoot::content(fixture().parent().unwrap()),
            "swatch.png",
        )
        .unwrap(),
        ludion_assets::load_png(ludion_assets::AssetRoot::content(&dir), "out.png").unwrap(),
    );
    assert_eq!(a.rgba, b.rgba);
}

#[test]
fn applying_a_channel_swap_swaps_red_and_blue() {
    let dir = scratch("lut-apply-swap");
    let lut = dir.join("swap.cube");
    swap_cube(&lut);
    let out = dir.join("out.png");
    let output = run_ludion(&[
        "lut",
        "apply",
        "--lut",
        s(&lut),
        "--image",
        s(&fixture()),
        "--out",
        s(&out),
        "--json",
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(one_json(&output)["ok"], true);
    let src_dir = fixture().parent().unwrap().to_path_buf();
    let a =
        ludion_assets::load_png(ludion_assets::AssetRoot::content(&src_dir), "swatch.png").unwrap();
    let b = ludion_assets::load_png(ludion_assets::AssetRoot::content(&dir), "out.png").unwrap();
    for (src, dst) in a.rgba.chunks_exact(4).zip(b.rgba.chunks_exact(4)) {
        assert_eq!(
            (src[1], src[3]),
            (dst[1], dst[3]),
            "green and alpha are kept"
        );
        assert!(
            src[0].abs_diff(dst[2]) <= 1 && src[2].abs_diff(dst[0]) <= 1,
            "{src:?} {dst:?}"
        );
    }
}

#[test]
fn stats_of_the_identity_reports_zero_deltas() {
    let dir = scratch("lut-stats-identity");
    let lut = dir.join("id.cube");
    assert!(
        run_ludion(&["lut", "identity", "--out", s(&lut)])
            .status
            .success()
    );
    let output = run_ludion(&["lut", "stats", s(&lut), "--json"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let json = one_json(&output);
    let bands = json["bands"].as_array().unwrap();
    assert_eq!(bands.len(), 6);
    for band in bands {
        for key in ["d_sat", "d_light"] {
            assert!(band[key].as_f64().unwrap().abs() < 1e-4, "{band}");
        }
        assert!(
            band["d_hue"].is_null() || band["d_hue"].as_f64().unwrap().abs() < 0.01,
            "{band}"
        );
    }
}

#[test]
fn stats_of_a_channel_swap_moves_ambers_hue() {
    let dir = scratch("lut-stats-swap");
    let lut = dir.join("swap.cube");
    swap_cube(&lut);
    let text = run_ludion(&["lut", "stats", s(&lut)]);
    assert!(text.status.success(), "{}", stderr(&text));
    assert!(stdout(&text).contains("amber"), "{}", stdout(&text));
    let json = one_json(&run_ludion(&["lut", "stats", s(&lut), "--json"]));
    let amber = json["bands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["band"] == "amber")
        .unwrap();
    assert!(amber["d_hue"].as_f64().unwrap().abs() > 30.0, "{amber}");
}

#[test]
fn json_failures_are_one_object_with_ok_false() {
    let dir = scratch("lut-json-fail");
    let output = run_ludion(&["lut", "stats", s(&dir.join("missing.cube")), "--json"]);
    assert!(!output.status.success());
    let json = one_json(&output);
    assert_eq!(json["ok"], false);
    assert!(
        json["diagnostics"]
            .as_array()
            .is_some_and(|d| !d.is_empty())
    );
}

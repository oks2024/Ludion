//! `ludion check-scene` on `kind: paired` documents (`Plan/paired-animations.md` W4): the role,
//! align, event and contact report, and the structural, cross-file and gap diagnostics.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn run_check_scene(path: &Path, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ludion"))
        .arg("check-scene")
        .arg(path)
        .args(extra)
        .output()
        .expect("failed to run the ludion binary")
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("paired")
        .join(name)
}

fn gallery() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery/voxels/choke.paired.yaml")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn the_gallery_document_prints_its_report() {
    let output = run_check_scene(&gallery(), &[]);
    assert!(output.status.success(), "stderr: {}", text(&output.stderr));
    let stdout = text(&output.stdout);
    for expected in [
        "OK (paired choke, 2 roles, 3 events, 1 contact)",
        "role instigator: assembly humanoid.assembly.yaml, clip carry (1 s), scale 0.125, reference",
        "role target: assembly humanoid.assembly.yaml, clip idle (1.2 s), scale 0.125 | at [0, 0, -0.5] m yaw 180\u{b0}, 0.500 m from instigator",
        "align: 12 ticks, moves instigator",
        "event grab @ 0.25",
        "contact instigator/lower-arm-r -> target/head @ hold (t 0.5): gap ",
        "max 8",
    ] {
        assert!(
            stdout.contains(expected),
            "missing `{expected}` in:\n{stdout}"
        );
    }
}

#[test]
fn json_carries_the_role_and_contact_fields() {
    let output = run_check_scene(&gallery(), &["--json"]);
    assert!(output.status.success(), "stderr: {}", text(&output.stderr));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("one JSON object");
    assert_eq!(value["kind"], "paired");
    assert_eq!(value["name"], "choke");
    assert_eq!(value["roles"][1]["name"], "target");
    assert_eq!(value["roles"][1]["clip"], "idle");
    assert_eq!(value["roles"][1]["offset"][2], -0.5);
    assert_eq!(value["roles"][0]["reference"], true);
    assert_eq!(value["align"]["ticks"], 12);
    assert_eq!(value["events"][0]["name"], "grab");
    assert_eq!(value["contacts"][0]["at"], "hold");
    assert!(value["contacts"][0]["gap_cells"].is_number());
}

#[test]
fn a_good_rest_gap_reports_it_exactly() {
    let output = run_check_scene(&fixture("good.paired.yaml"), &[]);
    assert!(output.status.success(), "stderr: {}", text(&output.stderr));
    assert!(text(&output.stdout).contains("gap 3.000 cells (3.0000 m), max 10"));
}

#[test]
fn a_data_error_fails_with_the_pointer() {
    let output = run_check_scene(&fixture("bad-data.paired.yaml"), &[]);
    assert!(!output.status.success());
    let stderr = text(&output.stderr);
    assert!(stderr.contains("L1753"), "{stderr}");
    assert!(stderr.contains("/align/ticks"), "{stderr}");
}

#[test]
fn a_role_mismatch_fails_with_the_nearest_clip() {
    let output = run_check_scene(&fixture("bad-clip.paired.yaml"), &[]);
    assert!(!output.status.success());
    let stderr = text(&output.stderr);
    assert!(stderr.contains("L1754"), "{stderr}");
    assert!(stderr.contains("/roles/a/clip"), "{stderr}");
    assert!(stderr.contains("carry"), "{stderr}");
}

#[test]
fn an_over_gap_contact_warns_and_exits_zero() {
    let output = run_check_scene(&fixture("over-gap.paired.yaml"), &[]);
    assert!(output.status.success(), "stderr: {}", text(&output.stderr));
    assert!(text(&output.stderr).contains("L1755"));
    assert!(text(&output.stdout).contains("OK (paired over-gap"));
}

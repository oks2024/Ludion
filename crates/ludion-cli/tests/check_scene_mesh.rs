//! Integration tests for `ludion check-scene`'s mesh-document path (Phase 8 Wave 1): dispatch on
//! the top-level `kind:` key, driven against the built `ludion` binary — the mesh analog of
//! `check_scene.rs`.

use std::path::Path;
use std::process::{Command, Output};

fn run_check_scene(path: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ludion"))
        .arg("check-scene")
        .arg(path)
        .output()
        .expect("failed to run the ludion binary")
}

fn run_check_scene_args(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ludion"))
        .arg("check-scene")
        .args(args)
        .output()
        .expect("failed to run the ludion binary")
}

fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

/// A valid mesh document checks clean: zero exit and a one-line summary with vertex/triangle
/// counts.
#[test]
fn good_mesh_succeeds() {
    let output = run_check_scene(&fixture("good_mesh.mesh.yaml"));
    assert!(
        output.status.success(),
        "expected success, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("OK"), "stdout was: {stdout}");
    assert!(stdout.contains("3 vertices"), "stdout was: {stdout}");
    assert!(stdout.contains("1 triangles"), "stdout was: {stdout}");
}

/// A mesh document with inconsistent data (a truncated `normals` list, an out-of-range index)
/// exits non-zero and reports `L1111` for each defect, with a JSON Pointer to the exact list.
#[test]
fn broken_mesh_reports_l1111() {
    let output = run_check_scene(&fixture("broken_mesh.mesh.yaml"));
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(stderr.contains("error[L1111]"), "stderr was: {stderr}");
    assert!(stderr.contains("/normals"), "stderr was: {stderr}");
    assert!(stderr.contains("/indices/2"), "stderr was: {stderr}");
    assert!(stderr.contains("2 errors found"), "stderr was: {stderr}");
}

/// A document with a `kind:` the format does not define (not `mesh`, and not absent for a scene)
/// is a readable `L1119` error, not a crash or a silent scene-parse attempt.
#[test]
fn unrecognized_kind_is_l1119() {
    let output = run_check_scene(&fixture("unknown_kind.yaml"));
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L1119]"), "stderr was: {stderr}");
    assert!(stderr.contains("tilemap"), "stderr was: {stderr}");
    assert!(stderr.contains("/kind"), "stderr was: {stderr}");
}

/// A near-miss `kind:` names the nearest engine kind rather than listing them all.
#[test]
fn a_misspelled_kind_suggests_the_nearest() {
    let output = run_check_scene(&fixture("typo_kind.yaml"));
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L1119]"), "stderr was: {stderr}");
    assert!(
        stderr.contains("nearest kind is `mesh`"),
        "stderr was: {stderr}"
    );
}

/// A kind `ludion-scene` checks but `check-scene` has no rich summary for is validated through the
/// kind table — here the committed generation recipe.
#[test]
fn a_committed_generation_recipe_checks_ok() {
    let recipe = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/idol-generation/generation/recipes/idol-portrait.gen.yaml");
    assert!(recipe.is_file(), "{}", recipe.display());
    let output = run_check_scene(&recipe);
    assert!(
        output.status.success(),
        "expected success, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("OK (generation)"), "stdout was: {stdout}");
}

/// A kind another verb owns is a usage error naming that verb, not an "unrecognized kind".
#[test]
fn a_kind_checked_elsewhere_names_its_verb() {
    let output = run_check_scene(&fixture("district.map.yaml"));
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("generate-map"), "stderr was: {stderr}");
    assert!(!stderr.contains("L1119"), "stderr was: {stderr}");
}

/// Scene documents (no `kind:` key at all) still dispatch to the scene path unaffected — the
/// pre-existing `check-scene` behavior for `good.yaml` (see `check_scene.rs`).
#[test]
fn scene_documents_are_unaffected_by_mesh_dispatch() {
    let output = run_check_scene(&fixture("good.yaml"));
    assert!(
        output.status.success(),
        "expected success, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("2 entities"), "stdout was: {stdout}");
}

/// Writes a mesh document just over the 10,000-vertex style budget (`L1112`) to `path`.
fn write_over_budget_mesh(path: &Path) {
    let mut source = String::from("format: 1\nkind: mesh\npositions:\n");
    for _ in 0..10_001 {
        source.push_str("  - 0.0\n  - 0.0\n  - 0.0\n");
    }
    source.push_str("indices:\n  - 0\n  - 1\n  - 2\n");
    std::fs::write(path, &source).expect("failed to write the temp fixture");
}

/// A mesh above the 10,000-vertex style budget still checks clean (`L1112` is a warning, not a
/// validation failure) but prints the warning to stderr. The fixture is generated at test time
/// rather than committed — a >10k-vertex mesh is tens of thousands of lines of YAML (the risk
/// this budget itself warns about).
#[test]
fn over_budget_mesh_warns_but_succeeds() {
    let path = std::env::temp_dir().join(format!(
        "ludion-check-scene-over-budget-mesh-{}.mesh.yaml",
        std::process::id()
    ));
    write_over_budget_mesh(&path);

    let output = run_check_scene(&path);
    let _ = std::fs::remove_file(&path);

    assert!(
        output.status.success(),
        "an over-budget mesh must still succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("warning[L1112]"), "stderr was: {stderr}");
    assert!(stderr.contains("10001 vertices"), "stderr was: {stderr}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("OK"), "stdout was: {stdout}");
}

/// `--json` keeps stdout a single JSON document when a warning fires: diagnostics go to stderr.
#[test]
fn bounds_json_stdout_stays_pure_json_when_l1112_fires() {
    let path = std::env::temp_dir().join(format!(
        "ludion-check-scene-bounds-json-over-budget-{}.mesh.yaml",
        std::process::id()
    ));
    write_over_budget_mesh(&path);

    let output = run_check_scene_args(&[path.to_str().unwrap(), "--bounds", "--json"]);
    let _ = std::fs::remove_file(&path);

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout is exactly one JSON document");
    assert_eq!(report["diagnostics"][0]["code"], "L1112");
    assert_eq!(report["diagnostics"][0]["severity"], "warning");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("warning[L1112]"), "stderr was: {stderr}");
}

/// Every mesh L-code has a working `ludion explain` entry.
#[test]
fn mesh_codes_are_explainable() {
    for code in ["L1110", "L1111", "L1112"] {
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

/// **`--bounds` reads a mesh document.** The box its `positions` span, with the centre and the size
/// spelled out — the numbers an author places anything on the model from, which before this were a
/// throwaway script (`Plan/artifacts/comfy-generation-log.md`, wave-3 item 6).
#[test]
fn bounds_reports_a_mesh_documents_box() {
    let path = fixture("good_mesh.mesh.yaml");
    let output = run_check_scene_args(&[path.to_str().unwrap(), "--bounds"]);
    assert!(
        output.status.success(),
        "expected success, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // The fixture triangle spans (0,0,0)..(1,1,0).
    assert!(stdout.contains("bounds:"), "stdout was: {stdout}");
    assert!(
        stdout.contains("model: (0.000, 0.000, 0.000)..(1.000, 1.000, 0.000)"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("centre (0.500, 0.500, 0.000) size (1.000, 1.000, 0.000)"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("3 vertices, 1 triangles"),
        "stdout was: {stdout}"
    );
    // The ordinary summary still closes the report.
    assert!(stdout.contains("OK ("), "stdout was: {stdout}");
}

/// **`--bounds --json` on a mesh** is the summary object with the box, the counts and the joints —
/// `[]` for an unskinned mesh — and no `OK` line; the text report has no joints line for it either.
#[test]
fn bounds_json_reads_an_unskinned_mesh_document() {
    let path = fixture("good_mesh.mesh.yaml");
    let output = run_check_scene_args(&[path.to_str().unwrap(), "--bounds", "--json"]);
    assert!(
        output.status.success(),
        "expected success, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("OK"), "stdout was: {stdout}");
    let report: serde_json::Value =
        serde_json::from_str(&stdout).expect("--bounds --json prints one JSON object");
    assert_eq!(report["ok"], true);
    assert_eq!(report["kind"], "mesh");
    assert_eq!(report["vertices"], 3);
    assert_eq!(report["triangles"], 1);
    assert_eq!(report["joints"], serde_json::json!([]));
    assert_eq!(report["model"]["max"], serde_json::json!([1.0, 1.0, 0.0]));

    let output = run_check_scene_args(&[path.to_str().unwrap(), "--bounds"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("joints:"), "stdout was: {stdout}");
}

/// `--json` beside a document that is neither a scene nor a mesh (a voxel model) stays a usage error.
#[test]
fn bounds_json_is_refused_on_other_kinds() {
    let path = fixture("good_voxel.voxel.yaml");
    let output = run_check_scene_args(&[path.to_str().unwrap(), "--bounds", "--json"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--json applies only"),
        "stderr was: {stderr}"
    );
}

/// Without the flag the mesh report is exactly what it always was — `--bounds` adds a block, it does
/// not change one.
#[test]
fn a_mesh_without_bounds_prints_no_block() {
    let output = run_check_scene(&fixture("good_mesh.mesh.yaml"));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("bounds:"), "stdout was: {stdout}");
}

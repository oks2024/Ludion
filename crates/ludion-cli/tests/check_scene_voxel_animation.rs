//! Integration tests for `ludion check-scene`'s voxel-animation document paths
//! (`Plan/voxel-animation.md`): dispatch on the top-level `kind: part` / `kind: animation` /
//! `kind: assembly` keys, driven against the built `ludion` binary — the part/animation/assembly
//! analog of `check_scene_mesh.rs`/`check_scene_voxel.rs`.

use std::path::Path;
use std::process::{Command, Output};

fn run_check_scene(path: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ludion"))
        .arg("check-scene")
        .arg(path)
        .output()
        .expect("failed to run the ludion binary")
}

/// Runs `check-scene <path> <extra args…>` — the authoring-view flags append to the path (the
/// `check_scene_voxel.rs` precedent).
fn run_check_scene_args(path: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ludion"))
        .arg("check-scene")
        .arg(path)
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

// --- kind: part -----------------------------------------------------------------------------

/// A valid part document checks clean: zero exit and a one-line summary with the name, size, and
/// filled-cell count.
#[test]
fn good_part_succeeds() {
    let output = run_check_scene(&fixture("good_part.part.yaml"));
    assert!(
        output.status.success(),
        "expected success, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("OK"), "stdout was: {stdout}");
    assert!(stdout.contains("torso"), "stdout was: {stdout}");
    assert!(stdout.contains("2x2x1"), "stdout was: {stdout}");
    assert!(stdout.contains("3 filled cells"), "stdout was: {stdout}");
}

/// A part whose row width does not match its declared `size.x` is `L1140`, with a JSON Pointer into
/// `slices`.
#[test]
fn broken_part_reports_l1140() {
    let output = run_check_scene(&fixture("broken_part.part.yaml"));
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L1140]"), "stderr was: {stderr}");
    assert!(stderr.contains("/slices"), "stderr was: {stderr}");
}

// --- kind: animation --------------------------------------------------------------------------

/// A valid animation clip checks clean: the summary names it, its duration/loop, and its track
/// count; a per-track line follows with the channel/key counts.
#[test]
fn good_animation_succeeds() {
    let output = run_check_scene(&fixture("good_animation.anim.yaml"));
    assert!(
        output.status.success(),
        "expected success, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("OK"), "stdout was: {stdout}");
    assert!(stdout.contains("humanoid-walk"), "stdout was: {stdout}");
    assert!(stdout.contains("duration 0.6"), "stdout was: {stdout}");
    assert!(stdout.contains("loop true"), "stdout was: {stdout}");
    assert!(stdout.contains("1 track"), "stdout was: {stdout}");
    assert!(
        stdout.contains("upper-leg-l: rotation 3 keys, translation 0 keys"),
        "stdout was: {stdout}"
    );
}

/// The clip report adds key-value ranges and, for a looping clip, the local seam; `--json` prints
/// the same data as one object, and `--bounds` stays a usage error on a clip.
#[test]
fn animation_report_has_ranges_seam_and_json() {
    let output = run_check_scene(&fixture("good_animation.anim.yaml"));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("rotation key values"),
        "stdout was: {stdout}"
    );
    assert!(stdout.contains("seam (local"), "stdout was: {stdout}");

    let path = fixture("good_animation.anim.yaml");
    let json = run_check_scene_args(&path, &["--json"]);
    assert!(json.status.success());
    let value: serde_json::Value = serde_json::from_slice(&json.stdout).expect("one JSON object");
    assert_eq!(value["tracks"][0]["rotation"]["keys"], 3, "{value}");
    assert!(value["tracks"][0]["rotation"]["before"].is_null());
    assert!(value["source"].is_null());

    let bounds = run_check_scene_args(&path, &["--bounds"]);
    assert!(!bounds.status.success());
}

/// A clip whose keyframe times are not strictly increasing within a channel is `L1142`, with a
/// JSON Pointer to the offending keyframe.
#[test]
fn broken_animation_reports_l1142() {
    let output = run_check_scene(&fixture("broken_animation.anim.yaml"));
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L1142]"), "stderr was: {stderr}");
    assert!(
        stderr.contains("/tracks/torso/translation/1/t"),
        "stderr was: {stderr}"
    );
}

// --- kind: assembly --------------------------------------------------------------------------

/// A valid assembly (two parts, one mirrored, one clip binding) checks clean: the summary names
/// the model, then one line per part (file, place, parent, mirror, meshed primitive counts), then
/// the clip bindings — the full multi-file pass through `ludion_engine::load_assembly`.
#[test]
fn good_assembly_succeeds() {
    let output = run_check_scene(&fixture("assembly/good.assembly.yaml"));
    assert!(
        output.status.success(),
        "expected success, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("OK (assembly fixture, 4x3x2, 2 parts, 1 clip)"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains(
            "body: file parts/body.part.yaml, place (0,0,0), parent none (root), mirror none, \
             meshed 6 quads / 12 triangles / 24 vertices"
        ),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains(
            "arm: file parts/arm.part.yaml, place (3,1,1), parent body, mirror x, meshed 6 \
             quads / 12 triangles / 24 vertices"
        ),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("clip walk: anims/walk.anim.yaml"),
        "stdout was: {stdout}"
    );
}

/// An assembly's `masks:` and `blends:` are listed after its clips, a 2D blend with its triangles; an
/// assembly with neither prints no such line and its JSON carries no such key.
#[test]
fn assembly_lists_its_masks_and_blends() {
    let path = fixture("assembly_blends/humanoid.assembly.yaml");
    let output = run_check_scene(&path);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in [
        "  mask arm_only: arm (descendants)
",
        "  blend cross (2D, sync foot_l): idle@(0,0), walk@(2,0), run@(0,2)
    triangles: (0,1,2)
",
        "  blend locomotion (1D, sync foot_l): idle@0, walk@2, run@5
",
    ] {
        assert!(stdout.contains(line), "missing {line:?} in: {stdout}");
    }

    let json = run_check_scene_args(&path, &["--json"]);
    let value: serde_json::Value = serde_json::from_slice(&json.stdout).expect("one JSON object");
    assert_eq!(
        value["masks"],
        serde_json::json!([{
            "name": "arm_only", "joints": ["arm"], "descendants": true, "weights": []
        }])
    );
    assert_eq!(value["blends"][0]["name"], "cross");
    assert_eq!(value["blends"][0]["dimension"], 2);
    assert_eq!(
        value["blends"][0]["triangles"],
        serde_json::json!([[0, 1, 2]])
    );
    assert_eq!(
        value["blends"][0]["points"][1]["at"],
        serde_json::json!([2.0, 0.0])
    );
    assert_eq!(value["blends"][1]["name"], "locomotion");
    assert_eq!(value["blends"][1]["dimension"], 1);
    assert_eq!(value["blends"][1]["sync"], "foot_l");
    assert_eq!(value["blends"][1]["points"][2]["at"], 5.0);
    assert_eq!(value["blends"][1]["triangles"], serde_json::json!([]));

    let plain = fixture("assembly/good.assembly.yaml");
    let stdout = String::from_utf8_lossy(&run_check_scene(&plain).stdout).into_owned();
    assert!(
        !stdout.contains("  mask ") && !stdout.contains("  blend "),
        "{stdout}"
    );
    let json = run_check_scene_args(&plain, &["--json"]);
    let value: serde_json::Value = serde_json::from_slice(&json.stdout).expect("one JSON object");
    assert!(
        value.get("masks").is_none() && value.get("blends").is_none(),
        "{value}"
    );
}

/// An assembly whose part draws a glyph the palette does not define is `L1138`, naming the glyph,
/// the part, and its file — the cross-file check only `ludion_engine::load_assembly` can run.
#[test]
fn assembly_with_unknown_glyph_reports_l1138() {
    let output = run_check_scene(&fixture("assembly/broken-glyph.assembly.yaml"));
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L1138]"), "stderr was: {stderr}");
    assert!(stderr.contains("`Z`"), "stderr was: {stderr}");
    assert!(stderr.contains("body"), "stderr was: {stderr}");
    assert!(
        stderr.contains("parts/glyph-bad.part.yaml"),
        "stderr was: {stderr}"
    );
}

// --- kind: assembly, composed-grid analysis (`Plan/voxel-animation.md` decisions 10-11) -------

/// `--exposure` runs over the *composed* grid, not any one part: `core` alone has an open face
/// where `cap` will land, so cell (1,1,1) is not enclosed within `core`'s own document — only once
/// `cap` plugs the hole does the composed grid enclose it. The sample line names `core` as the
/// owner (the cap itself never overwrote that cell).
#[test]
fn assembly_exposure_finds_a_composition_only_enclosed_cell() {
    let output = run_check_scene_args(
        &fixture("assembly/composed/enclosed.assembly.yaml"),
        &["--exposure"],
    );
    assert!(
        output.status.success(),
        "expected success, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("exposure: 27 filled, 26 surface, 1 fully enclosed"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("glyph A: 26 cells, 1 enclosed — samples (1,1,1) [core]"),
        "stdout was: {stdout}"
    );
    assert!(stdout.contains("overlap: none"), "stdout was: {stdout}");
}

/// Two parts placed on the same cell (the joint-plug technique, decision 10): the shared cell
/// counts as filled exactly once in the exposure totals, and the `overlap:` addendum names both
/// owning parts.
#[test]
fn assembly_exposure_counts_overlap_once_and_attributes_both_parts() {
    let output = run_check_scene_args(
        &fixture("assembly/composed/overlap.assembly.yaml"),
        &["--exposure"],
    );
    assert!(
        output.status.success(),
        "expected success, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("exposure: 2 filled, 2 surface, 0 fully enclosed"),
        "the shared cell is not double-counted: {stdout}"
    );
    assert!(
        stdout.contains("overlap: 1 cell shared by more than one part"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("(0,0,0): first, second"),
        "both owning parts are named, authored order: {stdout}"
    );
}

/// `--mirror-x` over an assembly runs against the composed grid and names, per asymmetric region,
/// every part touching it — the file to open, not just the coordinates.
#[test]
fn assembly_mirror_x_names_owning_parts_per_region() {
    let output = run_check_scene_args(&fixture("assembly/good.assembly.yaml"), &["--mirror-x"]);
    assert!(
        output.status.success(),
        "expected success, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("mirror-x: 3 asymmetric cells in 2 regions"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("glyphs . A  parts body"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("glyphs . B  parts arm"),
        "stdout was: {stdout}"
    );
}

/// Connectivity (`L1129`) always runs over an assembly's composed grid, exactly as it always runs
/// over a plain voxel document — never behind a view flag — and the warning's stray/main sample
/// cells get a part-attribution addendum on stdout.
#[test]
fn assembly_connectivity_warning_names_owning_parts() {
    let output = run_check_scene(&fixture("assembly/good.assembly.yaml"));
    assert!(
        output.status.success(),
        "a connectivity warning does not fail the check: stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("warning[L1129]"), "stderr was: {stderr}");
    assert!(
        stderr.contains("2 disconnected components"),
        "stderr was: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("connectivity owners:"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains(
            "stray sample (3,1,1) owned by arm; nearest main-body cell (1,0,0) owned \
                          by body"
        ),
        "stdout was: {stdout}"
    );
}

/// The overlap / joint-plug lint (`L1143`, decision 10) runs over an assembly's composed grid
/// unconditionally, like connectivity — and says nothing about a limb sunk into its parent at its
/// declared pivot, which is the joint-plug technique working as intended.
#[test]
fn assembly_plug_at_its_pivot_prints_no_overlap_warning() {
    for name in [
        "assembly/composed/plug.assembly.yaml",
        // The one-cell plug of the older composition fixture, pivot-less (so its pivot is the
        // part's own origin, a cell away): silent too, so every assembly report predating this
        // lint still reads exactly as it did.
        "assembly/composed/overlap.assembly.yaml",
    ] {
        let output = run_check_scene(&fixture(name));
        assert!(
            output.status.success(),
            "expected success for {name}, stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stderr.contains("L1143"), "{name} warned: {stderr}");
    }
}

/// The lint's valuable catch: the same plug with the pivot authored at the limb's far end warns,
/// naming the pair, the cluster, the distance to the pivot, and the plug radius it was measured
/// against — with a JSON Pointer at the child part whose `pivot` is suspect.
#[test]
fn assembly_overlap_far_from_the_pivot_reports_l1143() {
    let output = run_check_scene(&fixture("assembly/composed/displaced-pivot.assembly.yaml"));
    assert!(
        output.status.success(),
        "a lint does not fail the check: stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("warning[L1143]"), "stderr was: {stderr}");
    assert!(
        stderr.contains("part `limb` overlaps its parent `trunk`"),
        "stderr was: {stderr}"
    );
    assert!(
        stderr.contains("a cluster of 1 cell spanning (2,5,0)-(2,5,0)"),
        "stderr was: {stderr}"
    );
    assert!(
        stderr.contains("centered 5.00 cells from `limb`'s pivot at (2.5,10.5,0.5)"),
        "stderr was: {stderr}"
    );
    assert!(stderr.contains("plug radius 3.00"), "stderr was: {stderr}");
    assert!(stderr.contains("--> /parts/1"), "stderr was: {stderr}");
}

/// Two parts with no parent/child relation have no joint to plug, so their shared cells warn with
/// no pivot distance at all, pointed at the later-authored part.
#[test]
fn assembly_overlap_between_unrelated_parts_reports_l1143() {
    let output = run_check_scene(&fixture("assembly/composed/unrelated.assembly.yaml"));
    assert!(
        output.status.success(),
        "a lint does not fail the check: stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("warning[L1143]"), "stderr was: {stderr}");
    assert!(
        stderr.contains("parts `limb` and `cap` overlap in a cluster of 1 cell"),
        "stderr was: {stderr}"
    );
    assert!(
        stderr.contains("neither is the other's parent"),
        "stderr was: {stderr}"
    );
    assert!(stderr.contains("--> /parts/2"), "stderr was: {stderr}");
    assert_eq!(
        stderr.matches("warning[L1143]").count(),
        1,
        "the trunk/limb plug at its pivot stays silent: {stderr}"
    );
}

/// The voxel-view flags are also legal against `kind: assembly` (they used to be a usage error for
/// any non-voxel document); the usage-error wording now names both legal kinds.
#[test]
fn voxel_flags_on_a_mesh_name_both_legal_kinds() {
    let output = run_check_scene_args(&fixture("good_mesh.mesh.yaml"), &["--mirror-x"]);
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("kind: voxel, kind: assembly"),
        "stderr was: {stderr}"
    );
}

// --- Shared unrecognized-kind coverage ----------------------------------------------------------

/// The unrecognized-`kind` error names `part`/`animation`/`assembly` alongside `mesh`/`voxel`.
#[test]
fn unrecognized_kind_mentions_the_new_kinds() {
    let output = run_check_scene(&fixture("unknown_kind.yaml"));
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L1119]"), "stderr was: {stderr}");
    assert!(stderr.contains("`part`"), "stderr was: {stderr}");
    assert!(stderr.contains("`animation`"), "stderr was: {stderr}");
    assert!(stderr.contains("`assembly`"), "stderr was: {stderr}");
}

/// Every new voxel-animation L-code has a working `ludion explain` entry.
#[test]
fn voxel_animation_codes_are_explainable() {
    for code in [
        "L1137", "L1138", "L1139", "L1140", "L1141", "L1142", "L1143",
    ] {
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

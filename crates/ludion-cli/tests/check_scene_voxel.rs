//! Integration tests for `ludion check-scene`'s voxel-document path (Phase 10 Wave 1): dispatch on
//! the top-level `kind: voxel` key, driven against the built `ludion` binary — the voxel analog of
//! `check_scene_mesh.rs`.

use std::path::Path;
use std::process::{Command, Output};

fn run_check_scene(path: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ludion"))
        .arg("check-scene")
        .arg(path)
        .output()
        .expect("failed to run the ludion binary")
}

/// Runs `check-scene <path> <extra args…>` — the authoring-view flags append to the path.
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

/// A frozen copy of a real model vigil authored, the content the authoring views were built for.
fn voxel_fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/voxels")
        .join(name)
}

/// A valid voxel model checks clean: zero exit and a one-line summary with the dimensions and
/// filled-voxel count.
#[test]
fn good_voxel_succeeds() {
    let output = run_check_scene(&fixture("good_voxel.voxel.yaml"));
    assert!(
        output.status.success(),
        "expected success, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("OK"), "stdout was: {stdout}");
    assert!(stdout.contains("2x2x1"), "stdout was: {stdout}");
    assert!(stdout.contains("3 filled voxels"), "stdout was: {stdout}");
    // This fixture declares no `scale`, so the summary says nothing about one.
    assert!(!stdout.contains("scale"), "stdout was: {stdout}");
}

/// Every voxel summary carries the greedy-meshed primitive counts — always on, because a number
/// that exists to inform `budget_exempt:` is useless behind a flag the author would have to know to
/// ask for. The fixture is a 2×2×1 slab with one cell missing: 3 filled cells, hand-countable.
#[test]
fn meshed_primitive_counts_are_always_reported() {
    let output = run_check_scene(&fixture("good_voxel.voxel.yaml"));
    assert!(
        output.status.success(),
        "expected success, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("meshed 14 quads / 28 triangles / 56 vertices"),
        "stdout was: {stdout}"
    );
}

/// A model that declares an intended world scale echoes it in the `OK (…)` summary — the check that
/// a model's declared intent still matches how the game places it.
#[test]
fn declared_scale_is_reported() {
    let output = run_check_scene(&voxel_fixture("statue.voxel.yaml"));
    assert!(
        output.status.success(),
        "expected success, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("scale 0.24"), "stdout was: {stdout}");
}

/// A voxel model with inconsistent data (a missing layer, an undefined glyph) exits non-zero and
/// reports `L1114`, with a JSON Pointer into `slices`.
#[test]
fn broken_voxel_reports_l1114() {
    let output = run_check_scene(&fixture("broken_voxel.voxel.yaml"));
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L1114]"), "stderr was: {stderr}");
    assert!(stderr.contains("/slices"), "stderr was: {stderr}");
}

/// The unrecognized-`kind` error now names `voxel` alongside `mesh` in its suggestion.
#[test]
fn unrecognized_kind_mentions_voxel() {
    let output = run_check_scene(&fixture("unknown_kind.yaml"));
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L1119]"), "stderr was: {stderr}");
    assert!(stderr.contains("voxel"), "stderr was: {stderr}");
}

/// A model above the 64³ cell budget still checks clean (`L1115` is a warning) but prints the
/// warning to stderr. The fixture is generated at test time — a huge model is not committed.
#[test]
fn over_budget_voxel_warns_but_succeeds() {
    // 262,145 cells in a single row: one layer, one row, just over 64³ = 262,144.
    let width = 262_145usize;
    let mut source = String::from("format: 1\nkind: voxel\nsize:\n");
    source.push_str(&format!("  - {width}\n  - 1\n  - 1\n"));
    source.push_str("palette: {}\nslices:\n  -\n    - \"");
    source.push_str(&".".repeat(width));
    source.push_str("\"\n");

    let path = std::env::temp_dir().join(format!(
        "ludion-check-scene-over-budget-{}.voxel.yaml",
        std::process::id()
    ));
    std::fs::write(&path, &source).expect("failed to write the temp fixture");

    let output = run_check_scene(&path);
    let _ = std::fs::remove_file(&path);

    assert!(
        output.status.success(),
        "an over-budget model must still succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("warning[L1115]"), "stderr was: {stderr}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("OK"), "stdout was: {stdout}");
}

/// The three authoring views compose against the committed statue (odd width 9, so it has a centre
/// column): all print between the validation and the closing `OK`, and the model checks clean.
#[test]
fn statue_authoring_views_compose() {
    let output = run_check_scene_args(
        &voxel_fixture("statue.voxel.yaml"),
        &["--mirror-x", "--slice", "0", "--column", "4,4"],
    );
    assert!(
        output.status.success(),
        "expected success, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // The hand-mirrored statue is X-symmetric.
    assert!(
        stdout.contains("mirror-x: symmetric"),
        "stdout was: {stdout}"
    );
    // The slice view: header, units ruler, front/back markers.
    assert!(stdout.contains("-- y=0 --"), "stdout was: {stdout}");
    assert!(stdout.contains("012345678"), "stdout was: {stdout}");
    assert!(
        stdout.contains("z=0 (front, world -Z)"),
        "stdout was: {stdout}"
    );
    assert!(stdout.contains("z=8 (back)"), "stdout was: {stdout}");
    // The column view + its run-length summary.
    assert!(stdout.contains("column (x=4, z=4)"), "stdout was: {stdout}");
    assert!(stdout.contains("filled y: 0-8"), "stdout was: {stdout}");
    // The OK summary stays last.
    let ok = stdout.rfind("OK").expect("an OK summary");
    assert!(
        ok > stdout.find("mirror-x").unwrap(),
        "OK must come after the views: {stdout}"
    );
}

/// A layer wider than ten columns (the upscaled lantern, width 15) gains the tens ruler line above the
/// units line; the model is X-symmetric too.
#[test]
fn wide_slice_has_a_tens_ruler() {
    let output = run_check_scene_args(
        &voxel_fixture("lantern-x3.voxel.yaml"),
        &["--mirror-x", "--slice", "10"],
    );
    assert!(
        output.status.success(),
        "expected success, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("mirror-x: symmetric"),
        "stdout was: {stdout}"
    );
    // The units ruler spans 0..14 (012345678901234); the tens line marks columns 10-14.
    assert!(stdout.contains("012345678901234"), "stdout was: {stdout}");
    let has_tens = stdout
        .lines()
        .any(|line| line.trim_end().ends_with("111") && !line.contains('0'));
    assert!(has_tens, "expected a tens ruler line, stdout was: {stdout}");
}

/// The authoring-view flags are voxel-only: passing one against a scene is a usage error naming
/// `voxel`, and exits non-zero.
#[test]
fn views_on_a_scene_are_a_usage_error() {
    let output = run_check_scene_args(&fixture("good.yaml"), &["--mirror-x"]);
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("voxel"), "stderr was: {stderr}");
}

/// `--exposure` reports the surface/enclosed split of the committed statue, one line per glyph in
/// use, and the emissive glyph's faces in the fixed axis order. Running it twice is byte-identical.
#[test]
fn exposure_reports_enclosed_cells_and_emissive_faces() {
    let output = run_check_scene_args(&voxel_fixture("statue.voxel.yaml"), &["--exposure"]);
    assert!(
        output.status.success(),
        "expected success, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // The statue is 289 filled cells; the totals line must account for every one of them.
    assert!(
        stdout.contains("exposure: 289 filled, 226 surface, 63 fully enclosed"),
        "stdout was: {stdout}"
    );
    // The structural glyph carries the buried cells (normal); the sample list caps at five.
    assert!(
        stdout.contains("glyph S: 255 cells, 63 enclosed — samples "),
        "stdout was: {stdout}"
    );
    assert!(stdout.contains(", ... and 58 more"), "stdout was: {stdout}");
    // A glyph with nothing buried still gets its row — a positive answer, not a silence.
    assert!(
        stdout.contains("glyph D: 32 cells, 0 enclosed\n"),
        "stdout was: {stdout}"
    );
    // The eyes are the model's one emissive glyph; the histogram is in fixed axis order.
    assert!(
        stdout.contains("emissive faces:\n  glyph E (emissive 5.5): 4 exposed — +X 1, -X 1, +Y 0, -Y 0, +Z 0, -Z 2"),
        "stdout was: {stdout}"
    );
    // The OK summary stays last, as with every other view.
    let ok = stdout.rfind("OK").expect("an OK summary");
    assert!(
        ok > stdout.find("exposure:").unwrap(),
        "OK must come after the views: {stdout}"
    );

    let again = run_check_scene_args(&voxel_fixture("statue.voxel.yaml"), &["--exposure"]);
    assert_eq!(
        output.stdout, again.stdout,
        "the exposure view must not vary between runs"
    );
}

/// `--exposure --from game` adds the viewpoint line: how many exposed faces point at the gameplay
/// camera, and how many of those the model hides from it. The statue's eyes sit on the surface, so
/// every facing face is clear — the shape of the answer, against a committed model.
#[test]
fn exposure_from_game_adds_the_viewpoint_line() {
    let output = run_check_scene_args(
        &voxel_fixture("statue.voxel.yaml"),
        &["--exposure", "--from", "game"],
    );
    assert!(
        output.status.success(),
        "expected success, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(
            "  glyph E (emissive 5.5): 4 exposed — +X 1, -X 1, +Y 0, -Y 0, +Z 0, -Z 2\n    from \
             game view (58 deg): 3 face the camera, 0 occluded by own geometry"
        ),
        "stdout was: {stdout}"
    );
    // Without `--from`, the viewpoint line is absent entirely.
    let plain = run_check_scene_args(&voxel_fixture("statue.voxel.yaml"), &["--exposure"]);
    assert!(
        !String::from_utf8_lossy(&plain.stdout).contains("from game view"),
        "the direction-only report must not answer a question it was not asked"
    );
}

/// A panel recessed behind its own lintel scores perfectly on the direction histogram and is still
/// hidden at the gameplay camera's pitch — the defect this view exists to catch. Shallowing the
/// recess is the fix, and the same command says so.
#[test]
fn exposure_from_game_catches_a_recessed_panel() {
    let occluded = |depth: u32| -> String {
        let path = std::env::temp_dir().join(format!(
            "ludion-check-scene-recess-{}-{depth}.voxel.yaml",
            std::process::id()
        ));
        std::fs::write(&path, recess_model(depth)).expect("failed to write the temp fixture");
        let output = run_check_scene_args(&path, &["--exposure", "--from", "game"]);
        let _ = std::fs::remove_file(&path);
        assert!(
            output.status.success(),
            "expected success, stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .find(|line| line.contains("from game view"))
            .expect("a viewpoint line")
            .to_string()
    };
    assert!(
        occluded(4).contains("1 face the camera, 1 occluded by own geometry"),
        "four cells deep, the lintel is in the way: {}",
        occluded(4)
    );
    assert!(
        occluded(1).contains("1 face the camera, 0 occluded by own geometry"),
        "one cell deep, the ray clears the lintel: {}",
        occluded(1)
    );
}

/// A wall with a window opening, an emissive panel recessed `depth` cells behind it, and the lintel
/// above: `z = depth` is the solid back wall carrying the panel at its foot, `z < depth` is open
/// below the opening height and solid above it.
fn recess_model(depth: u32) -> String {
    const SX: u32 = 6;
    const SY: u32 = 10;
    const OPENING: u32 = 6;
    let sz = depth + 1;
    let mut source = format!("format: 1\nkind: voxel\nsize:\n  - {SX}\n  - {SY}\n  - {sz}\n");
    source.push_str(
        "palette:\n  E:\n    color:\n      - 1.0\n      - 1.0\n      - 1.0\n      - 1.0\n    \
         emissive: 2.0\n  S:\n    color:\n      - 0.5\n      - 0.5\n      - 0.5\n      - 1.0\n\
         slices:\n",
    );
    for y in 0..SY {
        source.push_str("  -\n");
        for z in 0..sz {
            let row: String = (0..SX)
                .map(|x| {
                    if z == sz - 1 {
                        if x == 2 && y == 0 { 'E' } else { 'S' }
                    } else if y >= OPENING {
                        'S'
                    } else {
                        '.'
                    }
                })
                .collect();
            source.push_str(&format!("    - \"{row}\"\n"));
        }
    }
    source
}

/// `--from` is only meaningful with `--exposure`, and clap refuses it alone rather than silently
/// ignoring it. (`requires` no-ops when its target carries a `conflicts_with`; `exposure` does not,
/// which is what this pins.)
#[test]
fn from_game_requires_exposure() {
    let output = run_check_scene_args(&voxel_fixture("statue.voxel.yaml"), &["--from", "game"]);
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--exposure"), "stderr was: {stderr}");
}

/// `--mirror-x` clusters differing cells into connected regions with a bbox, a count and a glyph
/// set — the question is "is my asymmetry confined to the features I intended", which a list of
/// sample cells (all from one feature) cannot answer.
#[test]
fn mirror_x_reports_regions_not_samples() {
    // 10 wide (low half x < 5), 4 tall, 2 deep: a 2×2×2 'B' block whose mirror is empty, and one
    // stray 'C' four layers up. Two features ⇒ two regions.
    let mut source =
        String::from("format: 1\nkind: voxel\nsize:\n  - 10\n  - 4\n  - 2\npalette:\n");
    for glyph in ['A', 'B', 'C'] {
        source.push_str(&format!(
            "  {glyph}:\n    color:\n      - 1.0\n      - 1.0\n      - 1.0\n      - 1.0\n"
        ));
    }
    source.push_str("slices:\n");
    for rows in [
        ["BBAAAAAA..", "BBAAAAAA.."],
        ["BBAAAAAA..", "BBAAAAAA.."],
        ["AAAAAAAAAA", "AAAAAAAAAA"],
        ["AAACAAAAAA", "AAAAAAAAAA"],
    ] {
        source.push_str("  -\n");
        for row in rows {
            source.push_str(&format!("    - \"{row}\"\n"));
        }
    }
    let path = std::env::temp_dir().join(format!(
        "ludion-check-scene-mirror-regions-{}.voxel.yaml",
        std::process::id()
    ));
    std::fs::write(&path, &source).expect("failed to write the temp fixture");

    let output = run_check_scene_args(&path, &["--mirror-x"]);
    let _ = std::fs::remove_file(&path);

    assert!(
        output.status.success(),
        "expected success, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("mirror-x: 9 asymmetric cells in 2 regions (low-half x < 5)"),
        "stdout was: {stdout}"
    );
    // Largest region first, counts right-aligned to the widest of them.
    assert!(
        stdout.contains("  8 cells  x 0..1  y 0..1  z 0..1  glyphs . B\n"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("  1 cells  x 3..3  y 3..3  z 0..0  glyphs A C\n"),
        "stdout was: {stdout}"
    );
    // The old per-cell sample list is gone.
    assert!(!stdout.contains("vs ("), "stdout was: {stdout}");
}

/// A model whose palette declares no emissive says so, rather than printing an empty section.
#[test]
fn exposure_names_the_absence_of_emissive_glyphs() {
    let output = run_check_scene_args(&fixture("good_voxel.voxel.yaml"), &["--exposure"]);
    assert!(output.status.success(), "expected success");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("emissive faces: none"),
        "stdout was: {stdout}"
    );
}

/// `--exposure` is voxel-only too, and the usage error names it alongside the other three flags.
#[test]
fn exposure_on_a_scene_is_a_usage_error() {
    let output = run_check_scene_args(&fixture("good.yaml"), &["--exposure"]);
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--exposure"), "stderr was: {stderr}");
    assert!(stderr.contains("kind: voxel"), "stderr was: {stderr}");
}

/// The `L1129` gap report reaches the CLI: a model with a stray piece names the nearest pair of
/// cells and the bridge that would close it, not just a sample coordinate.
#[test]
fn disconnected_model_reports_the_gap() {
    // A 5×1×1 row: three cells at x=0..2, one stray at x=4, two steps clear of the main body.
    let source = "format: 1\nkind: voxel\nsize:\n  - 5\n  - 1\n  - 1\npalette:\n  A:\n    color:\n      - 1.0\n      - 1.0\n      - 1.0\n      - 1.0\nslices:\n  -\n    - \"AAA.A\"\n";
    let path = std::env::temp_dir().join(format!(
        "ludion-check-scene-gap-{}.voxel.yaml",
        std::process::id()
    ));
    std::fs::write(&path, source).expect("failed to write the temp fixture");

    let output = run_check_scene(&path);
    let _ = std::fs::remove_file(&path);

    assert!(output.status.success(), "L1129 is a lint, not a failure");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("2 disconnected components (main body 3 cells)"),
        "stderr was: {stderr}"
    );
    assert!(
        stderr.contains("stray (4, 0, 0) vs main (2, 0, 0), gap 2 (1 bridge cell to connect)"),
        "stderr was: {stderr}"
    );
}

/// An out-of-range `--slice` is a usage error naming the valid range, exiting non-zero — the model
/// itself is fine.
#[test]
fn out_of_range_slice_is_a_usage_error() {
    let output = run_check_scene_args(&voxel_fixture("statue.voxel.yaml"), &["--slice", "99"]);
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("out of range"), "stderr was: {stderr}");
    assert!(stderr.contains("0..=8"), "stderr was: {stderr}");
}

/// Every voxel L-code has a working `ludion explain` entry.
#[test]
fn voxel_codes_are_explainable() {
    for code in ["L1113", "L1114", "L1115"] {
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

/// A document `rim:` out of range fails with the voxel data code at the offending `/rim` pointer.
#[test]
fn out_of_range_rim_power_reports_l1114_at_the_pointer() {
    let source = std::fs::read_to_string(fixture("good_voxel.voxel.yaml")).unwrap();
    let source = source.replace("palette:", "rim:\n  power: 0\npalette:");
    let dir = std::env::temp_dir().join(format!("ludion-check-rim-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("rim.voxel.yaml");
    std::fs::write(&path, source).unwrap();
    let output = run_check_scene(&path);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("L1114"), "stderr was: {stderr}");
    assert!(stderr.contains("/rim/power"), "stderr was: {stderr}");
}

/// `L1398`: a one-cell-thick row halves to nothing (a coarse cell needs four of its eight cells),
/// so the level it names draws nothing. A warning, not a failure.
#[test]
fn a_voxel_lod_level_that_halves_to_nothing_is_warned() {
    let source = "format: 1\nkind: voxel\nsize:\n  - 4\n  - 1\n  - 1\npalette:\n  A:\n    color:\n      - 1.0\n      - 1.0\n      - 1.0\n      - 1.0\nslices:\n  -\n    - \"AAAA\"\nlods:\n  - screen_size: 0.2\n";
    let path = std::env::temp_dir().join(format!(
        "ludion-check-scene-lod-empty-{}.voxel.yaml",
        std::process::id()
    ));
    std::fs::write(&path, source).expect("failed to write the temp fixture");

    let output = run_check_scene(&path);
    let _ = std::fs::remove_file(&path);

    assert!(output.status.success(), "L1398 is a lint, not a failure");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("L1398"), "stderr was: {stderr}");
    assert!(stderr.contains("`lods` level 1"), "stderr was: {stderr}");
}

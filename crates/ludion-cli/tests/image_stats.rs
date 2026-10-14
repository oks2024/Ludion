//! Integration tests for `ludion image-stats`, driven against the built binary and fixed copies of two
//! gallery goldens in `tests/fixtures/image-stats/`, so a re-bless never moves the pinned numbers —
//! the instrument half of wave 1 of `Plan/post-process-pipeline.md`.

use std::path::PathBuf;
use std::process::{Command as ProcCommand, Output};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/image-stats")
        .canonicalize()
        .expect("the image-stats fixtures directory exists")
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

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

// --- default table ------------------------------------------------------------------------------

/// The default table has a header naming all four metrics and one row per image, with real
/// (non-degenerate) measured numbers — a golden PNG is neither blank nor pure noise.
#[test]
fn table_reports_all_four_metrics_over_a_real_golden() {
    let image = fixtures().join("night-sky.png");
    let output = run_ludion(&["image-stats", image.to_str().unwrap()]);
    assert!(
        output.status.success(),
        "image-stats failed: {}",
        stderr(&output)
    );
    let out = stdout(&output);
    let mut lines = out.lines();
    let header = lines.next().expect("a header line");
    for column in [
        "banding_score",
        "edge_aliasing",
        "clipped_low",
        "clipped_high",
    ] {
        assert!(header.contains(column), "header missing {column}: {header}");
    }
    let row = lines.next().expect("one data row");
    assert!(row.contains("night-sky.png"), "row missing the path: {row}");
    // The exact numbers `ludion image-stats` prints for this committed golden (pinned decode, pure
    // function of the PNG bytes) — a degenerate 0.0000/1.0000 table would mean the wiring is wrong.
    assert!(
        row.contains("0.2867") && row.contains("0.7546") && row.contains("0.0001"),
        "expected the real measured numbers for night-sky.png, got: {row}"
    );
}

/// A batch of several images prints one table with one row per image, in argument order.
#[test]
fn table_batches_several_images_into_one_report() {
    let dir = fixtures();
    let a = dir.join("night-sky.png");
    let b = dir.join("text.png");
    let output = run_ludion(&["image-stats", a.to_str().unwrap(), b.to_str().unwrap()]);
    assert!(output.status.success(), "{}", stderr(&output));
    let out = stdout(&output);
    let night_sky_line = out.lines().position(|l| l.contains("night-sky.png"));
    let text_line = out.lines().position(|l| l.contains("text.png"));
    assert!(night_sky_line.is_some() && text_line.is_some());
    assert!(
        night_sky_line < text_line,
        "images must report in argument order"
    );
    // `text.png` is the set's extreme on both axes — a flat background carries almost no gradient to
    // contour (banding 0.0090, the lowest in the repo; it was exactly 0 before the output dither
    // became a default) while every glyph edge is a bare step (aliasing 1.0000). A second,
    // independent real-number sanity check, and one that would fail on a degenerate table.
    let text_row = out.lines().find(|l| l.contains("text.png")).unwrap();
    assert!(
        text_row.contains("0.0090") && text_row.contains("1.0000"),
        "expected in: {text_row}"
    );
}

// --- --json --------------------------------------------------------------------------------------

/// `--json` emits a self-describing array: each object carries its own `path` plus the four metrics,
/// matching the plain table's numbers for the same image.
#[test]
fn json_report_is_self_describing_and_matches_the_table() {
    let image = fixtures().join("night-sky.png");
    let output = run_ludion(&["image-stats", image.to_str().unwrap(), "--json"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output).lines().count(), 1, "one compact line");
    let value: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    let entries = value.as_array().expect("a JSON array");
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert!(
        entry["path"].as_str().unwrap().ends_with("night-sky.png"),
        "{entry}"
    );
    assert!((entry["banding_score"].as_f64().unwrap() - 0.2867).abs() < 0.001);
    assert!((entry["clipped_high"].as_f64().unwrap() - 0.0001).abs() < 0.001);
}

// --- --check: passing ----------------------------------------------------------------------------

/// A threshold file whose ceilings sit above every measured value passes and exits zero, printing a
/// within-thresholds summary.
#[test]
fn check_passes_when_every_ceiling_is_generous() {
    let dir = scratch("check_pass");
    let toml_path = dir.join("thresholds.toml");
    std::fs::write(
        &toml_path,
        "banding_score = 0.99\nedge_aliasing = 1.0\nclipped_high = 0.05\n",
    )
    .unwrap();

    let image = fixtures().join("night-sky.png");
    let output = run_ludion(&[
        "image-stats",
        image.to_str().unwrap(),
        "--check",
        toml_path.to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "a generous threshold file must pass: {}",
        stderr(&output)
    );
    assert!(
        stderr(&output).contains("all images within thresholds"),
        "{}",
        stderr(&output)
    );
    assert!(
        !stdout(&output).contains("within thresholds"),
        "the verdict line must not reach stdout: {}",
        stdout(&output)
    );
}

/// `--json --check`: stdout is the JSON array and nothing else, so a consumer can parse it whole.
#[test]
fn json_check_stdout_is_only_the_json() {
    let dir = scratch("check_json_stdout");
    let toml_path = dir.join("thresholds.toml");
    std::fs::write(
        &toml_path,
        "banding_score = 0.99\nedge_aliasing = 1.0\nclipped_high = 0.05\n",
    )
    .unwrap();
    let image = fixtures().join("night-sky.png");
    let output = run_ludion(&[
        "image-stats",
        image.to_str().unwrap(),
        "--json",
        "--check",
        toml_path.to_str().unwrap(),
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    let parsed: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("the whole of stdout is one JSON value");
    assert!(parsed.is_array(), "{parsed}");
    assert!(stderr(&output).contains("within thresholds"));
}

// --- --check: breach ------------------------------------------------------------------------------

/// A threshold file whose ceiling sits below a measured value fails with `L4010` naming the image,
/// the metric, the measured value, and the ceiling — and the report still prints.
#[test]
fn check_fails_with_l4010_naming_metric_and_both_numbers() {
    let dir = scratch("check_fail");
    let toml_path = dir.join("thresholds.toml");
    std::fs::write(&toml_path, "banding_score = 0.10\n").unwrap();

    let image = fixtures().join("night-sky.png");
    let output = run_ludion(&[
        "image-stats",
        image.to_str().unwrap(),
        "--check",
        toml_path.to_str().unwrap(),
    ]);
    assert!(!output.status.success(), "an exceeded ceiling must fail");
    let err = stderr(&output);
    assert!(err.contains("error[L4010]"), "{err}");
    assert!(err.contains("banding_score"), "{err}");
    assert!(err.contains("0.2867"), "expected the measured value: {err}");
    assert!(err.contains("0.1000"), "expected the ceiling: {err}");
    assert!(
        stdout(&output).contains("night-sky.png"),
        "the table must still print on stdout: {}",
        stdout(&output)
    );
}

/// A per-image override (`[images."<file name>"]`) wins over the file's base ceiling.
#[test]
fn check_per_image_override_wins_over_base_ceiling() {
    let dir = scratch("check_override");
    let toml_path = dir.join("thresholds.toml");
    std::fs::write(
        &toml_path,
        "banding_score = 0.10\n\n[images.\"night-sky.png\"]\nbanding_score = 0.99\n",
    )
    .unwrap();

    let image = fixtures().join("night-sky.png");
    let output = run_ludion(&[
        "image-stats",
        image.to_str().unwrap(),
        "--check",
        toml_path.to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "the per-image override must win over the failing base ceiling: {}",
        stderr(&output)
    );
}

// --- --check: malformed threshold file ------------------------------------------------------------

/// A threshold file naming a key outside the four metrics is `L4009`, not `L4010` — one code for an
/// invalid file, not a second one for an "unknown key" (the metric set is fixed, unlike a profile
/// budget's dynamic system/pass names).
#[test]
fn check_unknown_metric_name_is_l4009() {
    let dir = scratch("check_unknown_key");
    let toml_path = dir.join("thresholds.toml");
    std::fs::write(&toml_path, "banding_scoer = 0.5\n").unwrap();

    let image = fixtures().join("night-sky.png");
    let output = run_ludion(&[
        "image-stats",
        image.to_str().unwrap(),
        "--check",
        toml_path.to_str().unwrap(),
    ]);
    assert!(!output.status.success());
    let err = stderr(&output);
    assert!(err.contains("error[L4009]"), "{err}");
    assert!(err.contains("banding_scoer"), "{err}");
}

/// A `--check` path that does not exist is also `L4009`.
#[test]
fn check_missing_file_is_l4009() {
    let image = fixtures().join("night-sky.png");
    let missing = scratch("check_missing").join("does-not-exist.toml");
    let output = run_ludion(&[
        "image-stats",
        image.to_str().unwrap(),
        "--check",
        missing.to_str().unwrap(),
    ]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("error[L4009]"),
        "{}",
        stderr(&output)
    );
}

// --- non-PNG input ---------------------------------------------------------------------------------

/// A file that is not a PNG fails cleanly (a readable diagnostic, non-zero exit), not a panic.
#[test]
fn non_png_input_fails_cleanly() {
    let not_a_png = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let output = run_ludion(&["image-stats", not_a_png.to_str().unwrap()]);
    assert!(!output.status.success());
    let err = stderr(&output);
    assert!(err.contains("not a valid PNG"), "{err}");
    assert!(err.contains("1 error found"), "{err}");
}

// --- multi-image batch, one failing ----------------------------------------------------------------

/// A batch where one input is not a PNG still measures and reports every other input, but the
/// overall exit code is non-zero.
#[test]
fn batch_with_one_failing_input_is_nonzero_but_reports_the_rest() {
    let good = fixtures().join("night-sky.png");
    let bad = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let output = run_ludion(&["image-stats", good.to_str().unwrap(), bad.to_str().unwrap()]);
    assert!(
        !output.status.success(),
        "a batch with any bad input must fail"
    );
    assert!(
        stdout(&output).contains("night-sky.png"),
        "the good image must still be reported: {}",
        stdout(&output)
    );
    assert!(
        stderr(&output).contains("not a valid PNG"),
        "{}",
        stderr(&output)
    );
}

//! Integration tests for `ludion voxel <new|apply|upscale|diff|from-json>`
//! (`Plan/voxel-authoring-tools.md` Waves 1-3), driven against the built `ludion` binary in a temp
//! directory — the same shape as `check_scene_voxel.rs`.
//!
//! The properties that matter here are the ones a unit test cannot see: that `--dry-run` writes
//! nothing, that a failed apply leaves the file byte-identical, that `diff`'s exit code is usable
//! as a gate, and that `from-json --check` is write-free on both the green and drifted paths.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn ludion(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ludion"))
        .args(args)
        .output()
        .expect("failed to run the ludion binary")
}

/// Runs the ludion binary with `input` piped to stdin, instead of an `--in` path.
fn ludion_with_stdin(args: &[&str], input: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ludion"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn the ludion binary");
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(input.as_bytes())
        .expect("write to stdin");
    child
        .wait_with_output()
        .expect("failed to run the ludion binary")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// A fresh temp directory named after the test, removed and recreated so a rerun starts clean.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-voxel-cmd-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn write(path: &Path, text: &str) {
    std::fs::write(path, text).expect("write fixture");
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).expect("read file")
}

/// A hand-authored model with a header docstring and per-layer comments — the bytes an edit must
/// not disturb.
const TOTEM: &str = "\
format: 1
kind: voxel
name: totem
# Totem — the CLI fixture. Front = row z=0.
size:
  - 4
  - 2
  - 1
scale: 0.25
palette:
  A:
    color:
      - 1.0
      - 1.0
      - 1.0
      - 1.0
  B:
    color:
      - 0.0
      - 0.0
      - 1.0
      - 1.0
slices:
  # y0 — base
  -
    - AAAA
  # y1 — top
  -
    - \".AA.\"
";

// --- new ----------------------------------------------------------------------------------------

/// `voxel new` writes a validated, canonical, all-empty model that `check-scene` accepts, carrying
/// the header template and one anchor comment per layer.
#[test]
fn new_scaffolds_a_checkable_model() {
    let dir = scratch("new");
    let out = dir.join("golem.voxel.yaml");
    let output = ludion(&[
        "voxel",
        "new",
        out.to_str().unwrap(),
        "--size",
        "4,3,2",
        "--scale",
        "0.25",
    ]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(stdout(&output).contains("4x3x2"), "{}", stdout(&output));

    let text = read(&out);
    assert!(text.contains("name: golem"), "{text}");
    assert!(text.contains("scale: 0.25"), "{text}");
    assert!(text.contains("# Front = row z=0"), "{text}");
    assert!(text.contains("  # y0\n  -\n"), "{text}");
    assert_eq!(text.matches("    - \"....\"\n").count(), 6);

    let check = ludion(&["check-scene", out.to_str().unwrap()]);
    assert!(check.status.success(), "stderr: {}", stderr(&check));
    assert!(
        stdout(&check).contains("0 filled voxels"),
        "{}",
        stdout(&check)
    );
}

/// The scaffold refuses to overwrite: `new` is for births, `apply` is for edits.
#[test]
fn new_refuses_to_overwrite() {
    let dir = scratch("new-overwrite");
    let out = dir.join("totem.voxel.yaml");
    write(&out, TOTEM);
    let output = ludion(&["voxel", "new", out.to_str().unwrap(), "--size", "2,2,2"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("already exists"),
        "{}",
        stderr(&output)
    );
    assert_eq!(read(&out), TOTEM, "the existing model must be untouched");
}

/// `--palette-from` copies an existing model's colour language into the new one.
#[test]
fn new_copies_a_palette() {
    let dir = scratch("new-palette");
    let source = dir.join("totem.voxel.yaml");
    write(&source, TOTEM);
    let out = dir.join("sibling.voxel.yaml");
    let output = ludion(&[
        "voxel",
        "new",
        out.to_str().unwrap(),
        "--size",
        "2,2,2",
        "--palette-from",
        source.to_str().unwrap(),
    ]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let text = read(&out);
    assert!(text.contains("  A:\n"), "{text}");
    assert!(text.contains("  B:\n"), "{text}");
    assert!(
        stdout(&output).contains("2 palette entries"),
        "{}",
        stdout(&output)
    );
}

// --- apply --------------------------------------------------------------------------------------

/// The end-to-end surgical contract: one `set` op rewrites exactly one character of the file on
/// disk, comments and all other bytes intact.
#[test]
fn apply_edits_one_character_in_place() {
    let dir = scratch("apply");
    let model = dir.join("totem.voxel.yaml");
    write(&model, TOTEM);
    let ops = dir.join("edit.ops.yaml");
    write(
        &ops,
        "format: 1\nkind: voxel-ops\nops:\n  - op: set\n    at: \"0,1,0\"\n    glyph: B\n",
    );

    let output = ludion(&[
        "voxel",
        "apply",
        model.to_str().unwrap(),
        "--ops",
        ops.to_str().unwrap(),
    ]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let summary = stdout(&output);
    assert!(
        summary.contains("applied 1 op, 1 cell changed"),
        "{summary}"
    );
    assert!(summary.contains("surgical"), "{summary}");
    assert!(summary.contains("now 4x2x1"), "{summary}");

    let after = read(&model);
    assert_eq!(after, TOTEM.replace("\".AA.\"", "\"BAA.\""));
    let differing = TOTEM
        .chars()
        .zip(after.chars())
        .filter(|(a, b)| a != b)
        .count();
    assert_eq!(differing, 1, "exactly one character may change");
}

/// `--dry-run` prints the same summary and writes nothing at all.
#[test]
fn apply_dry_run_writes_nothing() {
    let dir = scratch("apply-dry");
    let model = dir.join("totem.voxel.yaml");
    write(&model, TOTEM);
    let ops = dir.join("edit.ops.yaml");
    write(
        &ops,
        "format: 1\nkind: voxel-ops\nops:\n  - op: box\n    from: \"0,0,0\"\n    to: \"3,1,0\"\n    \
         glyph: B\n",
    );

    let output = ludion(&[
        "voxel",
        "apply",
        model.to_str().unwrap(),
        "--ops",
        ops.to_str().unwrap(),
        "--dry-run",
    ]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(
        stdout(&output).contains("would apply"),
        "{}",
        stdout(&output)
    );
    assert!(
        stdout(&output).contains("dry run — nothing written"),
        "{}",
        stdout(&output)
    );
    assert_eq!(read(&model), TOTEM);
    // No stray temp file survives a dry run either.
    let leftovers: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".tmp"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "temp files left behind: {leftovers:?}"
    );
}

/// A model carrying `generated_from:` is write-only: apply refuses with L1132 naming the script and writes
/// nothing; --dry-run still previews.
#[test]
fn apply_refuses_a_generated_model() {
    let dir = scratch("apply-generated");
    let model = dir.join("gen.voxel.yaml");
    let generated = TOTEM.replace("scale: 0.25\n", "scale: 0.25\ngenerated_from: gen.mjs\n");
    write(&model, &generated);
    let ops = dir.join("edit.ops.yaml");
    write(
        &ops,
        "format: 1\nkind: voxel-ops\nops:\n  - op: set\n    at: \"0,0,0\"\n    glyph: B\n",
    );
    let args = |extra: &[&'static str]| {
        let mut args = vec![
            "voxel",
            "apply",
            model.to_str().unwrap(),
            "--ops",
            ops.to_str().unwrap(),
        ];
        args.extend_from_slice(extra);
        ludion(&args)
    };

    let output = args(&[]);
    assert!(!output.status.success());
    let err = stderr(&output);
    assert!(err.contains("L1132") && err.contains("gen.mjs"), "{err}");
    let script_path = ludion_core::paths::slash(&dir.join("gen.mjs"));
    assert!(
        err.contains(&format!("node {script_path} ")),
        "the help names the script relative to the model's directory: {err}"
    );
    assert_eq!(read(&model), generated);

    let dry = args(&["--dry-run"]);
    assert!(dry.status.success(), "stderr: {}", stderr(&dry));
    assert!(stdout(&dry).contains("would apply"), "{}", stdout(&dry));
    assert_eq!(read(&model), generated);
}

/// An edit that buries a cell says so in the footer, in both the dry-run and the apply path — the
/// ops-side half of `check-scene --exposure`. A model with nothing buried prints no such line.
#[test]
fn apply_reports_cells_the_edit_buried() {
    // A 3×3×3 shell with a hollow centre: filling that centre makes it invisible.
    let shell = "\
format: 1
kind: voxel
size:
  - 3
  - 3
  - 3
palette:
  A:
    color:
      - 1.0
      - 1.0
      - 1.0
      - 1.0
  B:
    color:
      - 0.0
      - 0.0
      - 1.0
      - 1.0
slices:
  -
    - AAA
    - AAA
    - AAA
  -
    - AAA
    - \"A.A\"
    - AAA
  -
    - AAA
    - AAA
    - AAA
";
    let dir = scratch("apply-exposure");
    let model = dir.join("shell.voxel.yaml");
    write(&model, shell);
    let ops = dir.join("bury.ops.yaml");
    write(
        &ops,
        "format: 1\nkind: voxel-ops\nops:\n  - op: set\n    at: \"1,1,1\"\n    glyph: B\n",
    );
    let args = [
        "voxel",
        "apply",
        model.to_str().unwrap(),
        "--ops",
        ops.to_str().unwrap(),
    ];

    // The hollow model itself buries nothing, so the footer stays silent about exposure until the
    // op lands — and a dry run reports it just as the real apply does.
    let dry = ludion(&[args.as_slice(), &["--dry-run"]].concat());
    assert!(dry.status.success(), "stderr: {}", stderr(&dry));
    assert!(
        stdout(&dry).contains("exposure: 1 fully enclosed cell (B 1)"),
        "{}",
        stdout(&dry)
    );

    let applied = ludion(&args);
    assert!(applied.status.success(), "stderr: {}", stderr(&applied));
    assert!(
        stdout(&applied).contains("exposure: 1 fully enclosed cell (B 1)"),
        "{}",
        stdout(&applied)
    );

    // Re-applying the same op changes nothing, and the totem (no interior at all) never mentions
    // exposure.
    let totem_dir = scratch("apply-exposure-silent");
    let totem = totem_dir.join("totem.voxel.yaml");
    write(&totem, TOTEM);
    let totem_ops = totem_dir.join("edit.ops.yaml");
    write(
        &totem_ops,
        "format: 1\nkind: voxel-ops\nops:\n  - op: set\n    at: \"0,1,0\"\n    glyph: B\n",
    );
    let quiet = ludion(&[
        "voxel",
        "apply",
        totem.to_str().unwrap(),
        "--ops",
        totem_ops.to_str().unwrap(),
    ]);
    assert!(quiet.status.success(), "stderr: {}", stderr(&quiet));
    assert!(!stdout(&quiet).contains("exposure:"), "{}", stdout(&quiet));
}

/// A `mirror` that discards a cell an earlier `set` wrote prints the `L1134` warning in both the
/// dry-run and the real apply path, and never fails the command — it is a lint.
#[test]
fn apply_reports_the_mirror_shadow_lint() {
    let dir = scratch("apply-mirror-shadow");
    let model = dir.join("totem.voxel.yaml");
    write(&model, TOTEM);
    let ops = dir.join("shadow.ops.yaml");
    // TOTEM is 4x2x1; y0 is "AAAA". Writing B at x=3 (the discarded half once mirrored keep-low)
    // then mirroring erases it straight back to 'A'.
    write(
        &ops,
        "format: 1\nkind: voxel-ops\nops:\n  - op: set\n    at: \"3,0,0\"\n    glyph: B\n  - op: \
         mirror\n    axis: x\n    keep: low\n",
    );
    let args = [
        "voxel",
        "apply",
        model.to_str().unwrap(),
        "--ops",
        ops.to_str().unwrap(),
    ];
    let expected = "op 1 (mirror x keep low) overwrote 1 cell that op 0 (set) wrote in the \
                    discarded half (x >= 2)";

    let dry = ludion(&[args.as_slice(), &["--dry-run"]].concat());
    assert!(dry.status.success(), "stderr: {}", stderr(&dry));
    assert!(stderr(&dry).contains("warning[L1134]"), "{}", stderr(&dry));
    assert!(stderr(&dry).contains(expected), "{}", stderr(&dry));

    let applied = ludion(&args);
    assert!(applied.status.success(), "stderr: {}", stderr(&applied));
    assert!(
        stderr(&applied).contains("warning[L1134]"),
        "{}",
        stderr(&applied)
    );
    assert!(stderr(&applied).contains(expected), "{}", stderr(&applied));
}

/// A failing op reports its `L1132` and leaves the model byte-identical — the property that keeps a
/// half-applied model out of a live viewport.
#[test]
fn a_failing_apply_leaves_the_file_untouched() {
    let dir = scratch("apply-fail");
    let model = dir.join("totem.voxel.yaml");
    write(&model, TOTEM);
    let ops = dir.join("edit.ops.yaml");
    write(
        &ops,
        "format: 1\nkind: voxel-ops\nops:\n  - op: set\n    at: \"0,0,0\"\n    glyph: B\n  \
         - op: set\n    at: \"99,0,0\"\n    glyph: A\n",
    );

    let output = ludion(&[
        "voxel",
        "apply",
        model.to_str().unwrap(),
        "--ops",
        ops.to_str().unwrap(),
    ]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("error[L1132]"),
        "{}",
        stderr(&output)
    );
    assert_eq!(read(&model), TOTEM);
}

/// A malformed ops document is an `L1131` with a JSON Pointer at the offending op, and the model is
/// never opened for writing.
#[test]
fn a_malformed_ops_document_is_l1131() {
    let dir = scratch("apply-ops-bad");
    let model = dir.join("totem.voxel.yaml");
    write(&model, TOTEM);
    let ops = dir.join("edit.ops.yaml");
    write(
        &ops,
        "format: 1\nkind: voxel-ops\nops:\n  - op: sett\n    at: \"0,0,0\"\n    glyph: B\n",
    );

    let output = ludion(&[
        "voxel",
        "apply",
        model.to_str().unwrap(),
        "--ops",
        ops.to_str().unwrap(),
    ]);
    assert!(!output.status.success());
    let errors = stderr(&output);
    assert!(errors.contains("error[L1131]"), "{errors}");
    assert!(errors.contains("/ops/0/op"), "{errors}");
    assert!(errors.contains("nearest match is `set`"), "{errors}");
    assert_eq!(read(&model), TOTEM);
}

/// A reshaping op re-emits canonically, carries the layer comments to their new y levels, and the
/// result still checks clean.
#[test]
fn apply_grow_reshapes_and_carries_comments() {
    let dir = scratch("apply-grow");
    let model = dir.join("totem.voxel.yaml");
    write(&model, TOTEM);
    let ops = dir.join("edit.ops.yaml");
    write(
        &ops,
        "format: 1\nkind: voxel-ops\nops:\n  - op: grow\n    low: \"0,1,0\"\n    high: \"0,0,0\"\n",
    );

    let output = ludion(&[
        "voxel",
        "apply",
        model.to_str().unwrap(),
        "--ops",
        ops.to_str().unwrap(),
    ]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(
        stdout(&output).contains("re-emitted canonically"),
        "{}",
        stdout(&output)
    );

    let after = read(&model);
    assert!(after.contains("# Totem — the CLI fixture"), "{after}");
    assert!(after.contains("  # y1 — base\n"), "{after}");
    let check = ludion(&["check-scene", model.to_str().unwrap()]);
    assert!(check.status.success(), "stderr: {}", stderr(&check));
    assert!(stdout(&check).contains("4x3x1"), "{}", stdout(&check));
}

// --- upscale --------------------------------------------------------------------------------------

/// A blank cubic model of `size`^3, one glyph, for the budget-crossing fixture — mirrors
/// `ludion-scene`'s own `blank()` test helper.
fn blank_model(size: u32) -> String {
    let row = ".".repeat(size as usize);
    let mut out = format!(
        "format: 1\nkind: voxel\nsize:\n  - {size}\n  - {size}\n  - {size}\n\
         palette:\n  A:\n    color:\n      - 1.0\n      - 1.0\n      - 1.0\n      - 1.0\n"
    );
    out.push_str("slices:\n");
    for _ in 0..size {
        out.push_str("  -\n");
        for _ in 0..size {
            out.push_str(&format!("    - \"{row}\"\n"));
        }
    }
    out
}

/// `upscale` writes a canonical, checkable model at the default `<stem>-x<factor>.voxel.yaml` path.
#[test]
fn upscale_writes_the_default_out_path() {
    let dir = scratch("upscale-default");
    let model = dir.join("totem.voxel.yaml");
    write(&model, TOTEM);

    let output = ludion(&["voxel", "upscale", model.to_str().unwrap(), "--factor", "3"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let summary = stdout(&output);
    assert!(summary.contains("4x2x1 -> 12x6x3 (x3)"), "{summary}");
    assert!(summary.contains("scale: 0.25 -> 0.0833"), "{summary}");

    let out = dir.join("totem-x3.voxel.yaml");
    assert!(out.exists(), "expected the default out path to exist");
    let check = ludion(&["check-scene", out.to_str().unwrap()]);
    assert!(check.status.success(), "stderr: {}", stderr(&check));
    assert!(stdout(&check).contains("12x6x3"), "{}", stdout(&check));

    // The source is untouched.
    assert_eq!(read(&model), TOTEM);
}

/// `--out` overrides the default path.
#[test]
fn upscale_honors_explicit_out() {
    let dir = scratch("upscale-out");
    let model = dir.join("totem.voxel.yaml");
    write(&model, TOTEM);
    let out = dir.join("nested").join("golem.voxel.yaml");
    std::fs::create_dir_all(out.parent().unwrap()).expect("mkdir");

    let output = ludion(&[
        "voxel",
        "upscale",
        model.to_str().unwrap(),
        "--factor",
        "2",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(out.exists());
    assert!(!dir.join("totem-x2.voxel.yaml").exists());
}

/// `upscale` never overwrites an existing file at the output path, default or explicit.
#[test]
fn upscale_refuses_to_overwrite() {
    let dir = scratch("upscale-overwrite");
    let model = dir.join("totem.voxel.yaml");
    write(&model, TOTEM);
    let out = dir.join("totem-x2.voxel.yaml");
    write(&out, "sentinel");

    let output = ludion(&["voxel", "upscale", model.to_str().unwrap(), "--factor", "2"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("already exists"),
        "{}",
        stderr(&output)
    );
    assert_eq!(
        read(&out),
        "sentinel",
        "the existing file must be untouched"
    );
}

/// A factor outside `2..=8` is rejected with the `L1132` "op inapplicable" family, and nothing is
/// written.
#[test]
fn upscale_rejects_a_factor_outside_the_range() {
    let dir = scratch("upscale-bad-factor");
    let model = dir.join("totem.voxel.yaml");
    write(&model, TOTEM);

    let output = ludion(&[
        "voxel",
        "upscale",
        model.to_str().unwrap(),
        "--factor",
        "12",
    ]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("error[L1132]"),
        "{}",
        stderr(&output)
    );
    assert!(
        !dir.join("totem-x12.voxel.yaml").exists(),
        "a rejected factor must write nothing"
    );
}

/// Upscaling can legitimately cross the cell-count style budget; the `L1115` warning must still
/// print (nothing about upscaling suppresses it — that stays the author's explicit
/// `budget_exempt: true`).
#[test]
fn upscale_prints_the_budget_warning_when_crossed() {
    let dir = scratch("upscale-budget");
    let model = dir.join("cube.voxel.yaml");
    write(&model, &blank_model(40)); // 40^3 = 64,000, under the 64^3 (262,144) budget.

    let output = ludion(&[
        "voxel",
        "upscale",
        model.to_str().unwrap(),
        "--factor",
        "4", // 160^3 = 4,096,000, well over.
    ]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(
        stderr(&output).contains("warning[L1115]"),
        "{}",
        stderr(&output)
    );
    assert!(dir.join("cube-x4.voxel.yaml").exists());
}

/// The realism proof: upscale the committed `lantern-x3` model — itself an upscale — 3x into a
/// scratch directory (the source is never opened for writing) and check the result clean: the
/// "generate 216k cells from scratch" problem this verb exists to solve.
#[test]
fn upscale_a_committed_upscaled_model_end_to_end() {
    let source = PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/voxels/lantern-x3.voxel.yaml"
    ));
    assert!(source.exists(), "the frozen lantern-x3 model must exist");
    let before = read(&source);

    let dir = scratch("upscale-lantern-x3");
    let out = dir.join("lantern-x9.voxel.yaml");
    let output = ludion(&[
        "voxel",
        "upscale",
        source.to_str().unwrap(),
        "--factor",
        "3",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));

    let check = ludion(&["check-scene", out.to_str().unwrap()]);
    assert!(check.status.success(), "stderr: {}", stderr(&check));
    assert!(stdout(&check).contains("45x81x45"), "{}", stdout(&check));

    // Read-only: the committed source is byte-identical after the run.
    assert_eq!(read(&source), before);
}

// --- diff ---------------------------------------------------------------------------------------

/// Two identical models diff clean and exit zero.
#[test]
fn diff_of_identical_models_exits_zero() {
    let dir = scratch("diff-same");
    let a = dir.join("a.voxel.yaml");
    let b = dir.join("b.voxel.yaml");
    write(&a, TOTEM);
    write(&b, TOTEM);
    let output = ludion(&["voxel", "diff", a.to_str().unwrap(), b.to_str().unwrap()]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let report = stdout(&output);
    assert!(report.contains("palette: identical"), "{report}");
    assert!(report.contains("cells: identical"), "{report}");
}

/// A changed cell and a changed palette both show, and the exit code is non-zero so a script can
/// gate on "did my edit land".
#[test]
fn diff_reports_cells_and_palette_and_exits_non_zero() {
    let dir = scratch("diff-changed");
    let a = dir.join("a.voxel.yaml");
    let b = dir.join("b.voxel.yaml");
    write(&a, TOTEM);
    write(
        &b,
        &TOTEM.replace("\".AA.\"", "\"BAA.\"").replace(
            "      - 0.0\n      - 0.0\n      - 1.0",
            "      - 0.0\n      - 1.0\n      - 1.0",
        ),
    );
    let output = ludion(&["voxel", "diff", a.to_str().unwrap(), b.to_str().unwrap()]);
    assert!(!output.status.success(), "a difference must exit non-zero");
    let report = stdout(&output);
    assert!(report.contains("palette ~B: changed"), "{report}");
    assert!(report.contains("y1: 1 cell changed"), "{report}");
    assert!(report.contains("(0,0) '.'->'B'"), "{report}");
    assert!(report.contains("total: 1 cell changed"), "{report}");
}

/// Different sizes are stated prominently and the overlapping region is diffed anyway.
#[test]
fn diff_states_a_size_mismatch() {
    let dir = scratch("diff-size");
    let a = dir.join("a.voxel.yaml");
    let b = dir.join("b.voxel.yaml");
    write(&a, TOTEM);
    write(
        &b,
        &TOTEM
            .replace("  - 4\n  - 2", "  - 2\n  - 2")
            .replace("AAAA", "AA")
            .replace("\".AA.\"", "\".A\""),
    );
    let output = ludion(&["voxel", "diff", a.to_str().unwrap(), b.to_str().unwrap()]);
    assert!(!output.status.success());
    let report = stdout(&output);
    assert!(report.contains("SIZE DIFFERS: 4x2x1 vs 2x2x1"), "{report}");
    assert!(report.contains("overlapping 2x2x1"), "{report}");
}

/// A model that does not validate fails the diff with its own code rather than reporting nonsense.
#[test]
fn diff_rejects_an_invalid_model() {
    let dir = scratch("diff-invalid");
    let a = dir.join("a.voxel.yaml");
    let b = dir.join("b.voxel.yaml");
    write(&a, TOTEM);
    write(&b, &TOTEM.replace("    - AAAA", "    - AAA"));
    let output = ludion(&["voxel", "diff", a.to_str().unwrap(), b.to_str().unwrap()]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("error[L1114]"),
        "{}",
        stderr(&output)
    );
}

/// diff(1) exit codes: 0 identical, 1 differ, 2 a model cannot be read or validated.
#[test]
fn diff_exit_codes_are_0_1_2() {
    let dir = scratch("diff-codes");
    let a = dir.join("a.voxel.yaml");
    let b = dir.join("b.voxel.yaml");
    let bad = dir.join("bad.voxel.yaml");
    write(&a, TOTEM);
    write(&b, &TOTEM.replace("\".AA.\"", "\"BAA.\""));
    write(&bad, &TOTEM.replace("kind: voxel", "kind: nope"));
    let code = |x: &Path, y: &Path| {
        ludion(&["voxel", "diff", x.to_str().unwrap(), y.to_str().unwrap()])
            .status
            .code()
    };
    assert_eq!(code(&a, &a), Some(0));
    assert_eq!(code(&a, &b), Some(1));
    assert_eq!(code(&a, &dir.join("missing.voxel.yaml")), Some(2));
    assert_eq!(code(&a, &bad), Some(2));
}

// --- from-json ------------------------------------------------------------------------------------

/// A minimal, fully-filled generator intermediate (no disconnected-component warning to route
/// around): one layer, two rows, every cell filled.
const GEM_JSON: &str = "{\
  \"format\": 1, \"kind\": \"voxel-gen\", \"name\": \"gem\",\
  \"size\": [2, 1, 2], \"scale\": 0.5, \"generated_from\": \"gem.voxel.gen.mjs\",\
  \"palette\": {\"A\": {\"color\": [1.0, 0.0, 0.0, 1.0]}},\
  \"header\": [\"a gem \\u2014 testing\"],\
  \"layer_comments\": {\"0\": \"the only layer\"},\
  \"layers\": [[\"AA\", \"AA\"]]\
}";

/// The happy path: a valid intermediate through `--in`/`--out` writes a canonical model that
/// `check-scene` accepts, carrying `generated_from:`, the header comment, and the layer comment.
#[test]
fn from_json_writes_a_checkable_generated_model() {
    let dir = scratch("from-json-new");
    let input = dir.join("gem.json");
    write(&input, GEM_JSON);
    let out = dir.join("gem.voxel.yaml");

    let output = ludion(&[
        "voxel",
        "from-json",
        "--in",
        input.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(
        stdout(&output).contains("generated 2x1x2, 4 filled, 1 component, 1 palette entry"),
        "{}",
        stdout(&output)
    );
    assert!(
        stdout(&output).contains("generated_from: gem.voxel.gen.mjs"),
        "{}",
        stdout(&output)
    );

    let text = read(&out);
    assert!(text.contains("generated_from: gem.voxel.gen.mjs"), "{text}");
    assert!(text.contains("# a gem — testing"), "{text}");
    assert!(text.contains("  # the only layer\n"), "{text}");

    let check = ludion(&["check-scene", out.to_str().unwrap()]);
    assert!(check.status.success(), "stderr: {}", stderr(&check));
    assert!(
        stdout(&check).contains("4 filled voxels"),
        "{}",
        stdout(&check)
    );
}

/// `--check` against a byte-identical committed model exits 0, prints "identical", and writes
/// nothing (no temp files, the file's own bytes unchanged).
#[test]
fn from_json_check_is_green_on_a_matching_file() {
    let dir = scratch("from-json-check-green");
    let input = dir.join("gem.json");
    write(&input, GEM_JSON);
    let model = dir.join("gem.voxel.yaml");

    let generate = ludion(&[
        "voxel",
        "from-json",
        "--in",
        input.to_str().unwrap(),
        "--out",
        model.to_str().unwrap(),
    ]);
    assert!(generate.status.success(), "stderr: {}", stderr(&generate));
    let before = read(&model);

    let output = ludion(&[
        "voxel",
        "from-json",
        "--in",
        input.to_str().unwrap(),
        "--check",
        model.to_str().unwrap(),
    ]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(
        stdout(&output).contains("identical to its generator's output"),
        "{}",
        stdout(&output)
    );
    assert_eq!(read(&model), before, "--check must write nothing");
    let leftovers: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".tmp"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "temp files left behind: {leftovers:?}"
    );
}

/// `--check` against a drifted committed model exits non-zero, names the file and the first
/// differing line, and leaves the file byte-identical (the write-free contract the drift gate
/// relies on).
#[test]
fn from_json_check_reports_drift_and_leaves_the_file_untouched() {
    let dir = scratch("from-json-check-drift");
    let input = dir.join("gem.json");
    write(&input, GEM_JSON);
    let model = dir.join("gem.voxel.yaml");

    let generate = ludion(&[
        "voxel",
        "from-json",
        "--in",
        input.to_str().unwrap(),
        "--out",
        model.to_str().unwrap(),
    ]);
    assert!(generate.status.success(), "stderr: {}", stderr(&generate));
    let mut drifted = read(&model);
    drifted.push_str("# a stray hand-authored line\n");
    write(&model, &drifted);

    let output = ludion(&[
        "voxel",
        "from-json",
        "--in",
        input.to_str().unwrap(),
        "--check",
        model.to_str().unwrap(),
    ]);
    assert!(!output.status.success());
    let errors = stderr(&output);
    assert!(errors.contains("error[L1133]"), "{errors}");
    assert!(errors.contains("DRIFT"), "{errors}");
    assert!(
        errors.contains(model.to_str().unwrap()),
        "the report must name the drifted file: {errors}"
    );
    assert_eq!(read(&model), drifted, "a failed --check must not write");
}

/// An unknown top-level key in the intermediate is `L1133`, with a JSON Pointer at the offending
/// key and a nearest-match suggestion.
#[test]
fn from_json_shape_error_is_l1133_with_a_pointer_and_suggestion() {
    let dir = scratch("from-json-shape-error");
    let input = dir.join("bad.json");
    write(&input, &GEM_JSON.replace("\"layers\"", "\"layer\""));
    let out = dir.join("bad.voxel.yaml");

    let output = ludion(&[
        "voxel",
        "from-json",
        "--in",
        input.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(!output.status.success());
    let errors = stderr(&output);
    assert!(errors.contains("error[L1133]"), "{errors}");
    assert!(errors.contains("/layer"), "{errors}");
    assert!(errors.contains("nearest match is `layers`"), "{errors}");
    assert!(!out.exists(), "a shape error must write nothing");
}

/// A shape-valid but model-invalid intermediate (a row whose width does not match `size`) surfaces
/// the normal `L1114`, the same code a hand-authored model would raise — the intermediate only owns
/// its own shape, not the model's data consistency.
#[test]
fn from_json_model_invalid_intermediate_is_l1114() {
    let dir = scratch("from-json-model-invalid");
    let input = dir.join("bad-row.json");
    write(
        &input,
        &GEM_JSON.replace("[\"AA\", \"AA\"]", "[\"A\", \"AA\"]"),
    );
    let out = dir.join("bad-row.voxel.yaml");

    let output = ludion(&[
        "voxel",
        "from-json",
        "--in",
        input.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(!output.status.success());
    let errors = stderr(&output);
    assert!(errors.contains("error[L1114]"), "{errors}");
    assert!(errors.contains("/slices/0/0"), "{errors}");
    assert!(
        !out.exists(),
        "a model-invalid intermediate must write nothing"
    );
}

/// Neither `--out` nor `--check` is a clap usage error (the `ArgGroup` requires exactly one).
#[test]
fn from_json_requires_out_or_check() {
    let dir = scratch("from-json-neither");
    let input = dir.join("gem.json");
    write(&input, GEM_JSON);

    let output = ludion(&["voxel", "from-json", "--in", input.to_str().unwrap()]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("required arguments were not provided"),
        "{}",
        stderr(&output)
    );
}

/// Both `--out` and `--check` together is also a clap usage error (the group forbids both).
#[test]
fn from_json_rejects_both_out_and_check() {
    let dir = scratch("from-json-both");
    let input = dir.join("gem.json");
    write(&input, GEM_JSON);
    let out = dir.join("gem.voxel.yaml");
    let check = dir.join("other.voxel.yaml");

    let output = ludion(&[
        "voxel",
        "from-json",
        "--in",
        input.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--check",
        check.to_str().unwrap(),
    ]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("cannot be used with"),
        "{}",
        stderr(&output)
    );
    assert!(!out.exists());
}

// --- from-json --validate ---------------------------------------------------------------------------

/// A generator intermediate whose two filled cells sit far enough apart to land in separate
/// 6-connected components — the fixture the gap-report tests use.
const DISCONNECTED_JSON: &str = "{\
  \"format\": 1, \"kind\": \"voxel-gen\", \"name\": \"stray\",\
  \"size\": [4, 1, 4],\
  \"palette\": {\"A\": {\"color\": [1.0, 0.0, 0.0, 1.0]}},\
  \"layers\": [[\"A...\", \"....\", \"...A\", \"....\"]]\
}";

/// `--validate` prints the same summary shape `--out` reports, plus the `— validated, nothing
/// written` footer, and writes nothing at all — no `--out` needed.
#[test]
fn from_json_validate_reports_and_writes_nothing() {
    let dir = scratch("from-json-validate");
    let input = dir.join("gem.json");
    write(&input, GEM_JSON);

    let output = ludion(&[
        "voxel",
        "from-json",
        "--in",
        input.to_str().unwrap(),
        "--validate",
    ]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(
        stdout(&output)
            .contains("2x1x2, 4 filled, 1 component, 1 palette entry — validated, nothing written"),
        "{}",
        stdout(&output)
    );
    let entries: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert_eq!(
        entries.len(),
        1,
        "--validate must write nothing beyond the input: {entries:?}"
    );
}

/// A malformed intermediate is `L1133` under `--validate`, same as `--out` — the validation is the
/// same code path, only the write is skipped.
#[test]
fn from_json_validate_invalid_intermediate_is_l1133() {
    let dir = scratch("from-json-validate-invalid");
    let input = dir.join("bad.json");
    write(&input, &GEM_JSON.replace("\"layers\"", "\"layer\""));

    let output = ludion(&[
        "voxel",
        "from-json",
        "--in",
        input.to_str().unwrap(),
        "--validate",
    ]);
    assert!(!output.status.success());
    let errors = stderr(&output);
    assert!(errors.contains("error[L1133]"), "{errors}");
    assert!(errors.contains("nearest match is `layers`"), "{errors}");
}

/// `--validate` reads stdin exactly like `--out`/`--check` do when `--in` is omitted.
#[test]
fn from_json_validate_reads_stdin() {
    let output = ludion_with_stdin(&["voxel", "from-json", "--validate"], GEM_JSON);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(
        stdout(&output).contains("validated, nothing written"),
        "{}",
        stdout(&output)
    );
}

/// A disconnected generated model prints the `L1129` gap report under `--validate` — the same
/// warning `--out` would print, just with nothing written.
#[test]
fn from_json_validate_prints_the_gap_report_for_a_disconnected_model() {
    let dir = scratch("from-json-validate-disconnected");
    let input = dir.join("stray.json");
    write(&input, DISCONNECTED_JSON);

    let output = ludion(&[
        "voxel",
        "from-json",
        "--in",
        input.to_str().unwrap(),
        "--validate",
    ]);
    assert!(
        output.status.success(),
        "a lint must not fail the command: {}",
        stderr(&output)
    );
    let errors = stderr(&output);
    assert!(errors.contains("warning[L1129]"), "{errors}");
    assert!(errors.contains("disconnected components"), "{errors}");
}

/// `--check` prints the same `L1129` warning `--validate` does — even on the green path, where the
/// committed file matches and the exit code is still governed purely by the byte comparison.
#[test]
fn from_json_check_prints_warnings_on_the_green_path() {
    let dir = scratch("from-json-check-warnings");
    let input = dir.join("stray.json");
    write(&input, DISCONNECTED_JSON);
    let model = dir.join("stray.voxel.yaml");

    let generate = ludion(&[
        "voxel",
        "from-json",
        "--in",
        input.to_str().unwrap(),
        "--out",
        model.to_str().unwrap(),
    ]);
    assert!(generate.status.success(), "stderr: {}", stderr(&generate));

    let output = ludion(&[
        "voxel",
        "from-json",
        "--in",
        input.to_str().unwrap(),
        "--check",
        model.to_str().unwrap(),
    ]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let errors = stderr(&output);
    assert!(errors.contains("warning[L1129]"), "{errors}");
    assert!(
        stdout(&output).contains("identical to its generator's output"),
        "{}",
        stdout(&output)
    );
}

/// The three-way `ArgGroup` (`--out`/`--check`/`--validate`) still enforces exactly one now that
/// `--validate` joined it.
#[test]
fn from_json_validate_conflicts_with_out() {
    let dir = scratch("from-json-validate-conflicts");
    let input = dir.join("gem.json");
    write(&input, GEM_JSON);
    let out = dir.join("gem.voxel.yaml");

    let output = ludion(&[
        "voxel",
        "from-json",
        "--in",
        input.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--validate",
    ]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("cannot be used with"),
        "{}",
        stderr(&output)
    );
    assert!(!out.exists());
}

// --- the format keys ------------------------------------------------------------------------------

/// `check-scene` names the two provenance keys in its summary: `generated` says the script owns the
/// file, `budget-exempt` says the missing `L1115` was authored.
#[test]
fn check_scene_reports_the_provenance_keys() {
    let dir = scratch("provenance");
    let model = dir.join("totem.voxel.yaml");
    write(
        &model,
        &TOTEM.replace(
            "palette:\n",
            "generated_from: totem.voxel.gen.mjs\nbudget_exempt: true\npalette:\n",
        ),
    );
    let output = ludion(&["check-scene", model.to_str().unwrap()]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let summary = stdout(&output);
    assert!(
        summary.contains("generated from totem.voxel.gen.mjs"),
        "{summary}"
    );
    assert!(summary.contains("budget-exempt"), "{summary}");
}

/// `ludion explain` serves both new codes — the discovery path a diagnostic points at.
#[test]
fn the_new_codes_are_explainable() {
    for code in ["L1131", "L1132"] {
        let output = ludion(&["explain", code]);
        assert!(output.status.success(), "{code}: {}", stderr(&output));
        assert!(
            stdout(&output).contains("voxel"),
            "{code}: {}",
            stdout(&output)
        );
    }
}

/// `ludion explain L1134` (the mirror-shadow lint) is registered too.
#[test]
fn the_mirror_shadow_lint_is_explainable() {
    let output = ludion(&["explain", "L1134"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("mirror"), "{}", stdout(&output));
}

// --- L1135: the double-encoding canary ------------------------------------------------------------
//
// Every non-ASCII character in these fixtures is written as an escape on purpose: the defect under
// test is a file whose bytes went through UTF-8 twice, and a test that spells the mojibake as
// literal source characters is one careless re-save away from testing nothing.

/// `GEM_JSON`'s em-dash, but corrupted the way a PowerShell pipe corrupts it: U+2014's bytes
/// (E2 80 94) read as CP1252 give `â` `€` `"`, and re-encoding those is what lands in the file.
fn double_encoded_json() -> String {
    GEM_JSON.replace("\\u2014", "\u{e2}\u{20ac}\u{201d}")
}

/// The false-positive guard: ordinary prose whose accented letters are `â` and `Ã` — the same lead
/// characters the heuristic keys on — followed by ASCII, plus a correctly encoded em-dash.
fn accented_json() -> String {
    GEM_JSON.replace(
        "a gem \\u2014 testing",
        "ch\u{e2}teau, S\u{e3}o Paulo, caf\u{e9} \u{2014} testing",
    )
}

/// A doubly-encoded intermediate warns with `L1135` and still writes the model — the mojibake is
/// already in the text, so refusing would only leave the author without the file *or* the reason.
#[test]
fn from_json_warns_on_a_double_encoded_intermediate() {
    let dir = scratch("from-json-double-encoded");
    let input = dir.join("gem.json");
    write(&input, &double_encoded_json());
    let out = dir.join("gem.voxel.yaml");

    let output = ludion(&[
        "voxel",
        "from-json",
        "--in",
        input.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "a lint must not fail the command: {}",
        stderr(&output)
    );
    let errors = stderr(&output);
    assert!(errors.contains("warning[L1135]"), "{errors}");
    assert!(
        errors.contains("double-encoded character sequence"),
        "{errors}"
    );
    assert!(errors.contains("out: process.argv[2]"), "{errors}");
    assert!(errors.contains("--in model.voxel.json"), "{errors}");
    assert!(out.exists(), "the model is still written");
}

/// The same scan runs on the stdin form — which is the one that gets corrupted in the first place.
#[test]
fn from_json_warns_on_double_encoded_stdin() {
    let output = ludion_with_stdin(
        &["voxel", "from-json", "--validate"],
        &double_encoded_json(),
    );
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(
        stderr(&output).contains("warning[L1135]"),
        "{}",
        stderr(&output)
    );
}

/// The heuristic stays quiet on plain ASCII and on ordinary accented text. A canary that cries on
/// `ch\u{e2}teau` is a canary nobody reads.
#[test]
fn from_json_is_silent_on_clean_and_on_ordinary_accented_text() {
    for (name, source) in [
        ("clean", GEM_JSON.to_string()),
        ("accented", accented_json()),
    ] {
        let dir = scratch(&format!("from-json-no-mojibake-{name}"));
        let input = dir.join("gem.json");
        write(&input, &source);

        let output = ludion(&[
            "voxel",
            "from-json",
            "--in",
            input.to_str().unwrap(),
            "--validate",
        ]);
        assert!(output.status.success(), "{name}: {}", stderr(&output));
        assert!(
            !stderr(&output).contains("L1135"),
            "{name}: {}",
            stderr(&output)
        );
    }
}

/// `ludion explain L1135` serves the fix the warning points at.
#[test]
fn the_double_encoding_lint_is_explainable() {
    let output = ludion(&["explain", "L1135"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("PowerShell"), "{text}");
    assert!(text.contains("--in"), "{text}");
}

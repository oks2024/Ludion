//! Integration tests for `ludion render --model <FILE>`, driven against the built binary.
//!
//! Mirrors `render.rs`'s style: cross-process determinism is the headline (Phase 11's model-view
//! harness at the CLI level), skipping gracefully when the machine has no GPU adapter (L2003). The
//! `--scene`/`--model`/`--views` arg-wiring tests (conflicts, bad values) are pure clap and need no
//! GPU. `--scene`-mode behavior itself is untouched — see `render.rs`, unmodified by this change.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A small, asymmetric voxel fixture (3×2×2, 4 cells across two non-identical slices) written fresh
/// per test into `CARGO_TARGET_TMPDIR` so tests don't share or race on a checked-in file.
const FIXTURE_VOXEL: &str = r#"format: 1
kind: voxel
name: fixture
size:
  - 3
  - 2
  - 2
palette:
  A:
    color:
      - 1.0
      - 0.0
      - 0.0
      - 1.0
slices:
  -
    - "A.."
    - ".A."
  -
    - "A.A"
    - "..."
"#;

fn scene_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/scenes/empty.yaml")
}

fn target_tmpdir() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
}

/// Writes the fixture voxel doc to `CARGO_TARGET_TMPDIR/<name>` and returns its path.
fn write_voxel_fixture(name: &str) -> PathBuf {
    let path = target_tmpdir().join(name);
    std::fs::write(&path, FIXTURE_VOXEL).expect("fixture voxel doc writes");
    path
}

/// The `parts/body.part.yaml` a single-part assembly fixture names — the same 3×2×2 grid as
/// [`FIXTURE_VOXEL`], so the two fixtures' bounds line up for a lineup that mixes both kinds.
const FIXTURE_PART: &str = r#"format: 1
kind: part
size:
  - 3
  - 2
  - 2
slices:
  -
    - "A.."
    - ".A."
  -
    - "A.A"
    - "..."
"#;

/// A single-part `kind: assembly` fixture whose one part is [`FIXTURE_PART`], placed at the assembly
/// origin — the module doc's "the rest pose is the assembled grid" made concrete, and (per
/// `mesh_assembly`'s own pinned proof) meshed to bounds identical to [`FIXTURE_VOXEL`]'s.
fn fixture_assembly(size: &str) -> String {
    format!(
        r#"format: 1
kind: assembly
name: fixture
size:
{size}
palette:
  A:
    color:
      - 1.0
      - 0.0
      - 0.0
      - 1.0
parts:
  - name: body
    file: parts/body.part.yaml
    place:
      - 0
      - 0
      - 0
"#
    )
}

const FIXTURE_ASSEMBLY_SIZE: &str = "  - 3\n  - 2\n  - 2";

/// Writes an assembly fixture (and its one part file) under `dir`, naming the assembly
/// `<stem>.assembly.yaml` and the part `parts/body.part.yaml` beside it — [`FIXTURE_ASSEMBLY_SIZE`]
/// unless overridden, so a caller can shrink/grow the assembled grid relative to its one part.
fn write_assembly_fixture_sized(dir: &Path, stem: &str, size: &str) -> PathBuf {
    std::fs::create_dir_all(dir.join("parts")).expect("parts dir creates");
    std::fs::write(dir.join("parts").join("body.part.yaml"), FIXTURE_PART)
        .expect("fixture part doc writes");
    let path = dir.join(format!("{stem}.assembly.yaml"));
    std::fs::write(&path, fixture_assembly(size)).expect("fixture assembly doc writes");
    path
}

/// [`write_assembly_fixture_sized`] at the part's own size — the assembly's grid *is* the part's, so
/// meshing it renders identically to [`FIXTURE_VOXEL`] (`mesh_assembly`'s own pinned equivalence).
fn write_assembly_fixture(dir: &Path, stem: &str) -> PathBuf {
    write_assembly_fixture_sized(dir, stem, FIXTURE_ASSEMBLY_SIZE)
}

fn run_ludion(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ludion"))
        .args(args)
        .output()
        .expect("failed to run the ludion binary")
}

fn run_ludion_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ludion"))
        .args(args)
        .current_dir(dir)
        .output()
        .expect("failed to run the ludion binary")
}

/// Whether a failed `render` was due to a missing GPU adapter (L2003), in which case the test skips.
fn is_no_adapter(output: &Output) -> bool {
    ludion_core::require_gpu::skip_no_adapter(
        String::from_utf8_lossy(&output.stderr).contains("error[L2003]"),
    )
}

/// Decodes a PNG file's dimensions (width, height).
fn png_dimensions(path: &Path) -> (u32, u32) {
    let file = std::fs::File::open(path).expect("PNG exists");
    let decoder = png::Decoder::new(std::io::BufReader::new(file));
    let reader = decoder.read_info().expect("valid PNG");
    let info = reader.info();
    (info.width, info.height)
}

/// **Cross-process determinism.** Two invocations of the binary rendering the same model sheet write
/// byte-identical PNGs. Skips when no GPU adapter is available.
#[test]
fn needs_gpu_two_processes_render_identical_model_sheet() {
    let model = write_voxel_fixture("fixture_determinism.voxel.yaml");
    let first_out = target_tmpdir().join("model_sheet_1.png");
    let second_out = target_tmpdir().join("model_sheet_2.png");

    let args1 = [
        "render",
        "--model",
        model.to_str().unwrap(),
        "--out",
        first_out.to_str().unwrap(),
        "--width",
        "300",
        "--height",
        "200",
    ];
    let first = run_ludion(&args1);
    if !first.status.success() {
        if is_no_adapter(&first) {
            eprintln!("SKIPPED two_processes_render_identical_model_sheet: no GPU adapter");
            return;
        }
        panic!(
            "render --model failed: {}",
            String::from_utf8_lossy(&first.stderr)
        );
    }

    let args2 = [
        "render",
        "--model",
        model.to_str().unwrap(),
        "--out",
        second_out.to_str().unwrap(),
        "--width",
        "300",
        "--height",
        "200",
    ];
    let second = run_ludion(&args2);
    assert!(
        second.status.success(),
        "second render --model failed: {}",
        String::from_utf8_lossy(&second.stderr)
    );

    let a = std::fs::read(&first_out).expect("first sheet PNG exists");
    let b = std::fs::read(&second_out).expect("second sheet PNG exists");
    assert_eq!(
        a, b,
        "cross-process model-sheet render determinism is broken"
    );
    assert_eq!(&a[1..4], b"PNG", "output is a PNG");
}

/// A `--views` subset produces a smaller sheet than the default all-six, at the same
/// `--width`/`--height`.
///
/// Per `model_sheet.rs`'s layout rule (`rows = 1 if N ≤ 3 else 2`, `cols = ceil(N/rows)`, cells
/// dividing the requested size by integer division), a *smaller* view count does not uniformly
/// shrink the sheet: an N ≤ 3 request always gets the full requested height (`rows == 1` never
/// floors), so e.g. `front,iso` (N=2) is bounded *at least* as large as all six on the height axis.
/// The reliable way to get a strictly smaller sheet is to stay in the `rows == 2` bucket (4–6
/// views, same as all six) and pick a request whose narrower `cols` floor-divides `--width` worse:
/// `front,back,left,right` (N=4, `cols=2`) vs. all six (N=6, `cols=3`) at `--width 603 --height
/// 400` gives `floor(603/2)*2 = 602 < floor(603/3)*3 = 603`, with height identical (400 is even, so
/// neither loses a row) — a verified, deterministic 602×400 vs. 603×400. Skips when no GPU adapter
/// is available.
#[test]
fn needs_gpu_views_subset_produces_a_smaller_sheet_than_all_six() {
    let model = write_voxel_fixture("fixture_views.voxel.yaml");
    let all_out = target_tmpdir().join("model_sheet_all.png");
    let partial_out = target_tmpdir().join("model_sheet_partial.png");

    let all_args = [
        "render",
        "--model",
        model.to_str().unwrap(),
        "--out",
        all_out.to_str().unwrap(),
        "--width",
        "603",
        "--height",
        "400",
    ];
    let all = run_ludion(&all_args);
    if !all.status.success() {
        if is_no_adapter(&all) {
            eprintln!("SKIPPED views_subset_produces_a_smaller_sheet_than_all_six: no GPU adapter");
            return;
        }
        panic!(
            "render --model (all views) failed: {}",
            String::from_utf8_lossy(&all.stderr)
        );
    }

    let partial_args = [
        "render",
        "--model",
        model.to_str().unwrap(),
        "--views",
        "front,back,left,right",
        "--out",
        partial_out.to_str().unwrap(),
        "--width",
        "603",
        "--height",
        "400",
    ];
    let partial = run_ludion(&partial_args);
    assert!(
        partial.status.success(),
        "render --model --views front,back,left,right failed: {}",
        String::from_utf8_lossy(&partial.stderr)
    );

    let (all_w, all_h) = png_dimensions(&all_out);
    let (partial_w, partial_h) = png_dimensions(&partial_out);
    assert_eq!((all_w, all_h), (603, 400), "all-six sheet dims regressed");
    assert!(
        (partial_w as u64 * partial_h as u64) < (all_w as u64 * all_h as u64),
        "expected a smaller sheet for the 4-view subset: all={all_w}x{all_h}, partial={partial_w}x{partial_h}"
    );
}

/// A scene YAML passed to `--model` fails with `L1117` (no `kind:` key) — validated before any GPU
/// adapter is requested, so this needs no adapter skip.
#[test]
fn scene_passed_as_model_fails_with_l1117() {
    let scene = scene_fixture();
    let out = target_tmpdir().join("model_from_scene_should_not_exist.png");

    let output = run_ludion(&[
        "render",
        "--model",
        scene.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(
        !output.status.success(),
        "expected a non-zero exit rendering a scene as a model"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("error[L1117]"),
        "expected an L1117 diagnostic: {stderr}"
    );
    assert!(!out.exists(), "no PNG should be written on failure");
}

/// `--scene` and `--model` together is a clap arg-group conflict, not an engine error.
#[test]
fn scene_and_model_together_is_a_clap_error() {
    let scene = scene_fixture();
    let model = write_voxel_fixture("fixture_conflict.voxel.yaml");
    let out = target_tmpdir().join("should_not_render.png");

    let output = run_ludion(&[
        "render",
        "--scene",
        scene.to_str().unwrap(),
        "--model",
        model.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(
        !output.status.success(),
        "expected a non-zero exit for --scene + --model together"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cannot be used with"),
        "expected a clap conflict message: {stderr}"
    );
}

/// `--rig` is a `--model`-only flag: pairing it with `--scene` is a clap conflict, not an engine
/// error. (`--rig` carries `conflicts_with = "scene"` rather than `requires = "model"` — clap 4.6's
/// `requires` silently no-ops when its target carries a `conflicts_with`, and `model` does.)
#[test]
fn rig_with_scene_is_a_clap_error() {
    let scene = scene_fixture();
    let out = target_tmpdir().join("should_not_render_rig.png");

    let output = run_ludion(&[
        "render",
        "--scene",
        scene.to_str().unwrap(),
        "--rig",
        scene.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(
        !output.status.success(),
        "expected a non-zero exit for --scene + --rig together"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cannot be used with"),
        "expected a clap conflict message: {stderr}"
    );
    assert!(!out.exists(), "no PNG should be written on a clap conflict");
}

/// An invalid `--views` token is a clap value-parsing error naming the bad value.
#[test]
fn invalid_views_token_is_a_clap_error() {
    let model = write_voxel_fixture("fixture_bad_views.voxel.yaml");

    let output = run_ludion(&[
        "render",
        "--model",
        model.to_str().unwrap(),
        "--views",
        "front,sideways",
    ]);
    assert!(
        !output.status.success(),
        "expected a non-zero exit for an invalid --views token"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("sideways"),
        "expected the bad value named in the error: {stderr}"
    );
}

/// `--set` reaches a model's palette, echoes the change above the summary, and leaves the document
/// on disk untouched. Skips when no GPU adapter is available.
#[test]
fn needs_gpu_set_overrides_a_model_palette_and_echoes_it() {
    let model = write_voxel_fixture("fixture_set.voxel.yaml");
    let out = target_tmpdir().join("model_set.png");

    let output = run_ludion(&[
        "render",
        "--model",
        model.to_str().unwrap(),
        "--views",
        "game",
        "--set",
        "/palette/A/emissive=2.5",
        "--set",
        "/palette/A/color/1=0.4",
        "--out",
        out.to_str().unwrap(),
        "--width",
        "200",
        "--height",
        "200",
    ]);
    if !output.status.success() {
        if is_no_adapter(&output) {
            eprintln!("SKIPPED set_overrides_a_model_palette_and_echoes_it: no GPU adapter");
            return;
        }
        panic!(
            "render --model --set failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("set /palette/A/emissive: unset -> 2.5"),
        "a created field echoes as `unset`: {stdout}"
    );
    assert!(
        stdout.contains("set /palette/A/color/1: 0.0 -> 0.4"),
        "a replaced field echoes what it replaced: {stdout}"
    );
    assert_eq!(
        std::fs::read_to_string(&model).unwrap(),
        FIXTURE_VOXEL,
        "an override never writes to the model document"
    );
}

/// A `--sweep` over a model palette produces one sheet and names what it swept. Skips without a GPU.
#[test]
fn needs_gpu_sweep_over_a_model_palette_produces_one_sheet() {
    let model = write_voxel_fixture("fixture_sweep.voxel.yaml");
    let out = target_tmpdir().join("model_sweep.png");

    let output = run_ludion(&[
        "render",
        "--model",
        model.to_str().unwrap(),
        "--views",
        "game",
        "--sweep",
        "/palette/A/emissive=1.6,2.2,2.8",
        "--out",
        out.to_str().unwrap(),
        "--width",
        "400",
        "--height",
        "400",
    ]);
    if !output.status.success() {
        if is_no_adapter(&output) {
            eprintln!("SKIPPED sweep_over_a_model_palette_produces_one_sheet: no GPU adapter");
            return;
        }
        panic!(
            "render --model --sweep failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("sweep /palette/A/emissive: was unset, over 1.6, 2.2, 2.8"),
        "the sweep line names the pointer and the values: {stdout}"
    );
    assert_eq!(png_dimensions(&out), (400, 400));
}

/// A model `--sweep` renders one view per value, so more than one view is an actionable `L4008` —
/// raised before any GPU adapter is requested.
#[test]
fn model_sweep_across_two_views_is_l4008() {
    let model = write_voxel_fixture("fixture_sweep_views.voxel.yaml");
    let out = target_tmpdir().join("should_not_sweep.png");

    let output = run_ludion(&[
        "render",
        "--model",
        model.to_str().unwrap(),
        "--views",
        "game,iso",
        "--sweep",
        "/palette/A/emissive=1.0,2.0",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L4008]"), "{stderr}");
    assert!(stderr.contains("--views game"), "{stderr}");
}

/// A `--set` pointer outside a model's palette is an `L4008` listing the shapes that do work.
#[test]
fn a_non_palette_model_pointer_is_l4008() {
    let model = write_voxel_fixture("fixture_set_domain.voxel.yaml");

    let output = run_ludion(&[
        "render",
        "--model",
        model.to_str().unwrap(),
        "--set",
        "/size/1=9",
    ]);
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L4008]"), "{stderr}");
    assert!(stderr.contains("/palette/<glyph>/emissive"), "{stderr}");
}

/// `--true-size` frames the subject at the coverage its declared `scale:` earns, so the sheet size
/// follows the model rather than the request. Skips when no GPU adapter is available.
#[test]
fn needs_gpu_true_size_frames_from_the_declared_scale() {
    let work_dir = target_tmpdir().join("true_size_cli");
    std::fs::create_dir_all(&work_dir).expect("work dir creates");
    let model = work_dir.join("scaled.voxel.yaml");
    // 3×2×2 cells at `scale: 2.0` ⇒ 4 world units tall.
    std::fs::write(
        &model,
        FIXTURE_VOXEL.replace("name: fixture", "name: fixture\nscale: 2.0"),
    )
    .expect("fixture writes");
    let out = work_dir.join("true.png");

    let output = run_ludion(&[
        "render",
        "--model",
        model.to_str().unwrap(),
        "--views",
        "game",
        "--true-size",
        "--out",
        out.to_str().unwrap(),
    ]);
    if !output.status.success() {
        if is_no_adapter(&output) {
            eprintln!("SKIPPED true_size_frames_from_the_declared_scale: no GPU adapter");
            return;
        }
        panic!(
            "render --model --true-size failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("true-size: 4.00 units tall"),
        "the framing is reported: {stdout}"
    );
    let (w, h) = png_dimensions(&out);
    assert_ne!(
        (w, h),
        (1920, 1080),
        "the sheet size follows the model, not the request",
    );
    assert!(
        (w as f32 / h as f32 - 16.0 / 9.0).abs() < 0.01,
        "the requested size still sets the aspect, got {w}x{h}",
    );
}

/// `--true-size` reads `--width`/`--height` as an aspect ratio, so giving one explicitly is a
/// conflict — caught at the flag layer by a `ValueSource` check, since clap cannot express
/// "conflicts with a *defaulted* argument".
#[test]
fn true_size_with_an_explicit_size_is_a_conflict() {
    let model = write_voxel_fixture("fixture_true_size_conflict.voxel.yaml");

    for extra in [["--width", "400"], ["--height", "400"]] {
        let output = run_ludion(&[
            "render",
            "--model",
            model.to_str().unwrap(),
            "--true-size",
            extra[0],
            extra[1],
        ]);
        assert!(
            !output.status.success(),
            "expected a non-zero exit for --true-size {}",
            extra[0]
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("cannot be used with"),
            "expected a conflict message: {stderr}"
        );
    }
}

/// `--true-size` is a `--model`-only flag: pairing it with `--scene` is a clap conflict.
#[test]
fn true_size_with_scene_is_a_clap_error() {
    let scene = scene_fixture();
    let out = target_tmpdir().join("should_not_render_true_size.png");

    let output = run_ludion(&[
        "render",
        "--scene",
        scene.to_str().unwrap(),
        "--true-size",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cannot be used with"),
        "expected a clap conflict message: {stderr}"
    );
}

/// **The lineup.** Several comma-separated `--model` paths render one sheet holding every model, and
/// it really is a different image from either alone. Skips when no GPU adapter is available.
#[test]
fn needs_gpu_a_comma_separated_model_list_renders_a_lineup() {
    let dir = target_tmpdir().join("lineup_cli");
    std::fs::create_dir_all(&dir).expect("work dir creates");
    let a = dir.join("a.voxel.yaml");
    let b = dir.join("b.voxel.yaml");
    std::fs::write(&a, FIXTURE_VOXEL).expect("fixture writes");
    std::fs::write(
        &b,
        FIXTURE_VOXEL.replace("name: fixture", "name: fixture\nscale: 3.0"),
    )
    .expect("fixture writes");
    let lineup_out = dir.join("lineup.png");
    let single_out = dir.join("single.png");

    let pair = format!("{},{}", a.to_str().unwrap(), b.to_str().unwrap());
    let output = run_ludion(&[
        "render",
        "--model",
        &pair,
        "--out",
        lineup_out.to_str().unwrap(),
        "--width",
        "400",
        "--height",
        "300",
    ]);
    if !output.status.success() {
        if is_no_adapter(&output) {
            eprintln!("SKIPPED a_comma_separated_model_list_renders_a_lineup: no GPU adapter");
            return;
        }
        panic!(
            "render --model a,b failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert_eq!(
        png_dimensions(&lineup_out),
        (400, 300),
        "a lineup is one cell filling the requested sheet",
    );

    // The same first model alone, at the same view, must differ — the neighbour is really there.
    let single = run_ludion(&[
        "render",
        "--model",
        a.to_str().unwrap(),
        "--views",
        "game",
        "--out",
        single_out.to_str().unwrap(),
        "--width",
        "400",
        "--height",
        "300",
    ]);
    assert!(single.status.success());
    assert_ne!(
        std::fs::read(&lineup_out).unwrap(),
        std::fs::read(&single_out).unwrap(),
        "a lineup must not equal its first model alone",
    );
}

/// A lineup names no single model, so it has no default output name and says so.
#[test]
fn a_lineup_without_out_is_a_usage_error() {
    let model = write_voxel_fixture("fixture_lineup_out.voxel.yaml");
    let pair = format!("{0},{0}", model.to_str().unwrap());

    let output = run_ludion(&["render", "--model", &pair]);
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--out"), "{stderr}");
    assert!(stderr.contains("lineup"), "{stderr}");
}

/// A lineup renders one view, so more than one is an actionable `L4008` — no GPU needed.
#[test]
fn a_multi_view_lineup_is_l4008() {
    let model = write_voxel_fixture("fixture_lineup_views.voxel.yaml");
    let pair = format!("{0},{0}", model.to_str().unwrap());
    let out = target_tmpdir().join("should_not_line_up.png");

    let output = run_ludion(&[
        "render",
        "--model",
        &pair,
        "--views",
        "game,iso",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L4008]"), "{stderr}");
    assert!(stderr.contains("--views game"), "{stderr}");
}

/// `--set`/`--sweep` calibrate one model; a lineup renders several, and the `L4008` says which order
/// to do them in.
#[test]
fn a_lineup_with_set_is_l4008() {
    let model = write_voxel_fixture("fixture_lineup_set.voxel.yaml");
    let pair = format!("{0},{0}", model.to_str().unwrap());
    let out = target_tmpdir().join("should_not_line_up_set.png");

    let output = run_ludion(&[
        "render",
        "--model",
        &pair,
        "--set",
        "/palette/A/emissive=2.0",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L4008]"), "{stderr}");
    assert!(stderr.contains("calibrate"), "{stderr}");
}

/// Past the cap, a lineup's models are too narrow to compare.
#[test]
fn an_oversized_lineup_is_l4008() {
    let model = write_voxel_fixture("fixture_lineup_cap.voxel.yaml");
    let path = model.to_str().unwrap();
    let many = [path; ludion_engine::MAX_LINEUP_MODELS + 1].join(",");
    let out = target_tmpdir().join("should_not_line_up_cap.png");

    let output = run_ludion(&["render", "--model", &many, "--out", out.to_str().unwrap()]);
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L4008]"), "{stderr}");
    assert!(
        stderr.contains(&ludion_engine::MAX_LINEUP_MODELS.to_string()),
        "{stderr}"
    );
}

/// With `--model` and no `--out`, the sheet is written to `<name>.sheet.png` in the current
/// directory. Skips when no GPU adapter is available.
#[test]
fn needs_gpu_default_out_is_name_sheet_png_in_cwd() {
    let work_dir = target_tmpdir().join("default_out_cwd");
    std::fs::create_dir_all(&work_dir).expect("work dir creates");
    let model_path = work_dir.join("lantern.voxel.yaml");
    std::fs::write(&model_path, FIXTURE_VOXEL).expect("fixture voxel doc writes");

    let output = run_ludion_in(
        &work_dir,
        &[
            "render",
            "--model",
            "lantern.voxel.yaml",
            "--width",
            "300",
            "--height",
            "200",
        ],
    );
    if !output.status.success() {
        if is_no_adapter(&output) {
            eprintln!("SKIPPED default_out_is_name_sheet_png_in_cwd: no GPU adapter");
            return;
        }
        panic!(
            "render --model failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let expected = work_dir.join("lantern.sheet.png");
    assert!(
        expected.exists(),
        "expected default output `{}` to exist; stdout: {}",
        expected.display(),
        String::from_utf8_lossy(&output.stdout)
    );
}

/// A single-part `kind: assembly` document renders through `--model` exactly like the equivalent
/// voxel document — the two fixtures share a grid, so their rest-pose sheets are byte-identical
/// (`mesh_assembly`'s own "a single-part assembly equals the voxel model" proof, now end to end
/// through the CLI). Skips gracefully with no GPU adapter.
#[test]
fn needs_gpu_an_assembly_model_renders_identically_to_the_equivalent_voxel_model() {
    let dir = target_tmpdir().join("assembly_equals_voxel");
    std::fs::create_dir_all(&dir).expect("work dir creates");
    let voxel = write_voxel_fixture("equiv.voxel.yaml");
    let assembly = write_assembly_fixture(&dir, "equiv");

    let voxel_out = dir.join("voxel.png");
    let assembly_out = dir.join("assembly.png");
    let common = ["--width", "400", "--height", "300"];

    let voxel_result = run_ludion(
        &[
            "render",
            "--model",
            voxel.to_str().unwrap(),
            "--out",
            voxel_out.to_str().unwrap(),
        ]
        .iter()
        .chain(&common)
        .copied()
        .collect::<Vec<_>>(),
    );
    if !voxel_result.status.success() {
        if is_no_adapter(&voxel_result) {
            eprintln!(
                "SKIPPED an_assembly_model_renders_identically_to_the_equivalent_voxel_model: no \
                 GPU adapter"
            );
            return;
        }
        panic!(
            "render --model (voxel) failed: {}",
            String::from_utf8_lossy(&voxel_result.stderr)
        );
    }

    let assembly_result = run_ludion(
        &[
            "render",
            "--model",
            assembly.to_str().unwrap(),
            "--out",
            assembly_out.to_str().unwrap(),
        ]
        .iter()
        .chain(&common)
        .copied()
        .collect::<Vec<_>>(),
    );
    assert!(
        assembly_result.status.success(),
        "render --model (assembly) failed: {}",
        String::from_utf8_lossy(&assembly_result.stderr)
    );

    assert_eq!(
        std::fs::read(&voxel_out).unwrap(),
        std::fs::read(&assembly_out).unwrap(),
        "a single-part assembly at the origin must render byte-identically to the equivalent voxel \
         model"
    );
}

/// A lineup mixes an assembly beside a voxel model in one cell — proving the lineup staging path
/// carries an assembly's part files along with it (a missing one is `L1101`, surfaced only if the
/// staging is wrong). Skips gracefully with no GPU adapter.
#[test]
fn needs_gpu_a_lineup_mixes_an_assembly_with_a_voxel_model() {
    let dir = target_tmpdir().join("lineup_mixed_kinds");
    std::fs::create_dir_all(&dir).expect("work dir creates");
    let voxel = write_voxel_fixture("mixed.voxel.yaml");
    let assembly = write_assembly_fixture(&dir, "mixed");
    let out = dir.join("lineup.png");

    let pair = format!("{},{}", voxel.to_str().unwrap(), assembly.to_str().unwrap());
    let output = run_ludion(&[
        "render",
        "--model",
        &pair,
        "--out",
        out.to_str().unwrap(),
        "--width",
        "400",
        "--height",
        "300",
    ]);
    if !output.status.success() {
        if is_no_adapter(&output) {
            eprintln!("SKIPPED a_lineup_mixes_an_assembly_with_a_voxel_model: no GPU adapter");
            return;
        }
        panic!(
            "render --model <voxel>,<assembly> failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert_eq!(png_dimensions(&out), (400, 300));
}

/// With `--model` and no `--out`, an assembly's default output strips the `.assembly` tag the same
/// way a voxel/mesh model's strips `.voxel`/`.mesh`.
#[test]
fn needs_gpu_default_out_strips_the_assembly_tag() {
    let work_dir = target_tmpdir().join("default_out_assembly");
    std::fs::create_dir_all(&work_dir).expect("work dir creates");
    write_assembly_fixture(&work_dir, "figure");

    let output = run_ludion_in(
        &work_dir,
        &[
            "render",
            "--model",
            "figure.assembly.yaml",
            "--width",
            "300",
            "--height",
            "200",
        ],
    );
    if !output.status.success() {
        if is_no_adapter(&output) {
            eprintln!("SKIPPED default_out_strips_the_assembly_tag: no GPU adapter");
            return;
        }
        panic!(
            "render --model failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let expected = work_dir.join("figure.sheet.png");
    assert!(
        expected.exists(),
        "expected default output `{}` to exist; stdout: {}",
        expected.display(),
        String::from_utf8_lossy(&output.stdout)
    );
}

/// `--set` reaches an assembly's palette exactly as it does a voxel model's: the mutated copy
/// (plus its part) is staged and rendered, the override is echoed, and the source tree is
/// untouched. Skips when no GPU adapter is available.
#[test]
fn needs_gpu_set_overrides_an_assembly_palette_and_echoes_it() {
    let dir = target_tmpdir().join("assembly_set");
    std::fs::create_dir_all(&dir).expect("work dir creates");
    let assembly = write_assembly_fixture(&dir, "figure");
    let source = std::fs::read_to_string(&assembly).unwrap();
    let out = dir.join("assembly_set.png");

    let output = run_ludion(&[
        "render",
        "--model",
        assembly.to_str().unwrap(),
        "--views",
        "game",
        "--set",
        "/palette/A/emissive=2.0",
        "--out",
        out.to_str().unwrap(),
        "--width",
        "200",
        "--height",
        "200",
    ]);
    if !output.status.success() {
        if is_no_adapter(&output) {
            eprintln!("SKIPPED set_overrides_an_assembly_palette_and_echoes_it: no GPU adapter");
            return;
        }
        panic!(
            "render --model --set on an assembly failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("set /palette/A/emissive: unset -> 2.0"),
        "a created field echoes as `unset`: {stdout}"
    );
    assert_eq!(
        std::fs::read_to_string(&assembly).unwrap(),
        source,
        "an override never writes to the assembly document"
    );
    assert!(out.exists());
}

/// A `--sweep` over an assembly's palette produces one sheet, exactly as it does a voxel model's.
/// Skips without a GPU.
#[test]
fn needs_gpu_sweep_over_an_assembly_palette_produces_one_sheet() {
    let dir = target_tmpdir().join("assembly_sweep");
    std::fs::create_dir_all(&dir).expect("work dir creates");
    let assembly = write_assembly_fixture(&dir, "figure");
    let out = dir.join("assembly_sweep.png");

    let output = run_ludion(&[
        "render",
        "--model",
        assembly.to_str().unwrap(),
        "--views",
        "game",
        "--sweep",
        "/palette/A/emissive=1.6,2.2,2.8",
        "--out",
        out.to_str().unwrap(),
        "--width",
        "400",
        "--height",
        "400",
    ]);
    if !output.status.success() {
        if is_no_adapter(&output) {
            eprintln!("SKIPPED sweep_over_an_assembly_palette_produces_one_sheet: no GPU adapter");
            return;
        }
        panic!(
            "render --model --sweep on an assembly failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("sweep /palette/A/emissive: was unset, over 1.6, 2.2, 2.8"),
        "the sweep line names the pointer and the values: {stdout}"
    );
    assert_eq!(png_dimensions(&out), (400, 400));
}

/// `--sweep` and `--strip` both multiply cells, so combining them against an assembly that binds a
/// clip is `L4008`, not a silent pick — no GPU needed, the refusal fires before any staging IO.
#[test]
fn assembly_sweep_and_strip_together_is_l4008() {
    let dir = target_tmpdir().join("assembly_sweep_strip");
    std::fs::create_dir_all(&dir).expect("work dir creates");
    let assembly = write_clip_fixture(&dir, "poser");
    let out = dir.join("should_not_render.png");

    let output = run_ludion(&[
        "render",
        "--model",
        assembly.to_str().unwrap(),
        "--views",
        "game",
        "--clip",
        "slide",
        "--sweep",
        "/palette/A/emissive=1.0,2.0",
        "--strip",
        "3",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L4008]"), "{stderr}");
    assert!(stderr.contains("pick one"), "{stderr}");
    assert!(
        !out.exists(),
        "a refused render must not produce an output file"
    );
}

/// An assembly whose part file is missing fails with the loader's own `L1101`, surfaced before any
/// GPU adapter is touched — the extractor's cross-file validation passing through unchanged, exactly
/// as `check-scene` reports it.
#[test]
fn a_broken_assembly_fails_with_l1101_not_panic() {
    let dir = target_tmpdir().join("assembly_broken");
    std::fs::create_dir_all(&dir).expect("work dir creates");
    let assembly = dir.join("broken.assembly.yaml");
    std::fs::write(&assembly, fixture_assembly(FIXTURE_ASSEMBLY_SIZE)).expect("assembly writes");
    // No `parts/body.part.yaml` written: the assembly names a part file that does not exist.
    let out = dir.join("should_not_render.png");

    let output = run_ludion(&[
        "render",
        "--model",
        assembly.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L1101]"), "{stderr}");
    assert!(
        !out.exists(),
        "a broken model must not produce an output file"
    );
}

// --- `--clip` / `--time` / `--strip` (`Plan/voxel-animation.md` decision 11) --------------------

/// A **one-shot** clip that slides the assembly fixture's one `body` part three cells along +X, so
/// its final pose is unambiguously detached from where it started — the fixture the gap analysis is
/// read off.
const FIXTURE_CLIP_SLIDE: &str = r#"format: 1
kind: animation
name: slide
duration: 1.0
loop: false
tracks:
  arm:
    translation:
      - t: 0.0
        v:
          - 0.0
          - 0.0
          - 0.0
      - t: 1.0
        v:
          - 4.0
          - 0.0
          - 0.0
"#;

/// The `body` the clip fixture is built around: a solid 3×2×1 block, so the assembly is one
/// connected component at rest and every posed count is read against that.
const FIXTURE_CLIP_BODY: &str = r#"format: 1
kind: part
size:
  - 3
  - 2
  - 1
slices:
  -
    - "AAA"
  -
    - "AAA"
"#;

/// The `arm` part the slide clip moves: a 1×1×1 cell that starts face-adjacent to the body.
const FIXTURE_CLIP_ARM: &str = r#"format: 1
kind: part
size:
  - 1
  - 1
  - 1
slices:
  -
    - "A"
"#;

/// A two-part assembly binding one clip: a solid 3×2×1 `body` with a 1×1×1 `arm` face-adjacent to
/// it, in an 8-wide grid the clip has room to slide across.
const FIXTURE_CLIP_ASSEMBLY: &str = r#"format: 1
kind: assembly
name: poser
size:
  - 8
  - 2
  - 1
palette:
  A:
    color:
      - 1.0
      - 0.0
      - 0.0
      - 1.0
parts:
  - name: body
    file: parts/body.part.yaml
    place:
      - 0
      - 0
      - 0
  - name: arm
    file: parts/arm.part.yaml
    parent: body
    place:
      - 3
      - 0
      - 0
clips:
  slide: anims/slide.anim.yaml
"#;

/// Writes the clip fixture (assembly + both parts + the clip) under `dir`, returning the assembly's
/// own path.
fn write_clip_fixture(dir: &Path, stem: &str) -> PathBuf {
    std::fs::create_dir_all(dir.join("parts")).expect("parts dir creates");
    std::fs::create_dir_all(dir.join("anims")).expect("anims dir creates");
    std::fs::write(dir.join("parts").join("body.part.yaml"), FIXTURE_CLIP_BODY)
        .expect("body part writes");
    std::fs::write(dir.join("parts").join("arm.part.yaml"), FIXTURE_CLIP_ARM)
        .expect("arm part writes");
    std::fs::write(
        dir.join("anims").join("slide.anim.yaml"),
        FIXTURE_CLIP_SLIDE,
    )
    .expect("clip writes");
    let path = dir.join(format!("{stem}.assembly.yaml"));
    std::fs::write(&path, FIXTURE_CLIP_ASSEMBLY).expect("assembly writes");
    path
}

/// `--time` and `--strip` are modifiers of `--clip`: without one there is no clip to take an instant
/// of. Both are `L1117` with help naming the fix, and neither touches a GPU.
#[test]
fn time_or_strip_without_clip_is_l1117() {
    let dir = target_tmpdir().join("clip_orphan_flags");
    std::fs::create_dir_all(&dir).expect("work dir creates");
    let assembly = write_clip_fixture(&dir, "poser");
    let out = dir.join("should_not_render.png");

    for flag in [["--time", "0.4"], ["--strip", "3"]] {
        let output = run_ludion(&[
            "render",
            "--model",
            assembly.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            flag[0],
            flag[1],
        ]);
        assert!(!output.status.success(), "expected a non-zero exit");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("error[L1117]"), "{stderr}");
        assert!(stderr.contains("needs `--clip`"), "{stderr}");
        assert!(stderr.contains("help:"), "{stderr}");
        assert!(
            !out.exists(),
            "a usage error must not produce an output file"
        );
    }
}

/// `--clip` against a voxel document is `L1117`: one rigid piece has no parts to pose. No GPU.
#[test]
fn clip_on_a_voxel_model_is_l1117() {
    let model = write_voxel_fixture("clip_on_voxel.voxel.yaml");
    let out = target_tmpdir().join("clip_on_voxel_should_not_render.png");

    let output = run_ludion(&[
        "render",
        "--model",
        model.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--clip",
        "slide",
    ]);
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L1117]"), "{stderr}");
    assert!(
        stderr.contains("only an assembly, a character or a prefab"),
        "{stderr}"
    );
}

/// A clip name the assembly does not bind is `L1117`, and the message lists the ones it does — the
/// difference between "no such clip" and "you spelled it wrong". No GPU.
#[test]
fn an_unbound_clip_name_is_l1117_and_lists_the_bound_ones() {
    let dir = target_tmpdir().join("clip_unbound_name");
    std::fs::create_dir_all(&dir).expect("work dir creates");
    let assembly = write_clip_fixture(&dir, "poser");
    let out = dir.join("should_not_render.png");

    let output = run_ludion(&[
        "render",
        "--model",
        assembly.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--clip",
        "slyde",
    ]);
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L1117]"), "{stderr}");
    assert!(stderr.contains("binds no clip named `slyde`"), "{stderr}");
    assert!(stderr.contains("`slide`"), "{stderr}");
}

/// A strip of one phase is a posed sheet spelled the long way, so it is refused with the range in
/// the message. No GPU.
#[test]
fn a_single_phase_strip_is_l1117() {
    let dir = target_tmpdir().join("clip_strip_one");
    std::fs::create_dir_all(&dir).expect("work dir creates");
    let assembly = write_clip_fixture(&dir, "poser");
    let out = dir.join("should_not_render.png");

    let output = run_ludion(&[
        "render",
        "--model",
        assembly.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--clip",
        "slide",
        "--strip",
        "1",
    ]);
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L1117]"), "{stderr}");
    assert!(stderr.contains("out of range"), "{stderr}");
}

/// The per-phase gap analysis on stdout: the clip header, one line per phase naming its instant, its
/// tick, and its component count, and — for the detached final phase — the `connectivity owners:`
/// attribution block naming the parts on both sides of the gap. Needs a GPU (the report prints
/// alongside a successful render).
#[test]
fn needs_gpu_a_strip_prints_the_per_phase_gap_analysis() {
    let dir = target_tmpdir().join("clip_gap_report");
    std::fs::create_dir_all(&dir).expect("work dir creates");
    let assembly = write_clip_fixture(&dir, "poser");
    let out = dir.join("strip.png");

    let output = run_ludion(&[
        "render",
        "--model",
        assembly.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--clip",
        "slide",
        "--strip",
        "3",
        "--width",
        "400",
        "--height",
        "300",
    ]);
    if !output.status.success() {
        if is_no_adapter(&output) {
            eprintln!("SKIPPED a_strip_prints_the_per_phase_gap_analysis: no GPU adapter");
            return;
        }
        panic!(
            "render --model --clip --strip failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("clip slide: duration 1, loop false, 3 phases quantized to 60 Hz ticks"),
        "{stdout}"
    );
    // A one-shot spaces its phases over `n - 1`, so three phases are t = 0, 0.5, and the final 1.0.
    assert!(
        stdout.contains("phase 0: t 0 (tick 0), 1 component"),
        "{stdout}"
    );
    assert!(stdout.contains("phase 1: t 0.5 (tick 30)"), "{stdout}");
    assert!(
        stdout.contains("phase 2: t 1 (tick 60), 2 components"),
        "{stdout}"
    );
    assert!(stdout.contains("connectivity owners:"), "{stdout}");
    assert!(
        stdout.contains("owned by arm") && stdout.contains("nearest main-body cell"),
        "the stray must be attributed to the part that moved: {stdout}"
    );
    assert_eq!(png_dimensions(&out), (400, 300));
}

/// A strip is one cell of phases, not a grid of views, and its default `--out` is
/// `<name>-<clip>-strip.png`; a single posed sheet's is `<name>-<clip>-t<seconds>.png`. Needs a GPU
/// (the name is only proven by the file the render writes).
#[test]
fn needs_gpu_posed_renders_default_their_output_names() {
    let dir = target_tmpdir().join("clip_default_names");
    std::fs::create_dir_all(&dir).expect("work dir creates");
    write_clip_fixture(&dir, "poser");

    let strip = run_ludion_in(
        &dir,
        &[
            "render",
            "--model",
            "poser.assembly.yaml",
            "--clip",
            "slide",
            "--strip",
            "2",
            "--width",
            "300",
            "--height",
            "200",
        ],
    );
    if !strip.status.success() {
        if is_no_adapter(&strip) {
            eprintln!("SKIPPED posed_renders_default_their_output_names: no GPU adapter");
            return;
        }
        panic!(
            "render --strip failed: {}",
            String::from_utf8_lossy(&strip.stderr)
        );
    }
    assert!(
        dir.join("poser-slide-strip.png").exists(),
        "expected `poser-slide-strip.png`; stdout: {}",
        String::from_utf8_lossy(&strip.stdout)
    );

    let posed = run_ludion_in(
        &dir,
        &[
            "render",
            "--model",
            "poser.assembly.yaml",
            "--clip",
            "slide",
            "--time",
            "0.4",
            "--width",
            "300",
            "--height",
            "200",
        ],
    );
    assert!(
        posed.status.success(),
        "render --clip --time failed: {}",
        String::from_utf8_lossy(&posed.stderr)
    );
    assert!(
        dir.join("poser-slide-t0.4.png").exists(),
        "expected `poser-slide-t0.4.png`; stdout: {}",
        String::from_utf8_lossy(&posed.stdout)
    );
}

/// **The rest pose is untouched.** Posing at `t = 0` — where the clip's first keyframe is the
/// identity — writes a PNG byte-identical to the same model rendered with no clip at all, at the
/// CLI level and across two processes. Needs a GPU.
#[test]
fn needs_gpu_a_clip_at_time_zero_writes_the_rest_pose_bytes() {
    let dir = target_tmpdir().join("clip_t0_bytes");
    std::fs::create_dir_all(&dir).expect("work dir creates");
    let assembly = write_clip_fixture(&dir, "poser");
    let rest_out = dir.join("rest.png");
    let posed_out = dir.join("posed.png");

    let rest = run_ludion(&[
        "render",
        "--model",
        assembly.to_str().unwrap(),
        "--out",
        rest_out.to_str().unwrap(),
        "--width",
        "300",
        "--height",
        "200",
    ]);
    if !rest.status.success() {
        if is_no_adapter(&rest) {
            eprintln!("SKIPPED a_clip_at_time_zero_writes_the_rest_pose_bytes: no GPU adapter");
            return;
        }
        panic!(
            "rest render failed: {}",
            String::from_utf8_lossy(&rest.stderr)
        );
    }

    let posed = run_ludion(&[
        "render",
        "--model",
        assembly.to_str().unwrap(),
        "--out",
        posed_out.to_str().unwrap(),
        "--clip",
        "slide",
        "--time",
        "0",
        "--width",
        "300",
        "--height",
        "200",
    ]);
    assert!(
        posed.status.success(),
        "posed render failed: {}",
        String::from_utf8_lossy(&posed.stderr)
    );
    assert_eq!(
        std::fs::read(&rest_out).unwrap(),
        std::fs::read(&posed_out).unwrap(),
        "posing at t = 0 must write the rest pose's exact bytes"
    );
}

/// A two-part sculpt: a body with a head on top.
const FIXTURE_SCULPT: &str = r#"format: 1
kind: sculpt
resolution: 16
triangles: 600
palette:
  S:
    color:
      - 0.5
      - 0.5
      - 0.5
      - 1.0
bones:
  - name: root
parts:
  - name: body
    shape: sphere
    radius: 0.5
    material: S
  - name: head
    shape: sphere
    radius: 0.25
    at:
      - 0.0
      - 0.6
      - 0.0
    blend: 0.05
    material: S
"#;

/// A `--part` the sculpt does not name is an `L4008` suggesting the nearest part — refused before any
/// GPU adapter is requested.
#[test]
fn an_unknown_sculpt_part_is_l4008() {
    let model = target_tmpdir().join("fixture_part.sculpt.yaml");
    std::fs::write(&model, FIXTURE_SCULPT).expect("fixture sculpt doc writes");
    let out = target_tmpdir().join("should_not_frame.png");

    let output = run_ludion(&[
        "render",
        "--model",
        model.to_str().unwrap(),
        "--part",
        "haed",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L4008]"), "{stderr}");
    assert!(stderr.contains("nearest match is `head`"), "{stderr}");
    assert!(!out.exists(), "a refused render writes nothing");
}

/// The textured cube `examples/import-textured/` commits — the one fixture in the tree that
/// references an `emissive_texture` (a black map with one bright cyan corner square, a quarter of
/// each face across).
fn textured_example_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/import-textured/expected")
}

/// Stages that cube into `dir` as a lit/dark pair: the same document with and without its
/// `emissive_texture` key, beside one copy of the textures they reference (`../textures/…`, relative
/// to the document's own directory). Returns `(lit, dark)`.
fn stage_emissive_pair(dir: &Path) -> (PathBuf, PathBuf) {
    let source_dir = textured_example_dir();
    let meshes = dir.join("meshes");
    let textures = dir.join("textures");
    std::fs::create_dir_all(&meshes).expect("meshes dir");
    std::fs::create_dir_all(&textures).expect("textures dir");
    for entry in std::fs::read_dir(source_dir.join("textures")).expect("example textures") {
        let entry = entry.expect("a texture entry");
        std::fs::copy(entry.path(), textures.join(entry.file_name())).expect("texture copies");
    }

    let source = std::fs::read_to_string(source_dir.join("meshes/tangent-cube.mesh.yaml"))
        .expect("the example mesh document reads");
    assert!(
        source.contains("emissive_texture:"),
        "the fixture must reference an emissive map"
    );
    // Drop the `emissive_texture:` key and its indented block — the only difference between the two.
    let mut dark = String::with_capacity(source.len());
    let mut skipping = false;
    for line in source.lines() {
        if line.starts_with("emissive_texture:") {
            skipping = true;
            continue;
        }
        if skipping && line.starts_with(' ') {
            continue;
        }
        skipping = false;
        dark.push_str(line);
        dark.push('\n');
    }

    let lit = meshes.join("lit.mesh.yaml");
    let unlit = meshes.join("dark.mesh.yaml");
    std::fs::write(&lit, &source).expect("lit document writes");
    std::fs::write(&unlit, &dark).expect("dark document writes");
    (lit, unlit)
}

/// Decodes a PNG to RGBA8.
fn decode_rgba(path: &Path) -> Vec<u8> {
    let file = std::fs::File::open(path).expect("PNG exists");
    let decoder = png::Decoder::new(std::io::BufReader::new(file));
    let mut reader = decoder.read_info().expect("valid PNG");
    let mut buf = vec![0u8; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).expect("frame decodes");
    buf.truncate(info.buffer_size());
    buf
}

/// The brightest RGB channel anywhere in an RGBA8 image, and how many pixels reach at least `floor`.
fn brightness(rgba: &[u8], floor: u8) -> (u8, usize) {
    let mut peak = 0u8;
    let mut lit = 0usize;
    for pixel in rgba.chunks_exact(4) {
        let max = pixel[0].max(pixel[1]).max(pixel[2]);
        peak = peak.max(max);
        if max >= floor {
            lit += 1;
        }
    }
    (peak, lit)
}

/// **`--channel emissive` is a categorical mask.** The same cube, rendered twice: with its emissive
/// map the channel shows the map and nothing else; with the key removed every pixel is black. That
/// is the answer to "this glows *and nothing else does*" without anyone squinting at a picture.
#[test]
fn needs_gpu_the_emissive_channel_separates_a_mapped_mesh_from_an_unmapped_one() {
    let dir = target_tmpdir().join("emissive-channel");
    let _ = std::fs::remove_dir_all(&dir);
    let (lit, dark) = stage_emissive_pair(&dir);
    let lit_out = dir.join("lit.png");
    let dark_out = dir.join("dark.png");

    let render = |model: &Path, out: &Path| {
        run_ludion(&[
            "render",
            "--model",
            model.to_str().unwrap(),
            "--views",
            "front",
            "--channel",
            "emissive",
            "--width",
            "200",
            "--height",
            "200",
            "--out",
            out.to_str().unwrap(),
        ])
    };

    let output = render(&lit, &lit_out);
    if !output.status.success() {
        if is_no_adapter(&output) {
            eprintln!("SKIPPED the emissive channel test: no GPU adapter");
            return;
        }
        panic!(
            "render --channel emissive failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let pixels = decode_rgba(&lit_out);
    let (peak, lit_pixels) = brightness(&pixels, 128);
    assert!(
        peak > 200,
        "the mapped corner should read bright, peak {peak}"
    );
    assert!(
        lit_pixels > 50,
        "the map corner covers a quarter of the face, got {lit_pixels} lit pixels"
    );
    // A mask, not a picture: the great majority of the cell is black.
    let total = pixels.len() / 4;
    assert!(
        lit_pixels * 4 < total,
        "{lit_pixels} of {total} pixels lit — the channel is not masking"
    );

    let output = render(&dark, &dark_out);
    assert!(
        output.status.success(),
        "render --channel emissive failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let pixels = decode_rgba(&dark_out);
    let (peak, _) = brightness(&pixels, 128);
    assert!(
        peak <= 1,
        "a mesh with no emissive map must be black, peak {peak}"
    );
}

/// **The ordinary sheet shows a mapped glow and stays a lit picture.** Without `--channel` the same
/// model renders the lit, captioned sheet it always did, and the emissive map now shows on it: the
/// document with the map differs from the one without it where the map glows.
#[test]
fn needs_gpu_the_plain_sheet_shows_a_mapped_glow_and_stays_lit() {
    let dir = target_tmpdir().join("emissive-channel-plain");
    let _ = std::fs::remove_dir_all(&dir);
    let (lit, dark) = stage_emissive_pair(&dir);
    let lit_out = dir.join("plain.png");
    let dark_out = dir.join("plain-dark.png");

    let render = |model: &Path, out: &Path| {
        run_ludion(&[
            "render",
            "--model",
            model.to_str().unwrap(),
            "--views",
            "front",
            "--width",
            "200",
            "--height",
            "200",
            "--out",
            out.to_str().unwrap(),
        ])
    };
    let output = render(&lit, &lit_out);
    if !output.status.success() {
        if is_no_adapter(&output) {
            eprintln!("SKIPPED the plain-sheet comparison: no GPU adapter");
            return;
        }
        panic!("render failed: {}", String::from_utf8_lossy(&output.stderr));
    }
    let pixels = decode_rgba(&lit_out);
    let (_, lit_pixels) = brightness(&pixels, 128);
    let total = pixels.len() / 4;
    assert!(
        lit_pixels * 2 > total,
        "the ordinary sheet is a lit picture, not a mask: {lit_pixels} of {total}"
    );

    let output = render(&dark, &dark_out);
    assert!(
        output.status.success(),
        "render failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let unlit = decode_rgba(&dark_out);
    let differing = pixels
        .chunks_exact(4)
        .zip(unlit.chunks_exact(4))
        .filter(|(a, b)| a != b)
        .count();
    assert!(
        differing >= 50,
        "the mapped corner should glow on the ordinary sheet: {differing} pixels differ"
    );
}

/// `--channel` is a model-mode flag with a closed value set: an unknown value lists the legal ones,
/// and pairing it with `--scene` or `--rig` is refused. All three are clap-level, no GPU involved.
#[test]
fn channel_is_model_only_and_closed() {
    let model = write_voxel_fixture("fixture_channel_args.voxel.yaml");
    let out = target_tmpdir().join("should_not_render.png");

    let unknown = run_ludion(&[
        "render",
        "--model",
        model.to_str().unwrap(),
        "--channel",
        "albedo",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(!unknown.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&unknown.stderr);
    assert!(stderr.contains("emissive"), "the legal values: {stderr}");

    let with_scene = run_ludion(&[
        "render",
        "--scene",
        scene_fixture().to_str().unwrap(),
        "--channel",
        "emissive",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(!with_scene.status.success(), "expected a non-zero exit");
    assert!(
        String::from_utf8_lossy(&with_scene.stderr).contains("--channel"),
        "{}",
        String::from_utf8_lossy(&with_scene.stderr)
    );

    let with_rig = run_ludion(&[
        "render",
        "--model",
        model.to_str().unwrap(),
        "--rig",
        scene_fixture().to_str().unwrap(),
        "--channel",
        "emissive",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(!with_rig.status.success(), "expected a non-zero exit");
}

// --- Characters and `--crossfade` ----------------------------------------------------------------

/// The gallery's mannequin character, read-only.
fn mannequin() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/gallery/characters/mannequin.character.yaml")
}

/// `--crossfade` and `--clip` both pose the model, so together they are a clap conflict.
#[test]
fn crossfade_with_clip_is_a_clap_error() {
    let output = run_ludion(&[
        "render",
        "--model",
        mannequin().to_str().unwrap(),
        "--clip",
        "run",
        "--crossfade",
        "run:idle:0.25",
    ]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cannot be used with"), "{stderr}");
}

/// A malformed crossfade spec is `L1117` with the spelling that works. No GPU.
#[test]
fn a_malformed_crossfade_spec_is_l1117() {
    let dir = target_tmpdir().join("crossfade_bad_spec");
    std::fs::create_dir_all(&dir).expect("work dir creates");
    for spec in ["run:idle", "run:idle:-1"] {
        let output = run_ludion_in(
            &dir,
            &[
                "render",
                "--model",
                mannequin().to_str().unwrap(),
                "--crossfade",
                spec,
            ],
        );
        assert!(!output.status.success(), "{spec}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("error[L1117]"), "{spec}: {stderr}");
        assert!(stderr.contains(spec), "{spec}: {stderr}");
        assert!(stderr.contains("--crossfade run:idle:0.25"), "{stderr}");
        assert_eq!(
            std::fs::read_dir(&dir).expect("dir reads").count(),
            0,
            "a usage error must not produce an output file"
        );
    }
}

/// A character crossfade writes `<name>-<from>-<to>-crossfade.png` and reports each phase's weight.
#[test]
fn needs_gpu_a_character_crossfade_names_its_output_and_prints_weights() {
    let dir = target_tmpdir().join("crossfade_character");
    std::fs::create_dir_all(&dir).expect("work dir creates");
    let output = run_ludion_in(
        &dir,
        &[
            "render",
            "--model",
            mannequin().to_str().unwrap(),
            "--crossfade",
            "run:idle:0.4",
            "--width",
            "480",
            "--height",
            "270",
        ],
    );
    if !output.status.success() {
        if is_no_adapter(&output) {
            eprintln!("SKIPPED a_character_crossfade_names_its_output: no GPU adapter");
            return;
        }
        panic!(
            "render --crossfade failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert!(
        dir.join("mannequin-run-idle-crossfade.png").exists(),
        "stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("crossfade run -> idle over 0.4s, 5 phases quantized to 60 Hz ticks"),
        "{stdout}"
    );
    assert!(
        stdout.contains("phase 0: t 0 (tick 0), weight 0.000"),
        "{stdout}"
    );
    assert!(stdout.contains("(tick 12), weight 0.500"), "{stdout}");
    assert!(
        !stdout.contains("component"),
        "a character has no gap analysis: {stdout}"
    );
}

/// `--hold 0` is `L1317`, before any render. No GPU.
#[test]
fn hold_zero_is_l1317() {
    let dir = target_tmpdir().join("hold_zero");
    std::fs::create_dir_all(&dir).expect("work dir creates");
    let output = run_ludion_in(
        &dir,
        &[
            "render",
            "--model",
            mannequin().to_str().unwrap(),
            "--crossfade",
            "run:idle:0.4",
            "--hold",
            "0",
        ],
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("L1317"), "{stderr}");
}

/// `--hold` with no `--clip` or `--crossfade` is `L1117`, naming the flag. No GPU.
#[test]
fn an_orphan_hold_is_l1117() {
    let dir = target_tmpdir().join("hold_orphan");
    std::fs::create_dir_all(&dir).expect("work dir creates");
    let output = run_ludion_in(
        &dir,
        &[
            "render",
            "--model",
            mannequin().to_str().unwrap(),
            "--hold",
            "5",
        ],
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("L1117"), "{stderr}");
    assert!(stderr.contains("--hold"), "{stderr}");
}

/// `--hold` is a `--model` flag: beside `--scene` it is a clap conflict.
#[test]
fn hold_with_scene_is_a_clap_error() {
    let output = run_ludion(&["render", "--scene", "x.scene.yaml", "--hold", "5"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cannot be used with"), "{stderr}");
}

/// A held crossfade strip names its output `-hold<N>` and the report says how long a pose is held.
#[test]
fn needs_gpu_a_held_crossfade_names_its_output_and_reports_the_hold() {
    let dir = target_tmpdir().join("crossfade_hold");
    std::fs::create_dir_all(&dir).expect("work dir creates");
    let output = run_ludion_in(
        &dir,
        &[
            "render",
            "--model",
            mannequin().to_str().unwrap(),
            "--crossfade",
            "run:idle:0.5",
            "--hold",
            "5",
            "--strip",
            "8",
            "--width",
            "480",
            "--height",
            "270",
        ],
    );
    if !output.status.success() {
        if is_no_adapter(&output) {
            eprintln!("SKIPPED a_held_crossfade_names_its_output: no GPU adapter");
            return;
        }
        panic!(
            "render --crossfade --hold failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert!(
        dir.join("mannequin-run-idle-crossfade-hold5.png").exists(),
        "stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("held 5 ticks per pose"), "{stdout}");
}

/// A character `--clip` strip defaults to `<name>-<clip>-strip.png`.
#[test]
fn needs_gpu_a_character_clip_strip_names_its_output() {
    let dir = target_tmpdir().join("character_strip");
    std::fs::create_dir_all(&dir).expect("work dir creates");
    let output = run_ludion_in(
        &dir,
        &[
            "render",
            "--model",
            mannequin().to_str().unwrap(),
            "--clip",
            "run",
            "--strip",
            "2",
            "--width",
            "480",
            "--height",
            "270",
        ],
    );
    if !output.status.success() {
        if is_no_adapter(&output) {
            eprintln!("SKIPPED a_character_clip_strip_names_its_output: no GPU adapter");
            return;
        }
        panic!(
            "render --clip --strip failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert!(dir.join("mannequin-run-strip.png").exists());
}

// --- Prefabs -----------------------------------------------------------------------------------

/// The hinge prefab the node-clip import emits (`open` and `snap`). Read-only.
fn hinge() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/import-node-clips/expected/model.prefab.yaml")
}

/// A prefab `--clip` strip defaults to `<name>-<clip>-strip.png` (the `.prefab` suffix stripped) and
/// prints one report line per phase, with no gap analysis.
#[test]
fn needs_gpu_a_prefab_clip_strip_names_its_output() {
    let dir = target_tmpdir().join("prefab_strip");
    std::fs::create_dir_all(&dir).expect("work dir creates");
    let output = run_ludion_in(
        &dir,
        &[
            "render",
            "--model",
            hinge().to_str().unwrap(),
            "--clip",
            "snap",
            "--strip",
            "4",
            "--width",
            "480",
            "--height",
            "270",
        ],
    );
    if !output.status.success() {
        if is_no_adapter(&output) {
            eprintln!("SKIPPED a_prefab_clip_strip_names_its_output: no GPU adapter");
            return;
        }
        panic!(
            "render --clip --strip failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert!(dir.join("model-snap-strip.png").exists());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("phase 3:"), "{stdout}");
    assert!(
        !stdout.contains("component"),
        "a prefab has no gap analysis: {stdout}"
    );
}

/// A prefab crossfade writes `<name>-<from>-<to>-crossfade.png` and reports each phase's weight.
#[test]
fn needs_gpu_a_prefab_crossfade_names_its_output_and_prints_weights() {
    let dir = target_tmpdir().join("prefab_crossfade");
    std::fs::create_dir_all(&dir).expect("work dir creates");
    let output = run_ludion_in(
        &dir,
        &[
            "render",
            "--model",
            hinge().to_str().unwrap(),
            "--crossfade",
            "open:snap:0.25",
            "--width",
            "480",
            "--height",
            "270",
        ],
    );
    if !output.status.success() {
        if is_no_adapter(&output) {
            eprintln!("SKIPPED a_prefab_crossfade_names_its_output: no GPU adapter");
            return;
        }
        panic!(
            "render --crossfade failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert!(dir.join("model-open-snap-crossfade.png").exists());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("crossfade open -> snap over 0.25s"),
        "{stdout}"
    );
    assert!(stdout.contains("weight 0.000"), "{stdout}");
}

/// **The rest pose is untouched.** `--clip open --time 0` — where every track's first key is the
/// part's rest — writes the same bytes as the prefab with no clip at all. Needs a GPU.
#[test]
fn needs_gpu_a_prefab_clip_at_time_zero_writes_the_rest_pose_bytes() {
    let dir = target_tmpdir().join("prefab_t0_bytes");
    std::fs::create_dir_all(&dir).expect("work dir creates");
    let (rest_out, posed_out) = (dir.join("rest.png"), dir.join("posed.png"));
    let render = |extra: &[&str], out: &Path| {
        let model = hinge();
        let mut args = vec!["render", "--model", model.to_str().unwrap()];
        args.extend_from_slice(extra);
        args.extend([
            "--out",
            out.to_str().unwrap(),
            "--width",
            "300",
            "--height",
            "200",
        ]);
        run_ludion(&args)
    };
    let rest = render(&[], &rest_out);
    if !rest.status.success() {
        if is_no_adapter(&rest) {
            eprintln!(
                "SKIPPED a_prefab_clip_at_time_zero_writes_the_rest_pose_bytes: no GPU adapter"
            );
            return;
        }
        panic!(
            "rest render failed: {}",
            String::from_utf8_lossy(&rest.stderr)
        );
    }
    let posed = render(&["--clip", "open", "--time", "0"], &posed_out);
    assert!(
        posed.status.success(),
        "render --clip --time failed: {}",
        String::from_utf8_lossy(&posed.stderr)
    );
    assert_eq!(
        std::fs::read(&rest_out).unwrap(),
        std::fs::read(&posed_out).unwrap(),
        "a clip at t = 0 draws the rest pose"
    );
}

// --- Assembly `--blend` and `--layer` ------------------------------------------------------------

/// The humanoid fixture: three parts, three looping clips, `arm_only` and the `locomotion` blend.
/// Read-only; every test renders into its own work directory.
fn blend_humanoid() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/assembly_blends/humanoid.assembly.yaml")
}

/// The component count a report line of `phase` names.
fn components_of(stdout: &str, phase: usize) -> String {
    let prefix = format!("  phase {phase}:");
    let line = stdout
        .lines()
        .find(|line| line.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no `{prefix}` line in: {stdout}"));
    line.rsplit(", ")
        .next()
        .expect("a component count ends the line")
        .to_string()
}

/// `--blend` beside `--clip` is a clap conflict, and a malformed spec, an unknown blend, a layer with no
/// base, an unknown mask and a fifth layer are each readable errors that write no file. No GPU.
#[test]
fn blend_and_layer_misuse_is_readable() {
    let model = blend_humanoid();
    let model = model.to_str().unwrap();
    let output = run_ludion(&[
        "render",
        "--model",
        model,
        "--clip",
        "walk",
        "--blend",
        "locomotion:0",
    ]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot be used with"));

    let dir = target_tmpdir().join("blend_misuse");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("work dir creates");
    let five: Vec<&str> = ["--layer", "run:1.0"].repeat(5);
    let cases: [(&[&str], &str); 5] = [
        (&["--blend", "locomotion"], "error[L1117]"),
        (&["--blend", "gait:0..1"], "`locomotion`"),
        (&["--layer", "run:1.0"], "needs a base pose"),
        (&["--clip", "walk", "--layer", "run:1.0:legs"], "`arm_only`"),
        (&[], "error[L1670]"),
    ];
    for (index, (extra, expected)) in cases.iter().enumerate() {
        let mut args = vec!["render", "--model", model];
        if index == 4 {
            args.extend(["--clip", "walk"]);
            args.extend(&five);
        }
        args.extend(*extra);
        let output = run_ludion_in(&dir, &args);
        assert!(!output.status.success(), "{extra:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(expected), "{extra:?}: {stderr}");
        assert_eq!(
            std::fs::read_dir(&dir).expect("dir reads").count(),
            0,
            "an error must not produce an output file"
        );
    }
}

/// `--blend locomotion:0..2 --strip 3` names `<name>-<blend>-blend.png`, reports each phase's params,
/// and the phase at the `walk` point poses like `walk` itself at its sync instant.
#[test]
fn needs_gpu_a_blend_sweep_names_its_output_and_reports_params() {
    let dir = target_tmpdir().join("blend_sweep");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("work dir creates");
    let model = blend_humanoid();
    let model = model.to_str().unwrap();
    let output = run_ludion_in(
        &dir,
        &[
            "render",
            "--model",
            model,
            "--blend",
            "locomotion:0..2",
            "--strip",
            "3",
            "--width",
            "480",
            "--height",
            "270",
        ],
    );
    if !output.status.success() {
        if is_no_adapter(&output) {
            eprintln!("SKIPPED a_blend_sweep_names_its_output_and_reports_params: no GPU adapter");
            return;
        }
        panic!(
            "render --blend failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert!(dir.join("humanoid-locomotion-blend.png").exists());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("blend locomotion: one cycle"), "{stdout}");
    assert!(
        stdout.contains("3 phases quantized to 60 Hz ticks"),
        "{stdout}"
    );
    for (phase, params) in ["[0, 0]", "[1, 0]", "[2, 0]"].iter().enumerate() {
        let line = stdout
            .lines()
            .find(|line| line.starts_with(&format!("  phase {phase}:")))
            .unwrap_or_else(|| panic!("no phase {phase} in: {stdout}"));
        assert!(
            line.contains(&format!("params {params}, phase 0.000")),
            "{line}"
        );
    }

    // `walk` syncs on a quarter of its second: that is the instant the blend plays it at phase 0.
    let walk = run_ludion_in(
        &dir,
        &[
            "render", "--model", model, "--clip", "walk", "--time", "0.25", "--width", "480",
            "--height", "270",
        ],
    );
    assert!(
        walk.status.success(),
        "{}",
        String::from_utf8_lossy(&walk.stderr)
    );
    let walk = String::from_utf8_lossy(&walk.stdout);
    assert_eq!(components_of(&stdout, 2), components_of(&walk, 0));
}

/// `--layer` over a clip echoes the stack, appends `-layered` and reports the posed gap analysis.
#[test]
fn needs_gpu_a_layer_over_a_clip_echoes_the_stack() {
    let dir = target_tmpdir().join("layer_echo");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("work dir creates");
    let model = blend_humanoid();
    let output = run_ludion_in(
        &dir,
        &[
            "render",
            "--model",
            model.to_str().unwrap(),
            "--clip",
            "walk",
            "--layer",
            "run:1.0:arm_only",
            "--layer",
            "idle:0.5::additive",
            "--width",
            "480",
            "--height",
            "270",
        ],
    );
    if !output.status.success() {
        if is_no_adapter(&output) {
            eprintln!("SKIPPED a_layer_over_a_clip_echoes_the_stack: no GPU adapter");
            return;
        }
        panic!(
            "render --layer failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert!(dir.join("humanoid-walk-t0-layered.png").exists());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("layers: run 1.0 arm_only override, idle 0.5 - additive"),
        "{stdout}"
    );
    assert!(
        components_of(&stdout, 0).ends_with("component")
            || components_of(&stdout, 0).ends_with("components")
    );
}

// --- Assembly `--look` and `--ik` -------------------------------------------------------------------

/// A six-cell assembly with a head look-at, a defaulted `hand_r` chain and an explicit `arm_l` chain,
/// two looping clips. Read-only; every test renders into its own work directory.
fn ik_humanoid() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/assembly_ik/humanoid.assembly.yaml")
}

/// Misusing `--look` and `--ik` is a readable `L1117` that writes no file. No GPU.
#[test]
fn look_and_ik_misuse_is_readable() {
    let model = ik_humanoid();
    let model = model.to_str().unwrap();
    let dir = target_tmpdir().join("ik_misuse");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("work dir creates");
    let cases: [(&[&str], &str); 8] = [
        (&["--look", "40,0"], "`--look` needs a base pose"),
        (&["--ik", "hand_r=1,2,3"], "`--ik` needs a base pose"),
        (&["--clip", "walk", "--look", "40"], "`<yaw>,<pitch>`"),
        (&["--clip", "walk", "--ik", "hand_r=1,2"], "`<x>,<y>,<z>`"),
        (&["--clip", "walk", "--ik", "hand_l=1,2,3"], "`arm_l`"),
        (&["--clip", "walk", "--ik", "look=1,2,3"], "names a look-at"),
        (&["--clip", "walk", "--look", "arm_l=0,0"], "names a chain"),
        (
            &["--blend", "locomotion:0..2", "--look", "0..9,0"],
            "both sweep the strip",
        ),
    ];
    for (extra, expected) in cases {
        let mut args = vec!["render", "--model", model];
        args.extend(extra);
        let output = run_ludion_in(&dir, &args);
        assert!(!output.status.success(), "{extra:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("error[L1117]"), "{extra:?}: {stderr}");
        assert!(stderr.contains(expected), "{extra:?}: {stderr}");
        assert_eq!(
            std::fs::read_dir(&dir).expect("dir reads").count(),
            0,
            "an error must not produce an output file"
        );
    }
}

/// `--look 0..90,0 --strip 4` writes the strip, names it `-ik`, and reports each phase's look.
#[test]
fn needs_gpu_a_look_sweep_reports_each_phases_look() {
    let dir = target_tmpdir().join("look_sweep");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("work dir creates");
    let model = ik_humanoid();
    let output = run_ludion_in(
        &dir,
        &[
            "render",
            "--model",
            model.to_str().unwrap(),
            "--clip",
            "walk",
            "--look",
            "0..90,0",
            "--strip",
            "4",
            "--width",
            "480",
            "--height",
            "270",
        ],
    );
    if !output.status.success() {
        if is_no_adapter(&output) {
            eprintln!("SKIPPED a_look_sweep_reports_each_phases_look: no GPU adapter");
            return;
        }
        panic!(
            "render --look failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert!(dir.join("humanoid-walk-strip-ik.png").exists());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("4 phases"), "{stdout}");
    for index in 0..4 {
        let prefix = format!("  phase {index}:");
        let line = stdout
            .lines()
            .find(|line| line.starts_with(&prefix))
            .unwrap_or_else(|| panic!("no `{prefix}` line in: {stdout}"));
        assert!(line.contains(", look ["), "{line}");
        assert!(line.ends_with("1 component"), "{line}");
    }
    // The endpoints of the sweep are exact; the middle phases are fractions of it.
    assert!(stdout.contains("look [0, 0]"), "{stdout}");
    assert!(stdout.contains("look [90, 0]"), "{stdout}");
    assert!(!stdout.contains("ik:"), "no --ik, no echo: {stdout}");
}

/// `--ik` echoes its target once and reports the gap analysis of the solved pose.
#[test]
fn needs_gpu_an_ik_target_is_echoed_and_analysed() {
    let dir = target_tmpdir().join("ik_echo");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("work dir creates");
    let model = ik_humanoid();
    let output = run_ludion_in(
        &dir,
        &[
            "render",
            "--model",
            model.to_str().unwrap(),
            "--clip",
            "idle",
            "--ik",
            "hand_r=4.5,2,0.5",
            "--ik",
            "arm_l=1.5,1,0.5",
            "--width",
            "480",
            "--height",
            "270",
        ],
    );
    if !output.status.success() {
        if is_no_adapter(&output) {
            eprintln!("SKIPPED an_ik_target_is_echoed_and_analysed: no GPU adapter");
            return;
        }
        panic!(
            "render --ik failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert!(dir.join("humanoid-idle-t0-ik.png").exists());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("ik: hand_r=[4.5, 2.0, 0.5], arm_l=[1.5, 1.0, 0.5]"),
        "{stdout}"
    );
    assert!(
        components_of(&stdout, 0).ends_with("component")
            || components_of(&stdout, 0).ends_with("components")
    );
}

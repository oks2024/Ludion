//! Integration tests for the model view harness ([`render_model_sheet`]).
//!
//! The CPU-only tests (L1117 dispatch, doc-validation pass-through) always run. The GPU tests skip
//! gracefully when no adapter is available (an `L2003` in the error list), matching `gpu_render.rs`.

use std::path::{Path, PathBuf};

use ludion_core::diagnostics::Diagnostic;
use ludion_engine::{ModelSheetRequest, ModelSweep, ModelView, SheetOutcome, render_model_sheet};
use ludion_scene::SceneOverride;
use serde_json::{Value, json};

/// A plain request for `models` at `w`×`h` with the default views.
fn plain<'a>(models: &'a [&'a Path], w: u32, h: u32) -> ModelSheetRequest<'a> {
    ModelSheetRequest::new(models, w, h)
}

/// A unique temp directory for one test's fixtures (under the crate's integration temp dir).
fn temp_dir(tag: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("model_sheet_{tag}"));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// Writes `contents` to `dir/name` and returns the path.
fn write_fixture(dir: &Path, name: &str, contents: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, contents).expect("write fixture");
    path
}

/// An asymmetric L-shaped voxel model: a green foot along +X and a blue arm along +Z in the base
/// layer, plus a red tower rising at the corner — so front, left, and top views must all differ.
const L_SHAPE_VOXEL: &str = "\
format: 1
kind: voxel
name: \"l-shape\"
size:
  - 4
  - 6
  - 3
palette:
  B:
    color:
      - 0.2
      - 0.35
      - 0.9
      - 1.0
  G:
    color:
      - 0.15
      - 0.8
      - 0.2
      - 1.0
  R:
    color:
      - 0.9
      - 0.15
      - 0.15
      - 1.0
slices:
  -
    - \"GGGG\"
    - \"B...\"
    - \"B...\"
  -
    - \"R...\"
    - \"....\"
    - \"....\"
  -
    - \"R...\"
    - \"....\"
    - \"....\"
  -
    - \"R...\"
    - \"....\"
    - \"....\"
  -
    - \"R...\"
    - \"....\"
    - \"....\"
  -
    - \"R...\"
    - \"....\"
    - \"....\"
";

/// A small asymmetric mesh (a tetrahedron), each face wound both ways so it reads regardless of the
/// pipeline's back-face culling — enough to render a non-flat cell.
const TETRA_MESH: &str = "\
format: 1
kind: mesh
positions:
  - 0.0
  - 0.0
  - 0.0
  - 2.0
  - 0.0
  - 0.0
  - 1.0
  - 0.0
  - 1.7
  - 1.0
  - 2.5
  - 0.6
indices:
  - 0
  - 1
  - 2
  - 0
  - 2
  - 1
  - 0
  - 1
  - 3
  - 0
  - 3
  - 1
  - 1
  - 2
  - 3
  - 1
  - 3
  - 2
  - 2
  - 0
  - 3
  - 2
  - 3
  - 0
";

/// A night rig shaped like `games/vigil/scenes/env-night.yaml`: an `Environment` + `PostProcess`
/// carrier plus a `moon` `DirectionalLight`.
const NIGHT_RIG: &str = "\
format: 1
entities:
  - name: environment
    components:
      Environment:
        sky_zenith:
          - 0.01
          - 0.02
          - 0.055
        sky_horizon:
          - 0.05
          - 0.08
          - 0.14
        ibl_intensity: 0.9
      PostProcess:
        exposure: 0.6
        vignette: 0.82
  - name: moon
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - -56.0
      DirectionalLight:
        direction:
          - 0.32
          - -0.45
          - 0.83
        color:
          - 0.55
          - 0.65
          - 0.9
        intensity: 0.8
";

fn no_adapter(errors: &[Diagnostic]) -> bool {
    ludion_core::require_gpu::is_no_adapter(errors)
}

/// Renders a sheet, or `None` (after printing a skip reason) when there is no GPU adapter.
fn render_or_skip(
    test: &str,
    model: &Path,
    views: &[ModelView],
    w: u32,
    h: u32,
) -> Option<SheetOutcome> {
    render_rig_or_skip(test, model, views, w, h, None)
}

/// [`render_or_skip`] with an optional `--rig` scene.
fn render_rig_or_skip(
    test: &str,
    model: &Path,
    views: &[ModelView],
    w: u32,
    h: u32,
    rig: Option<&Path>,
) -> Option<SheetOutcome> {
    let models = [model];
    let request = ModelSheetRequest {
        views,
        rig,
        ..plain(&models, w, h)
    };
    match render_model_sheet(&request) {
        Ok(outcome) => {
            eprintln!("[{test}] on {}", outcome.adapter);
            Some(outcome)
        }
        Err(errors) if no_adapter(&errors) => {
            eprintln!("[{test}] SKIPPED: no GPU adapter available");
            None
        }
        Err(errors) => panic!("[{test}] render failed: {errors:?}"),
    }
}

// --- CPU-only tests (always run) --------------------------------------------------------------

/// A missing file is an `L1117` whose message names the path.
#[test]
fn missing_file_is_l1117() {
    let dir = temp_dir("missing");
    let path = dir.join("nope.voxel.yaml");
    let errors = render_model_sheet(&plain(&[path.as_path()], 64, 64)).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|d| d.code.as_str() == "L1117" && d.message.contains("nope.voxel.yaml")),
        "expected an L1117 naming the path: {errors:?}",
    );
}

/// A scene document (no `kind:` key) is an `L1117`, not a render — a scene is not a model.
#[test]
fn scene_document_is_l1117() {
    let dir = temp_dir("scene");
    let path = write_fixture(&dir, "scene.yaml", "format: 1\nentities: []\n");
    let errors = render_model_sheet(&plain(&[path.as_path()], 64, 64)).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|d| d.code.as_str() == "L1117" && d.message.contains("scene.yaml")),
        "expected an L1117 for a scene document: {errors:?}",
    );
}

/// A document whose `kind:` is neither `mesh` nor `voxel` is an `L1117`.
#[test]
fn unknown_kind_is_l1117() {
    let dir = temp_dir("unknown_kind");
    let path = write_fixture(&dir, "thing.yaml", "format: 1\nkind: sprite\n");
    let errors = render_model_sheet(&plain(&[path.as_path()], 64, 64)).unwrap_err();
    assert!(
        errors.iter().any(|d| d.code.as_str() == "L1117"
            && d.message.contains("sprite")
            && d.message.contains("`sculpt`")),
        "expected an L1117 naming the unknown kind and the four accepted ones: {errors:?}",
    );
}

/// A small sculpt: a body with a blended nose, and a pearl hidden inside the body that meshes to no
/// surface.
const BLOB_SCULPT: &str = "\
format: 1
kind: sculpt
resolution: 16
triangles: 600
palette:
  N:
    color:
      - 0.9
      - 0.3
      - 0.2
      - 1.0
  S:
    color:
      - 0.5
      - 0.5
      - 0.55
      - 1.0
bones:
  - name: root
parts:
  - name: body
    shape: sphere
    radius: 0.5
    material: S
  - name: nose
    shape: sphere
    radius: 0.18
    at:
      - 0.0
      - 0.0
      - -0.5
    blend: 0.05
    material: N
  - name: pearl
    shape: sphere
    radius: 0.2
    material: S
";

/// A `--part` the sculpt does not name is an `L4008` that suggests the nearest part — before the GPU.
#[test]
fn an_unknown_sculpt_part_is_l4008_with_the_nearest_name() {
    let dir = temp_dir("sculpt_unknown_part");
    let path = write_fixture(&dir, "blob.sculpt.yaml", BLOB_SCULPT);
    let models = [path.as_path()];
    let errors = render_model_sheet(&ModelSheetRequest {
        part: Some("nsoe"),
        ..plain(&models, 64, 64)
    })
    .unwrap_err();
    assert_eq!(errors[0].code.as_str(), "L4008", "{errors:?}");
    assert!(errors[0].message.contains("nsoe"), "{errors:?}");
    let help = errors[0].help.as_deref().unwrap();
    assert!(help.contains("nearest match is `nose`"), "{help}");
}

/// A part that meshes to no surface has no bounds to frame — an `L4008` that says so.
#[test]
fn a_surfaceless_sculpt_part_is_l4008() {
    let dir = temp_dir("sculpt_hidden_part");
    let path = write_fixture(&dir, "blob.sculpt.yaml", BLOB_SCULPT);
    let models = [path.as_path()];
    let errors = render_model_sheet(&ModelSheetRequest {
        part: Some("pearl"),
        ..plain(&models, 64, 64)
    })
    .unwrap_err();
    assert_eq!(errors[0].code.as_str(), "L4008", "{errors:?}");
    assert!(errors[0].message.contains("no surface"), "{errors:?}");
}

/// `isolate` needs a `part`, and a `part` needs a sculpt: both are `L4008`, before the GPU.
#[test]
fn part_flags_need_a_part_and_a_sculpt() {
    let dir = temp_dir("sculpt_part_flags");
    let sculpt = write_fixture(&dir, "blob.sculpt.yaml", BLOB_SCULPT);
    let models = [sculpt.as_path()];
    let errors = render_model_sheet(&ModelSheetRequest {
        isolate: true,
        ..plain(&models, 64, 64)
    })
    .unwrap_err();
    assert_eq!(errors[0].code.as_str(), "L4008");
    assert!(errors[0].message.contains("--part"), "{errors:?}");

    let voxel = write_fixture(&dir, "l-shape.voxel.yaml", L_SHAPE_VOXEL);
    let models = [voxel.as_path()];
    let errors = render_model_sheet(&ModelSheetRequest {
        part: Some("head"),
        ..plain(&models, 64, 64)
    })
    .unwrap_err();
    assert_eq!(errors[0].code.as_str(), "L4008");
    assert!(errors[0].message.contains("kind: voxel"), "{errors:?}");
}

/// A recognized-but-invalid voxel document surfaces its own `L1113`/`L1114`, never `L1117`.
#[test]
fn broken_voxel_surfaces_the_underlying_code() {
    let dir = temp_dir("broken_voxel");
    // `kind: voxel` is recognized, but `size`/`palette`/`slices` are missing — an `L1113`.
    let path = write_fixture(&dir, "broken.voxel.yaml", "format: 1\nkind: voxel\n");
    let errors = render_model_sheet(&plain(&[path.as_path()], 64, 64)).unwrap_err();
    assert!(
        !errors.iter().any(|d| d.code.as_str() == "L1117"),
        "a recognized voxel document must not report L1117: {errors:?}",
    );
    assert!(
        errors
            .iter()
            .any(|d| matches!(d.code.as_str(), "L1113" | "L1114")),
        "expected the underlying voxel diagnostic: {errors:?}",
    );
}

/// The part file [`write_assembly_fixture`]'s single-part assembly names — the exact `L_SHAPE_VOXEL`
/// grid and palette, minus the palette (a part has none of its own), so meshing the two must produce
/// identical geometry (`mesh_assembly`'s own "single-part assembly equals the voxel model" proof).
const L_SHAPE_PART: &str = "\
format: 1
kind: part
size:
  - 4
  - 6
  - 3
slices:
  -
    - \"GGGG\"
    - \"B...\"
    - \"B...\"
  -
    - \"R...\"
    - \"....\"
    - \"....\"
  -
    - \"R...\"
    - \"....\"
    - \"....\"
  -
    - \"R...\"
    - \"....\"
    - \"....\"
  -
    - \"R...\"
    - \"....\"
    - \"....\"
  -
    - \"R...\"
    - \"....\"
    - \"....\"
";

/// A single-part `kind: assembly` document whose one part is [`L_SHAPE_PART`], placed at the
/// assembly origin — the assembled grid equals [`L_SHAPE_VOXEL`]'s own `size`, so the two render the
/// same model through two different document families.
const L_SHAPE_ASSEMBLY: &str = "\
format: 1
kind: assembly
name: \"l-shape\"
size:
  - 4
  - 6
  - 3
palette:
  B:
    color:
      - 0.2
      - 0.35
      - 0.9
      - 1.0
  G:
    color:
      - 0.15
      - 0.8
      - 0.2
      - 1.0
  R:
    color:
      - 0.9
      - 0.15
      - 0.15
      - 1.0
parts:
  - name: body
    file: parts/body.part.yaml
    place:
      - 0
      - 0
      - 0
";

/// Writes [`L_SHAPE_ASSEMBLY`] and its one part file ([`L_SHAPE_PART`]) under `dir`, returning the
/// assembly's own path.
fn write_assembly_fixture(dir: &Path) -> PathBuf {
    std::fs::create_dir_all(dir.join("parts")).expect("create parts dir");
    write_fixture(&dir.join("parts"), "body.part.yaml", L_SHAPE_PART);
    write_fixture(dir, "l-shape.assembly.yaml", L_SHAPE_ASSEMBLY)
}

/// An assembly's palette is a palette: a glyph it does not have is the ordinary `L4008` that lists
/// the ones it does, raised before any staging IO. CPU-only.
#[test]
fn an_unknown_assembly_glyph_is_l4008() {
    let dir = temp_dir("set_assembly_glyph");
    let model = write_assembly_fixture(&dir);
    let models = [model.as_path()];
    let set = [SceneOverride::parse("/palette/Q/emissive=2.0").unwrap()];
    let errors = render_model_sheet(&ModelSheetRequest {
        set: &set,
        ..plain(&models, 64, 64)
    })
    .unwrap_err();
    assert_eq!(errors[0].code.as_str(), "L4008");
    let help = errors[0].help.as_deref().unwrap();
    assert!(help.contains('B') && help.contains('R'), "{help}");
}

// --- GPU tests (skip without an adapter) ------------------------------------------------------

/// A sculpt renders a sheet with a model in it; framing on a part moves the camera, and isolating
/// it hides the rest of the model.
#[test]
fn needs_gpu_a_sculpt_sheet_renders_whole_part_framed_and_isolated() {
    let dir = temp_dir("sculpt_sheet");
    let path = write_fixture(&dir, "blob.sculpt.yaml", BLOB_SCULPT);
    let models = [path.as_path()];
    let views = [ModelView::Iso];
    let render = |part: Option<&str>, isolate: bool| {
        render_model_sheet(&ModelSheetRequest {
            views: &views,
            part,
            isolate,
            ..plain(&models, 160, 120)
        })
    };
    let whole = match render(None, false) {
        Ok(outcome) => outcome,
        Err(errors) if no_adapter(&errors) => {
            eprintln!("[sculpt_sheet] SKIPPED: no GPU adapter available");
            return;
        }
        Err(errors) => panic!("sculpt sheet failed: {errors:?}"),
    };
    assert_eq!((whole.image.width, whole.image.height), (160, 120));
    let first = &whole.image.rgba[..4];
    assert!(
        whole.image.rgba.chunks_exact(4).any(|pixel| pixel != first),
        "the sheet is one flat color"
    );

    let framed = render(Some("nose"), false).unwrap_or_else(|e| panic!("{e:?}"));
    let isolated = render(Some("nose"), true).unwrap_or_else(|e| panic!("{e:?}"));
    assert_ne!(
        whole.image.rgba, framed.image.rgba,
        "the part framing moved nothing"
    );
    assert_ne!(
        framed.image.rgba, isolated.image.rgba,
        "isolating hid nothing"
    );
}

/// Determinism: rendering the same sheet twice yields byte-identical images and PNG bytes.
#[test]
fn needs_gpu_same_inputs_are_byte_identical() {
    let dir = temp_dir("determinism");
    let model = write_fixture(&dir, "l-shape.voxel.yaml", L_SHAPE_VOXEL);
    let Some(first) = render_or_skip("determinism", &model, &[], 600, 400) else {
        return;
    };
    let second = render_model_sheet(&plain(&[model.as_path()], 600, 400)).unwrap();
    assert_eq!(
        first.image, second.image,
        "two renders of the same sheet must be byte-identical",
    );
    assert_eq!(
        first.image.encode_png().unwrap(),
        second.image.encode_png().unwrap(),
        "the PNG bytes must be byte-identical too",
    );
}

/// Extracts cell `(row, col)`'s pixels from a `sheet_w`-wide RGBA8 sheet of `cell_w`×`cell_h` cells.
fn crop_cell(rgba: &[u8], sheet_w: u32, cell_w: u32, cell_h: u32, row: u32, col: u32) -> Vec<u8> {
    let stride = (sheet_w * 4) as usize;
    let row_bytes = (cell_w * 4) as usize;
    let x0 = (col * cell_w * 4) as usize;
    let mut out = Vec::with_capacity(row_bytes * cell_h as usize);
    for r in 0..cell_h as usize {
        let y = (row * cell_h) as usize + r;
        out.extend_from_slice(&rgba[y * stride + x0..y * stride + x0 + row_bytes]);
    }
    out
}

/// True if `cell` has real pixel variance (not one flat color).
fn is_non_flat(cell: &[u8]) -> bool {
    let first = &cell[0..4];
    cell.chunks_exact(4).any(|px| px != first)
}

/// Orientation sanity: the six views of an asymmetric model produce pairwise-different, non-flat
/// front/left/top cells (proving the per-view camera really points where it claims).
#[test]
fn needs_gpu_views_are_distinct_and_non_flat() {
    let dir = temp_dir("orientation");
    let model = write_fixture(&dir, "l-shape.voxel.yaml", L_SHAPE_VOXEL);
    // 600×400 over the 3×2 canonical grid ⇒ exact 200×200 cells.
    let Some(outcome) = render_or_skip("orientation", &model, &[], 600, 400) else {
        return;
    };
    let (cw, ch) = (200, 200);
    let sw = outcome.image.width;
    let rgba = &outcome.image.rgba;
    // Canonical row-major order: front=0 (0,0), left=2 (0,2), top=4 (1,1).
    let front = crop_cell(rgba, sw, cw, ch, 0, 0);
    let left = crop_cell(rgba, sw, cw, ch, 0, 2);
    let top = crop_cell(rgba, sw, cw, ch, 1, 1);

    for (name, cell) in [("front", &front), ("left", &left), ("top", &top)] {
        assert!(is_non_flat(cell), "{name} cell is a flat single color");
    }
    assert_ne!(front, left, "front and left cells must differ");
    assert_ne!(front, top, "front and top cells must differ");
    assert_ne!(left, top, "left and top cells must differ");
}

/// A `--rig` sheet is deterministic in its own right, and really is a different image from the
/// default neutral one — the borrowed night rig reaches the pixels.
#[test]
fn needs_gpu_rig_sheet_is_deterministic_and_differs_from_the_default() {
    let dir = temp_dir("rig");
    let model = write_fixture(&dir, "l-shape.voxel.yaml", L_SHAPE_VOXEL);
    let models = [model.as_path()];
    let rig = write_fixture(&dir, "env-night.yaml", NIGHT_RIG);

    let Some(first) = render_rig_or_skip("rig", &model, &[ModelView::Iso], 256, 256, Some(&rig))
    else {
        return;
    };
    let iso_rig = ModelSheetRequest {
        views: &[ModelView::Iso],
        rig: Some(&rig),
        ..plain(&models, 256, 256)
    };
    let second = render_model_sheet(&iso_rig).unwrap();
    assert_eq!(
        first.image, second.image,
        "two rigged renders of the same sheet must be byte-identical",
    );

    let neutral = render_model_sheet(&ModelSheetRequest {
        views: &[ModelView::Iso],
        ..plain(&models, 256, 256)
    })
    .unwrap();
    assert_ne!(
        first.image, neutral.image,
        "the borrowed night rig must change the sheet",
    );
}

/// A rig scene with none of the four liftable component types is an `L1117` naming the rig — raised
/// before any GPU adapter is requested, so this test needs no adapter.
#[test]
fn rig_without_liftable_entities_is_l1117() {
    let dir = temp_dir("rig_bare");
    let model = write_fixture(&dir, "l-shape.voxel.yaml", L_SHAPE_VOXEL);
    let models = [model.as_path()];
    let rig = write_fixture(
        &dir,
        "bare.yaml",
        "format: 1\nentities:\n  - name: subject\n    components:\n      Mesh:\n        shape: plane\n",
    );
    let errors = render_model_sheet(&ModelSheetRequest {
        rig: Some(&rig),
        ..plain(&models, 64, 64)
    })
    .unwrap_err();
    assert!(
        errors
            .iter()
            .any(|d| d.code.as_str() == "L1117" && d.message.contains("bare.yaml")),
        "expected an L1117 naming the rig: {errors:?}",
    );
}

/// An invalid rig reports the scene's own diagnostics verbatim, never an `L1117`.
#[test]
fn broken_rig_surfaces_the_scene_diagnostics() {
    let dir = temp_dir("rig_broken");
    let model = write_fixture(&dir, "l-shape.voxel.yaml", L_SHAPE_VOXEL);
    let models = [model.as_path()];
    let rig = write_fixture(&dir, "broken.yaml", "format: 1\nentites: []\n");
    let errors = render_model_sheet(&ModelSheetRequest {
        rig: Some(&rig),
        ..plain(&models, 64, 64)
    })
    .unwrap_err();
    assert!(
        !errors.iter().any(|d| d.code.as_str() == "L1117"),
        "a readable rig scene must surface its own diagnostics: {errors:?}",
    );
    assert!(
        errors.iter().any(|d| d.code.as_str() == "L1010"),
        "expected the scene's own unknown-key diagnostic: {errors:?}",
    );
}

// --- Palette overrides and `--true-size` -------------------------------------------------------

/// `--set`/`--sweep` need a palette, and a mesh asset has none — an `L4008` that says so, raised
/// before any GPU adapter is requested.
#[test]
fn overriding_a_mesh_document_is_l4008() {
    let dir = temp_dir("set_mesh");
    let model = write_fixture(&dir, "tetra.mesh.yaml", TETRA_MESH);
    let models = [model.as_path()];
    let set = [SceneOverride::parse("/palette/A/emissive=2.0").unwrap()];
    let errors = render_model_sheet(&ModelSheetRequest {
        set: &set,
        ..plain(&models, 64, 64)
    })
    .unwrap_err();
    assert_eq!(errors[0].code.as_str(), "L4008");
    assert!(
        errors[0].message.contains("palette"),
        "expected the no-palette message: {errors:?}"
    );
}

/// A pointer outside the palette domain is an `L4008`, before the GPU.
#[test]
fn a_non_palette_pointer_is_l4008() {
    let dir = temp_dir("set_domain");
    let model = write_fixture(&dir, "l-shape.voxel.yaml", L_SHAPE_VOXEL);
    let models = [model.as_path()];
    let set = [SceneOverride::parse("/size/1=9").unwrap()];
    let errors = render_model_sheet(&ModelSheetRequest {
        set: &set,
        ..plain(&models, 64, 64)
    })
    .unwrap_err();
    assert_eq!(errors[0].code.as_str(), "L4008");
}

/// A sweep across more than one view is an `L4008`, before the GPU.
#[test]
fn a_multi_view_sweep_is_l4008() {
    let dir = temp_dir("sweep_views");
    let model = write_fixture(&dir, "l-shape.voxel.yaml", L_SHAPE_VOXEL);
    let models = [model.as_path()];
    let values = [json!(0.5), json!(1.5)];
    let errors = render_model_sheet(&ModelSheetRequest {
        views: &[ModelView::Front, ModelView::Iso],
        sweep: Some(ModelSweep {
            pointer: "/palette/R/emissive",
            values: &values,
        }),
        ..plain(&models, 64, 64)
    })
    .unwrap_err();
    assert_eq!(errors[0].code.as_str(), "L4008");
    assert!(errors[0].help.as_deref().unwrap().contains("--views"));
}

/// `--set` reaches the pixels, echoes what it replaced, and leaves the model file alone.
#[test]
fn needs_gpu_set_changes_the_sheet_and_echoes_the_change() {
    let dir = temp_dir("set_pixels");
    let model = write_fixture(&dir, "l-shape.voxel.yaml", L_SHAPE_VOXEL);
    let models = [model.as_path()];
    let set = [SceneOverride::parse("/palette/R/color/1=0.95").unwrap()];
    let request = ModelSheetRequest {
        views: &[ModelView::Front],
        set: &set,
        ..plain(&models, 192, 192)
    };
    let outcome = match render_model_sheet(&request) {
        Ok(outcome) => outcome,
        Err(errors) if no_adapter(&errors) => {
            eprintln!("[set] SKIPPED: no GPU adapter available");
            return;
        }
        Err(errors) => panic!("[set] render failed: {errors:?}"),
    };
    assert_eq!(outcome.applied.len(), 1);
    assert_eq!(outcome.applied[0].from, Some(json!(0.15)));

    let neutral = render_model_sheet(&ModelSheetRequest {
        views: &[ModelView::Front],
        ..plain(&models, 192, 192)
    })
    .unwrap();
    assert_ne!(
        outcome.image, neutral.image,
        "the palette override must change the sheet"
    );
    assert_eq!(
        std::fs::read_to_string(&model).unwrap(),
        L_SHAPE_VOXEL,
        "an override never writes to the model file"
    );
    assert!(
        !leftover_stage_dirs(),
        "the staged temp tree must be removed"
    );
}

/// Any staged directory this process created and did not clean up.
fn leftover_stage_dirs() -> bool {
    let prefix = format!("ludion-model-sheet-{}-", std::process::id());
    std::fs::read_dir(std::env::temp_dir())
        .map(|entries| {
            entries.flatten().any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(prefix.as_str())
            })
        })
        .unwrap_or(false)
}

/// Five views leave one grid slot empty, so the staged copies must follow the *view* count, not the
/// grid's. Skips when no GPU adapter is available.
#[test]
fn needs_gpu_set_survives_a_view_count_that_leaves_an_empty_slot() {
    let dir = temp_dir("set_five_views");
    let model = write_fixture(&dir, "l-shape.voxel.yaml", L_SHAPE_VOXEL);
    let models = [model.as_path()];
    let set = [SceneOverride::parse("/palette/R/emissive=1.5").unwrap()];
    let request = ModelSheetRequest {
        views: &[
            ModelView::Front,
            ModelView::Back,
            ModelView::Left,
            ModelView::Right,
            ModelView::Top,
        ],
        set: &set,
        ..plain(&models, 300, 200)
    };
    match render_model_sheet(&request) {
        Ok(outcome) => assert_eq!((outcome.image.width, outcome.image.height), (300, 200)),
        Err(errors) if no_adapter(&errors) => {
            eprintln!("[five views] SKIPPED: no GPU adapter available")
        }
        Err(errors) => panic!("[five views] render failed: {errors:?}"),
    }
}

/// A sweep lays one cell per value over a near-square grid, and the cells really differ.
#[test]
fn needs_gpu_sweep_lays_one_cell_per_value() {
    let dir = temp_dir("sweep_cells");
    let model = write_fixture(&dir, "l-shape.voxel.yaml", L_SHAPE_VOXEL);
    let models = [model.as_path()];
    let values: [Value; 3] = [json!(0.0), json!(1.5), json!(3.0)];
    let request = ModelSheetRequest {
        views: &[ModelView::Game],
        sweep: Some(ModelSweep {
            pointer: "/palette/R/emissive",
            values: &values,
        }),
        ..plain(&models, 400, 400)
    };
    let outcome = match render_model_sheet(&request) {
        Ok(outcome) => outcome,
        Err(errors) if no_adapter(&errors) => {
            eprintln!("[sweep] SKIPPED: no GPU adapter available");
            return;
        }
        Err(errors) => panic!("[sweep] render failed: {errors:?}"),
    };
    let sweep = outcome.sweep.expect("a sweep reports its grid");
    assert_eq!((sweep.cols, sweep.rows), (2, 2), "3 values → a 2×2 grid");
    assert_eq!(sweep.original, None, "`emissive` was unset in the file");
    assert_eq!((outcome.image.width, outcome.image.height), (400, 400));

    let (cw, ch) = (200, 200);
    let first = crop_cell(&outcome.image.rgba, 400, cw, ch, 0, 0);
    let last = crop_cell(&outcome.image.rgba, 400, cw, ch, 0, 1);
    assert_ne!(first, last, "two swept emissive values must render apart");
}

/// A sweep spends the sheet's cells on values and a strip spends them on phases, so one sheet
/// cannot hold both — an `L4008` in the same class as the multi-view sweep. CPU-only.
#[test]
fn a_sweep_with_a_strip_is_l4008() {
    let dir = temp_dir("sweep_strip");
    let model = write_clip_fixture(&dir);
    let models = [model.as_path()];
    let values = [json!(0.5), json!(1.5)];
    let errors = render_model_sheet(&ModelSheetRequest {
        clip: Some("snap"),
        strip: Some(4),
        sweep: Some(ModelSweep {
            pointer: "/palette/A/emissive",
            values: &values,
        }),
        ..plain(&models, 64, 64)
    })
    .unwrap_err();
    assert_eq!(errors[0].code.as_str(), "L4008");
    assert!(
        errors[0].help.as_deref().unwrap().contains("pick one"),
        "{errors:?}"
    );
}

/// **The overridden copy is the authored model plus the override, and nothing else.** Writing a
/// palette value an assembly already holds renders byte-identically to the un-overridden sheet, so
/// the staging round trip (re-emit the assembly, copy its parts and clips beside it, render from the
/// temp root) provably adds nothing of its own.
#[test]
fn needs_gpu_setting_an_assemblys_authored_value_renders_identically() {
    let dir = temp_dir("set_assembly_identity");
    let model = write_assembly_fixture(&dir);
    let models = [model.as_path()];
    let Some(neutral) = render_or_skip(
        "set_assembly_identity",
        &model,
        &[ModelView::Game],
        256,
        256,
    ) else {
        return;
    };
    // `R`'s authored red channel, written back over itself.
    let set = [SceneOverride::parse("/palette/R/color/0=0.9").unwrap()];
    let overridden = render_model_sheet(&ModelSheetRequest {
        views: &[ModelView::Game],
        set: &set,
        ..plain(&models, 256, 256)
    })
    .expect("the overridden assembly renders");

    assert_eq!(overridden.applied.len(), 1);
    assert_eq!(overridden.applied[0].from, Some(json!(0.9)));
    assert_eq!(
        neutral.image.rgba, overridden.image.rgba,
        "an override that changes nothing must change nothing",
    );
    assert_eq!(
        std::fs::read_to_string(&model).unwrap(),
        L_SHAPE_ASSEMBLY,
        "an override never writes to the assembly file"
    );
    assert!(
        !leftover_stage_dirs(),
        "the staged temp tree must be removed"
    );
}

/// `--set` reaches an assembly's pixels: the palette every part resolves against is the one written.
#[test]
fn needs_gpu_setting_an_assemblys_palette_changes_the_sheet() {
    let dir = temp_dir("set_assembly_pixels");
    let model = write_assembly_fixture(&dir);
    let models = [model.as_path()];
    let Some(neutral) = render_or_skip("set_assembly_pixels", &model, &[ModelView::Game], 256, 256)
    else {
        return;
    };
    let set = [SceneOverride::parse("/palette/R/emissive=3.0").unwrap()];
    let overridden = render_model_sheet(&ModelSheetRequest {
        views: &[ModelView::Game],
        set: &set,
        ..plain(&models, 256, 256)
    })
    .expect("the overridden assembly renders");

    assert_eq!(
        overridden.applied[0].from, None,
        "`emissive` was unset in the file"
    );
    assert_ne!(
        neutral.image.rgba, overridden.image.rgba,
        "the palette override must change the sheet",
    );
}

/// `--set` composes with `--clip`: the posed cell renders the *mutated* copy, which only works if
/// the staged tree carries the clip file as well as the parts (a missing one is the extractor's own
/// `L1101`, not a wrong-looking image).
#[test]
fn needs_gpu_setting_an_assemblys_palette_composes_with_a_clip() {
    let dir = temp_dir("set_assembly_clip");
    let model = write_clip_fixture(&dir);
    let models = [model.as_path()];
    let set = [SceneOverride::parse("/palette/A/color/2=0.95").unwrap()];
    let posed = ModelSheetRequest {
        views: &[ModelView::Game],
        set: &set,
        clip: Some("snap"),
        time: Some(1.0),
        ..plain(&models, 256, 256)
    };
    let Some(outcome) = pose_or_skip("set_assembly_clip", &posed) else {
        return;
    };
    assert_eq!(outcome.applied[0].from, Some(json!(0.2)));
    let clip = outcome
        .clip
        .as_ref()
        .expect("a posed sheet reports its clip");
    assert_eq!(clip.phases[0].tick, 60, "1.0 s at 60 Hz is tick 60");

    let rest = render_model_sheet(&ModelSheetRequest {
        views: &[ModelView::Game],
        set: &set,
        ..plain(&models, 256, 256)
    })
    .expect("the overridden rest sheet renders");
    assert_ne!(
        rest.image.rgba, outcome.image.rgba,
        "the clip poses the overridden copy, rather than being dropped with it",
    );
}

/// A sweep over an assembly lays one cell per value, exactly as it does over a voxel document.
#[test]
fn needs_gpu_a_sweep_over_an_assembly_lays_one_cell_per_value() {
    let dir = temp_dir("sweep_assembly");
    let model = write_assembly_fixture(&dir);
    let models = [model.as_path()];
    let values: [Value; 3] = [json!(0.0), json!(1.5), json!(3.0)];
    let request = ModelSheetRequest {
        views: &[ModelView::Game],
        sweep: Some(ModelSweep {
            pointer: "/palette/R/emissive",
            values: &values,
        }),
        ..plain(&models, 400, 400)
    };
    let outcome = match render_model_sheet(&request) {
        Ok(outcome) => outcome,
        Err(errors) if no_adapter(&errors) => {
            eprintln!("[sweep assembly] SKIPPED: no GPU adapter available");
            return;
        }
        Err(errors) => panic!("[sweep assembly] render failed: {errors:?}"),
    };
    let sweep = outcome.sweep.expect("a sweep reports its grid");
    assert_eq!((sweep.cols, sweep.rows), (2, 2), "3 values → a 2×2 grid");
    assert_eq!(sweep.original, None, "`emissive` was unset in the file");

    let (cw, ch) = (200, 200);
    let first = crop_cell(&outcome.image.rgba, 400, cw, ch, 0, 0);
    let last = crop_cell(&outcome.image.rgba, 400, cw, ch, 0, 1);
    assert_ne!(first, last, "two swept emissive values must render apart");
}

/// `--true-size` sizes the cell from the declared scale, so a model that declares itself ten times
/// taller gets a far larger cell — the whole point of the flag, and checkable without a GPU through
/// the sheet dimensions alone.
#[test]
fn needs_gpu_true_size_scales_the_cell_with_the_declared_scale() {
    let dir = temp_dir("true_size");
    let small = write_fixture(&dir, "small.voxel.yaml", L_SHAPE_VOXEL);
    let tall = write_fixture(
        &dir,
        "tall.voxel.yaml",
        &L_SHAPE_VOXEL.replace("name: \"l-shape\"", "name: \"l-shape\"\nscale: 4.0"),
    );

    let render = |model: &Path| {
        render_model_sheet(&ModelSheetRequest {
            views: &[ModelView::Game],
            true_size: true,
            ..plain(&[model], 1920, 1080)
        })
    };
    let small = match render(&small) {
        Ok(outcome) => outcome,
        Err(errors) if no_adapter(&errors) => {
            eprintln!("[true_size] SKIPPED: no GPU adapter available");
            return;
        }
        Err(errors) => panic!("[true_size] render failed: {errors:?}"),
    };
    let tall = render(&tall).unwrap();

    assert_eq!(small.true_size.unwrap().world_height, 6.0);
    assert_eq!(tall.true_size.unwrap().world_height, 24.0);
    // Within a pixel of exactly four times: each cell edge is rounded to an integer independently.
    assert!(
        tall.image.height.abs_diff(small.image.height * 4) <= 1,
        "four times the declared scale is four times the cell: {} vs {}",
        tall.image.height,
        small.image.height * 4,
    );
}

// --- Lineup ------------------------------------------------------------------------------------

/// A lineup renders **one** cell holding every model, and really is a different image from either
/// model alone. Skips when no GPU adapter is available.
#[test]
fn needs_gpu_a_lineup_is_one_cell_holding_every_model() {
    let dir = temp_dir("lineup");
    let a = write_fixture(&dir, "l-shape.voxel.yaml", L_SHAPE_VOXEL);
    let b = write_fixture(
        &dir,
        "tall.voxel.yaml",
        &L_SHAPE_VOXEL.replace("name: \"l-shape\"", "name: \"l-shape\"\nscale: 3.0"),
    );

    let pair = [a.as_path(), b.as_path()];
    let outcome = match render_model_sheet(&plain(&pair, 600, 400)) {
        Ok(outcome) => outcome,
        Err(errors) if no_adapter(&errors) => {
            eprintln!("[lineup] SKIPPED: no GPU adapter available");
            return;
        }
        Err(errors) => panic!("[lineup] render failed: {errors:?}"),
    };
    assert_eq!(
        (outcome.image.width, outcome.image.height),
        (600, 400),
        "a lineup is one cell filling the requested sheet",
    );

    let alone = [a.as_path()];
    let single = render_model_sheet(&ModelSheetRequest {
        views: &[ModelView::Game],
        ..plain(&alone, 600, 400)
    })
    .unwrap();
    assert_ne!(
        outcome.image, single.image,
        "the neighbour must reach the pixels",
    );
    assert!(
        !leftover_stage_dirs(),
        "the staged lineup copies must be removed"
    );
}

/// Two models with the same grid but different declared `scale:` render at that ratio — the honest
/// relative size a lineup exists for, checked through the whole pipeline. Skips without an adapter.
#[test]
fn needs_gpu_a_lineup_renders_declared_scale_honestly() {
    let dir = temp_dir("lineup_scale");
    let small = write_fixture(&dir, "small.voxel.yaml", L_SHAPE_VOXEL);
    let big = write_fixture(
        &dir,
        "big.voxel.yaml",
        &L_SHAPE_VOXEL.replace("name: \"l-shape\"", "name: \"l-shape\"\nscale: 4.0"),
    );

    // Swapping the order swaps which side each model stands on, so the image must change — proof the
    // row is laid out from the argument order and not from the file names.
    let forward = [small.as_path(), big.as_path()];
    let reverse = [big.as_path(), small.as_path()];
    let first = match render_model_sheet(&plain(&forward, 512, 288)) {
        Ok(outcome) => outcome,
        Err(errors) if no_adapter(&errors) => {
            eprintln!("[lineup_scale] SKIPPED: no GPU adapter available");
            return;
        }
        Err(errors) => panic!("[lineup_scale] render failed: {errors:?}"),
    };
    let flipped = render_model_sheet(&plain(&reverse, 512, 288)).unwrap();
    assert_ne!(first.image, flipped.image, "the row follows argument order");

    let again = render_model_sheet(&plain(&forward, 512, 288)).unwrap();
    assert_eq!(first.image, again.image, "a lineup is deterministic");
}

/// A lineup defaults to the game view alone, so `--true-size` frames it from the tallest model.
/// Skips when no GPU adapter is available.
#[test]
fn needs_gpu_a_true_size_lineup_is_framed_by_its_tallest_model() {
    let dir = temp_dir("lineup_true_size");
    let small = write_fixture(&dir, "small.voxel.yaml", L_SHAPE_VOXEL);
    let big = write_fixture(
        &dir,
        "big.voxel.yaml",
        &L_SHAPE_VOXEL.replace("name: \"l-shape\"", "name: \"l-shape\"\nscale: 4.0"),
    );

    let pair = [small.as_path(), big.as_path()];
    let outcome = match render_model_sheet(&ModelSheetRequest {
        true_size: true,
        ..plain(&pair, 1920, 1080)
    }) {
        Ok(outcome) => outcome,
        Err(errors) if no_adapter(&errors) => {
            eprintln!("[lineup_true_size] SKIPPED: no GPU adapter available");
            return;
        }
        Err(errors) => panic!("[lineup_true_size] render failed: {errors:?}"),
    };
    // The L-shape is 6 cells tall; at `scale: 4.0` the taller model stands 24 world units.
    assert_eq!(outcome.true_size.unwrap().world_height, 24.0);
}

/// A lineup takes one view; asking for more is an actionable `L4008`, raised before any GPU adapter.
#[test]
fn a_multi_view_lineup_is_l4008() {
    let dir = temp_dir("lineup_views");
    let a = write_fixture(&dir, "a.voxel.yaml", L_SHAPE_VOXEL);
    let b = write_fixture(&dir, "b.voxel.yaml", L_SHAPE_VOXEL);
    let pair = [a.as_path(), b.as_path()];

    let errors = render_model_sheet(&ModelSheetRequest {
        views: &[ModelView::Front, ModelView::Iso],
        ..plain(&pair, 64, 64)
    })
    .unwrap_err();
    assert_eq!(errors[0].code.as_str(), "L4008");
    assert!(errors[0].help.as_deref().unwrap().contains("--views"));
}

/// `--set`/`--sweep` calibrate one model, so pairing either with a lineup is an `L4008` that says so.
#[test]
fn a_lineup_with_overrides_is_l4008() {
    let dir = temp_dir("lineup_set");
    let a = write_fixture(&dir, "a.voxel.yaml", L_SHAPE_VOXEL);
    let b = write_fixture(&dir, "b.voxel.yaml", L_SHAPE_VOXEL);
    let pair = [a.as_path(), b.as_path()];
    let set = [SceneOverride::parse("/palette/R/emissive=2.0").unwrap()];

    let errors = render_model_sheet(&ModelSheetRequest {
        set: &set,
        ..plain(&pair, 64, 64)
    })
    .unwrap_err();
    assert_eq!(errors[0].code.as_str(), "L4008");
    assert!(errors[0].help.as_deref().unwrap().contains("calibrate"));
}

/// Every broken model in a lineup reports, not just the first — a lineup is several inputs and a
/// one-at-a-time loop is the cost the caller pays for it.
#[test]
fn a_lineup_reports_every_broken_model() {
    let dir = temp_dir("lineup_broken");
    let good = write_fixture(&dir, "good.voxel.yaml", L_SHAPE_VOXEL);
    let bad = write_fixture(&dir, "bad.voxel.yaml", "format: 1\nkind: sprite\n");
    let missing = dir.join("nope.voxel.yaml");
    let three = [good.as_path(), bad.as_path(), missing.as_path()];

    let errors = render_model_sheet(&plain(&three, 64, 64)).unwrap_err();
    assert!(
        errors.iter().filter(|d| d.code.as_str() == "L1117").count() >= 2,
        "expected an L1117 for each broken model: {errors:?}",
    );
}

/// Past the cap a lineup's cells are too narrow to compare — an `L4008` naming the cap.
#[test]
fn an_oversized_lineup_is_l4008() {
    let dir = temp_dir("lineup_cap");
    let model = write_fixture(&dir, "one.voxel.yaml", L_SHAPE_VOXEL);
    let paths: Vec<&Path> = (0..=ludion_engine::MAX_LINEUP_MODELS)
        .map(|_| model.as_path())
        .collect();

    let errors = render_model_sheet(&plain(&paths, 64, 64)).unwrap_err();
    assert_eq!(errors[0].code.as_str(), "L4008");
    assert!(
        errors[0]
            .message
            .contains(&ludion_engine::MAX_LINEUP_MODELS.to_string()),
        "{errors:?}"
    );
}

/// The mesh-document path renders a non-flat sheet (a single iso view of the tetrahedron).
#[test]
fn needs_gpu_mesh_document_renders_non_flat() {
    let dir = temp_dir("mesh");
    let model = write_fixture(&dir, "tetra.mesh.yaml", TETRA_MESH);
    let Some(outcome) = render_or_skip("mesh", &model, &[ModelView::Iso], 256, 256) else {
        return;
    };
    assert!(
        is_non_flat(&outcome.image.rgba),
        "the mesh sheet is a flat single color — the mesh path drew nothing",
    );
}

/// A single-part `kind: assembly` document renders **byte-identically** to the equivalent
/// `kind: voxel` document — `mesh_assembly`'s own "a single-part assembly equals the voxel model"
/// proof, carried all the way through the model-view harness (synthesized scene, extract, render).
#[test]
fn needs_gpu_assembly_document_renders_identically_to_the_equivalent_voxel_model() {
    let dir = temp_dir("assembly_equals_voxel");
    let voxel = write_fixture(&dir, "l-shape.voxel.yaml", L_SHAPE_VOXEL);
    let assembly = write_assembly_fixture(&dir);

    let Some(voxel_outcome) = render_or_skip("assembly_equals_voxel voxel", &voxel, &[], 300, 200)
    else {
        return;
    };
    let assembly_outcome = render_model_sheet(&plain(&[assembly.as_path()], 300, 200))
        .expect("the assembly model renders");

    assert_eq!(
        voxel_outcome.image.rgba, assembly_outcome.image.rgba,
        "a single-part assembly at the origin must render byte-identically to the equivalent voxel \
         model"
    );
}

/// A lineup mixes an assembly beside a voxel model in one cell, proving the lineup staging path
/// carries an assembly's part files along with it (a missing one would surface as `L1101`, not a
/// panic or a silently blank part).
#[test]
fn needs_gpu_a_lineup_mixes_an_assembly_with_a_voxel_model() {
    let dir = temp_dir("lineup_mixed_kinds");
    let voxel = write_fixture(&dir, "l-shape.voxel.yaml", L_SHAPE_VOXEL);
    let assembly = write_assembly_fixture(&dir);
    let models = [voxel.as_path(), assembly.as_path()];

    let request = ModelSheetRequest {
        views: &[ModelView::Game],
        ..plain(&models, 400, 300)
    };
    let outcome = match render_model_sheet(&request) {
        Ok(outcome) => outcome,
        Err(errors) if no_adapter(&errors) => {
            eprintln!("[lineup_mixed_kinds] SKIPPED: no GPU adapter available");
            return;
        }
        Err(errors) => panic!("lineup render failed: {errors:?}"),
    };
    assert_eq!((outcome.image.width, outcome.image.height), (400, 300));
    assert!(
        is_non_flat(&outcome.image.rgba),
        "a lineup of two real models must not render a flat sheet",
    );
}

// --- Posed sheets, strips, and the per-phase gap analysis (`Plan/voxel-animation.md` decision 11) ---

/// The `body` half of the two-part clip fixture: a 2×2×1 block at the assembly origin.
const CLIP_BODY_PART: &str = "\
format: 1
kind: part
size:
  - 2
  - 2
  - 1
slices:
  -
    - \"AA\"
  -
    - \"AA\"
";

/// The `nub` half: a 1×2×1 sliver that starts face-adjacent to the body and is what the clip moves.
const CLIP_NUB_PART: &str = "\
format: 1
kind: part
size:
  - 1
  - 2
  - 1
slices:
  -
    - \"A\"
  -
    - \"A\"
";

/// A two-part assembly binding two clips over the same parts. At rest the `nub` touches the `body`,
/// so the assembled grid is one connected component — the baseline every posed count is read
/// against.
const CLIP_ASSEMBLY: &str = "\
format: 1
kind: assembly
name: \"clip-fixture\"
size:
  - 8
  - 2
  - 1
palette:
  A:
    color:
      - 0.8
      - 0.3
      - 0.2
      - 1.0
parts:
  - name: body
    file: parts/body.part.yaml
    place:
      - 0
      - 0
      - 0
  - name: nub
    file: parts/nub.part.yaml
    parent: body
    place:
      - 2
      - 0
      - 0
clips:
  snap: anims/snap.anim.yaml
  swing: anims/swing.anim.yaml
";

/// A **one-shot** clip that slides the `nub` four cells clear of the `body` — an unambiguous
/// detachment at its final pose, and nothing subtle enough for the rounding to be blamed for it.
const CLIP_SNAP: &str = "\
format: 1
kind: animation
name: snap
duration: 1.0
loop: false
tracks:
  nub:
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
";

/// A **looping** clip over the same parts, so the two spacing rules can be compared on one fixture.
const CLIP_SWING: &str = "\
format: 1
kind: animation
name: swing
duration: 1.0
loop: true
tracks:
  nub:
    translation:
      - t: 0.0
        v:
          - 0.0
          - 0.0
          - 0.0
      - t: 0.5
        v:
          - 4.0
          - 0.0
          - 0.0
      - t: 1.0
        v:
          - 0.0
          - 0.0
          - 0.0
";

/// Writes the two-part clip fixture (assembly, both parts, both clips) under `dir`, returning the
/// assembly's own path.
fn write_clip_fixture(dir: &Path) -> PathBuf {
    std::fs::create_dir_all(dir.join("parts")).expect("create parts dir");
    std::fs::create_dir_all(dir.join("anims")).expect("create anims dir");
    write_fixture(&dir.join("parts"), "body.part.yaml", CLIP_BODY_PART);
    write_fixture(&dir.join("parts"), "nub.part.yaml", CLIP_NUB_PART);
    write_fixture(&dir.join("anims"), "snap.anim.yaml", CLIP_SNAP);
    write_fixture(&dir.join("anims"), "swing.anim.yaml", CLIP_SWING);
    write_fixture(dir, "clip-fixture.assembly.yaml", CLIP_ASSEMBLY)
}

/// `--time` names an instant of a clip, and without `--clip` there is no clip to take one of.
/// CPU-only.
#[test]
fn time_without_a_clip_is_l1117() {
    let dir = temp_dir("time_no_clip");
    let model = write_clip_fixture(&dir);
    let models = [model.as_path()];
    let errors = render_model_sheet(&ModelSheetRequest {
        time: Some(0.4),
        ..plain(&models, 64, 64)
    })
    .unwrap_err();
    assert_eq!(errors[0].code.as_str(), "L1117");
    assert!(
        errors[0].message.contains("`--time` needs `--clip`"),
        "{errors:?}"
    );
}

/// `--strip` is the same modifier in plural. CPU-only.
#[test]
fn strip_without_a_clip_is_l1117() {
    let dir = temp_dir("strip_no_clip");
    let model = write_clip_fixture(&dir);
    let models = [model.as_path()];
    let errors = render_model_sheet(&ModelSheetRequest {
        strip: Some(4),
        ..plain(&models, 64, 64)
    })
    .unwrap_err();
    assert_eq!(errors[0].code.as_str(), "L1117");
    assert!(
        errors[0].message.contains("`--strip` needs `--clip`"),
        "{errors:?}"
    );
}

/// A voxel document is one rigid piece: it has no parts to pose and no `clips:` map to name one
/// from. CPU-only — caught while dispatching on `kind:`, before any staging IO.
#[test]
fn a_clip_against_a_voxel_document_is_l1117() {
    let dir = temp_dir("clip_on_voxel");
    let model = write_fixture(&dir, "l-shape.voxel.yaml", L_SHAPE_VOXEL);
    let models = [model.as_path()];
    let errors = render_model_sheet(&ModelSheetRequest {
        clip: Some("snap"),
        ..plain(&models, 64, 64)
    })
    .unwrap_err();
    assert_eq!(errors[0].code.as_str(), "L1117");
    assert!(
        errors[0]
            .message
            .contains("only an assembly, a character or a prefab"),
        "{errors:?}"
    );
}

/// An unbound clip name lists the ones the assembly does bind — the difference between "no such
/// clip" and "you spelled it wrong" is one line of output. CPU-only.
#[test]
fn an_unbound_clip_name_is_l1117_listing_the_bound_ones() {
    let dir = temp_dir("clip_unbound");
    let model = write_clip_fixture(&dir);
    let models = [model.as_path()];
    let errors = render_model_sheet(&ModelSheetRequest {
        clip: Some("wave"),
        ..plain(&models, 64, 64)
    })
    .unwrap_err();
    assert_eq!(errors[0].code.as_str(), "L1117");
    assert!(errors[0].message.contains("`snap`"), "{errors:?}");
    assert!(errors[0].message.contains("`swing`"), "{errors:?}");
}

/// A strip of one phase is a posed sheet spelled the long way; past the cap the phases are too
/// narrow to read. CPU-only.
#[test]
fn an_out_of_range_strip_is_l1117() {
    let dir = temp_dir("strip_range");
    let model = write_clip_fixture(&dir);
    let models = [model.as_path()];
    for phases in [1, ludion_engine::MAX_STRIP_PHASES + 1] {
        let errors = render_model_sheet(&ModelSheetRequest {
            clip: Some("snap"),
            strip: Some(phases),
            ..plain(&models, 64, 64)
        })
        .unwrap_err();
        assert_eq!(errors[0].code.as_str(), "L1117");
        assert!(errors[0].message.contains("out of range"), "{errors:?}");
    }
}

/// A clip poses one model; a lineup compares the rest poses of several. CPU-only.
#[test]
fn a_clip_with_a_lineup_is_l1117() {
    let dir = temp_dir("clip_lineup");
    let model = write_clip_fixture(&dir);
    let models = [model.as_path(), model.as_path()];
    let errors = render_model_sheet(&ModelSheetRequest {
        clip: Some("snap"),
        ..plain(&models, 64, 64)
    })
    .unwrap_err();
    assert_eq!(errors[0].code.as_str(), "L1117");
    assert!(errors[0].message.contains("poses one model"), "{errors:?}");
}

/// A strip lays every phase out in one cell, so — exactly like a lineup — it takes one view.
/// CPU-only.
#[test]
fn a_multi_view_strip_is_l1117() {
    let dir = temp_dir("strip_views");
    let model = write_clip_fixture(&dir);
    let models = [model.as_path()];
    let errors = render_model_sheet(&ModelSheetRequest {
        clip: Some("snap"),
        strip: Some(3),
        views: &[ModelView::Front, ModelView::Iso],
        ..plain(&models, 64, 64)
    })
    .unwrap_err();
    assert_eq!(errors[0].code.as_str(), "L1117");
    assert!(errors[0].message.contains("takes one view"), "{errors:?}");
}

// --- Posed GPU tests (skip without an adapter) -------------------------------------------------

/// Renders a posed sheet, or `None` (after printing a skip reason) when there is no GPU adapter.
fn pose_or_skip(test: &str, request: &ModelSheetRequest) -> Option<SheetOutcome> {
    match render_model_sheet(request) {
        Ok(outcome) => Some(outcome),
        Err(errors) if no_adapter(&errors) => {
            eprintln!("[{test}] SKIPPED: no GPU adapter available");
            None
        }
        Err(errors) => panic!("[{test}] posed render failed: {errors:?}"),
    }
}

/// **The rest pose is untouched.** A clip evaluated at `t = 0` — where every track's first keyframe
/// is the identity — renders byte-identically to the same model with no clip at all, which is what
/// makes every committed rest-pose golden safe from this feature by construction.
#[test]
fn needs_gpu_a_clip_at_time_zero_renders_the_rest_pose_byte_identically() {
    let dir = temp_dir("clip_t0_is_rest");
    let model = write_clip_fixture(&dir);
    let models = [model.as_path()];

    let Some(rest) = pose_or_skip("clip_t0_is_rest", &plain(&models, 300, 200)) else {
        return;
    };
    let posed = render_model_sheet(&ModelSheetRequest {
        clip: Some("snap"),
        ..plain(&models, 300, 200)
    })
    .expect("the posed sheet renders");

    assert!(rest.clip.is_none(), "a rest sheet reports no clip");
    let clip = posed.clip.as_ref().expect("a posed sheet reports its clip");
    assert_eq!(clip.phases.len(), 1);
    assert_eq!(clip.phases[0].tick, 0);
    assert_eq!(
        clip.phases[0].components,
        Some(1),
        "the fixture is one connected piece at rest"
    );
    assert_eq!(
        rest.image.rgba, posed.image.rgba,
        "posing at t = 0 must reproduce the rest pose exactly"
    );
}

/// A posed sheet at a mid-clip instant is a *different* image from the rest pose — the pose actually
/// reaches the renderer, rather than being computed and dropped.
#[test]
fn needs_gpu_a_posed_sheet_differs_from_the_rest_pose() {
    let dir = temp_dir("clip_pose_differs");
    let model = write_clip_fixture(&dir);
    let models = [model.as_path()];

    let Some(rest) = pose_or_skip("clip_pose_differs", &plain(&models, 300, 200)) else {
        return;
    };
    let posed = render_model_sheet(&ModelSheetRequest {
        clip: Some("snap"),
        time: Some(1.0),
        ..plain(&models, 300, 200)
    })
    .expect("the posed sheet renders");

    let clip = posed.clip.as_ref().expect("a posed sheet reports its clip");
    assert_eq!(clip.phases[0].tick, 60, "1.0 s at 60 Hz is tick 60");
    assert_eq!(clip.phases[0].time, 1.0);
    assert_ne!(
        rest.image.rgba, posed.image.rgba,
        "the nub slid four cells: the posed sheet cannot be the rest sheet"
    );
}

/// The gap analysis names the piece that came away and the parts owning it — "the nub detaches at
/// t = 1.0" as text, in the same `stray sample … nearest main-body cell …` shape `check-scene`
/// prints over the rest pose.
#[test]
fn needs_gpu_a_pose_that_detaches_a_part_reports_the_stray_and_its_owners() {
    let dir = temp_dir("clip_gap_analysis");
    let model = write_clip_fixture(&dir);
    let models = [model.as_path()];

    let Some(outcome) = pose_or_skip(
        "clip_gap_analysis",
        &ModelSheetRequest {
            clip: Some("snap"),
            time: Some(1.0),
            ..plain(&models, 300, 200)
        },
    ) else {
        return;
    };

    let phase = &outcome.clip.as_ref().expect("a clip report").phases[0];
    assert_eq!(
        phase.components,
        Some(2),
        "the nub is four cells clear of the body: two components"
    );
    let stray = &phase.strays[0];
    assert_eq!(stray.owners, vec!["nub".to_string()]);
    let main = stray
        .main
        .as_ref()
        .expect("a stray has a nearest main cell");
    assert_eq!(main.owners, vec!["body".to_string()]);
}

/// A strip is one cell holding `n` phases at the requested sheet size, and its phase times follow
/// the **looping** rule: `i × duration / n`, so the last phase is not the first repeated.
#[test]
fn needs_gpu_a_looping_strip_is_one_cell_of_n_phases() {
    let dir = temp_dir("clip_strip_loop");
    let model = write_clip_fixture(&dir);
    let models = [model.as_path()];

    let Some(outcome) = pose_or_skip(
        "clip_strip_loop",
        &ModelSheetRequest {
            clip: Some("swing"),
            strip: Some(4),
            ..plain(&models, 400, 300)
        },
    ) else {
        return;
    };

    assert_eq!((outcome.image.width, outcome.image.height), (400, 300));
    let clip = outcome.clip.as_ref().expect("a strip reports its clip");
    assert!(clip.looped);
    assert_eq!(
        clip.phases.iter().map(|p| p.tick).collect::<Vec<_>>(),
        vec![0, 15, 30, 45],
        "a 1.0 s looping clip in four phases lands on quarter-second ticks"
    );
    assert!(
        is_non_flat(&outcome.image.rgba),
        "a strip of four real poses must not render a flat sheet"
    );
}

/// The **one-shot** rule instead: `i × duration / (n − 1)`, so both endpoints show — and the last
/// phase is the detached one, which the gap analysis reports while the first phase stays whole.
#[test]
fn needs_gpu_a_one_shot_strip_shows_both_endpoints_and_reports_each_phase() {
    let dir = temp_dir("clip_strip_one_shot");
    let model = write_clip_fixture(&dir);
    let models = [model.as_path()];

    let Some(outcome) = pose_or_skip(
        "clip_strip_one_shot",
        &ModelSheetRequest {
            clip: Some("snap"),
            strip: Some(3),
            ..plain(&models, 400, 300)
        },
    ) else {
        return;
    };

    let clip = outcome.clip.as_ref().expect("a strip reports its clip");
    assert!(!clip.looped);
    assert_eq!(
        clip.phases.iter().map(|p| p.tick).collect::<Vec<_>>(),
        vec![0, 30, 60],
        "a 1.0 s one-shot in three phases shows 0, 0.5, and the final 1.0"
    );
    assert_eq!(
        clip.phases[0].components,
        Some(1),
        "whole at the first phase"
    );
    assert!(clip.phases[0].strays.is_empty());
    assert_eq!(
        clip.phases[2].components,
        Some(2),
        "apart at the last phase"
    );
    assert_eq!(clip.phases[2].strays[0].owners, vec!["nub".to_string()]);
}

/// **The conservative rasterization's headline property, on real committed content.** The gallery
/// `figure` assembly's `wave` clip rotates two 1-cell-wide arms about their shoulders, which a
/// point-sampled rasterization reports as detached at every non-zero phase — the rotated column
/// staircases, and a diagonal step is not 6-connected. Marking every cell the transformed cube's
/// AABB touches keeps the column whole, so every phase of the strip reports exactly one component:
/// "the arm detaches at t = 0.4" stays a statement worth grepping for.
#[test]
fn needs_gpu_the_gallery_wave_clip_holds_together_at_every_phase() {
    let gallery = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/gallery/voxels/figure.assembly.yaml");
    let models = [gallery.as_path()];

    let Some(outcome) = pose_or_skip(
        "gallery_wave_connected",
        &ModelSheetRequest {
            clip: Some("wave"),
            strip: Some(4),
            ..plain(&models, 300, 200)
        },
    ) else {
        return;
    };

    let clip = outcome.clip.as_ref().expect("a strip reports its clip");
    assert_eq!(clip.phases.len(), 4);
    for phase in &clip.phases {
        assert_eq!(
            phase.components,
            Some(1),
            "a rotating limb must not read as detached from itself: phase t = {} reported {:?} \
             components, strays {:?}",
            phase.time,
            phase.components,
            phase.strays,
        );
    }
}

// --- Characters and crossfades -----------------------------------------------------------------

/// The gallery's mannequin: a skinned character binding `idle`, `jump`, `run` (at overlay speed
/// 1.2) and `spin`. Read-only; every test renders from the repo's own copy.
fn mannequin() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/gallery/characters/mannequin.assembly.yaml")
}

fn sheet_bytes(outcome: &SheetOutcome) -> &[u8] {
    &outcome.image.rgba
}

fn spec(text: &str) -> Option<ludion_engine::CrossfadeSpec<'_>> {
    Some(ludion_engine::parse_crossfade(text).expect("a well-formed spec"))
}

/// A character renders at rest through the model harness, and a posed instant differs from rest.
#[test]
fn needs_gpu_a_character_renders_at_rest_and_posed() {
    let path = mannequin();
    let models = [path.as_path()];
    let rest_request = ModelSheetRequest {
        views: &[ModelView::Game],
        ..plain(&models, 480, 270)
    };
    let Some(rest) = pose_or_skip("character_rest", &rest_request) else {
        return;
    };
    assert!(
        is_non_flat(sheet_bytes(&rest)),
        "the mannequin draws something"
    );
    assert!(rest.clip.is_none());

    let posed = pose_or_skip(
        "character_posed",
        &ModelSheetRequest {
            clip: Some("run"),
            time: Some(0.25),
            ..rest_request
        },
    )
    .expect("the rest render found an adapter");
    assert_ne!(
        sheet_bytes(&rest),
        sheet_bytes(&posed),
        "run at 0.25 s is not the rest pose"
    );
    let clip = posed.clip.expect("a posed sheet reports its clip");
    assert_eq!(clip.phases.len(), 1);
    assert_eq!(clip.phases[0].components, None);
    assert!(clip.phases[0].strays.is_empty());
}

/// A character's clip time is `tick × spt × overlay speed`: `run` is bound at 1.2, so a strip's
/// reported times carry the factor the render samples with.
#[test]
fn needs_gpu_a_character_strip_reports_clip_time_through_the_overlay_speed() {
    let path = mannequin();
    let models = [path.as_path()];
    let Some(outcome) = pose_or_skip(
        "character_strip",
        &ModelSheetRequest {
            clip: Some("run"),
            strip: Some(4),
            ..plain(&models, 480, 270)
        },
    ) else {
        return;
    };
    let clip = outcome.clip.expect("a strip reports its clip");
    assert_eq!(clip.phases.len(), 4);
    let spt = 1.0 / 60.0;
    for phase in &clip.phases {
        assert_eq!(phase.components, None);
        assert_eq!(
            phase.time,
            phase.tick as f64 * spt * 1.2,
            "tick {}",
            phase.tick
        );
    }
    // 0.8 s looped over 4 phases is 0.2 clip seconds apart: 0.2 / 1.2 s is 10 ticks.
    assert_eq!(
        clip.phases
            .iter()
            .map(|phase| phase.tick)
            .collect::<Vec<_>>(),
        vec![0, 10, 20, 30]
    );
}

/// Character refusals that need no GPU: an unbound clip lists the bound ones, a lineup and an
/// override are refused.
#[test]
fn character_request_errors_are_readable() {
    let path = mannequin();
    let models = [path.as_path()];
    let errors = render_model_sheet(&ModelSheetRequest {
        clip: Some("fly"),
        ..plain(&models, 64, 64)
    })
    .unwrap_err();
    assert_eq!(errors[0].code.as_str(), "L1117");
    for name in ["idle", "jump", "run", "spin"] {
        assert!(
            errors[0].message.contains(&format!("`{name}`")),
            "{errors:?}"
        );
    }

    let dir = temp_dir("character_lineup");
    let voxel = write_fixture(&dir, "l-shape.voxel.yaml", L_SHAPE_VOXEL);
    let pair = [path.as_path(), voxel.as_path()];
    let errors = render_model_sheet(&plain(&pair, 64, 64)).unwrap_err();
    assert_eq!(errors[0].code.as_str(), "L1117");
    assert!(errors[0].message.contains("renders alone"), "{errors:?}");

    let errors = render_model_sheet(&ModelSheetRequest {
        set: &[SceneOverride {
            pointer: "/palette/a/color".to_string(),
            value: json!([1.0, 0.0, 0.0]),
        }],
        ..plain(&models, 64, 64)
    })
    .unwrap_err();
    assert_eq!(errors[0].code.as_str(), "L4008");
}

/// `run:idle:0.4` over five phases lands on ticks 0, 6, 12, 18, 24 with weights 0, ¼, ½, ¾, 1, and
/// two renders are byte-identical.
#[test]
fn needs_gpu_a_character_crossfade_strip_reports_its_weights_and_is_deterministic() {
    let path = mannequin();
    let models = [path.as_path()];
    let request = ModelSheetRequest {
        crossfade: spec("run:idle:0.4"),
        ..plain(&models, 480, 270)
    };
    let Some(first) = pose_or_skip("character_crossfade", &request) else {
        return;
    };
    let second = pose_or_skip("character_crossfade", &request).expect("adapter");
    assert_eq!(sheet_bytes(&first), sheet_bytes(&second));

    let clip = first.clip.as_ref().expect("a crossfade reports its clip");
    assert_eq!(clip.name, "idle");
    let fade = clip.crossfade.as_ref().expect("the crossfade is reported");
    assert_eq!(fade.from, "run");
    assert_eq!(
        clip.phases
            .iter()
            .map(|phase| phase.tick)
            .collect::<Vec<_>>(),
        vec![0, 6, 12, 18, 24]
    );
    // The fade length is an f32 (0.4 is not exact), and the report names the weight the render drew.
    for (phase, want) in clip.phases.iter().zip([0.0, 0.25, 0.5, 0.75, 1.0]) {
        let weight = phase.weight.expect("a crossfade phase has a weight");
        assert!(
            (weight - want).abs() < 1e-6,
            "tick {}: {weight} vs {want}",
            phase.tick
        );
        assert_eq!(phase.components, None);
    }

    let same = pose_or_skip(
        "character_crossfade_same_clip",
        &ModelSheetRequest {
            crossfade: spec("run:run:0.4"),
            ..request
        },
    )
    .expect("adapter");
    assert_ne!(
        sheet_bytes(&first),
        sheet_bytes(&same),
        "blending into idle differs from blending run into itself"
    );
}

/// `hold: 5` snaps each phase tick of a 0.5 s fade (ticks 0,4,9,13,17,21,26,30 at eight phases) down
/// to the 5-tick grid, and the weights follow the snapped ticks.
#[test]
fn needs_gpu_a_held_crossfade_strip_snaps_its_phases_to_the_grid() {
    let path = mannequin();
    let models = [path.as_path()];
    let Some(outcome) = pose_or_skip(
        "character_crossfade_hold",
        &ModelSheetRequest {
            crossfade: spec("run:idle:0.5"),
            strip: Some(8),
            hold: Some(5),
            ..plain(&models, 480, 270)
        },
    ) else {
        return;
    };
    let clip = outcome.clip.expect("a crossfade reports its clip");
    assert_eq!(clip.hold, 5);
    assert_eq!(
        clip.phases
            .iter()
            .map(|phase| phase.tick)
            .collect::<Vec<_>>(),
        vec![0, 0, 5, 10, 15, 20, 25, 30]
    );
    for (phase, want) in clip.phases.iter().zip([
        0.0,
        0.0,
        1.0 / 6.0,
        1.0 / 3.0,
        0.5,
        2.0 / 3.0,
        5.0 / 6.0,
        1.0,
    ]) {
        let weight = phase.weight.expect("a crossfade phase has a weight");
        assert!(
            (weight - want).abs() < 1e-6,
            "tick {}: {weight} vs {want}",
            phase.tick
        );
    }
}

/// A hold of 1 is every tick: the sheet is byte-identical to one with no hold.
#[test]
fn needs_gpu_a_hold_of_one_changes_nothing() {
    let path = mannequin();
    let models = [path.as_path()];
    let request = ModelSheetRequest {
        clip: Some("run"),
        strip: Some(4),
        ..plain(&models, 480, 270)
    };
    let Some(none) = pose_or_skip("character_hold_none", &request) else {
        return;
    };
    let one = pose_or_skip(
        "character_hold_one",
        &ModelSheetRequest {
            hold: Some(1),
            ..request
        },
    )
    .expect("adapter");
    assert_eq!(sheet_bytes(&none), sheet_bytes(&one));
}

/// `--hold` misuse needs no GPU: zero is `L1317`, and a hold with no clip or crossfade is `L1117`.
#[test]
fn hold_request_errors_are_readable() {
    let path = mannequin();
    let models = [path.as_path()];

    let errors = render_model_sheet(&ModelSheetRequest {
        crossfade: spec("run:idle:0.25"),
        hold: Some(0),
        ..plain(&models, 64, 64)
    })
    .unwrap_err();
    assert_eq!(errors[0].code.as_str(), "L1317");

    let errors = render_model_sheet(&ModelSheetRequest {
        hold: Some(5),
        ..plain(&models, 64, 64)
    })
    .unwrap_err();
    assert_eq!(errors[0].code.as_str(), "L1117");
    assert!(errors[0].message.contains("--hold"), "{errors:?}");
}

/// A crossfade on an assembly reports the gap analysis for every phase.
#[test]
fn needs_gpu_an_assembly_crossfade_keeps_its_gap_analysis() {
    let dir = temp_dir("crossfade_assembly");
    let model = write_clip_fixture(&dir);
    let models = [model.as_path()];
    let Some(outcome) = pose_or_skip(
        "assembly_crossfade",
        &ModelSheetRequest {
            crossfade: spec("snap:snap:0.5"),
            ..plain(&models, 480, 270)
        },
    ) else {
        return;
    };
    let clip = outcome.clip.expect("a crossfade reports its clip");
    assert_eq!(clip.phases.len(), 5);
    for phase in &clip.phases {
        assert!(phase.components.is_some(), "tick {}", phase.tick);
        assert!(phase.weight.is_some());
    }
}

/// Crossfade misuse needs no GPU: beside `--clip`, in a lineup, or naming an unbound clip.
#[test]
fn crossfade_request_errors_are_readable() {
    let path = mannequin();
    let models = [path.as_path()];

    let errors = render_model_sheet(&ModelSheetRequest {
        clip: Some("run"),
        crossfade: spec("run:idle:0.25"),
        ..plain(&models, 64, 64)
    })
    .unwrap_err();
    assert_eq!(errors[0].code.as_str(), "L1117");

    let dir = temp_dir("crossfade_lineup");
    let a = write_fixture(&dir, "a.voxel.yaml", L_SHAPE_VOXEL);
    let b = write_fixture(&dir, "b.voxel.yaml", L_SHAPE_VOXEL);
    let pair = [a.as_path(), b.as_path()];
    let errors = render_model_sheet(&ModelSheetRequest {
        crossfade: spec("run:idle:0.25"),
        ..plain(&pair, 64, 64)
    })
    .unwrap_err();
    assert_eq!(errors[0].code.as_str(), "L1117");

    for text in ["fly:idle:0.25", "run:fly:0.25"] {
        let errors = render_model_sheet(&ModelSheetRequest {
            crossfade: spec(text),
            ..plain(&models, 64, 64)
        })
        .unwrap_err();
        assert_eq!(errors[0].code.as_str(), "L1117", "{text}");
        assert!(errors[0].message.contains("`fly`"), "{errors:?}");
        assert!(errors[0].message.contains("`spin`"), "{errors:?}");
    }

    let errors = render_model_sheet(&ModelSheetRequest {
        time: Some(0.1),
        ..plain(&models, 64, 64)
    })
    .unwrap_err();
    assert!(
        errors[0].message.contains("`--clip` or `--crossfade`"),
        "{errors:?}"
    );
}

// --- Blend sweeps and layers -------------------------------------------------------------------

/// The clip fixture's parts with two looping clips and a `slide` blend between them: `home` leaves the
/// `nub` against the `body`, `far` holds it four cells clear, and `nub_only` masks it alone.
fn write_blend_fixture(dir: &Path) -> PathBuf {
    let held = |name: &str, x: f64| {
        format!(
            "format: 1\nkind: animation\nname: {name}\nduration: 1.0\nloop: true\ntracks:\n  nub:\n    \
             translation:\n      - t: 0.0\n        v:\n          - {x:?}\n          - 0.0\n          - 0.0\n      \
             - t: 1.0\n        v:\n          - {x:?}\n          - 0.0\n          - 0.0\n"
        )
    };
    std::fs::create_dir_all(dir.join("parts")).expect("create parts dir");
    std::fs::create_dir_all(dir.join("anims")).expect("create anims dir");
    write_fixture(&dir.join("parts"), "body.part.yaml", CLIP_BODY_PART);
    write_fixture(&dir.join("parts"), "nub.part.yaml", CLIP_NUB_PART);
    write_fixture(&dir.join("anims"), "home.anim.yaml", &held("home", 0.0));
    write_fixture(&dir.join("anims"), "far.anim.yaml", &held("far", 4.0));
    let assembly = CLIP_ASSEMBLY.replace(
        "clips:\n  snap: anims/snap.anim.yaml\n  swing: anims/swing.anim.yaml\n",
        "clips:\n  home: anims/home.anim.yaml\n  far: anims/far.anim.yaml\nmasks:\n  nub_only:\n    \
         joints:\n      - nub\nblends:\n  slide:\n    points:\n      - clip: home\n        at: 0.0\n      \
         - clip: far\n        at: 1.0\n",
    );
    write_fixture(dir, "blend-fixture.assembly.yaml", &assembly)
}

fn blend(text: &str) -> Option<ludion_engine::BlendSweepSpec<'_>> {
    Some(ludion_engine::parse_blend(text).expect("a well-formed spec"))
}

/// A swept blend reports each phase's parameters, and its ends pose as the point clips alone: the
/// gap analysis of `slide` at 0 is `home`'s and at 1 is `far`'s.
#[test]
fn needs_gpu_a_blend_sweep_reports_params_and_poses_as_its_points() {
    let dir = temp_dir("blend_sweep");
    let model = write_blend_fixture(&dir);
    let models = [model.as_path()];
    let Some(outcome) = pose_or_skip(
        "blend_sweep",
        &ModelSheetRequest {
            blend: blend("slide:0..1"),
            strip: Some(3),
            ..plain(&models, 480, 270)
        },
    ) else {
        return;
    };
    let clip = outcome.clip.expect("a blend sweep reports its clip");
    assert_eq!(clip.blend.as_deref(), Some("slide"));
    assert!(clip.looped);
    assert_eq!(
        clip.phases
            .iter()
            .map(|phase| phase.params)
            .collect::<Vec<_>>(),
        [Some([0.0, 0.0]), Some([0.5, 0.0]), Some([1.0, 0.0])]
    );
    let alone = |name: &str| {
        let outcome = pose_or_skip(
            "blend_point",
            &ModelSheetRequest {
                clip: Some(name),
                ..plain(&models, 480, 270)
            },
        )
        .expect("adapter");
        outcome.clip.expect("a clip reports").phases[0].components
    };
    assert_eq!(clip.phases[0].components, alone("home"));
    assert_eq!(clip.phases[2].components, alone("far"));
    assert_eq!(clip.phases[0].components, Some(1));
    assert_eq!(clip.phases[2].components, Some(2));
}

/// A masked override layer moves the masked part and the gap analysis sees it; a weight of zero is
/// no layer at all.
#[test]
fn needs_gpu_a_layer_over_a_clip_reports_the_layered_pose() {
    let dir = temp_dir("layer_over_clip");
    let model = write_blend_fixture(&dir);
    let models = [model.as_path()];
    let layers = [ludion_engine::parse_layer("far:1.0:nub_only").expect("a layer")];
    let Some(outcome) = pose_or_skip(
        "layer_over_clip",
        &ModelSheetRequest {
            clip: Some("home"),
            layers: &layers,
            ..plain(&models, 480, 270)
        },
    ) else {
        return;
    };
    let clip = outcome.clip.expect("a layered sheet reports its clip");
    assert_eq!(clip.layers, ["far 1.0 nub_only override"]);
    assert_eq!(clip.phases[0].components, Some(2));

    let off = [ludion_engine::parse_layer("far:0.0:nub_only").expect("a layer")];
    let outcome = pose_or_skip(
        "layer_off",
        &ModelSheetRequest {
            clip: Some("home"),
            layers: &off,
            ..plain(&models, 480, 270)
        },
    )
    .expect("adapter");
    assert_eq!(outcome.clip.expect("clip").phases[0].components, Some(1));
}

/// Blend and layer misuse needs no GPU.
#[test]
fn blend_and_layer_request_errors_are_readable() {
    let dir = temp_dir("blend_errors");
    let model = write_blend_fixture(&dir);
    let models = [model.as_path()];
    let layer = |text| [ludion_engine::parse_layer(text).expect("a layer")];

    let errors = render_model_sheet(&ModelSheetRequest {
        blend: blend("gait:0..1"),
        ..plain(&models, 64, 64)
    })
    .unwrap_err();
    assert_eq!(errors[0].code.as_str(), "L1117");
    assert!(errors[0].message.contains("`slide`"), "{errors:?}");

    let errors = render_model_sheet(&ModelSheetRequest {
        blend: blend("slide:0..1"),
        clip: Some("home"),
        ..plain(&models, 64, 64)
    })
    .unwrap_err();
    assert_eq!(errors[0].code.as_str(), "L1117");

    let errors = render_model_sheet(&ModelSheetRequest {
        blend: blend("slide:0:1"),
        ..plain(&models, 64, 64)
    })
    .unwrap_err();
    assert!(errors[0].message.contains("1D"), "{errors:?}");

    let errors = render_model_sheet(&ModelSheetRequest {
        layers: &layer("far:1.0"),
        ..plain(&models, 64, 64)
    })
    .unwrap_err();
    assert!(errors[0].message.contains("--layer"), "{errors:?}");

    let errors = render_model_sheet(&ModelSheetRequest {
        clip: Some("home"),
        layers: &layer("far:1.0:legs"),
        ..plain(&models, 64, 64)
    })
    .unwrap_err();
    assert_eq!(errors[0].code.as_str(), "L1117");
    assert!(errors[0].message.contains("`nub_only`"), "{errors:?}");

    let five = [ludion_engine::parse_layer("far:1.0").expect("a layer"); 5];
    let errors = render_model_sheet(&ModelSheetRequest {
        clip: Some("home"),
        layers: &five,
        ..plain(&models, 64, 64)
    })
    .unwrap_err();
    assert_eq!(errors[0].code.as_str(), "L1670");

    let arm = arm();
    let prefab = [arm.as_path()];
    let errors = render_model_sheet(&ModelSheetRequest {
        blend: blend("gait:0..1"),
        ..plain(&prefab, 64, 64)
    })
    .unwrap_err();
    assert_eq!(errors[0].code.as_str(), "L1117");
    assert!(errors[0].message.contains("`sway`"), "{errors:?}");

    let errors = render_model_sheet(&ModelSheetRequest {
        clip: Some("swing"),
        layers: &layer("curl:1.0:legs"),
        ..plain(&prefab, 64, 64)
    })
    .unwrap_err();
    assert_eq!(errors[0].code.as_str(), "L1117");
    assert!(errors[0].message.contains("`forearm`"), "{errors:?}");

    let errors = render_model_sheet(&ModelSheetRequest {
        clip: Some("swing"),
        layers: &layer("wave:1.0"),
        ..plain(&prefab, 64, 64)
    })
    .unwrap_err();
    assert_eq!(errors[0].code.as_str(), "L1117");
}

/// The hand-authored arm prefab, with a `forearm` mask and a `sway` blend.
fn arm() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/gallery/prefabs/arm/arm.prefab.yaml")
}

/// `--blend` and `--layer` on a prefab: each phase reports its parameters, and the masked layer
/// reaches the drawn root part.
#[test]
fn needs_gpu_a_prefab_blend_sweep_takes_params_and_layers() {
    let arm = arm();
    let models = [arm.as_path()];
    let request = ModelSheetRequest {
        blend: blend("sway:0..1"),
        strip: Some(3),
        views: &[ModelView::Game],
        ..plain(&models, 480, 270)
    };
    let Some(plain_sweep) = pose_or_skip("prefab_blend", &request) else {
        return;
    };
    let layers = [ludion_engine::parse_layer("curl:1:forearm").expect("a layer")];
    let layered = pose_or_skip(
        "prefab_blend_layer",
        &ModelSheetRequest {
            layers: &layers,
            time: Some(0.5),
            ..request
        },
    )
    .expect("adapter");
    let unlayered = pose_or_skip(
        "prefab_blend_unlayered",
        &ModelSheetRequest {
            time: Some(0.5),
            ..request
        },
    )
    .expect("adapter");
    let clip = plain_sweep.clip.as_ref().expect("a sweep reports its clip");
    assert_eq!(clip.blend.as_deref(), Some("sway"));
    assert_eq!(
        clip.phases
            .iter()
            .map(|phase| phase.params)
            .collect::<Vec<_>>(),
        [Some([0.0, 0.0]), Some([0.5, 0.0]), Some([1.0, 0.0])]
    );
    assert!(clip.phases.iter().all(|phase| phase.components.is_none()));
    let reported = layered.clip.as_ref().expect("a layered sheet reports");
    assert_eq!(reported.layers, ["curl 1.0 forearm override"]);
    assert_ne!(sheet_bytes(&layered), sheet_bytes(&unlayered));
}

// --- Prefabs -----------------------------------------------------------------------------------

/// The hinge prefab the node-clip import emits: `base`, `lid` and `latch`, with `open` and `snap`.
/// Read-only; every test renders from the repo's own copy.
fn hinge() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/import-node-clips/expected/model.prefab.yaml")
}

/// A prefab poses through the model harness: a strip's phases differ from rest and from each other.
#[test]
fn needs_gpu_a_prefab_clip_strip_poses_the_hinge() {
    let path = hinge();
    let models = [path.as_path()];
    let rest_request = ModelSheetRequest {
        views: &[ModelView::Game],
        ..plain(&models, 480, 270)
    };
    let Some(rest) = pose_or_skip("prefab_rest", &rest_request) else {
        return;
    };
    let strip = pose_or_skip(
        "prefab_strip",
        &ModelSheetRequest {
            clip: Some("snap"),
            strip: Some(4),
            ..rest_request
        },
    )
    .expect("adapter");
    assert_ne!(sheet_bytes(&strip), sheet_bytes(&rest));
    let clip = strip.clip.as_ref().expect("a posed sheet reports its clip");
    assert_eq!((clip.name.as_str(), clip.phases.len()), ("snap", 4));
    assert!(clip.phases.iter().all(|phase| phase.components.is_none()));
}

/// Prefab refusals that need no GPU: an unbound clip lists the bound ones, a lineup and an
/// override are refused.
#[test]
fn prefab_request_errors_are_readable() {
    let path = hinge();
    let models = [path.as_path()];
    let errors = render_model_sheet(&ModelSheetRequest {
        clip: Some("nope"),
        ..plain(&models, 64, 64)
    })
    .unwrap_err();
    assert_eq!(errors[0].code.as_str(), "L1117");
    for name in ["open", "snap"] {
        assert!(
            errors[0].message.contains(&format!("`{name}`")),
            "{errors:?}"
        );
    }

    let dir = temp_dir("prefab_lineup");
    let voxel = write_fixture(&dir, "l-shape.voxel.yaml", L_SHAPE_VOXEL);
    let pair = [path.as_path(), voxel.as_path()];
    let errors = render_model_sheet(&plain(&pair, 64, 64)).unwrap_err();
    assert_eq!(errors[0].code.as_str(), "L1117");
    assert!(
        errors[0].message.contains("a prefab renders alone"),
        "{errors:?}"
    );

    let errors = render_model_sheet(&ModelSheetRequest {
        set: &[SceneOverride {
            pointer: "/palette/a/color".to_string(),
            value: json!([1.0, 0.0, 0.0]),
        }],
        ..plain(&models, 64, 64)
    })
    .unwrap_err();
    assert_eq!(errors[0].code.as_str(), "L4008");
    assert!(errors[0].message.contains("a prefab has no palette"));
}

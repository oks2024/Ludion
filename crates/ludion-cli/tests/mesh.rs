//! Integration tests for `ludion mesh pick` and `ludion mesh apply`, driven against the built
//! binary. CPU only: neither verb opens a device.
//!
//! The derive's own conformance (region distances, the rasteriser's tie rule, the dilate pass, the
//! byte-exact PNG) lives in `ludion-mesh-edit`'s tests; these cover the CLI surface — that a pick
//! lands where the fixture's geometry says it must, that a miss and a bad view are `L1175` naming
//! the axis the author read the pixel off, that `apply` writes and `--check` catches a drifted
//! byte, and that `--json` says what a consumer reads.
//!
//! **The fixture's shape is the assertion.** Two quads face each other across the origin, the front
//! one at `z = -0.25` and the back at `z = +0.25`, with disjoint UV squares. A `front`-view pick
//! down the middle must therefore report `z = -0.25` and a UV in the front quad's square: the sheet
//! camera's `front` sits on world **−Z**, and reading the authored front as `+Z` would answer with
//! the other quad rather than with an error.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Two quads, each 1×1 in `x`/`y`, at `z = ∓0.25`. Triangles 0–1 are the front quad (wound to face
/// −Z, UVs in `[0, 0.5]²`), 2–3 the back (facing +Z, UVs in `[0.5, 1]²`).
const PLATE_MESH: &str = "\
format: 1
kind: mesh
name: plate
positions:
  - -0.5
  - -0.5
  - -0.25
  - 0.5
  - -0.5
  - -0.25
  - 0.5
  - 0.5
  - -0.25
  - -0.5
  - 0.5
  - -0.25
  - -0.5
  - -0.5
  - 0.25
  - 0.5
  - -0.5
  - 0.25
  - 0.5
  - 0.5
  - 0.25
  - -0.5
  - 0.5
  - 0.25
uvs:
  - 0.0
  - 0.5
  - 0.5
  - 0.5
  - 0.5
  - 0.0
  - 0.0
  - 0.0
  - 0.5
  - 1.0
  - 1.0
  - 1.0
  - 1.0
  - 0.5
  - 0.5
  - 0.5
base_color_texture:
  path: \"../textures/base.png\"
indices:
  - 0
  - 2
  - 1
  - 0
  - 3
  - 2
  - 4
  - 5
  - 6
  - 4
  - 6
  - 7
";

/// One sphere on the front quad's centre, painting an emissive map the source does not have. No
/// `size:`, so the created map takes the base-colour map's dimensions.
const PLATE_EDIT: &str = "\
format: 1
kind: mesh-edit
name: plate-lit
source: plate.mesh.yaml
ops:
  - op: paint
    map: emissive
    color:
      - 0.05
      - 1.0
      - 0.35
    region:
      sphere:
        at:
          - 0.0
          - 0.0
          - -0.25
        radius: 0.2
";

/// The base-colour map's one colour, so a pick can name the bytes under the hit.
const BASE_RGBA: [u8; 4] = [10, 200, 40, 255];
const MAP_EXTENT: u32 = 64;

/// A stable (not per-process) scratch dir, cleared each run — the `generate_atlas.rs` rationale.
fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    if let Err(error) = std::fs::remove_dir_all(&dir) {
        assert!(
            error.kind() == std::io::ErrorKind::NotFound,
            "stale scratch dir {dir:?} could not be removed: {error}"
        );
    }
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A fresh tree laid out the way `gen accept` leaves one: `meshes/` beside `textures/`, the edit
/// document next to the mesh it edits. Returns the tree's root.
fn tree(name: &str) -> PathBuf {
    let dir = scratch(name);
    std::fs::create_dir_all(dir.join("meshes")).unwrap();
    std::fs::create_dir_all(dir.join("textures")).unwrap();
    std::fs::write(dir.join("meshes/plate.mesh.yaml"), PLATE_MESH).unwrap();
    std::fs::write(dir.join("meshes/plate-lit.mesh-edit.yaml"), PLATE_EDIT).unwrap();
    let png = ludion_assets::encode_png(
        MAP_EXTENT,
        MAP_EXTENT,
        &BASE_RGBA.repeat((MAP_EXTENT * MAP_EXTENT) as usize),
    )
    .expect("the encoder takes a 64x64 image");
    std::fs::write(dir.join("textures/base.png"), &png).unwrap();
    dir
}

fn mesh_path(dir: &Path) -> PathBuf {
    dir.join("meshes/plate.mesh.yaml")
}

fn edit_path(dir: &Path) -> PathBuf {
    dir.join("meshes/plate-lit.mesh-edit.yaml")
}

fn run_ludion(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ludion"))
        .args(args)
        .output()
        .expect("failed to run the ludion binary")
}

fn pick(mesh: &Path, view: &str, pixel: &str, extra: &[&str]) -> Output {
    let mut args = vec![
        "mesh",
        "pick",
        mesh.to_str().unwrap(),
        "--view",
        view,
        "--pixel",
        pixel,
    ];
    args.extend_from_slice(extra);
    run_ludion(&args)
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn pick_json(mesh: &Path, view: &str, pixel: &str) -> serde_json::Value {
    let output = pick(mesh, view, pixel, &["--json"]);
    assert!(output.status.success(), "pick failed: {}", stderr(&output));
    serde_json::from_str(&stdout(&output)).expect("pick --json emits one JSON object")
}

fn close(value: f64, expected: f64) -> bool {
    (value - expected).abs() < 0.01
}

/// The headline: a pick down the middle of the `front` cell lands on the quad the sheet's `front`
/// camera can see — the near one, at `z = -0.25`, in its own UV square — and reports the texel and
/// the sRGB bytes of the base-colour map under it.
#[test]
fn a_centre_pick_lands_on_the_front_quad() {
    let dir = tree("mesh_pick_centre");
    let hit = pick_json(&mesh_path(&dir), "front", "960,540");

    assert_eq!(hit["view"], "front");
    assert_eq!(hit["label"], "FRONT -Z");
    assert_eq!(hit["extent"], serde_json::json!([1920, 1080]));

    let position: Vec<f64> = serde_json::from_value(hit["position"].clone()).unwrap();
    assert!(close(position[0], 0.0), "x off centre: {position:?}");
    assert!(close(position[1], 0.0), "y off centre: {position:?}");
    assert!(
        close(position[2], -0.25),
        "the front quad is the near one: {position:?}"
    );

    let triangle = hit["triangle"].as_u64().unwrap();
    assert!(triangle < 2, "a front-quad triangle, got {triangle}");
    let uv: Vec<f64> = serde_json::from_value(hit["uv"].clone()).unwrap();
    assert!(close(uv[0], 0.25) && close(uv[1], 0.25), "uv {uv:?}");

    let map = &hit["maps"][0];
    assert_eq!(map["map"], "base_color");
    assert_eq!(map["width"], 64);
    // The quad's centre is a texel boundary (`0.25 × 64`), and the ray runs through the centre of
    // the *pixel*, half a pixel off the cell's exact middle — so either neighbour is right, and the
    // other quad's square (32..64) is not.
    let texel: Vec<u64> = serde_json::from_value(map["texel"].clone()).unwrap();
    assert!(
        (15..=16).contains(&texel[0]) && (15..=16).contains(&texel[1]),
        "{texel:?}"
    );
    assert_eq!(
        hit["base_color_srgb8"],
        serde_json::json!(BASE_RGBA.to_vec())
    );
}

/// The same pixel through the `back` camera finds the other quad — the two views are not the same
/// camera, and neither is 180° wrong.
#[test]
fn the_back_view_finds_the_other_quad() {
    let dir = tree("mesh_pick_back");
    let hit = pick_json(&mesh_path(&dir), "back", "960,540");
    let position: Vec<f64> = serde_json::from_value(hit["position"].clone()).unwrap();
    assert!(close(position[2], 0.25), "{position:?}");
    let uv: Vec<f64> = serde_json::from_value(hit["uv"].clone()).unwrap();
    assert!(close(uv[0], 0.75) && close(uv[1], 0.75), "uv {uv:?}");
}

/// A cell of another size frames the model differently, so the extent is taken from the flags and
/// reported back — the centre of a 320×180 cell is still the quad's centre.
#[test]
fn a_cell_size_is_honoured() {
    let dir = tree("mesh_pick_cell");
    let output = pick(
        &mesh_path(&dir),
        "front",
        "160,90",
        &["--width", "320", "--height", "180", "--json"],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    let hit: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(hit["extent"], serde_json::json!([320, 180]));
    let position: Vec<f64> = serde_json::from_value(hit["position"].clone()).unwrap();
    assert!(close(position[2], -0.25), "{position:?}");
}

/// A pixel in the corner of the cell is off the model: `L1175`, naming the axis the cell's caption
/// names, so an author who read the wrong sheet can tell.
#[test]
fn a_corner_pixel_misses_with_the_views_axis() {
    let dir = tree("mesh_pick_miss");
    let output = pick(&mesh_path(&dir), "front", "4,4", &[]);
    assert!(!output.status.success(), "a corner pixel must not hit");
    let stderr = stderr(&output);
    assert!(stderr.contains("error[L1175]"), "{stderr}");
    assert!(stderr.contains("FRONT -Z"), "{stderr}");
    assert!(stderr.contains("1920x1080"), "{stderr}");
}

#[test]
fn an_unknown_view_is_l1175() {
    let dir = tree("mesh_pick_view");
    let output = pick(&mesh_path(&dir), "side", "960,540", &[]);
    assert!(!output.status.success());
    let stderr = stderr(&output);
    assert!(stderr.contains("error[L1175]"), "{stderr}");
    assert!(stderr.contains("front (FRONT -Z)"), "{stderr}");
}

#[test]
fn a_pixel_outside_the_extent_is_l1175() {
    let dir = tree("mesh_pick_extent_error");
    let output = pick(&mesh_path(&dir), "front", "1920,0", &[]);
    assert!(!output.status.success());
    let stderr = stderr(&output);
    assert!(stderr.contains("error[L1175]"), "{stderr}");
    assert!(stderr.contains("outside the 1920x1080"), "{stderr}");
}

/// The text form carries the `at:` block a region is written with, so placing one is a copy and a
/// radius rather than three floats retyped.
#[test]
fn the_text_pick_prints_a_ready_region_block() {
    let dir = tree("mesh_pick_text");
    let output = pick(&mesh_path(&dir), "front", "960,540", &[]);
    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("FRONT -Z"), "{text}");
    assert!(text.contains("  at:\n"), "{text}");
    assert!(text.contains("base_color"), "{text}");
    assert!(text.contains("srgb8 10,200,40,255"), "{text}");
}

/// `apply` writes the tree the edit document owns, `--check` is green on it, and one flipped byte
/// turns the gate red with the `DRIFT` line the drift script reads the exit code of.
#[test]
fn apply_writes_a_tree_that_checks_green_then_drifts() {
    let dir = tree("mesh_apply");
    let edit = edit_path(&dir);
    let output = run_ludion(&["mesh", "apply", edit.to_str().unwrap()]);
    assert!(output.status.success(), "apply failed: {}", stderr(&output));

    let out_dir = dir.join("meshes/plate-lit");
    let document = out_dir.join("mesh.mesh.yaml");
    let derived = ludion_scene::check_mesh(&std::fs::read_to_string(&document).unwrap())
        .expect("the derived document validates");
    assert_eq!(
        derived.generated_from.as_deref(),
        Some("../plate-lit.mesh-edit.yaml")
    );
    let emissive = derived
        .textures
        .emissive
        .expect("the edit created one")
        .path;
    assert!(out_dir.join(&emissive).exists(), "{emissive}");

    let check = run_ludion(&["mesh", "apply", edit.to_str().unwrap(), "--check"]);
    assert!(
        check.status.success(),
        "a freshly written tree drifted: {}",
        stderr(&check)
    );
    assert!(stdout(&check).contains("up to date"), "{}", stdout(&check));
    // The text report spells paths exactly as `--json` does: a document-relative `out` joined onto a
    // platform path would otherwise print one backslash mid-path on Windows.
    for line in [stdout(&output), stdout(&check)] {
        assert!(
            !line.contains('\\'),
            "the report mixes path separators: {line}"
        );
    }

    let map = out_dir.join(&emissive);
    let mut bytes = std::fs::read(&map).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    std::fs::write(&map, &bytes).unwrap();

    let drifted = run_ludion(&["mesh", "apply", edit.to_str().unwrap(), "--check"]);
    assert!(!drifted.status.success(), "the flipped byte went unnoticed");
    let stderr = stderr(&drifted);
    assert!(stderr.contains("error[L1173]"), "{stderr}");
    assert!(stderr.contains("DRIFT "), "{stderr}");
    let drift = stderr.lines().find(|line| line.contains("DRIFT ")).unwrap();
    assert!(
        !drift.contains('\\'),
        "a diagnostic mixes path separators: {drift}"
    );
}

/// A runtime failure under `--json` still prints one object — `ok: false` and the diagnostics —
/// where the text mode prints nothing on stdout.
#[test]
fn json_failures_print_one_object() {
    let dir = tree("mesh_json_failures");
    let missing = dir.join("meshes/absent.mesh.yaml");
    let picked = pick(&missing, "front", "1,1", &["--json"]);
    // An edit whose source mesh is gone: the edit parses, the derive fails.
    std::fs::remove_file(mesh_path(&dir)).unwrap();
    let applied = run_ludion(&["mesh", "apply", edit_path(&dir).to_str().unwrap(), "--json"]);

    for (name, output) in [("pick", picked), ("apply", applied)] {
        assert!(!output.status.success(), "{name} must fail");
        let object: serde_json::Value = serde_json::from_str(&stdout(&output))
            .unwrap_or_else(|error| panic!("{name} stdout is one JSON object: {error}"));
        assert_eq!(object["ok"], false, "{name}: {object}");
        assert!(
            !object["diagnostics"].as_array().unwrap().is_empty(),
            "{name}: {object}"
        );
    }
}

/// The report `--json` prints is the instrument an edit is verified with, so its keys are a
/// consumer contract — plus the two the CLI adds over the crate's own report.
#[test]
fn apply_json_reports_every_op_and_what_it_wrote() {
    let dir = tree("mesh_apply_json");
    let edit = edit_path(&dir);
    let output = run_ludion(&["mesh", "apply", edit.to_str().unwrap(), "--json"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let report: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("apply --json emits one JSON object");

    assert_eq!(report["ok"], true);
    assert_eq!(report["source"], "plate.mesh.yaml");
    assert_eq!(report["out"], "plate-lit");
    assert!(report["next"].as_str().unwrap().contains("Mesh.emissive"));
    assert!(report["out_dir"].as_str().unwrap().ends_with("plate-lit"));
    assert_eq!(report["written"].as_array().unwrap().len(), 2);

    let op = &report["ops"][0];
    for key in [
        "index",
        "op",
        "map",
        "texels",
        "share",
        "bounds",
        "triangles",
    ] {
        assert!(!op[key].is_null(), "ops[0].{key} is missing: {op}");
    }
    assert!(op["texels"].as_u64().unwrap() > 0, "{op}");
    // The sphere sits on the front quad, so the surface it painted stays there.
    let min: Vec<f64> = serde_json::from_value(op["bounds"]["min"].clone()).unwrap();
    assert!(close(min[2], -0.25), "{min:?}");

    let map = &report["maps"][0];
    assert_eq!(map["map"], "emissive");
    assert_eq!(map["width"], 64);
    assert_eq!(map["created"], true);
    assert!(map["non_black_texels"].as_u64().unwrap() > 0, "{map}");
}

/// None of the top-level render knobs (`--threads`, `--quality`, `--render-size`) can do anything
/// under `mesh` — neither verb drives a sim or a render pass — so none of their help text should
/// leak into a `mesh` subcommand's `--help`. The help does say which pixel of which image a pick
/// reads, because getting that wrong is the one mistake the verb cannot detect.
#[test]
fn mesh_verb_help_says_what_it_reads_and_not_what_it_ignores() {
    for verb in ["pick", "apply"] {
        let output = run_ludion(&["mesh", verb, "--help"]);
        assert!(output.status.success(), "{verb}: {}", stderr(&output));
        let text = stdout(&output);
        for flag in ["--threads", "--quality", "--render-size"] {
            assert!(
                !text.contains(flag),
                "`mesh {verb} --help` still shows `{flag}`:\n{text}"
            );
        }
    }
    let pick = stdout(&run_ludion(&["mesh", "pick", "--help"]));
    assert!(pick.contains("CENTRE"), "the pixel convention: {pick}");
    assert!(pick.contains("SINGLE-CELL"), "the sheet it reads: {pick}");
}

/// A region that misses the surface is the blind author's likeliest mistake and the hardest to see,
/// so it is an error with the two boxes in the message, not a quiet no-op.
#[test]
fn an_op_that_paints_nothing_is_l1177() {
    let dir = tree("mesh_apply_nothing");
    let edit = edit_path(&dir);
    let text = std::fs::read_to_string(&edit).unwrap();
    std::fs::write(
        &edit,
        text.replace("          - -0.25\n", "          - -9.0\n"),
    )
    .unwrap();

    let output = run_ludion(&["mesh", "apply", edit.to_str().unwrap()]);
    assert!(!output.status.success(), "the sphere is nowhere near");
    let stderr = stderr(&output);
    assert!(stderr.contains("error[L1177]"), "{stderr}");
    assert!(stderr.contains("op 0 painted nothing"), "{stderr}");
    assert!(
        !dir.join("meshes/plate-lit").exists(),
        "a refused edit writes nothing"
    );
}

/// The plate's edit with its sphere moved to `at` (model space) and shrunk to a 0.1 radius.
fn plate_edit_at(at: &[f64]) -> String {
    let old =
        "        at:\n          - 0.0\n          - 0.0\n          - -0.25\n        radius: 0.2\n";
    assert!(PLATE_EDIT.contains(old), "the fixture's sphere moved");
    let new = format!(
        "        at:\n          - {}\n          - {}\n          - {}\n        radius: 0.1\n",
        at[0], at[1], at[2]
    );
    PLATE_EDIT.replace(old, &new)
}

fn apply_view(edit: &Path, extra: &[&str]) -> Output {
    let mut args = vec!["mesh", "apply", edit.to_str().unwrap()];
    args.extend_from_slice(extra);
    run_ludion(&args)
}

/// **The projected box is the pick's own camera.** Paint a sphere where a pick landed, project the
/// op's box into the same cell, and the picked pixel lies inside it — at the default extent and at
/// another, so the projection follows `--width`/`--height` exactly as `pick` does.
#[test]
fn apply_view_box_contains_the_picked_pixel() {
    let cases: [(&str, [&str; 4], [f64; 2]); 2] = [
        (
            "1000,560",
            ["--width", "1920", "--height", "1080"],
            [1000.0, 560.0],
        ),
        (
            "210,170",
            ["--width", "400", "--height", "400"],
            [210.0, 170.0],
        ),
    ];
    for (index, (pixel, extent, at)) in cases.into_iter().enumerate() {
        let dir = tree(&format!("mesh_apply_view_{index}"));
        let hit = {
            let output = pick(
                &mesh_path(&dir),
                "front",
                pixel,
                &["--json"].into_iter().chain(extent).collect::<Vec<_>>(),
            );
            assert!(output.status.success(), "pick failed: {}", stderr(&output));
            serde_json::from_str::<serde_json::Value>(&stdout(&output)).unwrap()
        };
        let position: Vec<f64> = serde_json::from_value(hit["position"].clone()).unwrap();
        let edit = edit_path(&dir);
        std::fs::write(&edit, plate_edit_at(&position)).unwrap();

        let mut args = vec!["--view", "front", "--json"];
        args.extend(extent);
        let output = apply_view(&edit, &args);
        assert!(output.status.success(), "apply failed: {}", stderr(&output));
        let report: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();

        let view = &report["view"];
        assert_eq!(view["name"], "front");
        assert_eq!(view["label"], "FRONT -Z");
        let [width, height] = [extent[1], extent[3]].map(|n| n.parse::<u64>().unwrap());
        assert_eq!(view["extent"], serde_json::json!([width, height]));

        let boxed = &report["ops"][0]["view_box"];
        assert_eq!(boxed["clipped"], false, "{boxed}");
        let span =
            |axis: &str| -> [u64; 2] { serde_json::from_value(boxed[axis].clone()).unwrap() };
        for (axis, picked, extent) in [("x", at[0], width), ("y", at[1], height)] {
            let [lo, hi] = span(axis);
            assert!(
                lo as f64 <= picked && picked <= hi as f64,
                "{axis}: pixel {picked} outside {lo}..{hi} ({boxed})"
            );
            assert!(
                hi - lo + 1 < extent,
                "{axis}: the box fills the cell ({boxed})"
            );
        }
    }
}

#[test]
fn apply_view_text_names_the_axis_label() {
    let dir = tree("mesh_apply_view_text");
    let output = apply_view(&edit_path(&dir), &["--view", "front"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("op 0 covers x "), "{text}");
    assert!(text.contains("of a 1920x1080 FRONT -Z cell"), "{text}");
    assert!(text.contains("FRONT -Z cell"), "{text}");

    // `--check` prints the same line once the tree is written and identical.
    let checked = apply_view(&edit_path(&dir), &["--check", "--view", "front"]);
    assert!(checked.status.success(), "{}", stderr(&checked));
    let checked = stdout(&checked);
    assert!(checked.contains("up to date"), "{checked}");
    assert!(checked.contains("op 0 covers x "), "{checked}");
}

#[test]
fn apply_without_view_is_unchanged() {
    let dir = tree("mesh_apply_no_view");
    let output = apply_view(&edit_path(&dir), &["--json"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let report: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert!(report.get("view").is_none(), "{report}");
    assert!(report["ops"][0].get("view_box").is_none(), "{report}");
}

#[test]
fn apply_view_unknown_is_l1175_and_writes_nothing() {
    let dir = tree("mesh_apply_view_unknown");
    let output = apply_view(&edit_path(&dir), &["--view", "sideways"]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("L1175"), "{}", stderr(&output));
    assert!(
        !dir.join("meshes/plate-lit").exists(),
        "an unknown view writes nothing"
    );

    let orphan = apply_view(&edit_path(&dir), &["--width", "400"]);
    assert!(!orphan.status.success(), "--width needs --view");
    assert!(
        stderr(&orphan).contains("--view"),
        "clap names the missing flag: {}",
        stderr(&orphan)
    );
}

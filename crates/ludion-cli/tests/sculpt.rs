//! Integration tests for `ludion sculpt new` and `check-scene`'s `kind: sculpt` path
//! (`Plan/mesh-modeling.md` Wave 1 step 5), driven against the built `ludion` binary — the sculpt
//! analog of `voxel_cmd.rs` and `check_scene_voxel.rs`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn ludion(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ludion"))
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

/// A fresh temp directory named after the test, removed and recreated so a rerun starts clean.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-sculpt-cmd-{name}"));
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

/// A small two-part sculpt: a box base and a blended sphere head, both real material — the shape
/// `check-scene`'s non-zero-triangle summary and the `--bounds` view are read against.
const IDOL: &str = "\
format: 1
kind: sculpt
name: idol
resolution: 16
triangles: 500
palette:
  S:
    color:
      - 0.8
      - 0.8
      - 0.8
      - 1.0
bones:
  - name: root
parts:
  - name: base
    shape: box
    size:
      - 1.0
      - 0.4
      - 1.0
    at:
      - 0
      - 0.2
      - 0
    material: S
  - name: head
    shape: sphere
    radius: 0.3
    at:
      - 0
      - 0.7
      - 0
    material: S
    blend: 0.05
";

/// A sphere (`radius: 0.1`) authored entirely inside a much larger one (`radius: 1.0`), both at the
/// origin: the union's surface is exactly the outer sphere's, so `inner` contributes no output
/// vertex at all — the `L1158` "hidden by part `outer`" case.
const NESTED: &str = "\
format: 1
kind: sculpt
name: nested
resolution: 16
triangles: 500
palette:
  S:
    color:
      - 0.8
      - 0.8
      - 0.8
      - 1.0
bones:
  - name: root
parts:
  - name: outer
    shape: sphere
    radius: 1.0
    material: S
  - name: inner
    shape: sphere
    radius: 0.1
    material: S
";

/// Two spheres 2 world units apart (radius 0.3 each, so a 1.4-unit gap) — too far to ever touch, the
/// `L1159` disconnected-surface case.
const PAIR: &str = "\
format: 1
kind: sculpt
name: pair
resolution: 16
triangles: 500
palette:
  S:
    color:
      - 0.8
      - 0.8
      - 0.8
      - 1.0
bones:
  - name: root
parts:
  - name: left
    shape: sphere
    radius: 0.3
    at:
      - -1.0
      - 0
      - 0
    material: S
  - name: right
    shape: sphere
    radius: 0.3
    at:
      - 1.0
      - 0
      - 0
    material: S
";

/// A hand-authored voxel model with a two-entry palette, for the `--palette-from` cross-kind test.
const VOXEL_PALETTE_SOURCE: &str = "\
format: 1
kind: voxel
name: swatch
size:
  - 1
  - 1
  - 1
palette:
  A:
    color:
      - 1.0
      - 0.0
      - 0.0
      - 1.0
  B:
    color:
      - 0.0
      - 0.0
      - 1.0
      - 1.0
slices:
  -
    - A
";

// --- sculpt new -----------------------------------------------------------------------------------

/// `sculpt new` writes a validated, canonical (apart from its header comment) scaffold that
/// `check-scene` accepts as an empty (0-part) model.
#[test]
fn new_scaffolds_a_checkable_model() {
    let dir = scratch("new");
    let out = dir.join("golem.sculpt.yaml");
    let output = ludion(&["sculpt", "new", out.to_str().unwrap()]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(stdout(&output).contains("0 parts"), "{}", stdout(&output));

    let text = read(&out);
    assert!(text.contains("# golem"), "{text}");
    assert!(text.contains("kind: sculpt"), "{text}");
    assert!(text.contains("name: golem"), "{text}");
    assert!(text.contains("bones:\n  - name: root\n"), "{text}");
    assert!(text.contains("parts: []\n"), "{text}");

    let check = ludion(&["check-scene", out.to_str().unwrap()]);
    assert!(check.status.success(), "stderr: {}", stderr(&check));
    assert!(
        stdout(&check).contains("OK (sculpt, 0 parts, 1 bone"),
        "{}",
        stdout(&check)
    );
}

/// The scaffold refuses to overwrite: `new` is for births, hand-editing is for growth.
#[test]
fn new_refuses_to_overwrite() {
    let dir = scratch("new-overwrite");
    let out = dir.join("idol.sculpt.yaml");
    write(&out, IDOL);
    let output = ludion(&["sculpt", "new", out.to_str().unwrap()]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("already exists"),
        "{}",
        stderr(&output)
    );
    // Untouched: still the hand-authored fixture, not a scaffold.
    assert_eq!(read(&out), IDOL);
}

/// `--palette-from` a voxel document copies its palette glyphs into the new sculpt scaffold —
/// `voxel new --palette-from` and `sculpt new --palette-from` share one colour-language source.
#[test]
fn new_palette_from_a_voxel_document_copies_its_glyphs() {
    let dir = scratch("new-palette-from-voxel");
    let source = dir.join("swatch.voxel.yaml");
    write(&source, VOXEL_PALETTE_SOURCE);
    let out = dir.join("golem.sculpt.yaml");
    let output = ludion(&[
        "sculpt",
        "new",
        out.to_str().unwrap(),
        "--palette-from",
        source.to_str().unwrap(),
    ]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(
        stdout(&output).contains("2 palette entries"),
        "{}",
        stdout(&output)
    );

    let text = read(&out);
    assert!(text.contains("A:\n"), "{text}");
    assert!(text.contains("B:\n"), "{text}");
}

// --- check-scene ------------------------------------------------------------------------------

/// A small two-part sculpt meshes to a non-zero triangle count and prints the sculpt summary line.
#[test]
fn a_small_sculpt_checks_clean_with_a_non_zero_triangle_count() {
    let dir = scratch("check-idol");
    let path = dir.join("idol.sculpt.yaml");
    write(&path, IDOL);
    let output = ludion(&["check-scene", path.to_str().unwrap()]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let text = stdout(&output);
    assert!(
        text.contains("OK (sculpt, 2 parts, 1 bone, grid "),
        "{text}"
    );
    assert!(text.contains("triangles after decimation from"), "{text}");
    assert!(text.contains("meshed in"), "{text}");
    // A degenerate zero-triangle mesh would defeat the point of this test.
    assert!(!text.contains(", 0 triangles after"), "{text}");
}

/// `--bounds` prints a line for each part and each bone.
#[test]
fn bounds_prints_part_and_bone_lines() {
    let dir = scratch("bounds");
    let path = dir.join("idol.sculpt.yaml");
    write(&path, IDOL);
    let output = ludion(&["check-scene", path.to_str().unwrap(), "--bounds"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("bounds:\n"), "{text}");
    assert!(text.contains("part base:"), "{text}");
    assert!(text.contains("part head:"), "{text}");
    assert!(text.contains("bone root:"), "{text}");
    assert!(
        text.contains("surface vertex") || text.contains("surface vertices"),
        "{text}"
    );
}

/// `--bounds` is refused against a voxel document — a voxel report has its own `--slice`/`--column`
/// views, not the sculpt/mesh/scene ones.
#[test]
fn bounds_on_a_voxel_document_is_refused() {
    let dir = scratch("bounds-wrong-kind");
    let path = dir.join("swatch.voxel.yaml");
    write(&path, VOXEL_PALETTE_SOURCE);
    let output = ludion(&["check-scene", path.to_str().unwrap(), "--bounds"]);
    assert!(!output.status.success());
    let text = stderr(&output);
    assert!(text.contains("--bounds"), "{text}");
    assert!(text.contains("sculpt documents"), "{text}");
    assert!(text.contains("this document is a voxel"), "{text}");
}

/// A part entirely hidden inside another (the union's surface never reaches it) warns `L1158`.
#[test]
fn a_hidden_part_warns_l1158() {
    let dir = scratch("hidden-part");
    let path = dir.join("nested.sculpt.yaml");
    write(&path, NESTED);
    let output = ludion(&["check-scene", path.to_str().unwrap()]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let text = stdout(&output);
    let err = stderr(&output);
    let combined = format!("{text}{err}");
    assert!(combined.contains("L1158"), "{combined}");
    assert!(combined.contains("inner"), "{combined}");
    assert!(combined.contains("hidden by part `outer`"), "{combined}");
}

/// Two spheres too far apart to touch warn `L1159` (the disconnected-surface report).
#[test]
fn two_disconnected_spheres_warn_l1159() {
    let dir = scratch("disconnected");
    let path = dir.join("pair.sculpt.yaml");
    write(&path, PAIR);
    let output = ludion(&["check-scene", path.to_str().unwrap()]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let text = stdout(&output);
    let err = stderr(&output);
    let combined = format!("{text}{err}");
    assert!(combined.contains("L1159"), "{combined}");
    assert!(combined.contains("2 disconnected components"), "{combined}");
}

// --- shape: voxel -----------------------------------------------------------------------------

/// A 4x4x4 solid voxel model — the source a `shape: voxel` part reads.
const BLOCK_VOXEL: &str = "\
format: 1
kind: voxel
size:
  - 4
  - 4
  - 4
palette:
  S:
    color:
      - 0.8
      - 0.8
      - 0.8
      - 1.0
slices:
  - - SSSS
    - SSSS
    - SSSS
    - SSSS
  - - SSSS
    - SSSS
    - SSSS
    - SSSS
  - - SSSS
    - SSSS
    - SSSS
    - SSSS
  - - SSSS
    - SSSS
    - SSSS
    - SSSS
";

/// A sculpt whose one part is that voxel model, blended with a sphere: the `path` is read relative
/// to the sculpt document's own directory, so a sibling `voxels/` folder is `../voxels/...`.
const VOXEL_PART: &str = "\
format: 1
kind: sculpt
name: tower
resolution: 16
triangles: 500
palette:
  S:
    color:
      - 0.8
      - 0.8
      - 0.8
      - 1.0
bones:
  - name: root
parts:
  - name: block
    shape: voxel
    path: \"../voxels/block.voxel.yaml\"
    size: 0.25
    material: S
  - name: cap
    shape: sphere
    radius: 0.3
    at:
      - 0
      - 1.0
      - 0
    blend: 0.1
    material: S
";

/// Writes the `sculpts/` + `voxels/` pair into `dir` and answers the sculpt's path.
fn voxel_part_pair(dir: &Path, sculpt: &str) -> PathBuf {
    std::fs::create_dir_all(dir.join("sculpts")).expect("sculpts dir");
    std::fs::create_dir_all(dir.join("voxels")).expect("voxels dir");
    write(&dir.join("voxels/block.voxel.yaml"), BLOCK_VOXEL);
    let path = dir.join("sculpts/tower.sculpt.yaml");
    write(&path, sculpt);
    path
}

/// A `shape: voxel` part meshes against the document its `path` names, and `--bounds` reports it
/// like any other part.
#[test]
fn a_voxel_shaped_part_checks_clean_against_its_source() {
    let dir = scratch("voxel-part");
    let path = voxel_part_pair(&dir, VOXEL_PART);
    let output = ludion(&["check-scene", path.to_str().unwrap(), "--bounds"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let text = stdout(&output);
    assert!(
        text.contains("OK (sculpt, 2 parts, 1 bone, grid "),
        "{text}"
    );
    assert!(!text.contains(", 0 triangles after"), "{text}");
    assert!(text.contains("part block:"), "{text}");
}

/// A `path` naming nothing on disk is the ordinary missing-asset `L1101`, pointed at the part that
/// names it rather than at the file that is absent.
#[test]
fn a_missing_voxel_source_is_l1101_at_the_part_path() {
    let dir = scratch("voxel-part-missing");
    let path = voxel_part_pair(
        &dir,
        &VOXEL_PART.replace("block.voxel.yaml", "absent.voxel.yaml"),
    );
    let output = ludion(&["check-scene", path.to_str().unwrap()]);
    assert!(!output.status.success(), "stdout: {}", stdout(&output));
    let text = stderr(&output);
    assert!(text.contains("L1101"), "{text}");
    assert!(text.contains("--> /parts/0/path"), "{text}");
    assert!(text.contains("absent.voxel.yaml"), "{text}");
}

/// A source that is on disk but does not check reports the voxel document's own code, at the same
/// part pointer — the sculpt is the document its author is editing.
#[test]
fn an_unparsable_voxel_source_is_reported_at_the_part_path() {
    let dir = scratch("voxel-part-broken");
    let path = voxel_part_pair(&dir, VOXEL_PART);
    write(
        &dir.join("voxels/block.voxel.yaml"),
        &BLOCK_VOXEL.replace("kind: voxel", "kind: voxle"),
    );
    let output = ludion(&["check-scene", path.to_str().unwrap()]);
    assert!(!output.status.success(), "stdout: {}", stdout(&output));
    let text = stderr(&output);
    assert!(text.contains("L1113"), "{text}");
    assert!(text.contains("--> /parts/0/path"), "{text}");
}

// --- shape: mesh ------------------------------------------------------------------------------

/// A tetrahedron wound outward — the smallest closed soup a `shape: mesh` part can read.
const TETRA_MESH: &str = "\
format: 1
kind: mesh
name: tetra
positions:
  - 0.0
  - 0.6
  - 0.0
  - -0.4
  - 0.0
  - -0.25
  - 0.4
  - 0.0
  - -0.25
  - 0.0
  - 0.0
  - 0.45
indices:
  - 0
  - 2
  - 1
  - 0
  - 1
  - 3
  - 0
  - 3
  - 2
  - 1
  - 2
  - 3
";

/// A sculpt whose one part is that mesh, blended with a sphere: the `path` is read relative to the
/// sculpt document's own directory, so a sibling `meshes/` folder is `../meshes/...`.
const MESH_PART: &str = "\
format: 1
kind: sculpt
name: cairn
resolution: 32
triangles: 500
palette:
  S:
    color:
      - 0.8
      - 0.8
      - 0.8
      - 1.0
bones:
  - name: root
parts:
  - name: stone
    shape: mesh
    path: \"../meshes/tetra.mesh.yaml\"
    size: 1.5
    material: S
  - name: cap
    shape: sphere
    radius: 0.3
    at:
      - 0
      - 0.9
      - 0
    blend: 0.1
    material: S
";

/// Writes the `sculpts/` + `meshes/` pair into `dir` and answers the sculpt's path.
fn mesh_part_pair(dir: &Path, sculpt: &str) -> PathBuf {
    std::fs::create_dir_all(dir.join("sculpts")).expect("sculpts dir");
    std::fs::create_dir_all(dir.join("meshes")).expect("meshes dir");
    write(&dir.join("meshes/tetra.mesh.yaml"), TETRA_MESH);
    let path = dir.join("sculpts/cairn.sculpt.yaml");
    write(&path, sculpt);
    path
}

/// A `shape: mesh` part meshes against the document its `path` names, and `--bounds` reports it
/// like any other part.
#[test]
fn a_mesh_shaped_part_checks_clean_against_its_source() {
    let dir = scratch("mesh-part");
    let path = mesh_part_pair(&dir, MESH_PART);
    let output = ludion(&["check-scene", path.to_str().unwrap(), "--bounds"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let text = stdout(&output);
    assert!(
        text.contains("OK (sculpt, 2 parts, 1 bone, grid "),
        "{text}"
    );
    assert!(!text.contains(", 0 triangles after"), "{text}");
    assert!(text.contains("part stone:"), "{text}");
}

/// A `path` naming nothing on disk is the ordinary missing-asset `L1101`, pointed at the part that
/// names it rather than at the file that is absent.
#[test]
fn a_missing_mesh_source_is_l1101_at_the_part_path() {
    let dir = scratch("mesh-part-missing");
    let path = mesh_part_pair(
        &dir,
        &MESH_PART.replace("tetra.mesh.yaml", "absent.mesh.yaml"),
    );
    let output = ludion(&["check-scene", path.to_str().unwrap()]);
    assert!(!output.status.success(), "stdout: {}", stdout(&output));
    let text = stderr(&output);
    assert!(text.contains("L1101"), "{text}");
    assert!(text.contains("--> /parts/0/path"), "{text}");
    assert!(text.contains("absent.mesh.yaml"), "{text}");
}

/// A source that is on disk but does not check reports the mesh document's own code, at the same
/// part pointer — the sculpt is the document its author is editing.
#[test]
fn an_unparsable_mesh_source_is_reported_at_the_part_path() {
    let dir = scratch("mesh-part-broken");
    let path = mesh_part_pair(&dir, MESH_PART);
    write(
        &dir.join("meshes/tetra.mesh.yaml"),
        &TETRA_MESH.replace("kind: mesh", "kind: mehs"),
    );
    let output = ludion(&["check-scene", path.to_str().unwrap()]);
    assert!(!output.status.success(), "stdout: {}", stdout(&output));
    let text = stderr(&output);
    assert!(text.contains("L1110"), "{text}");
    assert!(text.contains("--> /parts/0/path"), "{text}");
}

//! The document-level `rim:` block on voxel, sculpt and mesh documents: canonical round trip,
//! byte-identity without it, and range errors under `/rim`.

use ludion_scene::{
    DocRim, emit_mesh, emit_sculpt, emit_voxel, parse_mesh, parse_sculpt, parse_voxel,
    validate_mesh, validate_sculpt, validate_voxel,
};

const RIM: &str =
    "rim:\n  color:\n    - 1.0\n    - 0.5\n    - 0.25\n  strength: 2.0\n  power: 4.5\n";

fn expected() -> DocRim {
    DocRim {
        color: [1.0, 0.5, 0.25],
        strength: 2.0,
        power: 4.5,
    }
}

const VOXEL: &str = "\
format: 1
kind: voxel
size:
  - 1
  - 1
  - 1
@RIM@palette:
  A:
    color:
      - 1.0
      - 0.0
      - 0.0
      - 1.0
slices:
  -
    - \"A\"
";

const SCULPT: &str = "\
format: 1
kind: sculpt
resolution: 32
triangles: 4000
@RIM@palette:
  S:
    color:
      - 0.35
      - 0.33
      - 0.3
      - 1.0
bones:
  - name: root
parts:
  - name: plinth
    shape: box
    size:
      - 1.2
      - 0.3
      - 1.2
    material: S
";

const MESH: &str = "\
format: 1
kind: mesh
positions:
  - 0.0
  - 0.0
  - 0.0
  - 1.0
  - 0.0
  - 0.0
  - 0.0
  - 1.0
  - 0.0
@RIM@indices:
  - 0
  - 1
  - 2
";

fn with(template: &str, rim: &str) -> String {
    template.replace("@RIM@", rim)
}

#[test]
fn voxel_rim_round_trips_before_palette() {
    let source = with(VOXEL, RIM);
    let doc = parse_voxel(&source).expect("parses");
    assert_eq!(doc.rim, Some(expected()));
    assert!(validate_voxel(&doc).is_empty());
    assert_eq!(emit_voxel(&doc), source);
}

#[test]
fn voxel_without_rim_is_byte_identical() {
    let source = with(VOXEL, "");
    let doc = parse_voxel(&source).expect("parses");
    assert_eq!(doc.rim, None);
    assert_eq!(emit_voxel(&doc), source);
}

#[test]
fn voxel_out_of_range_strength_reports_pointer() {
    let doc = parse_voxel(&with(VOXEL, "rim:\n  strength: -1.0\n")).expect("parses");
    let diags = validate_voxel(&doc);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code.as_str(), "L1114");
    assert_eq!(diags[0].path.as_deref(), Some("/rim/strength"));
}

#[test]
fn sculpt_rim_round_trips_before_palette() {
    let source = with(SCULPT, RIM);
    let doc = parse_sculpt(&source).expect("parses");
    assert_eq!(doc.rim, Some(expected()));
    assert!(validate_sculpt(&doc).is_empty());
    assert_eq!(emit_sculpt(&doc), source);
}

#[test]
fn sculpt_without_rim_is_byte_identical() {
    let source = with(SCULPT, "");
    let doc = parse_sculpt(&source).expect("parses");
    assert_eq!(doc.rim, None);
    assert_eq!(emit_sculpt(&doc), source);
}

#[test]
fn sculpt_out_of_range_power_reports_pointer() {
    let doc = parse_sculpt(&with(SCULPT, "rim:\n  power: 0.0\n")).expect("parses");
    let diags = validate_sculpt(&doc);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code.as_str(), "L1157");
    assert_eq!(diags[0].path.as_deref(), Some("/rim/power"));
}

#[test]
fn mesh_rim_round_trips_after_double_sided() {
    let source = with(MESH, &format!("double_sided: true\n{RIM}"));
    let doc = parse_mesh(&source).expect("parses");
    assert_eq!(doc.rim, Some(expected()));
    assert!(validate_mesh(&doc).is_empty());
    assert_eq!(emit_mesh(&doc), source);
}

#[test]
fn mesh_without_rim_is_byte_identical() {
    let source = with(MESH, "");
    let doc = parse_mesh(&source).expect("parses");
    assert_eq!(doc.rim, None);
    assert_eq!(emit_mesh(&doc), source);
}

#[test]
fn mesh_out_of_range_colour_reports_pointer() {
    let doc = parse_mesh(&with(
        MESH,
        "rim:\n  color:\n    - 1.0\n    - -1.0\n    - 1.0\n",
    ))
    .expect("parses");
    let diags = validate_mesh(&doc);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code.as_str(), "L1111");
    assert_eq!(diags[0].path.as_deref(), Some("/rim/color/1"));
}

#[test]
fn unknown_rim_key_is_a_structure_error() {
    let diags = parse_voxel(&with(VOXEL, "rim:\n  glow: 1.0\n")).unwrap_err();
    assert_eq!(diags[0].path.as_deref(), Some("/rim/glow"));
}

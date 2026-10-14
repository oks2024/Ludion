//! Round-trip and canonical-form properties for part documents (`parts/*.part.yaml`,
//! `Plan/voxel-animation.md` decisions 3-4) — the part analog of `voxel_roundtrip.rs`, trimmed to
//! the fields a part has (no palette):
//!
//! - **value round trip**: doc → emit → check loses nothing;
//! - **emit idempotence**: `emit_part(parse_part(emit_part(d)))` is *byte-equal* to
//!   `emit_part(d)`;
//! - the emitted text is LF-only and validates cleanly;
//! - a hand-authored fixture pins the exact canonical text;
//! - every `L1139`/`L1140` misshape is reported with a JSON Pointer.

use ludion_scene::{PartDocument, check_part, emit_part, parse_part, validate_part};
use proptest::prelude::*;

// --- Strategies -------------------------------------------------------------------------------

/// The glyph pool, printable-ASCII, deliberately including characters `scalar_string` must quote
/// (`#`) alongside plain ones — a part has no palette to constrain the choice.
const GLYPH_POOL: [char; 5] = ['#', '1', 'A', 'B', 'C'];

/// A structurally consistent [`PartDocument`]: `size` dimensions in `1..=3` and slices whose
/// layers/rows/columns match `size`, drawn only from the glyph pool plus the empty `.` — exactly
/// the properties [`validate_part`] enforces.
fn part_doc() -> impl Strategy<Value = PartDocument> {
    (
        1u32..=3,
        1u32..=3,
        1u32..=3,
        proptest::option::of("[a-zA-Z0-9 ]{0,6}"),
    )
        .prop_flat_map(|(sx, sy, sz, name)| {
            let mut choices = GLYPH_POOL.to_vec();
            choices.push('.');
            let cells = proptest::collection::vec(
                proptest::sample::select(choices),
                (sx * sy * sz) as usize,
            );
            (Just([sx, sy, sz]), Just(name), cells)
        })
        .prop_map(|(size, name, cells)| {
            let [sx, sy, sz] = size;
            let mut cells = cells.into_iter();
            let mut slices = Vec::with_capacity(sy as usize);
            for _ in 0..sy {
                let mut layer = Vec::with_capacity(sz as usize);
                for _ in 0..sz {
                    let row: String = (0..sx).map(|_| cells.next().unwrap()).collect();
                    layer.push(row);
                }
                slices.push(layer);
            }
            PartDocument { name, size, slices }
        })
}

// --- Properties -------------------------------------------------------------------------------

proptest! {
    #[test]
    fn emit_then_check_loses_nothing(doc in part_doc()) {
        let text = emit_part(&doc);
        let checked = check_part(&text).expect("canonical output must check clean");
        prop_assert_eq!(checked, doc);
    }

    #[test]
    fn emit_is_idempotent_byte_for_byte(doc in part_doc()) {
        let first = emit_part(&doc);
        let reparsed = parse_part(&first).expect("canonical output must parse cleanly");
        let second = emit_part(&reparsed);
        prop_assert_eq!(&second, &first);
        let third = emit_part(&parse_part(&second).unwrap());
        prop_assert_eq!(&third, &first);
    }

    #[test]
    fn emitted_text_is_lf_only_and_validates(doc in part_doc()) {
        let text = emit_part(&doc);
        prop_assert!(!text.contains('\r'), "canonical output must use LF only");
        prop_assert!(text.ends_with('\n'), "canonical output ends with a newline");
        let parsed = parse_part(&text).expect("canonical output must parse cleanly");
        let diagnostics = validate_part(&parsed);
        prop_assert!(
            diagnostics.is_empty(),
            "canonical output of a consistent part must validate: {:?}",
            diagnostics.iter().map(ToString::to_string).collect::<Vec<_>>()
        );
    }
}

// --- Deterministic golden shape ---------------------------------------------------------------

/// One non-property test pinning the exact canonical text: field order is
/// `format, kind, name, size, slices`; rows follow the shared `scalar_string` quoting rule (`AB`
/// stays plain, `.A` is quoted because it opens with a non-identifier character).
#[test]
fn canonical_form_golden() {
    let source = "\
format: 1
kind: part
name: upper-arm
size:
  - 2
  - 2
  - 1
slices:
  -
    - AB
  -
    - \".A\"
";
    let part = parse_part(source).expect("part should parse");
    assert!(validate_part(&part).is_empty());
    assert_eq!(part.size, [2, 2, 1]);
    let canonical = emit_part(&part);
    assert_eq!(canonical, source);
    // Emission is a fixed point.
    assert_eq!(emit_part(&parse_part(&canonical).unwrap()), canonical);
}

/// A part with no `name` and empty slices emits the minimal flow-collection shape (no palette to
/// omit — a part never has one).
#[test]
fn minimal_part_uses_flow_empties() {
    let doc = PartDocument {
        name: None,
        size: [1, 1, 1],
        slices: Vec::new(),
    };
    assert_eq!(
        emit_part(&doc),
        "format: 1\nkind: part\nsize:\n  - 1\n  - 1\n  - 1\nslices: []\n"
    );
}

/// `for_each_cell` visits every cell in `(y, z, x)` order, yielding the raw glyph (not a palette
/// index — a part has no palette) and `None` for an empty cell.
#[test]
fn for_each_cell_visits_in_yzx_order_with_raw_glyphs() {
    let source = "format: 1\nkind: part\nsize:\n  - 2\n  - 1\n  - 1\nslices:\n  -\n    - \"A.\"\n";
    let part = check_part(source).expect("the part is valid");
    let mut cells = Vec::new();
    part.for_each_cell(|x, y, z, glyph| cells.push(([x, y, z], glyph)));
    assert_eq!(cells, [([0, 0, 0], Some('A')), ([1, 0, 0], None)]);
}

// --- L1139 structure ----------------------------------------------------------------------------

fn errors_for(source: &str) -> Vec<ludion_core::diagnostics::Diagnostic> {
    check_part(source).expect_err("expected the part to be rejected")
}

fn has_code_at(
    diagnostics: &[ludion_core::diagnostics::Diagnostic],
    code: &str,
    pointer: &str,
) -> bool {
    diagnostics
        .iter()
        .any(|d| d.code.as_str() == code && d.path.as_deref() == Some(pointer))
}

#[test]
fn empty_document_is_l1139() {
    let diagnostics = errors_for("");
    assert!(diagnostics.iter().any(|d| d.code.as_str() == "L1139"));
}

#[test]
fn wrong_kind_is_l1139() {
    let source = "format: 1\nkind: voxel\nsize:\n  - 1\n  - 1\n  - 1\nslices:\n  -\n    - \".\"\n";
    assert!(has_code_at(&errors_for(source), "L1139", "/kind"));
}

#[test]
fn unknown_key_is_l1139() {
    let source = "\
format: 1
kind: part
size:
  - 1
  - 1
  - 1
palette:
  A:
    color:
      - 1.0
slices:
  -
    - \".\"
";
    let diagnostics = errors_for(source);
    let d = diagnostics
        .iter()
        .find(|d| d.path.as_deref() == Some("/palette"))
        .expect("a /palette diagnostic — parts have no palette of their own");
    assert_eq!(d.code.as_str(), "L1139");
}

#[test]
fn missing_size_is_l1139() {
    let source = "format: 1\nkind: part\nslices:\n  -\n    - \".\"\n";
    let diagnostics = errors_for(source);
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code.as_str() == "L1139" && d.path.as_deref() == Some(""))
    );
}

#[test]
fn missing_slices_is_l1139() {
    let source = "format: 1\nkind: part\nsize:\n  - 1\n  - 1\n  - 1\n";
    let diagnostics = errors_for(source);
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code.as_str() == "L1139" && d.path.as_deref() == Some(""))
    );
}

#[test]
fn wrong_size_shape_is_l1139() {
    let source = "format: 1\nkind: part\nsize:\n  - 1\n  - 1\nslices:\n  -\n    - \".\"\n";
    assert!(has_code_at(&errors_for(source), "L1139", "/size"));
}

#[test]
fn non_string_slice_row_is_l1139() {
    let source = "format: 1\nkind: part\nsize:\n  - 1\n  - 1\n  - 1\nslices:\n  -\n    - 5\n";
    assert!(has_code_at(&errors_for(source), "L1139", "/slices/0/0"));
}

// --- L1140 data consistency (each with a JSON Pointer assertion) -------------------------------

#[test]
fn wrong_row_width_is_l1140() {
    // size.x = 3 but the row is two characters wide.
    let source = "format: 1\nkind: part\nsize:\n  - 3\n  - 1\n  - 1\nslices:\n  -\n    - AA\n";
    assert!(has_code_at(&errors_for(source), "L1140", "/slices/0/0"));
}

#[test]
fn wrong_layer_count_is_l1140() {
    // size.y = 2 but only one layer is supplied.
    let source = "format: 1\nkind: part\nsize:\n  - 1\n  - 2\n  - 1\nslices:\n  -\n    - A\n";
    assert!(has_code_at(&errors_for(source), "L1140", "/slices"));
}

#[test]
fn wrong_row_count_is_l1140() {
    // size.z = 2 but the layer has a single row.
    let source = "format: 1\nkind: part\nsize:\n  - 1\n  - 1\n  - 2\nslices:\n  -\n    - A\n";
    assert!(has_code_at(&errors_for(source), "L1140", "/slices/0"));
}

#[test]
fn illegal_glyph_is_l1140() {
    // A part has no palette, so a printable-ASCII glyph is always structurally legal; only a
    // non-printable/non-ASCII character (here, a control character) is illegal.
    let doc = PartDocument {
        name: None,
        size: [1, 1, 1],
        slices: vec![vec!["\u{7}".to_string()]],
    };
    assert!(has_code_at(&validate_part(&doc), "L1140", "/slices/0/0"));
}

#[test]
fn any_printable_ascii_glyph_is_legal_with_no_palette() {
    // Every printable-ASCII glyph is legal in a part — there is no palette to reject it against.
    let source = "format: 1\nkind: part\nsize:\n  - 1\n  - 1\n  - 1\nslices:\n  -\n    - \"~\"\n";
    let part = check_part(source).expect("any printable-ASCII glyph is legal in a part");
    let mut cells = Vec::new();
    part.for_each_cell(|x, y, z, glyph| cells.push(([x, y, z], glyph)));
    assert_eq!(cells, [([0, 0, 0], Some('~'))]);
}

//! Canonical-form and diagnostic properties for path documents (`kind: path`,
//! `Plan/character-controller.md` decision 9): canonical byte-idempotence, one spelling per meaning,
//! and every `L1213`/`L1214` rule keyed on its JSON Pointer.

use ludion_core::diagnostics::Diagnostic;
use ludion_scene::{PathInterpolation, check_path, emit_path, parse_path, validate_path};

const CANONICAL: &str = "\
format: 1
kind: path
name: lift
interpolation: catmull_rom
closed: false
points:
  - - 0.0
    - 0.0
    - 0.0
  - - 4.0
    - 0.1
    - -6.0
  - - 8.0
    - 0.5
    - -6.0
";

fn has(diagnostics: &[Diagnostic], code: &str, pointer: &str) -> bool {
    diagnostics
        .iter()
        .any(|d| d.code.as_str() == code && d.path.as_deref() == Some(pointer))
}

fn structure_errors(source: &str) -> Vec<Diagnostic> {
    parse_path(source).expect_err("the source is structurally invalid")
}

fn data_errors(source: &str) -> Vec<Diagnostic> {
    let doc = parse_path(source).expect("the source is well-shaped");
    validate_path(&doc)
}

/// A path document with the given `closed` value and one line per coordinate triple.
fn with_points(closed: bool, points: &[[&str; 3]]) -> String {
    let mut out = format!("format: 1\nkind: path\nclosed: {closed}\npoints:\n");
    for point in points {
        out.push_str(&format!(
            "  - - {}\n    - {}\n    - {}\n",
            point[0], point[1], point[2]
        ));
    }
    out
}

#[test]
fn canonical_text_is_a_fixed_point() {
    let doc = check_path(CANONICAL).expect("valid");
    assert_eq!(emit_path(&doc), CANONICAL);
    let again = check_path(&emit_path(&doc)).expect("re-parses");
    assert_eq!(again, doc);
}

#[test]
fn floats_keep_their_shortest_round_trip_spelling() {
    let doc = check_path(CANONICAL).expect("valid");
    assert_eq!(doc.points[1], [4.0, 0.1, -6.0]);
    let text = emit_path(&doc);
    assert!(text.contains("    - 0.1\n") && text.contains("    - -6.0\n"));
}

#[test]
fn defaults_fill_and_emit_explicitly() {
    let source = with_points(false, &[["0.0", "0.0", "0.0"], ["1.0", "0.0", "0.0"]]);
    let source = source.replace("closed: false\n", "");
    let doc = check_path(&source).expect("valid");
    assert_eq!(doc.interpolation, PathInterpolation::CatmullRom);
    assert!(!doc.closed);
    let text = emit_path(&doc);
    assert!(text.contains("interpolation: catmull_rom\nclosed: false\npoints:\n"));
    assert_eq!(emit_path(&check_path(&text).expect("re-parses")), text);
}

#[test]
fn name_is_omitted_when_absent() {
    let source = with_points(true, &[["0", "0", "0"], ["1", "0", "0"], ["1", "0", "1"]]);
    let doc = check_path(&source).expect("valid");
    assert_eq!(doc.name, None);
    let text = emit_path(&doc);
    assert!(!text.contains("name:"));
    assert!(text.contains("closed: true\n"));
}

#[test]
fn linear_round_trips() {
    let source = CANONICAL.replace("catmull_rom", "linear");
    let doc = check_path(&source).expect("valid");
    assert_eq!(doc.interpolation, PathInterpolation::Linear);
    assert_eq!(emit_path(&doc), source);
}

#[test]
fn empty_and_non_mapping_roots_are_l1213() {
    assert!(has(&structure_errors(""), "L1213", ""));
    assert!(has(&structure_errors("- a\n- b\n"), "L1213", ""));
}

#[test]
fn envelope_errors_are_l1213() {
    let no_kind = CANONICAL.replace("kind: path\n", "");
    assert!(
        structure_errors(&no_kind)
            .iter()
            .any(|d| d.code.as_str() == "L1213")
    );
    let wrong_kind = CANONICAL.replace("kind: path", "kind: character");
    assert!(
        structure_errors(&wrong_kind)
            .iter()
            .any(|d| d.code.as_str() == "L1213")
    );
    let version = CANONICAL.replace("format: 1", "format: 2");
    assert!(
        structure_errors(&version)
            .iter()
            .any(|d| d.code.as_str() == "L1213")
    );
}

#[test]
fn a_missing_points_key_is_l1213() {
    let source = "format: 1\nkind: path\n";
    assert!(
        structure_errors(source)
            .iter()
            .any(|d| d.code.as_str() == "L1213" && d.message.contains("points"))
    );
}

#[test]
fn wrong_types_are_l1213_at_their_pointer() {
    let name = CANONICAL.replace("name: lift", "name:\n  - a");
    assert!(has(&structure_errors(&name), "L1213", "/name"));
    let closed = CANONICAL.replace("closed: false", "closed: 3");
    assert!(has(&structure_errors(&closed), "L1213", "/closed"));
    let points = "format: 1\nkind: path\npoints: 4\n";
    assert!(has(&structure_errors(points), "L1213", "/points"));
}

#[test]
fn an_unknown_key_is_l1213_with_a_suggestion() {
    let source = CANONICAL.replace("closed:", "closd:");
    let diagnostics = structure_errors(&source);
    let unknown = diagnostics
        .iter()
        .find(|d| d.code.as_str() == "L1213" && d.message.contains("closd"))
        .expect("the unknown key is reported");
    assert!(
        unknown
            .help
            .as_deref()
            .is_some_and(|h| h.contains("closed"))
    );
}

#[test]
fn a_point_that_is_not_three_numbers_is_l1213() {
    let short = "format: 1\nkind: path\npoints:\n  - - 0.0\n    - 0.0\n";
    assert!(has(&structure_errors(short), "L1213", "/points/0"));
    let flat = "format: 1\nkind: path\npoints:\n  - 1.0\n  - 2.0\n";
    assert!(has(&structure_errors(flat), "L1213", "/points/0"));
    let word = "format: 1\nkind: path\npoints:\n  - - 0.0\n    - up\n    - 0.0\n";
    assert!(has(&structure_errors(word), "L1213", "/points/0/1"));
}

#[test]
fn an_unknown_interpolation_is_l1213_with_a_suggestion() {
    let source = CANONICAL.replace("catmull_rom", "catmull-rom");
    let diagnostics = structure_errors(&source);
    let bad = diagnostics
        .iter()
        .find(|d| d.code.as_str() == "L1213" && d.path.as_deref() == Some("/interpolation"))
        .expect("reported at /interpolation");
    assert!(
        bad.help
            .as_deref()
            .is_some_and(|h| h.contains("catmull_rom"))
    );
}

#[test]
fn too_few_points_is_l1214_at_points() {
    let one = with_points(false, &[["0", "0", "0"]]);
    assert!(has(&data_errors(&one), "L1214", "/points"));
    let two_closed = with_points(true, &[["0", "0", "0"], ["1", "0", "0"]]);
    assert!(has(&data_errors(&two_closed), "L1214", "/points"));
    let two_open = with_points(false, &[["0", "0", "0"], ["1", "0", "0"]]);
    assert!(data_errors(&two_open).is_empty());
}

#[test]
fn a_non_finite_or_out_of_range_component_is_l1214() {
    // The YAML layer refuses `.inf`/`.nan` text (`L1009`), so a non-finite value reaches the
    // validator only through a document built in code.
    let mut doc = check_path(CANONICAL).expect("valid");
    doc.points[1][1] = f64::NAN;
    assert!(has(&validate_path(&doc), "L1214", "/points/1"));
    doc.points[1][1] = f64::INFINITY;
    assert!(has(&validate_path(&doc), "L1214", "/points/1"));
    let huge = with_points(
        false,
        &[["0", "0", "0"], ["1", "0", "1e300"], ["2", "0", "0"]],
    );
    assert!(has(&data_errors(&huge), "L1214", "/points/1"));
}

#[test]
fn coincident_neighbours_are_l1214() {
    let open = with_points(
        false,
        &[
            ["0", "0", "0"],
            ["1", "0", "0"],
            ["1", "0", "0"],
            ["2", "0", "0"],
        ],
    );
    assert!(has(&data_errors(&open), "L1214", "/points/2"));
    // The first and last coincide: fine open, a zero-length closing segment when closed.
    let ring = [
        ["0", "0", "0"],
        ["1", "0", "0"],
        ["1", "0", "1"],
        ["0", "0", "0"],
    ];
    assert!(data_errors(&with_points(false, &ring)).is_empty());
    assert!(has(
        &data_errors(&with_points(true, &ring)),
        "L1214",
        "/points/0"
    ));
}

#[test]
fn generated_from_round_trips_after_name() {
    let source = CANONICAL.replace(
        "name: lift\n",
        "name: lift\ngenerated_from: generation/blender/level/build.py\n",
    );
    let doc = check_path(&source).expect("valid");
    assert_eq!(
        doc.generated_from.as_deref(),
        Some("generation/blender/level/build.py")
    );
    assert_eq!(emit_path(&doc), source);
    let reordered = source.replace(
        "name: lift\ngenerated_from: generation/blender/level/build.py\n",
        "generated_from: generation/blender/level/build.py\nname: lift\n",
    );
    assert_eq!(emit_path(&check_path(&reordered).expect("valid")), source);
    assert!(!emit_path(&check_path(CANONICAL).expect("valid")).contains("generated_from"));
}

#[test]
fn a_wrong_typed_generated_from_is_l1213() {
    let source = CANONICAL.replace("name: lift\n", "name: lift\ngenerated_from:\n  - a\n");
    assert!(has(&structure_errors(&source), "L1213", "/generated_from"));
}

#[test]
fn a_blank_absolute_or_climbing_generated_from_is_l1214() {
    for bad in [
        "\"  \"",
        "/abs/build.py",
        "\\share\\build.py",
        "C:/build.py",
        "a/../../b.py",
        "..",
    ] {
        let source = CANONICAL.replace(
            "name: lift\n",
            &format!("name: lift\ngenerated_from: {bad}\n"),
        );
        assert!(
            has(&data_errors(&source), "L1214", "/generated_from"),
            "{bad} should be refused"
        );
    }
}

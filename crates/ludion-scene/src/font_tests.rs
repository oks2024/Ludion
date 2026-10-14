//! Unit tests for the font document format (`font.rs`): parse structure, validate semantics, and
//! the canonical emitter's fixed point.

use super::*;

const GOOD: &str = "\
format: 1
kind: font
generated_from: body.font-source.yaml
image: body.png
mode: sdf
units_per_em: 2048
ascender: 1984
descender: -494
line_gap: 0
atlas:
  width: 64
  height: 32
  em_px: 56
  spread_px: 6
notdef:
  advance: 1000
glyphs:
  - char: \" \"
    advance: 500
  - char: \"-\"
    advance: 700
    bounds:
      - 10
      - 20
      - 600
      - 80
    rect:
      - 0
      - 0
      - 10
      - 8
  - char: \"1\"
    advance: 900
    bounds:
      - 0
      - 0
      - 800
      - 1400
    rect:
      - 10
      - 0
      - 12
      - 20
  - char: \":\"
    advance: 400
    bounds:
      - 0
      - 0
      - 300
      - 1000
    rect:
      - 22
      - 0
      - 6
      - 16
  - char: A
    advance: 1360
    bounds:
      - 0
      - 0
      - 1300
      - 1400
    rect:
      - 28
      - 0
      - 20
      - 20
  - char: V
    advance: 1300
    bounds:
      - 0
      - 0
      - 1300
      - 1400
    rect:
      - 48
      - 0
      - 16
      - 20
aliases:
  a: A
kerning:
  A:
    V: -76
missing:
  - \"~\"
";

fn check(source: &str) -> Result<FontDocument, Vec<Diagnostic>> {
    let doc = parse(source)?;
    let diagnostics = validate(&doc);
    if diagnostics.is_empty() {
        Ok(doc)
    } else {
        Err(diagnostics)
    }
}

fn errors(source: &str) -> Vec<Diagnostic> {
    match check(source) {
        Ok(_) => panic!("expected the font to be rejected"),
        Err(diagnostics) => diagnostics,
    }
}

fn rejects_at(source: &str, pointer: &str) {
    let diagnostics = errors(source);
    let hit = diagnostics
        .iter()
        .find(|d| d.path.as_deref() == Some(pointer))
        .unwrap_or_else(|| panic!("no diagnostic at {pointer}: {diagnostics:?}"));
    assert_eq!(hit.code.as_str(), "L1250");
}

#[test]
fn the_good_font_round_trips_byte_identically() {
    let doc = check(GOOD).expect("the font is valid");
    assert_eq!(doc.glyphs.len(), 6);
    assert_eq!(doc.aliases, vec![('a', 'A')]);
    assert_eq!(doc.kerning, vec![('A', 'V', -76)]);
    assert_eq!(emit(&doc), GOOD);
}

#[test]
fn quoted_punctuation_and_digit_chars_survive_the_round_trip() {
    let doc = check(GOOD).expect("the font is valid");
    let chars: Vec<char> = doc.glyphs.iter().map(|g| g.ch).collect();
    assert_eq!(chars, [' ', '-', '1', ':', 'A', 'V']);
    assert_eq!(parse(&emit(&doc)).expect("re-parses"), doc);
}

#[test]
fn a_bare_digit_char_is_a_number_not_a_string() {
    rejects_at(&GOOD.replace("char: \"1\"", "char: 1"), "/glyphs/2/char");
}

#[test]
fn a_bitmap_font_without_optional_sections_round_trips() {
    let source = "\
format: 1
kind: font
image: tiny.png
mode: bitmap
units_per_em: 8
ascender: 8
descender: 0
line_gap: 0
atlas:
  width: 8
  height: 8
  em_px: 8
notdef:
  advance: 8
glyphs: []
";
    let doc = check(source).expect("the font is valid");
    assert_eq!(emit(&doc), source);
}

#[test]
fn a_missing_required_key_is_reported() {
    let diagnostics = errors(&GOOD.replace("mode: sdf\n", ""));
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code.as_str() == "L1250" && d.message.contains("`mode`"))
    );
}

#[test]
fn the_wrong_kind_is_reported() {
    rejects_at(&GOOD.replace("kind: font", "kind: sprite"), "/kind");
}

#[test]
fn an_unknown_key_suggests_the_nearest() {
    let diagnostics = errors(&GOOD.replace("ascender:", "ascendr:"));
    let hit = diagnostics
        .iter()
        .find(|d| d.path.as_deref() == Some("/ascendr"))
        .expect("an /ascendr diagnostic");
    assert_eq!(hit.help.as_deref(), Some("nearest match is `ascender`"));
}

#[test]
fn zero_units_per_em_is_rejected() {
    rejects_at(
        &GOOD.replace("units_per_em: 2048", "units_per_em: 0"),
        "/units_per_em",
    );
}

#[test]
fn a_line_with_no_height_is_rejected() {
    rejects_at(
        &GOOD
            .replace("ascender: 1984", "ascender: 100")
            .replace("descender: -494", "descender: 100"),
        "/ascender",
    );
}

#[test]
fn an_sdf_face_needs_spread_and_a_bitmap_face_rejects_it() {
    rejects_at(&GOOD.replace("  spread_px: 6\n", ""), "/atlas/spread_px");
    rejects_at(
        &GOOD.replace("mode: sdf", "mode: bitmap"),
        "/atlas/spread_px",
    );
}

#[test]
fn glyphs_out_of_order_or_duplicated_are_rejected() {
    let swapped = GOOD.replace("char: \"1\"", "char: \"~\"");
    rejects_at(&swapped, "/glyphs/3/char");
    let duplicated = GOOD.replace("char: \"1\"", "char: \"-\"");
    rejects_at(&duplicated, "/glyphs/2/char");
}

#[test]
fn a_rect_outside_the_atlas_or_without_area_is_rejected() {
    rejects_at(&GOOD.replace("width: 64", "width: 60"), "/glyphs/5/rect");
    let flat = GOOD.replace(
        "      - 10\n      - 0\n      - 12\n      - 20\n",
        "      - 10\n      - 0\n      - 0\n      - 20\n",
    );
    rejects_at(&flat, "/glyphs/2/rect");
}

#[test]
fn inverted_bounds_are_rejected() {
    rejects_at(
        &GOOD.replace("      - 600\n      - 80\n", "      - 5\n      - 80\n"),
        "/glyphs/1/bounds",
    );
}

#[test]
fn bounds_without_rect_is_rejected() {
    let source = GOOD.replace(
        "    rect:\n      - 0\n      - 0\n      - 10\n      - 8\n",
        "",
    );
    rejects_at(&source, "/glyphs/1");
}

#[test]
fn a_negative_advance_is_rejected() {
    rejects_at(
        &GOOD.replace("advance: 500", "advance: -1"),
        "/glyphs/0/advance",
    );
}

#[test]
fn an_alias_to_a_missing_glyph_or_shadowing_a_glyph_is_rejected() {
    rejects_at(&GOOD.replace("a: A", "a: Z"), "/aliases/a");
    rejects_at(&GOOD.replace("a: A", "V: A"), "/aliases/V");
}

#[test]
fn kerning_over_a_missing_glyph_is_rejected() {
    rejects_at(&GOOD.replace("    V: -76", "    Z: -76"), "/kerning/A/Z");
}

#[test]
fn a_missing_char_that_has_a_glyph_is_rejected() {
    rejects_at(&GOOD.replace("\"~\"", "A"), "/missing/0");
}

#[test]
fn an_empty_image_is_rejected() {
    rejects_at(&GOOD.replace("image: body.png", "image: \"\""), "/image");
}

#[test]
fn an_escaping_image_path_is_rejected() {
    for image in ["../x.png", "'..\\x.png'", "/abs/x.png", "'C:/x.png'"] {
        rejects_at(
            &GOOD.replace("image: body.png", &format!("image: {image}")),
            "/image",
        );
    }
}

#[test]
fn metrics_beyond_the_cap_are_rejected() {
    rejects_at(
        &GOOD.replace("ascender: 1984", "ascender: 2147483647"),
        "/ascender",
    );
    rejects_at(
        &GOOD.replace("units_per_em: 2048", "units_per_em: 16385"),
        "/units_per_em",
    );
    rejects_at(
        &GOOD.replace("    V: -76", "    V: -2000000"),
        "/kerning/A/V",
    );
    rejects_at(
        &GOOD.replace("    advance: 700", "    advance: 2000000"),
        "/glyphs/1/advance",
    );
}

#[test]
fn the_committed_faces_validate() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for path in [
        "crates/ludion-font/assets/default.font.yaml",
        "examples/gallery/fonts/inter.font.yaml",
        "examples/gallery/fonts/silkscreen.font.yaml",
    ] {
        let source = std::fs::read_to_string(root.join(path)).expect("a committed face");
        if let Err(diagnostics) = check(&source) {
            panic!("{path}: {diagnostics:?}");
        }
    }
}

//! Unit tests for the font source document format (`font_source.rs`).

use super::*;

const SDF: &str = "\
format: 1
kind: font-source
name: inter
file: Inter-Regular.ttf
mode: sdf
em_px: 56
spread_px: 6
charset:
  ranges:
    - ascii
    - latin-1
  chars: \"\u{2026}€\"
";

const BITMAP: &str = "\
format: 1
kind: font-source
file: Silkscreen-Regular.ttf
mode: bitmap
em_px: 8
charset:
  ranges:
    - ascii
";

fn first(source: &str) -> Diagnostic {
    let diagnostics = match parse(source) {
        Ok(doc) => validate(&doc),
        Err(d) => d,
    };
    assert!(!diagnostics.is_empty(), "expected a diagnostic");
    for d in &diagnostics {
        assert_eq!(d.code, code::FONT_SOURCE_DOCUMENT_STRUCTURE);
    }
    diagnostics.into_iter().next().expect("one")
}

#[test]
fn sdf_and_bitmap_round_trip() {
    for source in [SDF, BITMAP] {
        let doc = parse(source).expect("parses");
        assert!(validate(&doc).is_empty());
        assert_eq!(emit(&doc), source);
    }
}

#[test]
fn emit_sorts_and_dedupes_ranges() {
    let source = SDF.replace(
        "    - ascii\n    - latin-1\n",
        "    - latin-1\n    - ascii\n    - latin-1\n",
    );
    let doc = parse(&source).expect("parses");
    assert_eq!(emit(&doc), SDF);
}

#[test]
fn spread_is_required_for_sdf() {
    let d = first(&SDF.replace("spread_px: 6\n", ""));
    assert_eq!(d.path.as_deref(), Some("/spread_px"));
}

#[test]
fn spread_is_rejected_for_bitmap() {
    let d = first(&BITMAP.replace("em_px: 8\n", "em_px: 8\nspread_px: 2\n"));
    assert_eq!(d.path.as_deref(), Some("/spread_px"));
}

#[test]
fn spread_cannot_exceed_em() {
    let d = first(&SDF.replace("spread_px: 6", "spread_px: 57"));
    assert_eq!(d.path.as_deref(), Some("/spread_px"));
}

#[test]
fn em_px_is_range_checked() {
    for bad in ["0", "257"] {
        let d = first(&BITMAP.replace("em_px: 8", &format!("em_px: {bad}")));
        assert_eq!(d.path.as_deref(), Some("/em_px"));
    }
}

#[test]
fn unknown_range_keyword_suggests() {
    let d = first(&BITMAP.replace("- ascii", "- latin1"));
    assert_eq!(d.path.as_deref(), Some("/charset/ranges/0"));
    assert!(d.help.as_deref().is_some_and(|h| h.contains("`latin-1`")));
}

#[test]
fn empty_charset_is_rejected() {
    let d = first(&BITMAP.replace("  ranges:\n    - ascii\n", "  chars: \"\"\n"));
    assert_eq!(d.path.as_deref(), Some("/charset"));
    let d = first(&BITMAP.replace("charset:\n  ranges:\n    - ascii\n", "charset: {}\n"));
    assert_eq!(d.path.as_deref(), Some("/charset"));
}

#[test]
fn unknown_key_suggests() {
    let d = first(&BITMAP.replace("em_px:", "em_pxx:"));
    assert_eq!(d.path.as_deref(), Some("/em_pxx"));
}

#[test]
fn missing_file_is_rejected() {
    let d = first(&BITMAP.replace("file: Silkscreen-Regular.ttf\n", ""));
    assert!(d.message.contains("`file`"));
}

#[test]
fn wrong_kind_is_rejected() {
    first(&BITMAP.replace("font-source", "font"));
}

#[test]
fn line_height_round_trips_on_both_modes() {
    for (source, anchor) in [(SDF, "spread_px: 6\n"), (BITMAP, "em_px: 8\n")] {
        let source = source.replace(anchor, &format!("{anchor}line_height: 1250\n"));
        let doc = parse(&source).expect("parses");
        assert_eq!(doc.line_height, Some(1250));
        assert!(validate(&doc).is_empty());
        assert_eq!(emit(&doc), source);
    }
}

#[test]
fn line_height_is_range_checked() {
    for bad in ["0", "70000"] {
        let d = first(&BITMAP.replace("em_px: 8\n", &format!("em_px: 8\nline_height: {bad}\n")));
        assert_eq!(d.path.as_deref(), Some("/line_height"));
    }
}

#[test]
fn name_must_be_a_plain_stem() {
    for bad in ["a/b", "'a\\b'", "../x", "'C:x'", "\"\"", "."] {
        let d = first(&SDF.replace("name: inter", &format!("name: {bad}")));
        assert_eq!(d.path.as_deref(), Some("/name"), "{bad}");
    }
}

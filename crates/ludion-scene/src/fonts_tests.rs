//! Unit tests for the font table format (`fonts.rs`).

use super::*;

const TABLE: &str = "\
format: 1
kind: fonts
fonts:
  body: fonts/inter.font.yaml
  default: fonts/inter.font.yaml
  title: fonts/silkscreen.font.yaml
";

fn first(source: &str) -> Diagnostic {
    let diagnostics = match parse(source) {
        Ok(doc) => validate(&doc),
        Err(d) => d,
    };
    assert!(!diagnostics.is_empty(), "expected a diagnostic");
    for d in &diagnostics {
        assert_eq!(d.code, code::FONTS_TABLE_STRUCTURE);
    }
    diagnostics.into_iter().next().expect("one")
}

fn with_entry(name: &str, path: &str) -> String {
    format!("format: 1\nkind: fonts\nfonts:\n  {name}: {path}\n")
}

#[test]
fn a_valid_table_round_trips_byte_identically() {
    let doc = parse(TABLE).expect("parses");
    assert!(validate(&doc).is_empty());
    assert_eq!(doc.fonts.len(), 3);
    assert_eq!(emit(&doc), TABLE);
}

#[test]
fn an_empty_table_round_trips() {
    let source = "format: 1\nkind: fonts\nfonts: {}\n";
    let doc = parse(source).expect("parses");
    assert!(doc.fonts.is_empty());
    assert_eq!(emit(&doc), source);
}

#[test]
fn emit_sorts_names() {
    let doc = FontsDocument {
        fonts: vec![
            ("zeta".into(), "z.font.yaml".into()),
            ("alpha".into(), "a.font.yaml".into()),
        ],
    };
    assert_eq!(
        emit(&doc),
        "format: 1\nkind: fonts\nfonts:\n  alpha: a.font.yaml\n  zeta: z.font.yaml\n"
    );
}

#[test]
fn parse_sorts_names() {
    let doc = parse("format: 1\nkind: fonts\nfonts:\n  b: b.font.yaml\n  a: a.font.yaml\n")
        .expect("parses");
    assert_eq!(doc.fonts[0].0, "a");
}

#[test]
fn an_unknown_key_is_rejected_with_a_pointer() {
    let d = first("format: 1\nkind: fonts\nfont: {}\nfonts: {}\n");
    assert!(d.message.contains("font"), "{}", d.message);
}

#[test]
fn a_missing_fonts_key_is_rejected() {
    let d = first("format: 1\nkind: fonts\n");
    assert!(d.message.contains("fonts"), "{}", d.message);
}

#[test]
fn fonts_must_be_a_mapping() {
    let d = first("format: 1\nkind: fonts\nfonts:\n  - a\n");
    assert_eq!(d.path.as_deref(), Some("/fonts"));
}

#[test]
fn a_wrong_kind_is_rejected() {
    first("format: 1\nkind: font\nfonts: {}\n");
}

#[test]
fn a_name_with_whitespace_or_slash_is_rejected() {
    let d = first(&with_entry("\"my face\"", "a.font.yaml"));
    assert_eq!(d.path.as_deref(), Some("/fonts/my face"));
    let d = first(&with_entry("\"a/b\"", "a.font.yaml"));
    assert_eq!(d.path.as_deref(), Some("/fonts/a~1b"));
    first(&with_entry("\"\"", "a.font.yaml"));
}

#[test]
fn a_bad_path_is_rejected() {
    for path in [
        "/abs/a.font.yaml",
        "../a.font.yaml",
        "fonts/../../a.font.yaml",
        "fonts/a.yaml",
        "fonts/a.png",
    ] {
        let d = first(&with_entry("body", path));
        assert_eq!(d.path.as_deref(), Some("/fonts/body"), "{path}");
    }
}

#[test]
fn escaping_and_non_canonical_paths_are_rejected() {
    for path in [
        "'..\\x.font.yaml'",
        "'C:/x.font.yaml'",
        "'\\\\host\\x.font.yaml'",
        "./fonts/x.font.yaml",
        "fonts//x.font.yaml",
        "/abs/x.font.yaml",
        "'fonts\\x.font.yaml'",
    ] {
        let d = first(&with_entry("body", path));
        assert_eq!(d.path.as_deref(), Some("/fonts/body"), "{path}");
    }
    let d = first(&with_entry("body", "./fonts/x.font.yaml"));
    assert_eq!(d.help.as_deref(), Some("write it as `fonts/x.font.yaml`"));
}

#[test]
fn a_non_string_path_is_rejected() {
    let d = first(&with_entry("body", "12"));
    assert_eq!(d.path.as_deref(), Some("/fonts/body"));
}

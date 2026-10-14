//! The shared-palette include's drift-safety property: a sprite that references a `kind: palette`
//! document (with local overrides) bakes to byte-identical pixels to the same sprite authored inline
//! with the resolved palette. This is what keeps the committed atlas PNG stable under the
//! `atlas-drift` gate no matter how a sprite's palette was authored (`Plan/sprite-engine.md`).

use ludion_scene::{check_sprite, resolve_sprite_include, validate_sprite};
use ludion_sprite::bake;

/// The shared palette the include references.
const PALETTE: &str = "\
format: 1
kind: palette
palette:
  \"#\":
    color:
      - 0.1
      - 0.2
      - 0.3
      - 1.0
  o:
    color:
      - 0.4
      - 0.5
      - 0.6
      - 1.0
";

/// The same sprite authored inline with the *resolved* palette: `#` overridden to a warm value, `o`
/// inherited from the shared palette, `O` added by the override. Effects included, so the comparison
/// exercises the whole bake, not just flat fills.
const INLINE: &str = "\
format: 1
kind: sprite
name: emblem
size:
  - 3
  - 3
palette:
  \"#\":
    color:
      - 0.9
      - 0.1
      - 0.1
      - 1.0
  O:
    color:
      - 1.0
      - 1.0
      - 0.0
      - 1.0
  o:
    color:
      - 0.4
      - 0.5
      - 0.6
      - 1.0
frames:
  crest:
    rows:
      - \"#o#\"
      - \"oOo\"
      - \"#o#\"
effects:
  - op: outline
    color:
      - 0.0
      - 0.0
      - 0.0
      - 1.0
";

/// The same sprite via a `from` include plus `overrides`: `#` replaced, `O` added, `o` inherited.
const INCLUDE: &str = "\
format: 1
kind: sprite
name: emblem
size:
  - 3
  - 3
palette:
  from: emblem.palette.yaml
  overrides:
    \"#\":
      color:
        - 0.9
        - 0.1
        - 0.1
        - 1.0
    O:
      color:
        - 1.0
        - 1.0
        - 0.0
        - 1.0
frames:
  crest:
    rows:
      - \"#o#\"
      - \"oOo\"
      - \"#o#\"
effects:
  - op: outline
    color:
      - 0.0
      - 0.0
      - 0.0
      - 1.0
";

#[test]
fn an_include_bakes_identically_to_the_equivalent_inline_sprite() {
    let inline = check_sprite(INLINE).expect("the inline sprite is valid");

    let include =
        check_sprite(INCLUDE).expect("the include sprite parses and structurally validates");
    assert!(
        include.palette_include.is_some(),
        "the `from` form must parse as an include"
    );
    assert!(
        include.palette.is_empty(),
        "an unresolved include holds no inline palette"
    );

    let resolved = resolve_sprite_include(&include, PALETTE).expect("the include resolves");
    assert!(
        resolved.palette_include.is_none(),
        "resolution clears the include"
    );
    // The merged palette equals the inline one as a set of entries (both are glyph-sorted).
    assert_eq!(resolved.palette, inline.palette);

    let inline_frames = bake(&inline).expect("the inline sprite bakes");
    let include_frames = bake(&resolved).expect("the resolved include bakes");
    assert_eq!(
        include_frames, inline_frames,
        "an included palette must bake byte-identically to the equivalent inline palette"
    );
}

#[test]
fn a_glyph_undefined_after_resolution_is_l1153() {
    // The frame uses `Z`, which neither the shared palette nor the overrides define.
    let source = INCLUDE.replace("      - \"oOo\"", "      - \"oZo\"");
    let include = check_sprite(&source).expect("structure validates without the palette in hand");
    let resolved = resolve_sprite_include(&include, PALETTE).expect("the include still resolves");
    // Once resolved, the sprite has a concrete palette, so the undefined glyph is the ordinary L1153.
    let diagnostics = validate_sprite(&resolved);
    assert!(
        diagnostics.iter().any(|d| d.code.as_str() == "L1153"),
        "an undefined glyph after resolution must be L1153: {diagnostics:#?}"
    );
    // The bake refuses it for the same reason.
    let bake_errors = bake(&resolved).expect_err("bake refuses an undefined glyph");
    assert!(bake_errors.iter().any(|d| d.code.as_str() == "L1153"));
}

#[test]
fn a_malformed_palette_document_is_l1147() {
    let include = check_sprite(INCLUDE).expect("the include sprite parses");
    let bad_palette = "format: 1\nkind: sprite\npalette: {}\n"; // wrong kind
    let diagnostics = resolve_sprite_include(&include, bad_palette)
        .expect_err("a non-palette document is refused");
    assert!(
        diagnostics.iter().any(|d| d.code.as_str() == "L1147"),
        "a malformed palette reference must be L1147: {diagnostics:#?}"
    );
}

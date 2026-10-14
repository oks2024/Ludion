//! Unit tests for the sprite document format (`sprite.rs`): parse structure, validate semantics,
//! and the canonical emitter's minimality rule.

use super::*;

/// A full-featured document: both optional envelope keys, an emissive palette entry, a pivot, two
/// frames, and every effect op exactly once with every optional parameter set to a **non-default**
/// value (a default would be omitted by the canonical writer, so a fixture spelling one would not
/// be a fixed point).
const GOOD: &str = "\
format: 1
kind: sprite
name: ship
size:
  - 4
  - 3
seed: 7
palette:
  \"#\":
    color:
      - 0.8
      - 0.8
      - 0.9
      - 1.0
  o:
    color:
      - 1.0
      - 0.4
      - 0.1
      - 1.0
    emissive: 2.0
frames:
  idle:
    pivot:
      - 2.0
      - 1.0
    rows:
      - \".##.\"
      - \"#oo#\"
      - \".##.\"
  bank-left:
    rows:
      - \"##..\"
      - \"#oo.\"
      - \"##..\"
effects:
  - op: gradient
    glyphs:
      - \"#\"
    frames:
      - idle
    from:
      - 0.0
      - 0.0
      - 0.0
      - 1.0
    to:
      - 1.0
      - 1.0
      - 1.0
      - 1.0
    direction: radial
    blend: multiply
  - op: outline
    color:
      - 0.0
      - 0.0
      - 0.0
      - 1.0
    side: inside
  - op: shade
    light:
      - 1.0
      - -1.0
    strength: 0.4
  - op: shadow
    offset:
      - 2
      - 1
    color:
      - 0.0
      - 0.0
      - 0.0
      - 0.75
  - op: dither
    glyphs:
      - \"#\"
      - o
    density: 0.25
  - op: noise
    amount: 0.05
  - op: blur
    radius: 2
";

const MINIMAL: &str = "\
format: 1
kind: sprite
size:
  - 1
  - 1
palette:
  A:
    color:
      - 1.0
      - 1.0
      - 1.0
      - 1.0
frames:
  idle:
    rows:
      - A
";

fn check(source: &str) -> Result<SpriteDocument, Vec<Diagnostic>> {
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
        Ok(_) => panic!("expected the sprite document to be rejected"),
        Err(diagnostics) => diagnostics,
    }
}

fn find<'a>(diagnostics: &'a [Diagnostic], path: &str) -> &'a Diagnostic {
    diagnostics
        .iter()
        .find(|d| d.path.as_deref() == Some(path))
        .unwrap_or_else(|| panic!("no diagnostic at {path}; got {diagnostics:#?}"))
}

// --- Round trip ---------------------------------------------------------------------------------

#[test]
fn the_good_sprite_round_trips_byte_identically() {
    let doc = check(GOOD).expect("the sprite is valid");
    assert_eq!(emit(&doc), GOOD);
}

#[test]
fn a_minimal_sprite_round_trips_byte_identically() {
    let doc = check(MINIMAL).expect("the sprite is valid");
    assert_eq!(emit(&doc), MINIMAL);
    assert_eq!(doc.name, None);
    assert_eq!(doc.seed, None);
    assert!(doc.effects.is_empty());
}

#[test]
fn a_spelled_out_default_re_emits_shorter_then_holds() {
    // Canonical is minimal: an authored value equal to the default is dropped on the way out, so
    // the first emit shrinks the text and every emit after that is byte-identical.
    let verbose = "\
format: 1
kind: sprite
size:
  - 1
  - 1
palette:
  A:
    color:
      - 1.0
      - 1.0
      - 1.0
      - 1.0
frames:
  idle:
    rows:
      - A
effects:
  - op: shade
    light:
      - -1.0
      - -1.0
    strength: 0.25
";
    let doc = check(verbose).expect("the sprite is valid");
    let once = emit(&doc);
    assert!(once.ends_with("effects:\n  - op: shade\n"), "{once}");
    let twice = emit(&check(&once).expect("the re-emitted sprite is valid"));
    assert_eq!(once, twice);
}

#[test]
fn the_emitted_text_is_lf_only() {
    let doc = check(GOOD).expect("the sprite is valid");
    assert!(!emit(&doc).contains('\r'));
}

// --- Accessors ----------------------------------------------------------------------------------

#[test]
fn accessors_answer_from_the_parsed_document() {
    let doc = check(GOOD).expect("the sprite is valid");
    assert_eq!(doc.frame("bank-left").map(|f| f.rows.len()), Some(3));
    assert!(doc.frame("fly").is_none());
    assert_eq!(doc.palette_entry('o').and_then(|e| e.emissive), Some(2.0));
    assert!(doc.palette_entry(SPRITE_EMPTY_GLYPH).is_none());
    assert!(doc.palette_entry('Z').is_none());
    assert_eq!(doc.default_pivot(), [2.0, 1.5]);
}

// --- Structure (`L1153`) ------------------------------------------------------------------------

#[test]
fn an_empty_document_is_rejected() {
    let diagnostics = parse("").expect_err("an empty document is not a sprite");
    assert_eq!(diagnostics[0].code.as_str(), "L1153");
    assert_eq!(diagnostics[0].path.as_deref(), Some(""));
}

#[test]
fn a_non_mapping_root_is_rejected() {
    let diagnostics = parse("- 1\n").expect_err("a sequence is not a sprite document");
    assert_eq!(diagnostics[0].code.as_str(), "L1153");
}

#[test]
fn the_missing_required_keys_are_each_named() {
    let diagnostics = parse("format: 1\nkind: sprite\n").expect_err("required keys are missing");
    for key in ["size", "palette", "frames"] {
        assert!(
            diagnostics.iter().any(|d| d.message.contains(key)),
            "no diagnostic named `{key}`",
        );
    }
}

#[test]
fn an_unknown_top_level_key_suggests_the_nearest() {
    let diagnostics = errors(&GOOD.replace("seed: 7", "sed: 7"));
    let diagnostic = find(&diagnostics, "/sed");
    assert_eq!(diagnostic.code.as_str(), "L1153");
    assert_eq!(diagnostic.help.as_deref(), Some("nearest match is `seed`"));
}

#[test]
fn an_unknown_frame_key_suggests_the_nearest() {
    let diagnostics = errors(&GOOD.replace("  idle:\n    pivot:", "  idle:\n    pivots:"));
    let diagnostic = find(&diagnostics, "/frames/idle/pivots");
    assert_eq!(diagnostic.code.as_str(), "L1153");
    assert_eq!(diagnostic.help.as_deref(), Some("nearest match is `pivot`"));
}

#[test]
fn the_wrong_kind_is_rejected() {
    let diagnostics = errors(&GOOD.replace("kind: sprite", "kind: voxel"));
    assert_eq!(find(&diagnostics, "/kind").code.as_str(), "L1153");
}

#[test]
fn a_non_integer_seed_is_rejected() {
    let diagnostics = errors(&GOOD.replace("seed: 7", "seed: 7.5"));
    assert_eq!(find(&diagnostics, "/seed").code.as_str(), "L1153");
}

// --- Semantics (`L1153`) ------------------------------------------------------------------------

#[test]
fn a_size_above_the_maximum_is_rejected() {
    let diagnostics = errors(&GOOD.replace("size:\n  - 4\n", "size:\n  - 2048\n"));
    let diagnostic = find(&diagnostics, "/size/0");
    assert_eq!(diagnostic.code.as_str(), "L1153");
    assert!(diagnostic.message.contains(&SPRITE_MAX_SIZE.to_string()));
}

#[test]
fn the_transparent_glyph_cannot_name_a_palette_entry() {
    let with_dot = GOOD.replace(
        "  o:\n",
        "  \".\":\n    color:\n      - 0.0\n      - 0.0\n      - 0.0\n      - 1.0\n  o:\n",
    );
    let diagnostics = errors(&with_dot);
    let diagnostic = find(&diagnostics, "/palette/.");
    assert_eq!(diagnostic.code.as_str(), "L1153");
    assert!(diagnostic.message.contains("reserved"));
}

#[test]
fn a_multi_character_palette_key_is_rejected() {
    let diagnostics = errors(&GOOD.replace("  o:\n", "  oo:\n"));
    let diagnostic = find(&diagnostics, "/palette/oo");
    assert_eq!(diagnostic.code.as_str(), "L1153");
    assert!(diagnostic.message.contains("exactly one character"));
}

#[test]
fn a_slash_glyph_is_rfc_6901_escaped_in_the_pointer() {
    // `/` and `~` are legal palette glyphs and both need escaping in a JSON Pointer, which is the
    // one place a glyph-keyed diagnostic can go wrong silently.
    let source = GOOD.replace(
        "  o:\n",
        "  \"/\":\n    color:\n      - 2.0\n      - 0.0\n      - 0.0\n      - 1.0\n  \"~\":\n    \
         color:\n      - 0.0\n      - 3.0\n      - 0.0\n      - 1.0\n  o:\n",
    );
    let diagnostics = errors(&source);
    assert_eq!(
        find(&diagnostics, "/palette/~1/color/0").code.as_str(),
        "L1153"
    );
    assert_eq!(
        find(&diagnostics, "/palette/~0/color/1").code.as_str(),
        "L1153"
    );
}

#[test]
fn a_color_outside_zero_to_one_is_rejected() {
    let diagnostics = errors(&GOOD.replace("      - 0.9\n", "      - 1.9\n"));
    let diagnostic = find(&diagnostics, "/palette/#/color/2");
    assert_eq!(diagnostic.code.as_str(), "L1153");
}

#[test]
fn a_negative_emissive_is_rejected() {
    let diagnostics = errors(&GOOD.replace("emissive: 2.0", "emissive: -1.0"));
    assert_eq!(
        find(&diagnostics, "/palette/o/emissive").code.as_str(),
        "L1153"
    );
}

#[test]
fn a_duplicate_glyph_is_rejected() {
    // Two entries with one glyph cannot be authored (the YAML layer rejects a duplicate key as
    // `L1007`), so the rule is proved against a hand-built document — which is exactly why
    // `validate` is independent of `parse`.
    let entry = SpritePaletteEntry {
        glyph: "A".to_string(),
        color: [1.0, 1.0, 1.0, 1.0],
        emissive: None,
    };
    let doc = SpriteDocument {
        size: [1, 1],
        palette: vec![entry.clone(), entry],
        frames: vec![SpriteFrame {
            name: "idle".to_string(),
            pivot: None,
            rows: vec!["A".to_string()],
        }],
        ..SpriteDocument::default()
    };
    let diagnostics = validate(&doc);
    let diagnostic = find(&diagnostics, "/palette/A");
    assert_eq!(diagnostic.code.as_str(), "L1153");
    assert!(diagnostic.message.contains("more than once"));
}

#[test]
fn an_empty_palette_is_rejected() {
    let doc = SpriteDocument {
        size: [1, 1],
        frames: vec![SpriteFrame {
            name: "idle".to_string(),
            pivot: None,
            rows: vec![".".to_string()],
        }],
        ..SpriteDocument::default()
    };
    assert_eq!(find(&validate(&doc), "/palette").code.as_str(), "L1153");
}

#[test]
fn a_document_with_no_frames_is_rejected() {
    let doc = SpriteDocument {
        size: [1, 1],
        palette: vec![SpritePaletteEntry {
            glyph: "A".to_string(),
            color: [1.0, 1.0, 1.0, 1.0],
            emissive: None,
        }],
        ..SpriteDocument::default()
    };
    assert_eq!(find(&validate(&doc), "/frames").code.as_str(), "L1153");
}

#[test]
fn a_row_of_the_wrong_width_is_rejected() {
    let diagnostics = errors(&GOOD.replace("\"#oo#\"", "\"#oo\""));
    let diagnostic = find(&diagnostics, "/frames/idle/rows/1");
    assert_eq!(diagnostic.code.as_str(), "L1153");
    assert!(diagnostic.message.contains("3 characters wide"));
}

#[test]
fn the_wrong_number_of_rows_is_rejected() {
    let diagnostics =
        errors(&GOOD.replace("      - \".##.\"\n      - \"#oo#\"\n", "      - \"#oo#\"\n"));
    assert_eq!(
        find(&diagnostics, "/frames/idle/rows").code.as_str(),
        "L1153"
    );
}

#[test]
fn a_glyph_that_names_no_palette_entry_is_rejected() {
    let diagnostics = errors(&GOOD.replace("\"#oo#\"", "\"#oZ#\""));
    let diagnostic = find(&diagnostics, "/frames/idle/rows/1");
    assert_eq!(diagnostic.code.as_str(), "L1153");
    assert_eq!(
        diagnostic.help.as_deref(),
        Some("add `Z` to the palette, or use `.` for a transparent texel")
    );
}

#[test]
fn a_frame_name_with_a_slash_is_rejected() {
    let diagnostics = errors(&GOOD.replace("  bank-left:", "  bank/left:"));
    let diagnostic = find(&diagnostics, "/frames/bank~1left");
    assert_eq!(diagnostic.code.as_str(), "L1153");
    assert!(diagnostic.message.contains("contains a `/`"));
}

#[test]
fn a_pivot_outside_the_frame_is_rejected() {
    let diagnostics = errors(&GOOD.replace(
        "    pivot:\n      - 2.0\n      - 1.0\n",
        "    pivot:\n      - 2.0\n      - 9.0\n",
    ));
    assert_eq!(
        find(&diagnostics, "/frames/idle/pivot/1").code.as_str(),
        "L1153"
    );
}

// --- Effects (`L1154`) --------------------------------------------------------------------------

#[test]
fn an_unknown_op_suggests_the_nearest() {
    let diagnostics = errors(&GOOD.replace("  - op: outline", "  - op: outlien"));
    let diagnostic = find(&diagnostics, "/effects/1/op");
    assert_eq!(diagnostic.code.as_str(), "L1154");
    assert_eq!(
        diagnostic.help.as_deref(),
        Some("nearest match is `outline`")
    );
}

#[test]
fn an_effect_entry_without_an_op_is_rejected() {
    let diagnostics = errors(&GOOD.replace("  - op: blur\n", "  - \n"));
    let diagnostic = find(&diagnostics, "/effects/6");
    assert_eq!(diagnostic.code.as_str(), "L1154");
}

#[test]
fn a_key_belonging_to_another_op_is_unknown() {
    // The op decides the entry's whole key set, so `radius` on a gradient is an unknown key rather
    // than a stray field the parser could shrug off.
    let diagnostics =
        errors(&GOOD.replace("  - op: gradient\n", "  - op: gradient\n    radius: 2\n"));
    let diagnostic = find(&diagnostics, "/effects/0/radius");
    assert_eq!(diagnostic.code.as_str(), "L1154");
    assert!(diagnostic.message.contains("`gradient` effect"));
}

#[test]
fn a_missing_required_effect_parameter_is_reported() {
    let diagnostics = errors(&GOOD.replace("    amount: 0.05\n", ""));
    let diagnostic = find(&diagnostics, "/effects/5");
    assert_eq!(diagnostic.code.as_str(), "L1154");
    assert!(diagnostic.message.contains("amount"));
}

#[test]
fn an_unknown_direction_suggests_the_nearest() {
    let diagnostics = errors(&GOOD.replace("direction: radial", "direction: radal"));
    let diagnostic = find(&diagnostics, "/effects/0/direction");
    assert_eq!(diagnostic.code.as_str(), "L1154");
    assert_eq!(
        diagnostic.help.as_deref(),
        Some("nearest match is `radial`")
    );
}

#[test]
fn an_effect_glyph_outside_the_palette_is_rejected() {
    let diagnostics = errors(&GOOD.replace(
        "    glyphs:\n      - \"#\"\n    frames:\n      - idle\n",
        "    glyphs:\n      - Z\n    frames:\n      - idle\n",
    ));
    let diagnostic = find(&diagnostics, "/effects/0/glyphs/0");
    assert_eq!(diagnostic.code.as_str(), "L1154");
}

#[test]
fn an_effect_frame_that_names_nothing_suggests_the_nearest() {
    let diagnostics =
        errors(&GOOD.replace("    frames:\n      - idle\n", "    frames:\n      - idel\n"));
    let diagnostic = find(&diagnostics, "/effects/0/frames/0");
    assert_eq!(diagnostic.code.as_str(), "L1154");
    assert_eq!(diagnostic.help.as_deref(), Some("nearest match is `idle`"));
}

#[test]
fn a_dither_needs_exactly_two_glyphs() {
    let diagnostics = errors(&GOOD.replace(
        "      - \"#\"\n      - o\n    density: 0.25\n",
        "      - \"#\"\n    density: 0.25\n",
    ));
    let diagnostic = find(&diagnostics, "/effects/4/glyphs");
    assert_eq!(diagnostic.code.as_str(), "L1154");
    assert!(diagnostic.message.contains("found 1"));
}

#[test]
fn a_blur_radius_outside_its_range_is_rejected() {
    let diagnostics = errors(&GOOD.replace("    radius: 2\n", "    radius: 12\n"));
    assert_eq!(
        find(&diagnostics, "/effects/6/radius").code.as_str(),
        "L1154"
    );
}

#[test]
fn a_shadow_offset_of_zero_is_rejected() {
    let diagnostics = errors(&GOOD.replace(
        "    offset:\n      - 2\n      - 1\n",
        "    offset:\n      - 0\n      - 0\n",
    ));
    assert_eq!(
        find(&diagnostics, "/effects/3/offset").code.as_str(),
        "L1154"
    );
}

#[test]
fn a_shade_light_of_zero_is_rejected() {
    let diagnostics = errors(&GOOD.replace(
        "    light:\n      - 1.0\n      - -1.0\n",
        "    light:\n      - 0.0\n      - 0.0\n",
    ));
    assert_eq!(
        find(&diagnostics, "/effects/2/light").code.as_str(),
        "L1154"
    );
}

#[test]
fn an_out_of_range_unit_parameter_is_rejected() {
    let diagnostics = errors(&GOOD.replace("    amount: 0.05", "    amount: 1.5"));
    assert_eq!(
        find(&diagnostics, "/effects/5/amount").code.as_str(),
        "L1154"
    );
    let diagnostics = errors(&GOOD.replace("    strength: 0.4", "    strength: 4.0"));
    assert_eq!(
        find(&diagnostics, "/effects/2/strength").code.as_str(),
        "L1154"
    );
}

#[test]
fn a_fractional_shadow_offset_is_rejected() {
    let diagnostics = errors(&GOOD.replace(
        "    offset:\n      - 2\n      - 1\n",
        "    offset:\n      - 2.5\n      - 1\n",
    ));
    assert_eq!(
        find(&diagnostics, "/effects/3/offset/0").code.as_str(),
        "L1154"
    );
}

#[test]
fn the_effects_key_itself_is_a_document_defect() {
    // The list's own shape is document structure (`L1153`); only an entry inside it is `L1154`.
    let diagnostics = errors(&GOOD.replace(
        "effects:\n  - op: gradient",
        "effects: {}\nunused:\n  - op: gradient",
    ));
    assert_eq!(find(&diagnostics, "/effects").code.as_str(), "L1153");
}

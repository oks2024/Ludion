//! Unit tests for the sprite document format (`sprite.rs`): parse structure, validate semantics,
//! and the canonical emitter's minimality rule.

use super::*;
use crate::check_sprite;

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
    from: \"#000000\"
    to: \"#ffffff\"
    direction: radial
    blend: multiply
  - op: outline
    color: \"#000000\"
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
    color: \"#ffffff\"
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
    color: \"#ffffff\"
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
    assert_eq!(
        doc.frame("bank-left").map(|f| doc.frame_rows(f).len()),
        Some(3)
    );
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
            grid: SpriteFrameGrid::Rows {
                rows: vec!["A".to_string()],
                mirror: None,
            },
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
            grid: SpriteFrameGrid::Rows {
                rows: vec![".".to_string()],
                mirror: None,
            },
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

/// A blank sprite document `width`x`height` with one frame, `idle`, of `.`-only rows.
fn blank(width: usize, height: usize) -> SpriteDocument {
    let rows: String = (0..height)
        .map(|_| format!("      - \"{}\"\n", ".".repeat(width)))
        .collect();
    let source = format!(
        "format: 1\nkind: sprite\nsize:\n  - {width}\n  - {height}\npalette:\n  A:\n    color:\n      \
         - 1.0\n      - 1.0\n      - 1.0\n      - 1.0\nframes:\n  idle:\n    rows:\n{rows}"
    );
    check_sprite(&source).expect("the blank sprite is valid")
}

#[test]
fn frame_rows_equals_the_authored_rows() {
    let sprite = check_sprite(GOOD).expect("GOOD is valid");
    let frame = sprite.frame("idle").expect("idle exists");
    let SpriteFrameGrid::Rows { rows, mirror: None } = &frame.grid else {
        panic!("idle authors plain rows");
    };
    assert_eq!(sprite.frame_rows(frame).as_ref(), rows.as_slice());
}

#[test]
fn render_frame_rows_has_a_units_ruler_and_row_labels() {
    let sprite = blank(4, 2);
    let frame = sprite.frame("idle").expect("idle exists");
    assert_eq!(
        sprite.render_frame_rows(frame),
        "   0123\n0  ....\n1  ....\n"
    );
}

#[test]
fn render_frame_rows_adds_a_tens_line_over_wide_frames() {
    let sprite = blank(12, 1);
    let frame = sprite.frame("idle").expect("idle exists");
    let text = sprite.render_frame_rows(frame);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "             11");
    assert_eq!(lines[1], "   012345678901");
}

#[test]
fn render_frame_rows_adds_a_hundreds_line_past_100_columns() {
    let sprite = blank(101, 1);
    let frame = sprite.frame("idle").expect("idle exists");
    let text = sprite.render_frame_rows(frame);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 4);
    assert_eq!(lines[0].trim_start(), "1");
    assert_eq!(lines[0].len(), 3 + 101);
    assert!(lines.iter().all(|line| !line.ends_with(' ')));
}

#[test]
fn render_frame_rows_right_aligns_labels_of_tall_frames() {
    let sprite = blank(2, 12);
    let frame = sprite.frame("idle").expect("idle exists");
    let text = sprite.render_frame_rows(frame);
    assert!(text.contains("\n 9  ..\n10  ..\n"), "{text}");
}

// --- Hex colours --------------------------------------------------------------------------------

fn sprite_with_color(color: &str) -> String {
    format!(
        "format: 1\nkind: sprite\nsize:\n  - 1\n  - 1\npalette:\n  A:\n    color:{}{color}\nframes:\n  idle:\n    rows:\n      - A\n",
        if color.starts_with('\n') { "" } else { " " }
    )
}

fn sprite_with_effect_color(color: &str) -> String {
    format!(
        "format: 1\nkind: sprite\nsize:\n  - 1\n  - 1\npalette:\n  A:\n    color: \"#ffffff\"\nframes:\n  idle:\n    rows:\n      - A\neffects:\n  - op: outline\n    color: {color}\n"
    )
}

#[test]
fn hex_colours_parse_in_either_case() {
    let upper = check(&sprite_with_color("\"#3A7BD5\"")).expect("valid");
    let lower = check(&sprite_with_color("\"#3a7bd5\"")).expect("valid");
    assert_eq!(upper.palette[0].color, lower.palette[0].color);
    assert_eq!(upper.palette[0].color[3], 1.0);
}

#[test]
fn hex_alpha_is_a_byte_over_255() {
    let doc = check(&sprite_with_color("\"#ff000080\"")).expect("valid");
    assert_eq!(doc.palette[0].color[3], f64::from(128.0f32 / 255.0));
    let doc = check(&sprite_with_effect_color("\"#ffffff80\"")).expect("valid");
    let SpriteEffectOp::Outline { color, .. } = &doc.effects[0].op else {
        panic!("an outline");
    };
    assert_eq!(color[3], f64::from(128.0f32 / 255.0));
}

#[test]
fn a_short_hex_palette_colour_is_refused_with_the_expansion() {
    let diagnostics = parse(&sprite_with_color("\"#f80\"")).expect_err("short hex");
    assert_eq!(diagnostics[0].code, code::SPRITE_DOCUMENT_STRUCTURE);
    assert_eq!(diagnostics[0].path.as_deref(), Some("/palette/A/color"));
    assert!(
        diagnostics[0].message.contains("\"#rrggbb\""),
        "{:?}",
        diagnostics[0]
    );
    assert!(
        diagnostics[0]
            .help
            .as_deref()
            .is_some_and(|h| h.contains("#ff8800")),
        "{:?}",
        diagnostics[0]
    );
}

#[test]
fn a_short_hex_effect_colour_is_an_effect_error() {
    let diagnostics = parse(&sprite_with_effect_color("\"#fff\"")).expect_err("short hex");
    assert_eq!(diagnostics[0].code, code::SPRITE_EFFECT_INVALID);
}

#[test]
fn a_non_hex_string_is_refused() {
    let diagnostics = parse(&sprite_with_color("red")).expect_err("a name");
    assert_eq!(diagnostics[0].code, code::SPRITE_DOCUMENT_STRUCTURE);
    assert!(
        diagnostics[0].message.contains("`red`"),
        "{:?}",
        diagnostics[0]
    );
}

#[test]
fn an_unquoted_hash_colour_names_the_yaml_comment() {
    let diagnostics = parse(&sprite_with_color("#ff0000")).expect_err("a comment");
    let help = diagnostics[0].help.as_deref().expect("help");
    assert!(help.contains("YAML comment"), "{help}");
}

#[test]
fn a_palette_document_takes_hex_and_keeps_its_own_code() {
    let ok = "format: 1\nkind: palette\npalette:\n  A:\n    color: \"#102030\"\n";
    let doc = crate::check_palette(ok).expect("valid");
    assert_eq!(crate::emit_palette(&doc), ok);
    let bad = ok.replace("#102030", "#123");
    let diagnostics = crate::parse_palette(&bad).expect_err("short hex");
    assert_eq!(diagnostics[0].code, code::SPRITE_PALETTE_STRUCTURE);
}

#[test]
fn legacy_float_colours_still_load_silently() {
    let source = sprite_with_color("\n      - 0.5\n      - 0.25\n      - 0.125\n      - 1.0");
    let doc = check(&source).expect("legacy floats load");
    assert_eq!(doc.palette[0].color, [0.5, 0.25, 0.125, 1.0]);
}

#[test]
fn a_hex_document_is_an_emit_fixed_point() {
    let source = sprite_with_color("\"#3a7bd5\"");
    let doc = check(&source).expect("valid");
    assert_eq!(emit(&doc), source);
    let translucent = sprite_with_color("\"#3a7bd580\"");
    assert_eq!(emit(&check(&translucent).expect("valid")), translucent);
}

#[test]
fn an_inexact_legacy_channel_keeps_its_float_spelling() {
    let source = sprite_with_color("\n      - 0.5\n      - 0.0\n      - 0.0\n      - 1.0");
    assert_eq!(emit(&check(&source).expect("valid")), source);
}

#[test]
fn an_exact_legacy_colour_re_emits_as_hex() {
    let source = sprite_with_color("\n      - 0.0\n      - 0.0\n      - 0.0\n      - 1.0");
    assert_eq!(
        emit(&check(&source).expect("valid")),
        sprite_with_color("\"#000000\"")
    );
}

#[test]
fn the_writer_quotes_a_hex_colour() {
    assert_eq!(crate::scalar::scalar_string("#ff0000"), "\"#ff0000\"");
}

// --- Mirror and patches --------------------------------------------------------------------------

/// A sprite of the given size with glyphs `A` and `B` and the frames text spliced in verbatim.
fn mirror_doc(width: u32, height: u32, frames: &str) -> String {
    format!(
        "format: 1\nkind: sprite\nsize:\n  - {width}\n  - {height}\npalette:\n  A:\n    color: \
         \"#ff0000\"\n  B:\n    color: \"#00ff00\"\nframes:\n{frames}"
    )
}

fn resolved(source: &str, frame: &str) -> Vec<String> {
    let doc = check(source).expect("the sprite is valid");
    let frame = doc.frame(frame).expect("the frame exists");
    doc.frame_rows(frame).into_owned()
}

#[test]
fn mirror_x_expands_an_even_and_an_odd_width() {
    let even = mirror_doc(4, 1, "  f:\n    mirror: x\n    rows:\n      - \"AB\"\n");
    assert_eq!(resolved(&even, "f"), ["ABBA"]);
    let odd = mirror_doc(5, 1, "  f:\n    mirror: x\n    rows:\n      - \"A.B\"\n");
    assert_eq!(resolved(&odd, "f"), ["A.B.A"]);
}

#[test]
fn mirror_y_shares_the_centre_row_of_an_odd_height() {
    let source = mirror_doc(
        2,
        3,
        "  f:\n    mirror: y\n    rows:\n      - \"AB\"\n      - \"B.\"\n",
    );
    assert_eq!(resolved(&source, "f"), ["AB", "B.", "AB"]);
}

#[test]
fn mirror_xy_reflects_both_axes() {
    let source = mirror_doc(
        4,
        3,
        "  f:\n    mirror: xy\n    rows:\n      - \"AB\"\n      - \".B\"\n",
    );
    assert_eq!(resolved(&source, "f"), ["ABBA", ".BB.", "ABBA"]);
}

#[test]
fn a_one_wide_x_mirror_is_the_identity() {
    let source = mirror_doc(
        1,
        2,
        "  f:\n    mirror: x\n    rows:\n      - \"A\"\n      - \"B\"\n",
    );
    assert_eq!(resolved(&source, "f"), ["A", "B"]);
}

#[test]
fn patches_keep_on_space_clear_on_dot_and_write_a_glyph() {
    let source = mirror_doc(
        4,
        2,
        "  base:\n    mirror: x\n    rows:\n      - \"AB\"\n      - \"BA\"\n  hit:\n    base: base\n    \
         patches:\n      - at:\n          - 1\n          - 0\n        rows:\n          - \" .B\"\n",
    );
    // Column 1 is kept (space), column 2 is cleared (`.`), column 3 gets `B`.
    assert_eq!(resolved(&source, "hit"), ["AB.B", "BAAB"]);
}

#[test]
fn a_patch_can_break_the_symmetry_of_a_mirrored_base() {
    let source = mirror_doc(
        4,
        1,
        "  base:\n    mirror: x\n    rows:\n      - \"AB\"\n  odd:\n    base: base\n    patches:\n      - at:\n          - 0\n          - 0\n        rows:\n          - \"B\"\n",
    );
    assert_eq!(resolved(&source, "base"), ["ABBA"]);
    assert_eq!(resolved(&source, "odd"), ["BBBA"]);
}

fn diagnostic_at<'a>(diagnostics: &'a [Diagnostic], path: &str) -> &'a Diagnostic {
    let found = find(diagnostics, path);
    assert_eq!(found.code.as_str(), "L1153", "{found:?}");
    found
}

#[test]
fn a_half_width_mismatch_names_the_expected_width() {
    let source = mirror_doc(7, 1, "  f:\n    mirror: x\n    rows:\n      - \"AAAAA\"\n");
    let diagnostics = errors(&source);
    let found = diagnostic_at(&diagnostics, "/frames/f/rows/0");
    assert!(
        found.message.contains("5 characters wide"),
        "{}",
        found.message
    );
    assert!(found.message.contains("expected 4"), "{}", found.message);
    assert!(found.message.contains("centre"), "{}", found.message);
}

#[test]
fn a_half_height_mismatch_names_the_expected_rows() {
    let source = mirror_doc(
        1,
        5,
        "  f:\n    mirror: y\n    rows:\n      - \"A\"\n      - \"A\"\n",
    );
    let diagnostics = errors(&source);
    let found = diagnostic_at(&diagnostics, "/frames/f/rows");
    assert!(found.message.contains("expected 3"), "{}", found.message);
}

#[test]
fn a_glyph_in_a_mirrored_half_still_needs_a_palette_entry() {
    let source = mirror_doc(2, 1, "  f:\n    mirror: x\n    rows:\n      - \"Z\"\n");
    let diagnostics = errors(&source);
    diagnostic_at(&diagnostics, "/frames/f/rows/0");
}

const PATCH_BASE: &str = "  a:\n    rows:\n      - \"AB\"\n      - \"BA\"\n";

#[test]
fn a_base_naming_no_frame_suggests_the_nearest() {
    let frames = format!(
        "{PATCH_BASE}  b:\n    base: aa\n    patches:\n      - at:\n          - 0\n          - 0\n        rows:\n          - \"B\"\n"
    );
    let diagnostics = errors(&mirror_doc(2, 2, &frames));
    let found = diagnostic_at(&diagnostics, "/frames/b/base");
    assert!(
        found.help.as_deref().is_some_and(|h| h.contains("`a`")),
        "{found:?}"
    );
}

#[test]
fn a_base_that_is_patched_is_refused_including_itself() {
    let frames = format!(
        "{PATCH_BASE}  b:\n    base: a\n    patches:\n      - at:\n          - 0\n          - 0\n        rows:\n          - \"B\"\n  c:\n    base: b\n    patches:\n      - at:\n          - 0\n          - 0\n        rows:\n          - \"B\"\n  d:\n    base: d\n    patches:\n      - at:\n          - 0\n          - 0\n        rows:\n          - \"B\"\n"
    );
    let diagnostics = errors(&mirror_doc(2, 2, &frames));
    let chained = diagnostic_at(&diagnostics, "/frames/c/base");
    assert!(
        chained
            .help
            .as_deref()
            .is_some_and(|h| h.contains("do not chain")),
        "{chained:?}"
    );
    diagnostic_at(&diagnostics, "/frames/d/base");
}

#[test]
fn a_patch_outside_its_frame_is_refused() {
    let frames = format!(
        "{PATCH_BASE}  b:\n    base: a\n    patches:\n      - at:\n          - 2\n          - 0\n        rows:\n          - \"B\"\n      - at:\n          - 1\n          - 0\n        rows:\n          - \"BB\"\n      - at:\n          - 0\n          - 1\n        rows:\n          - \"B\"\n          - \"B\"\n      - at:\n          - 0\n          - 0\n        rows:\n          - \"Z\"\n"
    );
    let diagnostics = errors(&mirror_doc(2, 2, &frames));
    diagnostic_at(&diagnostics, "/frames/b/patches/0/at");
    diagnostic_at(&diagnostics, "/frames/b/patches/1/rows/0");
    diagnostic_at(&diagnostics, "/frames/b/patches/2/rows/1");
    diagnostic_at(&diagnostics, "/frames/b/patches/3/rows/0");
}

#[test]
fn an_empty_patch_list_and_an_empty_patch_are_refused() {
    let frames = format!(
        "{PATCH_BASE}  b:\n    base: a\n    patches: []\n  c:\n    base: a\n    patches:\n      - at:\n          - 0\n          - 0\n        rows: []\n"
    );
    let diagnostics = errors(&mirror_doc(2, 2, &frames));
    diagnostic_at(&diagnostics, "/frames/b/patches");
    diagnostic_at(&diagnostics, "/frames/c/patches/0/rows");
}

#[test]
fn structural_frame_mistakes_are_l1153_at_the_frame() {
    let patch =
        "patches:\n      - at:\n          - 0\n          - 0\n        rows:\n          - \"B\"\n";
    let cases = [
        format!("  f:\n    rows:\n      - \"AB\"\n      - \"BA\"\n    base: f\n    {patch}"),
        format!("{PATCH_BASE}  f:\n    base: a\n    mirror: x\n    {patch}"),
        format!("{PATCH_BASE}  f:\n    rows:\n      - \"AB\"\n      - \"BA\"\n    {patch}"),
        format!("{PATCH_BASE}  f:\n    base: a\n"),
        "  f:\n    pivot:\n      - 0.0\n      - 0.0\n".to_string(),
    ];
    for frames in cases {
        let Err(diagnostics) = parse(&mirror_doc(2, 2, &frames)) else {
            panic!("expected a parse failure for:\n{frames}");
        };
        let found = diagnostic_at(&diagnostics, "/frames/f");
        assert!(!found.message.is_empty());
    }
}

#[test]
fn an_unknown_mirror_keyword_is_l1153_with_a_suggestion() {
    let source = mirror_doc(2, 1, "  f:\n    mirror: xx\n    rows:\n      - \"A\"\n");
    let Err(diagnostics) = parse(&source) else {
        panic!("expected a parse failure");
    };
    diagnostic_at(&diagnostics, "/frames/f/mirror");
}

#[test]
fn mirror_and_patches_round_trip_through_the_writer() {
    let frames = "  base:\n    mirror: xy\n    rows:\n      - AB\n      - B.\n  hit:\n    base: base\n    patches:\n      - at:\n          - 1\n          - 0\n        rows:\n          - \" B\"\n          - \"  \"\n          - \"   \"\n";
    let source = mirror_doc(4, 4, frames);
    let doc = check(&source).expect("the sprite is valid");
    let emitted = emit(&doc);
    assert_eq!(emitted, source, "{emitted}");
    assert_eq!(parse(&emitted).expect("re-parses"), doc);
}

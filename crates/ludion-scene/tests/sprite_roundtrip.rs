//! Round-trip and canonical-form properties for sprite documents (`sprites/*.sprite.yaml`) — the
//! sprite analog of `voxel_roundtrip.rs`:
//!
//! - **value round trip**: document → emit → check loses nothing;
//! - **emit idempotence**: `emit_sprite(parse_sprite(emit_sprite(d)))` is *byte-equal* to
//!   `emit_sprite(d)`;
//! - the emitted text is LF-only and validates cleanly;
//! - the committed fixtures pin the exact canonical text, full-featured and minimal;
//! - every semantic rule of the broken fixture is reported with its JSON Pointer.

use ludion_scene::{
    GradientBlend, GradientDirection, OutlineSide, SpriteDocument, SpriteEffect, SpriteEffectOp,
    SpriteFrame, SpritePaletteEntry, SpritePaletteInclude, check_sprite, emit_sprite, parse_sprite,
    validate_sprite,
};
use proptest::prelude::*;
#[path = "support/deterministic.rs"]
mod deterministic;
use deterministic::deterministic_config;

const GOOD: &str = include_str!("fixtures/good_sprite.sprite.yaml");
const BROKEN: &str = include_str!("fixtures/broken_sprite.sprite.yaml");

// --- Fixtures -----------------------------------------------------------------------------------

#[test]
fn the_full_featured_fixture_is_a_fixed_point() {
    let doc = check_sprite(GOOD).expect("the fixture is a valid sprite");
    assert_eq!(emit_sprite(&doc), GOOD);
}

#[test]
fn the_full_featured_fixture_uses_every_op_once() {
    let doc = check_sprite(GOOD).expect("the fixture is a valid sprite");
    let ops: Vec<&SpriteEffectOp> = doc.effects.iter().map(|effect| &effect.op).collect();
    assert_eq!(ops.len(), 8, "one entry per op: {ops:?}");
    assert!(matches!(ops[0], SpriteEffectOp::Gradient { .. }));
    assert!(matches!(ops[1], SpriteEffectOp::Outline { .. }));
    assert!(matches!(ops[2], SpriteEffectOp::Shade { .. }));
    assert!(matches!(ops[3], SpriteEffectOp::Shadow { .. }));
    assert!(matches!(ops[4], SpriteEffectOp::Dither { .. }));
    assert!(matches!(ops[5], SpriteEffectOp::Noise { .. }));
    assert!(matches!(ops[6], SpriteEffectOp::Blur { .. }));
    assert!(matches!(ops[7], SpriteEffectOp::Glow { .. }));
}

#[test]
fn a_minimal_document_is_a_fixed_point() {
    let source = "\
format: 1
kind: sprite
size:
  - 2
  - 1
palette:
  A:
    color: \"#ffffff\"
frames:
  idle:
    rows:
      - A.
";
    let doc = check_sprite(source).expect("the minimal sprite is valid");
    assert_eq!(emit_sprite(&doc), source);
    assert!(doc.effects.is_empty());
    assert_eq!(doc.frames[0].pivot, None);
}

#[test]
fn the_broken_fixture_parses_and_then_fails_every_semantic_rule() {
    // It parses: every defect here is a *meaning* defect, so this fixture is what proves the
    // parse/validate split is real rather than one pass wearing two names.
    let doc = parse_sprite(BROKEN).expect("the broken fixture is structurally well-formed");
    let diagnostics = validate_sprite(&doc);
    let found: Vec<(&str, &str)> = diagnostics
        .iter()
        .map(|d| (d.code.as_str(), d.path.as_deref().unwrap_or("")))
        .collect();
    for expected in [
        ("L1153", "/palette/#/color/2"),   // a colour component above 1
        ("L1153", "/frames/idle/pivot/1"), // a pivot below the frame's bottom edge
        ("L1153", "/frames/idle/rows/0"),  // `Z` names no palette entry
        ("L1153", "/frames/idle/rows/1"),  // three characters against `size` 4
        ("L1153", "/frames/bad~1name"),    // a `/` would make its atlas region name ambiguous
        ("L1154", "/effects/0/glyphs/0"),  // `Z` again, from the effect's filter
        ("L1154", "/effects/0/frames/0"),  // `fly` is not a frame of this document
        ("L1154", "/effects/0/radius"),    // 20 is past the blur ceiling
        ("L1154", "/effects/1/glyphs"),    // a dither with one tone
    ] {
        assert!(
            found.contains(&expected),
            "missing {expected:?}; got {found:#?}",
        );
    }
}

// --- Shared-palette includes --------------------------------------------------------------------

/// A `palette: { from, overrides }` include is canonical text like any other: `from` first, then the
/// overrides glyph-sorted one level deeper than an inline palette would sit.
#[test]
fn an_include_with_overrides_is_a_fixed_point() {
    let source = "\
format: 1
kind: sprite
size:
  - 2
  - 1
palette:
  from: palettes/world.palette.yaml
  overrides:
    A:
      color: \"#ff0000\"
      emissive: 2.0
    B:
      color:
        - 0.0
        - 0.5
        - 1.0
        - 1.0
frames:
  idle:
    rows:
      - AB
";
    let doc = check_sprite(source).expect("an include validates without the palette in hand");
    assert_eq!(emit_sprite(&doc), source);
    let include = doc
        .palette_include
        .expect("the `from` form parses as an include");
    assert_eq!(include.from, "palettes/world.palette.yaml");
    assert_eq!(include.overrides.len(), 2);
    // The inline palette stays empty until the include is resolved — the two are exclusive.
    assert!(doc.palette.is_empty());
}

/// An include with nothing to override omits the `overrides:` key entirely rather than writing an
/// empty mapping.
#[test]
fn a_bare_include_is_a_fixed_point() {
    let source = "\
format: 1
kind: sprite
size:
  - 1
  - 1
palette:
  from: world.palette.yaml
frames:
  idle:
    rows:
      - A
";
    let doc = check_sprite(source).expect("a bare include validates");
    assert_eq!(emit_sprite(&doc), source);
    assert!(
        doc.palette_include
            .expect("an include")
            .overrides
            .is_empty()
    );
}

/// An include's own defects are `L1147`, pointed inside the include block — a distinct concern from
/// the sprite's inline palette (`L1153`), because the fix is in a different file.
#[test]
fn an_include_reports_its_own_defects_as_l1147() {
    let source = "\
format: 1
kind: sprite
size:
  - 1
  - 1
palette:
  from: \"\"
  overrides:
    AB:
      color: \"#ffffff\"
frames:
  idle:
    rows:
      - A
";
    let doc = parse_sprite(source).expect("the include is structurally well-formed");
    let diagnostics = validate_sprite(&doc);
    let found: Vec<(&str, &str)> = diagnostics
        .iter()
        .map(|d| (d.code.as_str(), d.path.as_deref().unwrap_or("")))
        .collect();
    for expected in [
        ("L1147", "/palette/from"),         // an empty path names no document
        ("L1147", "/palette/overrides/AB"), // a two-character glyph
    ] {
        assert!(
            found.contains(&expected),
            "missing {expected:?}; got {found:#?}"
        );
    }
}

// --- Strategies ---------------------------------------------------------------------------------

/// The glyph pool, in ascending char order (so a subset taken in order is already sorted — the
/// canonical palette order the emitter writes).
const GLYPH_POOL: [char; 5] = ['#', '1', 'A', 'B', 'o'];

fn unit_f64() -> impl Strategy<Value = f64> {
    0.0f64..=1.0f64
}

/// Arbitrary unit floats (written back as the legacy sequence) or a hex-exact colour (written as
/// hex), so both writer branches run.
fn color() -> impl Strategy<Value = [f64; 4]> {
    prop_oneof![
        (unit_f64(), unit_f64(), unit_f64(), unit_f64()).prop_map(|(r, g, b, a)| [r, g, b, a]),
        (any::<u8>(), any::<u8>(), any::<u8>(), any::<u8>()).prop_map(|(r, g, b, a)| {
            let hex = format!("#{r:02x}{g:02x}{b:02x}{a:02x}");
            ludion_core::color::parse_hex(&hex)
                .expect("eight hex digits")
                .map(f64::from)
        }),
    ]
}

/// Effect ops whose parameters are valid for any document: no glyph or frame filter is attached, so
/// nothing here can dangle. `dither` and `shadow` carry cross-references and range rules the
/// fixtures pin instead.
fn effect_op() -> impl Strategy<Value = SpriteEffectOp> {
    prop_oneof![
        (
            color(),
            color(),
            prop_oneof![
                Just(GradientDirection::Vertical),
                Just(GradientDirection::Horizontal),
                Just(GradientDirection::Radial),
            ],
            prop_oneof![Just(GradientBlend::Replace), Just(GradientBlend::Multiply)],
        )
            .prop_map(|(from, to, direction, blend)| SpriteEffectOp::Gradient {
                from,
                to,
                direction,
                blend,
            }),
        (
            color(),
            prop_oneof![Just(OutlineSide::Outside), Just(OutlineSide::Inside)],
        )
            .prop_map(|(color, side)| SpriteEffectOp::Outline { color, side }),
        unit_f64().prop_map(|amount| SpriteEffectOp::Noise { amount }),
        (1u32..=8).prop_map(|radius| SpriteEffectOp::Blur { radius }),
        (proptest::option::of(color()), 0.001f64..=32.0, 0.0f64..=4.0,).prop_map(
            |(color, radius_px, strength)| SpriteEffectOp::Glow {
                color,
                radius_px,
                strength,
            }
        ),
    ]
}

/// A structurally consistent [`SpriteDocument`]: a sorted, unique, single-glyph palette with
/// in-range colors; `size` in `1..=4` by `1..=3`; an optional name, seed and in-frame pivot; rows
/// drawn only from the chosen glyphs plus the transparent `.`; and filter-free effects — exactly
/// the properties `validate_sprite` enforces.
fn sprite_doc() -> impl Strategy<Value = SpriteDocument> {
    (
        1u32..32,
        1u32..=4,
        1u32..=3,
        proptest::option::of("[a-z][a-z0-9-]{0,6}"),
        proptest::option::of(any::<u64>()),
        1usize..=3,
        proptest::collection::vec(effect_op(), 0..3),
    )
        .prop_flat_map(|(mask, sx, sy, name, seed, frame_count, effects)| {
            let glyphs: Vec<char> = GLYPH_POOL
                .iter()
                .enumerate()
                .filter(|(i, _)| mask & (1 << i) != 0)
                .map(|(_, c)| *c)
                .collect();
            let mut choices = glyphs.clone();
            choices.push('.');
            let colors = proptest::collection::vec(
                (color(), proptest::option::of(0.0f64..8.0)),
                glyphs.len(),
            );
            let cells = proptest::collection::vec(
                proptest::sample::select(choices),
                (sx * sy) as usize * frame_count,
            );
            let pivots = proptest::collection::vec(
                proptest::option::of((0.0f64..=f64::from(sx), 0.0f64..=f64::from(sy))),
                frame_count,
            );
            (
                Just(glyphs),
                Just([sx, sy]),
                Just(name),
                Just(seed),
                Just(frame_count),
                Just(effects),
                colors,
                cells,
                pivots,
            )
        })
        .prop_map(
            |(glyphs, size, name, seed, frame_count, ops, colors, cells, pivots)| {
                let palette = glyphs
                    .iter()
                    .zip(colors)
                    .map(|(g, (color, emissive))| SpritePaletteEntry {
                        glyph: g.to_string(),
                        color,
                        emissive,
                    })
                    .collect();
                let [sx, sy] = size;
                let mut cells = cells.into_iter();
                let frames = (0..frame_count)
                    .map(|index| SpriteFrame {
                        name: format!("frame-{index}"),
                        pivot: pivots[index].map(|(px, py)| [px, py]),
                        rows: (0..sy)
                            .map(|_| {
                                (0..sx)
                                    .map(|_| cells.next().expect("one cell per texel"))
                                    .collect()
                            })
                            .collect(),
                    })
                    .collect();
                let effects = ops
                    .into_iter()
                    .map(|op| SpriteEffect {
                        op,
                        glyphs: None,
                        frames: None,
                    })
                    .collect();
                SpriteDocument {
                    name,
                    size,
                    seed,
                    palette,
                    palette_include: None,
                    frames,
                    effects,
                }
            },
        )
}

/// `sprite_doc()`, with the palette moved wholesale into a `palette: { from, overrides }` include
/// half the time — so every round-trip property below covers both palette forms. Moving it preserves
/// validity: an unresolved include's glyphs are not checked against the frames (only the resolved
/// document faces that), and the entries are already glyph-sorted.
fn sprite_doc_either_palette() -> impl Strategy<Value = SpriteDocument> {
    (
        sprite_doc(),
        proptest::option::of(proptest::sample::select(vec![
            "world.palette.yaml",
            "palettes/world.palette.yaml",
            "../shared/world.palette.yaml",
        ])),
    )
        .prop_map(|(mut doc, from)| {
            if let Some(from) = from {
                doc.palette_include = Some(SpritePaletteInclude {
                    from: from.to_string(),
                    overrides: std::mem::take(&mut doc.palette),
                });
            }
            doc
        })
}

// --- Properties ---------------------------------------------------------------------------------

proptest! {
    #![proptest_config(deterministic_config())]
    #[test]
    fn emit_then_check_loses_nothing(doc in sprite_doc_either_palette()) {
        let text = emit_sprite(&doc);
        let checked = check_sprite(&text).expect("canonical output must check clean");
        prop_assert_eq!(checked, doc);
    }

    #[test]
    fn emit_is_idempotent_byte_for_byte(doc in sprite_doc_either_palette()) {
        let first = emit_sprite(&doc);
        let reparsed = parse_sprite(&first).expect("canonical output must parse cleanly");
        let second = emit_sprite(&reparsed);
        prop_assert_eq!(&second, &first);
        let third = emit_sprite(&parse_sprite(&second).unwrap());
        prop_assert_eq!(&third, &first);
    }

    #[test]
    fn emitted_text_is_lf_only_and_validates(doc in sprite_doc_either_palette()) {
        let text = emit_sprite(&doc);
        prop_assert!(!text.contains('\r'), "canonical output must use LF only");
        prop_assert!(text.ends_with('\n'), "canonical output ends with a newline");
        let parsed = parse_sprite(&text).expect("canonical output must parse cleanly");
        let diagnostics = validate_sprite(&parsed);
        prop_assert!(
            diagnostics.is_empty(),
            "canonical output of a consistent sprite must validate: {:?}",
            diagnostics.iter().map(ToString::to_string).collect::<Vec<_>>()
        );
    }
}

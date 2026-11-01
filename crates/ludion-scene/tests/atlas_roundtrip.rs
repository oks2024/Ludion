//! Round-trip and canonical-form properties for the two atlas document families
//! (`*.atlas-source.yaml`, `*.atlas.yaml`) — the sibling of `sprite_roundtrip.rs`:
//!
//! - **value round trip**: document → emit → check loses nothing;
//! - **emit idempotence**: emitting an emitted document is byte-equal;
//! - the committed fixtures pin the exact canonical text, and the pair is consistent with each
//!   other (the atlas carries every region the source's flipbooks name);
//! - regions emit in **name** order, whatever order they were inserted in.

use std::collections::BTreeMap;

use ludion_scene::{
    AtlasDocument, AtlasFilter, AtlasRegion, AtlasSourceDocument, AtlasSourceInput, Flipbook,
    SpritePaletteEntry, check_atlas, check_atlas_source, emit_atlas, emit_atlas_source,
    parse_atlas, parse_atlas_source, validate_atlas, validate_atlas_source,
};
use proptest::prelude::*;
#[path = "support/deterministic.rs"]
mod deterministic;
use deterministic::deterministic_config;

const SOURCE: &str = include_str!("fixtures/good_atlas_source.atlas-source.yaml");
const ATLAS: &str = include_str!("fixtures/good_atlas.atlas.yaml");

// --- Fixtures -----------------------------------------------------------------------------------

#[test]
fn the_source_fixture_is_a_fixed_point() {
    let doc = check_atlas_source(SOURCE).expect("the fixture is a valid atlas source");
    assert_eq!(emit_atlas_source(&doc), SOURCE);
    assert_eq!(doc.name.as_deref(), Some("ship"));
    assert_eq!(doc.filter, AtlasFilter::Nearest);
    assert_eq!(doc.inputs.len(), 2);
}

#[test]
fn the_atlas_fixture_is_a_fixed_point() {
    let doc = check_atlas(ATLAS).expect("the fixture is a valid atlas");
    assert_eq!(emit_atlas(&doc), ATLAS);
    assert_eq!(doc.regions.len(), 3);
    assert_eq!(doc.region("ship/idle").map(|r| r.rect), Some([0, 0, 8, 6]));
}

#[test]
fn the_two_fixtures_agree_with_each_other() {
    let source = check_atlas_source(SOURCE).expect("the fixture is a valid atlas source");
    let atlas = check_atlas(ATLAS).expect("the fixture is a valid atlas");
    // The source names the file the atlas claims to come from, modulo the fixture's own file name.
    assert!(atlas.generated_from.ends_with(".atlas-source.yaml"));
    assert_eq!(source.pixels_per_unit, atlas.pixels_per_unit);
    // Every region the source's flipbooks name exists on the baked side. The atlas validator
    // enforces this for its own flipbooks; the source cannot, so the pair is checked here.
    for flipbook in &source.flipbooks {
        for frame in &flipbook.frames {
            assert!(
                atlas.region(frame).is_some(),
                "the atlas has no region named {frame}",
            );
        }
    }
}

#[test]
fn a_did_you_mean_answers_a_near_miss_region_name() {
    let atlas = check_atlas(ATLAS).expect("the fixture is a valid atlas");
    assert_eq!(atlas.nearest_region_name("ship/idel"), Some("ship/idle"));
    assert_eq!(atlas.nearest_region_name("explosionn"), Some("explosion"));
    assert_eq!(atlas.nearest_region_name("nothing-like-it"), None);
}

#[test]
fn regions_emit_in_name_order_whatever_order_they_were_inserted_in() {
    let region = AtlasRegion {
        rect: [0, 0, 1, 1],
        pivot: [0.0, 0.0],
    };
    let mut regions = BTreeMap::new();
    for name in ["z/last", "a/first", "m/middle"] {
        regions.insert(name.to_string(), region);
    }
    let doc = AtlasDocument {
        generated_from: "x.atlas-source.yaml".to_string(),
        pixels_per_unit: 8.0,
        color_map: "x.png".to_string(),
        filter: AtlasFilter::Nearest,
        regions,
        flipbooks: Vec::new(),
    };
    let text = emit_atlas(&doc);
    let first = text.find("a/first").expect("emitted");
    let middle = text.find("m/middle").expect("emitted");
    let last = text.find("z/last").expect("emitted");
    assert!(first < middle && middle < last, "{text}");
}

// --- Strategies ---------------------------------------------------------------------------------

fn filter() -> impl Strategy<Value = AtlasFilter> {
    prop_oneof![Just(AtlasFilter::Nearest), Just(AtlasFilter::Linear)]
}

/// A string from `pattern` whose every `/` segment is a name a file can have on Windows: the
/// validators refuse a device name such as `con` or `aux.png`.
fn portable(pattern: &'static str) -> impl Strategy<Value = String> {
    pattern.prop_filter("a Windows device name", |s| {
        !s.split('/').any(ludion_core::paths::is_reserved_device)
    })
}

fn region_names() -> impl Strategy<Value = Vec<String>> {
    proptest::collection::vec(portable("[a-z]{1,4}/[a-z]{1,4}"), 1..4).prop_map(|mut names| {
        names.sort();
        names.dedup();
        names
    })
}

/// A consistent [`AtlasDocument`]: every rect at least one texel on each axis, every pivot inside
/// its own region, and every flipbook naming regions this atlas has.
fn atlas_doc() -> impl Strategy<Value = AtlasDocument> {
    (
        region_names(),
        0.5f64..64.0,
        filter(),
        portable("[a-z]{1,6}[.]atlas-source[.]yaml"),
        portable("[a-z]{1,6}[.]png"),
        any::<bool>(),
    )
        .prop_flat_map(
            |(names, pixels_per_unit, filter, generated_from, color_map, looped)| {
                let rects = proptest::collection::vec(
                    (0u32..64, 0u32..64, 1u32..32, 1u32..32),
                    names.len(),
                );
                let unit = proptest::collection::vec((0.0f64..=1.0, 0.0f64..=1.0), names.len());
                (
                    Just(names),
                    Just(pixels_per_unit),
                    Just(filter),
                    Just(generated_from),
                    Just(color_map),
                    Just(looped),
                    rects,
                    unit,
                )
            },
        )
        .prop_map(
            |(names, pixels_per_unit, filter, generated_from, color_map, looped, rects, unit)| {
                let mut regions = BTreeMap::new();
                for ((name, (x, y, w, h)), (u, v)) in names.iter().zip(rects).zip(unit) {
                    regions.insert(
                        name.clone(),
                        AtlasRegion {
                            rect: [x, y, w, h],
                            // A fraction of the extent, so the pivot is always inside its region.
                            pivot: [u * f64::from(w), v * f64::from(h)],
                        },
                    );
                }
                let flipbooks = vec![Flipbook {
                    name: "clip".to_string(),
                    frames: names.clone(),
                    fps: 8.0,
                    looped,
                    hold: Some(names.iter().map(|_| 1).collect()),
                }];
                AtlasDocument {
                    generated_from,
                    pixels_per_unit,
                    color_map,
                    filter,
                    regions,
                    flipbooks,
                }
            },
        )
}

/// A hex-exact colour (written as hex) or arbitrary unit floats (written as the legacy sequence),
/// so both writer branches run.
fn color() -> impl Strategy<Value = [f64; 4]> {
    prop_oneof![
        (0.0f64..=1.0, 0.0f64..=1.0, 0.0f64..=1.0, 0.0f64..=1.0)
            .prop_map(|(r, g, b, a)| [r, g, b, a]),
        (any::<u8>(), any::<u8>(), any::<u8>(), any::<u8>()).prop_map(|(r, g, b, a)| {
            let hex = format!("#{r:02x}{g:02x}{b:02x}{a:02x}");
            ludion_core::color::parse_hex(&hex)
                .expect("eight hex digits")
                .map(f64::from)
        }),
    ]
}

/// A glyph-sorted, glyph-unique override palette of 0–2 entries.
fn override_palette() -> impl Strategy<Value = Vec<SpritePaletteEntry>> {
    proptest::collection::btree_map(
        prop_oneof![Just("a"), Just("o"), Just("#")],
        (color(), proptest::option::of(0.0f64..4.0)),
        0..3,
    )
    .prop_map(|entries| {
        entries
            .into_iter()
            .map(|(glyph, (color, emissive))| SpritePaletteEntry {
                glyph: glyph.to_string(),
                color,
                emissive,
            })
            .collect()
    })
}

/// A plain input, or a PNG with an optional `name`, or a sprite document variant. A variant with no
/// name and no palette is a plain input, so it is generated as one.
fn source_input() -> impl Strategy<Value = AtlasSourceInput> {
    prop_oneof![
        portable("[a-z]{1,5}/[a-z]{1,5}[.]png").prop_map(AtlasSourceInput::plain),
        (
            portable("[a-z]{1,5}/[a-z]{1,5}[.]png"),
            portable("[a-z-]{1,6}"),
        )
            .prop_map(|(path, name)| AtlasSourceInput {
                name: Some(name),
                ..AtlasSourceInput::plain(path)
            }),
        (
            portable("[a-z]{1,5}/[a-z]{1,5}[.]sprite[.]yaml"),
            proptest::option::of(portable("[a-z-]{1,6}")),
            override_palette(),
        )
            .prop_map(|(path, name, palette)| AtlasSourceInput {
                path,
                name,
                palette,
            }),
    ]
}

/// A consistent [`AtlasSourceDocument`]. Flipbook frame names are unconstrained here on purpose:
/// this family deliberately does not resolve them (only the bake has read the inputs).
fn atlas_source_doc() -> impl Strategy<Value = AtlasSourceDocument> {
    (
        proptest::option::of(portable("[a-z]{1,6}")),
        0.5f64..64.0,
        filter(),
        proptest::collection::vec(source_input(), 1..4),
        proptest::collection::vec(portable("[a-z]{1,5}/[a-z]{1,5}"), 1..4),
        any::<bool>(),
    )
        .prop_map(|(name, pixels_per_unit, filter, inputs, frames, looped)| {
            AtlasSourceDocument {
                name,
                pixels_per_unit,
                filter,
                inputs,
                flipbooks: vec![Flipbook {
                    name: "clip".to_string(),
                    frames,
                    fps: 12.0,
                    looped,
                    hold: None,
                }],
            }
        })
}

// --- Properties ---------------------------------------------------------------------------------

proptest! {
    #![proptest_config(deterministic_config())]
    #[test]
    fn an_atlas_survives_emit_then_check(doc in atlas_doc()) {
        let text = emit_atlas(&doc);
        let checked = check_atlas(&text).expect("canonical output must check clean");
        prop_assert_eq!(checked, doc);
    }

    #[test]
    fn an_atlas_emit_is_idempotent_byte_for_byte(doc in atlas_doc()) {
        let first = emit_atlas(&doc);
        let second = emit_atlas(&parse_atlas(&first).expect("canonical output parses"));
        prop_assert_eq!(&second, &first);
        prop_assert!(!first.contains('\r'));
        prop_assert!(validate_atlas(&parse_atlas(&first).unwrap()).is_empty());
    }

    #[test]
    fn an_atlas_source_survives_emit_then_check(doc in atlas_source_doc()) {
        let text = emit_atlas_source(&doc);
        let checked = check_atlas_source(&text).expect("canonical output must check clean");
        prop_assert_eq!(checked, doc);
    }

    #[test]
    fn an_atlas_source_emit_is_idempotent_byte_for_byte(doc in atlas_source_doc()) {
        let first = emit_atlas_source(&doc);
        let second =
            emit_atlas_source(&parse_atlas_source(&first).expect("canonical output parses"));
        prop_assert_eq!(&second, &first);
        prop_assert!(!first.contains('\r'));
        prop_assert!(validate_atlas_source(&parse_atlas_source(&first).unwrap()).is_empty());
    }
}

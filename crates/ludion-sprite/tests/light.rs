//! The `light` op: baked self-light from emissive (or listed) source glyphs onto their neighbours.
//!
//! Exact equality on fixtures built from binary fractions, like `effects.rs`: with `radius_px: 2` the
//! squared distances 1 and 2 weigh exactly 0.75 and 0.5.

use ludion_scene::check_sprite;
use ludion_sprite::{FrameImage, bake, bake_steps};

/// The 4×4 Bayer thresholds `dither` and the ramp ops share, copied so the test states the
/// expectation independently of the bake.
const BAYER: [[u32; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// One palette entry written with float channels, so every colour is an exact binary fraction.
fn entry(glyph: char, color: [f32; 4], emissive: Option<f64>) -> String {
    let mut text = format!("  {glyph}:\n    color:\n");
    for channel in color {
        text.push_str(&format!("      - {channel:?}\n"));
    }
    if let Some(emissive) = emissive {
        text.push_str(&format!("    emissive: {emissive:?}\n"));
    }
    text
}

/// A document over the given `palette` entries, extra top-level blocks, `frames` body and `effects`
/// body, `size` texels wide and high.
fn document(
    size: [u32; 2],
    palette: &[String],
    extra: &str,
    frames: &str,
    effects: &str,
) -> String {
    format!(
        "format: 1\nkind: sprite\nsize:\n  - {}\n  - {}\npalette:\n{}{extra}frames:\n{frames}effects:\n{effects}",
        size[0],
        size[1],
        palette.concat()
    )
}

fn frame(name: &str, rows: &[&str]) -> String {
    let rows: String = rows
        .iter()
        .map(|row| format!("      - \"{row}\"\n"))
        .collect();
    format!("  {name}:\n    rows:\n{rows}")
}

fn baked(source: &str) -> Vec<(String, FrameImage)> {
    let doc = check_sprite(source).expect("the fixture sprite is valid");
    bake(&doc)
        .expect("the fixture sprite bakes")
        .into_iter()
        .map(|frame| (frame.name, frame.image))
        .collect()
}

fn one(source: &str) -> FrameImage {
    let mut frames = baked(source);
    assert_eq!(frames.len(), 1);
    frames.remove(0).1
}

const ORANGE: [f32; 4] = [1.0, 0.5, 0.0, 1.0];
const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

fn orange_source() -> String {
    entry('e', ORANGE, Some(2.0))
}

fn black() -> String {
    entry('b', BLACK, None)
}

fn color_light(extra_keys: &str) -> String {
    format!("  - op: light\n    radius_px: 2.0\n{extra_keys}")
}

#[test]
fn a_source_lights_an_adjacent_receiver_by_the_falloff() {
    let image = one(&document(
        [3, 1],
        &[orange_source(), black()],
        "",
        &frame("f", &["eb."]),
        &color_light("    strength: 0.5\n"),
    ));
    // d² = 1 inside r² = 4: weight 0.75, times strength 0.5, times the source's colour.
    assert_eq!(image.get(1, 0), [0.375, 0.1875, 0.0, 1.0]);
    assert_eq!(image.get(0, 0), ORANGE, "a source is never written");
    assert_eq!(
        image.get(2, 0),
        [0.0; 4],
        "transparent texels stay transparent"
    );
}

#[test]
fn a_receiver_at_the_radius_is_untouched() {
    let image = one(&document(
        [4, 1],
        &[orange_source(), black()],
        "",
        &frame("f", &["ebbb"]),
        &color_light("    strength: 1.0\n"),
    ));
    assert_ne!(image.get(1, 0), BLACK);
    assert_eq!(image.get(2, 0), BLACK, "d² = 4 is not inside r² = 4");
    assert_eq!(image.get(3, 0), BLACK);
}

#[test]
fn color_overrides_the_sources_own_colour_and_ignores_its_alpha() {
    let image = one(&document(
        [3, 1],
        &[orange_source(), black()],
        "",
        &frame("f", &["eb."]),
        &color_light(
            "    strength: 0.5\n    color:\n      - 0.0\n      - 1.0\n      - 0.0\n      - 0.25\n",
        ),
    ));
    assert_eq!(image.get(1, 0), [0.0, 0.375, 0.0, 1.0]);
}

#[test]
fn overlapping_sources_combine_by_per_channel_max_not_sum() {
    let image = one(&document(
        [3, 1],
        &[
            orange_source(),
            black(),
            entry('f', [0.5, 0.0, 1.0, 1.0], Some(1.0)),
        ],
        "",
        &frame("f", &["ebf"]),
        &color_light("    strength: 1.0\n"),
    ));
    // From `e`: [0.75, 0.375, 0]; from `f`: [0.375, 0, 0.75]. A sum would clamp red to 1.
    assert_eq!(image.get(1, 0), [0.75, 0.375, 0.75, 1.0]);
}

#[test]
fn bands_quantize_the_falloff_into_rings() {
    let image = one(&document(
        [3, 2],
        &[orange_source(), black()],
        "",
        &frame("f", &["eb.", ".b."]),
        &color_light("    strength: 1.0\n    bands: 2\n"),
    ));
    // Weights 0.75 and 0.5 land on the rings 1.0 and 0.5.
    assert_eq!(image.get(1, 0), [1.0, 0.5, 0.0, 1.0]);
    assert_eq!(image.get(1, 1), [0.5, 0.25, 0.0, 1.0]);
}

#[test]
fn a_glyph_filter_narrows_the_receivers() {
    let image = one(&document(
        [3, 2],
        &[
            orange_source(),
            black(),
            entry('c', [0.25, 0.25, 0.25, 1.0], None),
        ],
        "",
        &frame("f", &["ec.", "b.."]),
        "  - op: light\n    glyphs:\n      - b\n    radius_px: 2.0\n    strength: 1.0\n",
    ));
    assert_eq!(image.get(0, 1), [0.75, 0.375, 0.0, 1.0]);
    assert_eq!(
        image.get(1, 0),
        [0.25, 0.25, 0.25, 1.0],
        "`c` is not a receiver"
    );
}

#[test]
fn explicit_sources_replace_the_emissive_set() {
    let image = one(&document(
        [3, 1],
        &[
            orange_source(),
            black(),
            entry('f', [0.5, 0.0, 1.0, 1.0], None),
        ],
        "",
        &frame("f", &["ebf"]),
        &color_light("    sources:\n      - f\n    strength: 1.0\n"),
    ));
    // `f` is not emissive but listed; `e` is emissive but not listed, so it only receives.
    assert_eq!(image.get(1, 0), [0.375, 0.0, 0.75, 1.0]);
    assert_eq!(image.get(0, 0), ORANGE);
}

#[test]
fn the_light_follows_its_glyph_through_every_frame_and_mirror() {
    let frames = [
        frame("a", &["ebbb"]),
        frame("b", &["bbbe"]),
        "  m:\n    mirror: x\n    rows:\n      - \"eb\"\n".to_string(),
    ]
    .concat();
    let baked = baked(&document(
        [4, 1],
        &[orange_source(), black()],
        "",
        &frames,
        &color_light("    strength: 1.0\n"),
    ));
    let lit = [0.75, 0.375, 0.0, 1.0];
    let (a, b, m) = (&baked[0].1, &baked[1].1, &baked[2].1);
    assert_eq!((a.get(1, 0), a.get(3, 0)), (lit, BLACK));
    assert_eq!((b.get(2, 0), b.get(0, 0)), (lit, BLACK));
    assert_eq!(
        (m.get(1, 0), m.get(2, 0)),
        (lit, lit),
        "the mirrored source lights its side too"
    );
}

/// Four ramp glyphs, dark to light, and the orange source.
fn ramp_palette() -> Vec<String> {
    vec![
        orange_source(),
        entry('p', [0.0, 0.0, 0.0, 1.0], None),
        entry('q', [0.25, 0.0, 0.0, 1.0], None),
        entry('s', [0.5, 0.0, 0.0, 1.0], None),
        entry('t', [0.75, 0.0, 0.0, 1.0], None),
    ]
}

fn ramp_light(steps: i32) -> String {
    format!("  - op: light\n    mode: ramp\n    radius_px: 2.0\n    steps: {steps}\n")
}

#[test]
fn a_ramp_light_with_a_whole_step_count_lands_on_the_ramp_glyph() {
    let image = one(&document(
        [2, 1],
        &ramp_palette(),
        "ramps:\n  - pqst\n",
        &frame("f", &["ep"]),
        &ramp_light(4),
    ));
    // 0.75 × 4 = 3 whole steps: p → t.
    assert_eq!(image.get(1, 0), [0.75, 0.0, 0.0, 1.0]);
    assert_eq!(image.get(0, 0), ORANGE, "the source is never stepped");
}

#[test]
fn a_ramp_light_clamps_at_the_top_of_the_ramp() {
    let image = one(&document(
        [2, 1],
        &ramp_palette(),
        "ramps:\n  - pqst\n",
        &frame("f", &["ep"]),
        &ramp_light(40),
    ));
    assert_eq!(image.get(1, 0), [0.75, 0.0, 0.0, 1.0]);
}

#[test]
fn a_fractional_ramp_light_resolves_through_the_bayer_matrix() {
    let image = one(&document(
        [3, 2],
        &ramp_palette(),
        "ramps:\n  - pqst\n",
        &frame("f", &["ep.", "pp."]),
        &ramp_light(1),
    ));
    let red = |steps: i32| [0.25 * steps as f32, 0.0, 0.0, 1.0];
    for (x, y, weight) in [(1usize, 0usize, 0.75f32), (0, 1, 0.75), (1, 1, 0.5)] {
        let threshold = BAYER[y % 4][x % 4];
        let bump = i32::from((threshold as f32 + 0.5) / 16.0 < weight);
        assert_eq!(image.get(x as i32, y as i32), red(bump), "texel ({x}, {y})");
    }
    assert_eq!(image.get(2, 0), [0.0; 4]);
}

#[test]
fn the_last_step_equals_the_bake() {
    let source = document(
        [3, 1],
        &[orange_source(), black()],
        "",
        &frame("f", &["eb."]),
        &color_light("    strength: 0.5\n"),
    );
    let doc = check_sprite(&source).expect("valid");
    let steps = bake_steps(&doc).expect("bakes");
    let baked = bake(&doc).expect("bakes");
    assert_eq!(steps[0].steps.last().unwrap().image, baked[0].image);
}

//! The eight effect ops, each on a frame small enough to write the expected texels out by hand.
//!
//! Exact equality, never a tolerance: the bake feeds a committed PNG, so "close enough" is not a
//! property this crate has. Every expected value below is either a literal or an expression in
//! exactly representable binary fractions.

use ludion_scene::{SpriteDocument, check_sprite};
use ludion_sprite::{FrameImage, bake};

/// Bakes a document written inline, expecting it to be valid.
fn baked(source: &str) -> Vec<(String, FrameImage)> {
    let doc: SpriteDocument = check_sprite(source).expect("the fixture sprite is valid");
    bake(&doc)
        .expect("the fixture sprite bakes")
        .into_iter()
        .map(|frame| (frame.name, frame.image))
        .collect()
}

/// The single frame of a single-frame fixture.
fn one(source: &str) -> FrameImage {
    let mut frames = baked(source);
    assert_eq!(frames.len(), 1, "the fixture has one frame");
    frames.remove(0).1
}

#[test]
fn a_glyph_grid_paints_its_palette_colors() {
    let image = one("format: 1
kind: sprite
size:
  - 2
  - 2
palette:
  A:
    color:
      - 1.0
      - 0.0
      - 0.0
      - 1.0
  B:
    color:
      - 0.0
      - 0.5
      - 0.0
      - 0.25
frames:
  idle:
    rows:
      - \"AB\"
      - \".A\"
");
    assert_eq!(image.get(0, 0), [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(image.get(1, 0), [0.0, 0.5, 0.0, 0.25]);
    assert_eq!(image.get(0, 1), [0.0, 0.0, 0.0, 0.0]);
    assert_eq!(image.get(1, 1), [1.0, 0.0, 0.0, 1.0]);
}

#[test]
fn a_frames_filter_scopes_an_effect_to_the_frames_it_names() {
    let frames = baked(
        "format: 1
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
  lit:
    rows:
      - \"A\"
  dark:
    rows:
      - \"A\"
effects:
  - op: gradient
    frames:
      - dark
    from:
      - 0.0
      - 0.0
      - 0.0
      - 1.0
    to:
      - 0.0
      - 0.0
      - 0.0
      - 1.0
",
    );
    assert_eq!(frames[0].0, "lit");
    assert_eq!(frames[0].1.get(0, 0), [1.0, 1.0, 1.0, 1.0]);
    assert_eq!(frames[1].0, "dark");
    assert_eq!(frames[1].1.get(0, 0), [0.0, 0.0, 0.0, 1.0]);
}

#[test]
fn a_vertical_gradient_ramps_over_the_target_box_and_replaces() {
    let image = one("format: 1
kind: sprite
size:
  - 1
  - 3
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
      - \"A\"
      - \"A\"
      - \"A\"
effects:
  - op: gradient
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
");
    assert_eq!(image.get(0, 0), [0.0, 0.0, 0.0, 1.0]);
    assert_eq!(image.get(0, 1), [0.5, 0.5, 0.5, 1.0]);
    assert_eq!(image.get(0, 2), [1.0, 1.0, 1.0, 1.0]);
}

#[test]
fn a_horizontal_multiply_gradient_scales_every_channel() {
    let image = one("format: 1
kind: sprite
size:
  - 2
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
      - \"AA\"
effects:
  - op: gradient
    direction: horizontal
    blend: multiply
    from:
      - 1.0
      - 1.0
      - 1.0
      - 1.0
    to:
      - 0.0
      - 0.0
      - 0.0
      - 0.0
");
    assert_eq!(image.get(0, 0), [1.0, 1.0, 1.0, 1.0]);
    assert_eq!(image.get(1, 0), [0.0, 0.0, 0.0, 0.0]);
}

#[test]
fn an_outside_outline_rings_the_silhouette_without_its_corners() {
    let image = one("format: 1
kind: sprite
size:
  - 3
  - 3
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
      - \"...\"
      - \".A.\"
      - \"...\"
effects:
  - op: outline
    color:
      - 0.0
      - 0.0
      - 1.0
      - 1.0
");
    let blue = [0.0, 0.0, 1.0, 1.0];
    assert_eq!(image.get(1, 1), [1.0, 1.0, 1.0, 1.0]);
    for (x, y) in [(1, 0), (0, 1), (2, 1), (1, 2)] {
        assert_eq!(image.get(x, y), blue, "4-neighbour ({x}, {y}) is outlined");
    }
    for (x, y) in [(0, 0), (2, 0), (0, 2), (2, 2)] {
        assert_eq!(image.get(x, y), [0.0; 4], "corner ({x}, {y}) is untouched");
    }
}

#[test]
fn an_inside_outline_treats_the_frame_edge_as_outside() {
    let image = one("format: 1
kind: sprite
size:
  - 3
  - 3
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
      - \"AAA\"
      - \"AAA\"
      - \"AAA\"
effects:
  - op: outline
    side: inside
    color:
      - 0.0
      - 0.0
      - 1.0
      - 1.0
");
    assert_eq!(image.get(1, 1), [1.0, 1.0, 1.0, 1.0]);
    for y in 0..3 {
        for x in 0..3 {
            if (x, y) != (1, 1) {
                assert_eq!(image.get(x, y), [0.0, 0.0, 1.0, 1.0], "ring ({x}, {y})");
            }
        }
    }
}

#[test]
fn shade_lightens_the_edge_toward_the_light_and_darkens_the_one_away() {
    let image = one("format: 1
kind: sprite
size:
  - 3
  - 1
palette:
  A:
    color:
      - 0.5
      - 0.5
      - 0.5
      - 1.0
frames:
  idle:
    rows:
      - \"AAA\"
effects:
  - op: shade
    light:
      - 1.0
      - 0.0
    strength: 0.5
");
    assert_eq!(
        image.get(0, 0),
        [0.25, 0.25, 0.25, 1.0],
        "away from the light"
    );
    assert_eq!(image.get(1, 0), [0.5, 0.5, 0.5, 1.0], "interior");
    assert_eq!(image.get(2, 0), [0.75, 0.75, 0.75, 1.0], "toward the light");
}

#[test]
fn a_shadow_offset_past_the_i32_range_casts_nothing() {
    let image = one("format: 1
kind: sprite
size:
  - 3
  - 3
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
      - \"AA.\"
      - \"AA.\"
      - \"...\"
effects:
  - op: shadow
    offset:
      - 2147483647
      - 0
    color:
      - 0.0
      - 0.0
      - 0.0
      - 0.5
");
    assert_eq!(image.get(0, 0), [1.0, 1.0, 1.0, 1.0]);
    assert_eq!(image.get(2, 0), [0.0; 4], "no shadow from the huge offset");
    assert_eq!(image.get(2, 1), [0.0; 4]);
}

#[test]
fn a_shadow_lands_only_where_the_frame_is_transparent() {
    let image = one("format: 1
kind: sprite
size:
  - 3
  - 3
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
      - \"AA.\"
      - \"...\"
      - \"...\"
effects:
  - op: shadow
    offset:
      - 1
      - 1
    color:
      - 0.0
      - 0.0
      - 0.0
      - 0.5
");
    let shade = [0.0, 0.0, 0.0, 0.5];
    assert_eq!(
        image.get(0, 0),
        [1.0, 1.0, 1.0, 1.0],
        "the sprite is untouched"
    );
    assert_eq!(image.get(1, 0), [1.0, 1.0, 1.0, 1.0]);
    assert_eq!(image.get(1, 1), shade);
    assert_eq!(image.get(2, 1), shade);
    assert_eq!(image.get(0, 1), [0.0; 4], "nothing casts here");
}

/// A 4x4 sprite of all `A`, dithered `A` -> `B` at `density`.
fn dithered(density: &str) -> FrameImage {
    one(&format!(
        "format: 1
kind: sprite
size:
  - 4
  - 4
palette:
  A:
    color:
      - 1.0
      - 1.0
      - 1.0
      - 1.0
  B:
    color:
      - 0.0
      - 0.0
      - 0.0
      - 1.0
frames:
  idle:
    rows:
      - \"AAAA\"
      - \"AAAA\"
      - \"AAAA\"
      - \"AAAA\"
effects:
  - op: dither
    glyphs:
      - A
      - B
    density: {density}
"
    ))
}

/// Density 0.5 lights exactly the Bayer thresholds below 8, a checkerboard — which is symmetric
/// under transposition, so it cannot tell a row-major lookup from a column-major one.
#[test]
fn dither_thresholds_the_first_glyph_against_the_bayer_matrix() {
    let image = dithered("0.5");
    let bayer = [
        [0u32, 8, 2, 10],
        [12, 4, 14, 6],
        [3, 11, 1, 9],
        [15, 7, 13, 5],
    ];
    for y in 0..4 {
        for x in 0..4 {
            let expected = if bayer[y as usize][x as usize] < 8 {
                [0.0, 0.0, 0.0, 1.0]
            } else {
                [1.0, 1.0, 1.0, 1.0]
            };
            assert_eq!(image.get(x, y), expected, "texel ({x}, {y})");
        }
    }
}

/// At 3/16 only the Bayer thresholds 0, 1 and 2 pass. They sit at (0,0), (2,2) and (2,0) — and not
/// at the transposed (0,2) — so a swapped row/column index lights a different texel.
#[test]
fn dither_at_three_sixteenths_lights_only_thresholds_below_three() {
    let image = dithered("0.1875");
    let lit = [(0, 0), (2, 0), (2, 2)];
    for y in 0..4 {
        for x in 0..4 {
            let expected = if lit.contains(&(x, y)) {
                [0.0, 0.0, 0.0, 1.0]
            } else {
                [1.0, 1.0, 1.0, 1.0]
            };
            assert_eq!(image.get(x, y), expected, "texel ({x}, {y})");
        }
    }
}

#[test]
fn noise_draws_a_pinned_stream_from_the_seed_and_the_texel_address() {
    // These three literals ARE the hash: an edit to the mixer or to how the address folds in shows
    // up here rather than as a mystery diff in a committed atlas PNG.
    let image = one("format: 1
kind: sprite
size:
  - 1
  - 1
seed: 7
palette:
  A:
    color:
      - 0.5
      - 0.5
      - 0.5
      - 1.0
frames:
  idle:
    rows:
      - \"A\"
effects:
  - op: noise
    amount: 0.5
");
    assert_eq!(image.get(0, 0), [0.20338655, 0.25588435, 0.92359114, 1.0]);
}

#[test]
fn noise_leaves_alpha_alone() {
    let image = one("format: 1
kind: sprite
size:
  - 1
  - 1
palette:
  A:
    color:
      - 0.5
      - 0.5
      - 0.5
      - 0.25
frames:
  idle:
    rows:
      - \"A\"
effects:
  - op: noise
    amount: 1.0
");
    assert_eq!(image.get(0, 0)[3], 0.25);
}

#[test]
fn an_unfiltered_blur_spreads_into_transparency() {
    let image = one("format: 1
kind: sprite
size:
  - 3
  - 3
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
      - \"...\"
      - \".A.\"
      - \"...\"
effects:
  - op: blur
    radius: 1
");
    // One opaque texel over a 3x3 window: every texel of the frame sees it exactly once, so each
    // averages to a ninth of its coverage at full brightness.
    let expected = [1.0, 1.0, 1.0, 1.0f32 / 9.0];
    for y in 0..3 {
        for x in 0..3 {
            assert_eq!(image.get(x, y), expected, "texel ({x}, {y})");
        }
    }
}

#[test]
fn a_glyph_filtered_blur_writes_only_that_glyph() {
    let image = one("format: 1
kind: sprite
size:
  - 3
  - 3
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
      - \"...\"
      - \".A.\"
      - \"...\"
effects:
  - op: blur
    radius: 1
    glyphs:
      - A
");
    assert_eq!(image.get(1, 1), [1.0, 1.0, 1.0, 1.0f32 / 9.0]);
    for (x, y) in [
        (0, 0),
        (1, 0),
        (2, 0),
        (0, 1),
        (2, 1),
        (0, 2),
        (1, 2),
        (2, 2),
    ] {
        assert_eq!(
            image.get(x, y),
            [0.0; 4],
            "texel ({x}, {y}) is outside the filter"
        );
    }
}

#[test]
fn glow_grows_a_soft_halo_outside_a_red_core_and_leaves_the_core_crisp() {
    // A single opaque red texel with a radius-2 halo. The parabolic falloff `1 - d²/r²` at r²=4 is
    // exactly 0.75 for the 4-neighbours (d²=1) and 0.5 for the corners (d²=2), and the untinted
    // halo derives its colour from the one source, so a red core glows red — while the opaque core
    // texel is written by no halo and keeps its exact value.
    let image = one("format: 1
kind: sprite
size:
  - 3
  - 3
palette:
  A:
    color:
      - 1.0
      - 0.0
      - 0.0
      - 1.0
frames:
  idle:
    rows:
      - \"...\"
      - \".A.\"
      - \"...\"
effects:
  - op: glow
    radius_px: 2.0
    strength: 1.0
");
    assert_eq!(
        image.get(1, 1),
        [1.0, 0.0, 0.0, 1.0],
        "the core is preserved"
    );
    for (x, y) in [(1, 0), (0, 1), (2, 1), (1, 2)] {
        assert_eq!(
            image.get(x, y),
            [1.0, 0.0, 0.0, 0.75],
            "4-neighbour ({x}, {y}) took a red halo"
        );
    }
    for (x, y) in [(0, 0), (2, 0), (0, 2), (2, 2)] {
        assert_eq!(
            image.get(x, y),
            [1.0, 0.0, 0.0, 0.5],
            "corner ({x}, {y}) took a fainter red halo"
        );
    }
}

#[test]
fn an_explicit_glow_color_tints_the_halo_and_strength_scales_it() {
    // The same silhouette, but an explicit blue tint overrides the source-derived colour, and
    // strength 0.5 halves the accumulated alpha (0.75 -> 0.375, 0.5 -> 0.25).
    let image = one("format: 1
kind: sprite
size:
  - 3
  - 3
palette:
  A:
    color:
      - 1.0
      - 0.0
      - 0.0
      - 1.0
frames:
  idle:
    rows:
      - \"...\"
      - \".A.\"
      - \"...\"
effects:
  - op: glow
    color:
      - 0.0
      - 0.0
      - 1.0
      - 1.0
    radius_px: 2.0
    strength: 0.5
");
    assert_eq!(
        image.get(1, 1),
        [1.0, 0.0, 0.0, 1.0],
        "the core is untouched"
    );
    assert_eq!(
        image.get(1, 0),
        [0.0, 0.0, 1.0, 0.375],
        "a scaled blue halo"
    );
    assert_eq!(
        image.get(0, 0),
        [0.0, 0.0, 1.0, 0.25],
        "a fainter blue corner"
    );
}

#[test]
fn an_effect_created_texel_belongs_to_no_glyph() {
    // The outline paints texels that no glyph names, so the gradient filtered to `A` must leave
    // them alone even though they are opaque by the time it runs.
    let image = one("format: 1
kind: sprite
size:
  - 3
  - 3
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
      - \"...\"
      - \".A.\"
      - \"...\"
effects:
  - op: outline
    color:
      - 0.0
      - 0.0
      - 1.0
      - 1.0
  - op: gradient
    glyphs:
      - A
    from:
      - 1.0
      - 0.0
      - 0.0
      - 1.0
    to:
      - 1.0
      - 0.0
      - 0.0
      - 1.0
");
    assert_eq!(
        image.get(1, 1),
        [1.0, 0.0, 0.0, 1.0],
        "the glyph is regraded"
    );
    assert_eq!(image.get(1, 0), [0.0, 0.0, 1.0, 1.0], "the outline is not");
}

#[test]
fn baking_the_same_document_twice_gives_the_same_bytes() {
    let source = "format: 1
kind: sprite
size:
  - 6
  - 6
seed: 99
palette:
  A:
    color:
      - 0.8
      - 0.2
      - 0.1
      - 1.0
  B:
    color:
      - 0.1
      - 0.2
      - 0.8
      - 1.0
frames:
  idle:
    rows:
      - \"..AA..\"
      - \".AAAA.\"
      - \"AABBAA\"
      - \"AABBAA\"
      - \".AAAA.\"
      - \"..AA..\"
effects:
  - op: shade
    light:
      - -1.0
      - -1.0
    strength: 0.4
  - op: dither
    glyphs:
      - A
      - B
    density: 0.35
  - op: noise
    amount: 0.2
  - op: outline
    color:
      - 0.0
      - 0.0
      - 0.0
      - 1.0
  - op: blur
    radius: 1
";
    let doc = check_sprite(source).expect("the fixture sprite is valid");
    let first = bake(&doc).expect("the fixture sprite bakes");
    let second = bake(&doc).expect("the fixture sprite bakes");
    assert_eq!(first, second);
    assert_eq!(
        first[0].image.to_srgb8_premultiplied().rgba,
        second[0].image.to_srgb8_premultiplied().rgba
    );
}

#[test]
fn an_invalid_document_is_refused_rather_than_guessed_at() {
    let doc = SpriteDocument {
        size: [1, 1],
        ..SpriteDocument::default()
    };
    let diagnostics = bake(&doc).expect_err("an empty palette is not bakeable");
    assert!(diagnostics.iter().all(|d| d.code.as_str() == "L1153"));
}

#[test]
fn an_authored_pivot_wins_over_the_frame_centre() {
    let doc = check_sprite(
        "format: 1
kind: sprite
size:
  - 4
  - 4
palette:
  A:
    color:
      - 1.0
      - 1.0
      - 1.0
      - 1.0
frames:
  centred:
    rows:
      - \"AAAA\"
      - \"AAAA\"
      - \"AAAA\"
      - \"AAAA\"
  footed:
    pivot:
      - 2.0
      - 4.0
    rows:
      - \"AAAA\"
      - \"AAAA\"
      - \"AAAA\"
      - \"AAAA\"
",
    )
    .expect("the fixture sprite is valid");
    let frames = bake(&doc).expect("the fixture sprite bakes");
    assert_eq!(frames[0].pivot, [2.0, 2.0]);
    assert_eq!(frames[1].pivot, [2.0, 4.0]);
}

//! Ramp-mode `shade` and `gradient`: glyph-stepping against a palette ramp, on frames small enough to
//! write the expected texels out by hand. A glyph's colour is a grey of its own, so a texel's red
//! channel names the glyph it ended on.

use ludion_scene::{SpriteDocument, check_sprite};
use ludion_sprite::{FrameImage, bake, bake_steps};

/// The grey each palette glyph paints (exact binary fractions); `x` is in no ramp.
const GREYS: [(char, f32); 5] = [('a', 0.0), ('b', 0.25), ('c', 0.5), ('d', 0.75), ('x', 1.0)];

fn grey(glyph: char) -> [f32; 4] {
    let value = GREYS.iter().find(|(g, _)| *g == glyph).unwrap().1;
    [value, value, value, 1.0]
}

/// A sprite document over the grey palette: `rows` fix the size, `ramps` is the YAML list body (no
/// key) and `effects` the effect list body.
fn source(rows: &[&str], ramps: &[&str], effects: &str) -> String {
    let mut out = format!(
        "format: 1\nkind: sprite\nsize:\n  - {}\n  - {}\npalette:\n",
        rows[0].len(),
        rows.len()
    );
    for (glyph, value) in GREYS {
        out += &format!(
            "  {glyph}:\n    color:\n      - {value:?}\n      - {value:?}\n      - {value:?}\n      - 1.0\n"
        );
    }
    out += "ramps:\n";
    for ramp in ramps {
        out += &format!("  - {ramp}\n");
    }
    out += "frames:\n  idle:\n    rows:\n";
    for row in rows {
        out += &format!("      - \"{row}\"\n");
    }
    out += "effects:\n";
    out += effects;
    out
}

fn doc(source: &str) -> SpriteDocument {
    check_sprite(source).expect("the fixture sprite is valid")
}

fn one(source: &str) -> FrameImage {
    let mut frames = bake(&doc(source)).expect("the fixture sprite bakes");
    assert_eq!(frames.len(), 1);
    frames.remove(0).image
}

/// The glyph column of a one-column image, read back through the red channel.
fn column(image: &FrameImage) -> String {
    (0..image.height as i32)
        .map(|y| glyph_of(image.get(0, y)))
        .collect()
}

/// Every row of an image as glyphs.
fn grid(image: &FrameImage) -> Vec<String> {
    (0..image.height as i32)
        .map(|y| {
            (0..image.width as i32)
                .map(|x| glyph_of(image.get(x, y)))
                .collect()
        })
        .collect()
}

fn glyph_of(texel: [f32; 4]) -> char {
    GREYS
        .iter()
        .find(|(_, value)| *value == texel[0] && texel[3] == 1.0)
        .map_or('?', |(glyph, _)| *glyph)
}

const SHADE: &str = "  - op: shade\n    mode: ramp\n    steps: ";
const GRADIENT: &str = "  - op: gradient\n    mode: ramp\n    steps: ";

#[test]
fn ramp_shade_steps_lit_edges_up_and_shadowed_edges_down() {
    let rows = [".....", ".ccc.", ".ccc.", ".ccc.", "....."];
    let image = one(&source(&rows, &["abcd"], &format!("{SHADE}1\n")));
    // The default light comes from the upper left: the top row and left column are lit, the bottom
    // row and right column shadowed, the centre interior.
    let expected = [".....", ".ddd.", ".dcb.", ".dbb.", "....."];
    for (y, row) in expected.iter().enumerate() {
        for (x, glyph) in row.chars().enumerate() {
            let texel = image.get(x as i32, y as i32);
            if glyph == '.' {
                assert_eq!(texel[3], 0.0, "({x}, {y}) stays transparent");
            } else {
                assert_eq!(texel, grey(glyph), "({x}, {y})");
            }
        }
    }
}

#[test]
fn negative_ramp_shade_steps_swap_lit_and_shadowed() {
    let rows = [".....", ".ccc.", ".ccc.", ".ccc.", "....."];
    let image = one(&source(&rows, &["abcd"], &format!("{SHADE}-1\n")));
    assert_eq!(image.get(1, 1), grey('b'), "a lit edge steps down");
    assert_eq!(image.get(3, 3), grey('d'), "a shadowed edge steps up");
    assert_eq!(image.get(2, 2), grey('c'), "the interior stays");
}

#[test]
fn a_vertical_ramp_gradient_resolves_its_fraction_by_the_bayer_matrix() {
    // t = 0, .25, .5, .75, 1 times 2 steps = 0, .5, 1, 1.5, 2. The two half-steps (rows 1 and 3)
    // round up only where the matrix threshold falls under 0.5: column 1 (4 and 7), not column 0
    // (12 and 15).
    let image = one(&source(
        &["aa", "aa", "aa", "aa", "aa"],
        &["abcd"],
        &format!("{GRADIENT}2\n"),
    ));
    assert_eq!(grid(&image), ["aa", "ab", "bb", "bc", "cc"]);
}

#[test]
fn negative_gradient_steps_walk_down_the_ramp() {
    let image = one(&source(
        &["dd", "dd", "dd", "dd", "dd"],
        &["abcd"],
        &format!("{GRADIENT}-2\n"),
    ));
    assert_eq!(grid(&image), ["dd", "cd", "cc", "bc", "bb"]);
}

#[test]
fn stepping_clamps_at_both_ends_of_the_ramp() {
    let up = one(&source(&["b", "b"], &["abcd"], &format!("{GRADIENT}9\n")));
    assert_eq!(column(&up), "bd");
    let down = one(&source(&["b", "b"], &["abcd"], &format!("{GRADIENT}-9\n")));
    assert_eq!(column(&down), "ba");
}

#[test]
fn zero_steps_change_nothing() {
    let image = one(&source(&["b", "c"], &["abcd"], &format!("{GRADIENT}0\n")));
    assert_eq!(column(&image), "bc");
}

#[test]
fn a_glyph_in_two_ramps_steps_along_the_first() {
    // `b` is last in `ab`, so +1 clamps there; the second ramp would have moved it to `c`.
    let image = one(&source(
        &["b", "b"],
        &["ab", "bcd"],
        &format!("{GRADIENT}1\n"),
    ));
    assert_eq!(column(&image), "bb");
    // `c` only appears in the second ramp.
    let image = one(&source(
        &["c", "c"],
        &["ab", "bcd"],
        &format!("{GRADIENT}1\n"),
    ));
    assert_eq!(column(&image), "cd");
}

#[test]
fn a_dithered_glyph_is_what_a_later_ramp_op_steps() {
    // density 1.0 turns every `a` into `b`; the gradient then steps `b`, not the authored `a`.
    let effects = format!(
        "  - op: dither\n    glyphs:\n      - a\n      - b\n    density: 1.0\n{GRADIENT}1\n"
    );
    let image = one(&source(&["a", "a"], &["abcd"], &effects));
    assert_eq!(column(&image), "bc");
}

#[test]
fn dither_after_a_ramp_op_tests_the_stepped_glyph_not_the_authored_one() {
    let rows = [".....", ".ccc.", ".ccc.", ".ccc.", "....."];
    let effects =
        format!("{SHADE}1\n  - op: dither\n    glyphs:\n      - c\n      - a\n    density: 1.0\n");
    let image = one(&source(&rows, &["abcd"], &effects));
    // Only the interior texel is still `c` after the shade; the stepped edges are no longer `c`.
    assert_eq!(
        image.get(2, 2),
        grey('a'),
        "the unmoved interior is dithered"
    );
    assert_eq!(
        image.get(1, 1),
        grey('d'),
        "a stepped-up edge is left alone"
    );
    assert_eq!(
        image.get(3, 3),
        grey('b'),
        "a stepped-down edge is left alone"
    );
}

#[test]
fn an_outline_after_a_ramp_op_is_not_touched_by_the_plane() {
    let rows = [".....", "..b..", "....."];
    let effects = format!(
        "{SHADE}1\n  - op: outline\n    color:\n      - 1.0\n      - 0.0\n      - 0.0\n      - 1.0\n"
    );
    let image = one(&source(&rows, &["abcd"], &effects));
    assert_eq!(image.get(2, 1), grey('c'), "the lone texel is a lit edge");
    assert_eq!(image.get(1, 1), [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(image.get(2, 0), [1.0, 0.0, 0.0, 1.0]);
}

#[test]
fn the_last_bake_step_equals_the_bake() {
    let document = doc(&source(
        &["a", "a", "a", "a", "a"],
        &["abcd"],
        &format!("{GRADIENT}2\n{SHADE}1\n"),
    ));
    let baked = bake(&document).expect("bakes");
    let stepped = bake_steps(&document).expect("bakes in steps");
    assert_eq!(stepped[0].steps.len(), 2);
    assert_eq!(stepped[0].steps.last().unwrap().image, baked[0].image);
}

//! A tile is seamless: the step across the wrap seam is no bigger than the steps inside it.

mod common;

use common::{MemResolver, assert_seamless, recipe};
use ludion_texgen::{Image, evaluate, load};

fn height(source: &str, size: u32) -> Image {
    let loaded = load(source, &MemResolver::default()).expect("recipe loads");
    evaluate(&loaded, size, &mut |_, _| {}).height
}

fn single(step: &str) -> String {
    recipe(
        &format!("  n:\n{step}"),
        "  base_color: n\n  roughness: n\n  height: n\n",
    )
}

#[test]
fn every_noise_type_tiles() {
    for kind in ["value", "gradient", "cellular"] {
        for octaves in [1, 5] {
            let step = format!(
                "    op: noise\n    type: {kind}\n    scale: 4\n    octaves: {octaves}\n    seed: 3\n"
            );
            assert_seamless(&format!("{kind} x{octaves}"), &height(&single(&step), 128));
        }
    }
}

#[test]
fn warp_and_blur_chains_tile() {
    let steps = "  a:
    op: noise
    type: gradient
    scale: 3
    octaves: 3
  b:
    op: noise
    type: value
    scale: 5
    seed: 1
  w:
    op: warp
    input: a
    by: b
    amount: 0.1
    angle: 45
  n:
    op: blur
    input: w
    radius: 0.02
";
    let source = recipe(steps, "  base_color: n\n  roughness: n\n  height: n\n");
    assert_seamless("warp+blur", &height(&source, 128));
}

#[test]
fn scatter_tiles_with_every_jitter() {
    let step = "    op: tile_sampler
    columns: 5
    rows: 6
    shape: bell
    scale: 1.3
    scale_jitter: 0.3
    position_jitter: 0.6
    rotation_jitter: 1.0
    value_jitter: 0.3
    offset: 0.5
    combine: add
    seed: 2
";
    assert_seamless("tile_sampler", &height(&single(step), 128));
}

#[test]
fn scatter_tiles_with_the_layout_fields() {
    let steps = "  ramp:
    op: noise
    type: gradient
    scale: 2
  n:
    op: tile_sampler
    columns: 3
    rows: 4
    pattern: ramp
    scale: 1.2
    gap: 0.02
    offset: 0.5
    offset_jitter: 0.7
    pattern_scale_jitter: 0.4
    pattern_offset_jitter: 0.8
    rotation_jitter: 0.5
    position_jitter: 0.3
    seed: 5
";
    let source = recipe(steps, "  base_color: n\n  roughness: n\n  height: n\n");
    assert_seamless("tile_sampler layout", &height(&source, 128));
}

const REGIONS: &str = "  cells:
    op: noise
    type: cellular
    scale: 4
    return: f2_minus_f1
    seed: 1
  n:
    op: flood_fill
    input: cells
    threshold: 0.15
    output: gradient
    angle: 30
    angle_jitter: 0.5
";

#[test]
fn fills_and_distances_tile() {
    let fill = recipe(REGIONS, "  base_color: n\n  roughness: n\n  height: n\n");
    assert_seamless("flood_fill", &height(&fill, 128));
    let steps = REGIONS.replace(
        "op: flood_fill\n    input: cells\n    threshold: 0.15\n    output: gradient\n    angle: 30\n    angle_jitter: 0.5",
        "op: distance\n    input: cells\n    threshold: 0.15\n    max: 0.2",
    );
    let distance = recipe(&steps, "  base_color: n\n  roughness: n\n  height: n\n");
    assert_seamless("distance", &height(&distance, 128));
}

#[test]
fn occlusion_and_curvature_tile() {
    for op in [
        "ambient_occlusion\n    radius: 0.08",
        "curvature\n    radius: 0.03",
    ] {
        let steps = format!(
            "  h:\n    op: noise\n    type: gradient\n    scale: 3\n    octaves: 4\n  n:\n    op: {op}\n    input: h\n"
        );
        let source = recipe(&steps, "  base_color: n\n  roughness: n\n  height: n\n");
        assert_seamless(op, &height(&source, 128));
    }
}

const FBM: &str = "  h:
    op: noise
    type: gradient
    scale: 3
    octaves: 4
";

#[test]
fn tone_and_filter_ops_tile() {
    let mut cases: Vec<(String, String)> = vec![(
        "posterize".into(),
        format!("{FBM}  n:\n    op: posterize\n    input: h\n    steps: 5\n"),
    )];
    for side in ["both", "inside", "outside"] {
        cases.push((
            format!("band {side}"),
            format!("{FBM}  n:\n    op: band\n    input: h\n    width: 0.02\n    side: {side}\n"),
        ));
    }
    cases.push((
        "hsv_shift".into(),
        format!(
            "{FBM}  c:\n    op: colorize\n    input: h\n    stops:\n      - at: 0.0\n        color: \"#204080\"\n      - at: 1.0\n        color: \"#e0c040\"\n  s:\n    op: hsv_shift\n    input: c\n    hue: 90\n    saturation: 1.5\n  n:\n    op: grayscale\n    input: s\n"
        ),
    ));
    for kind in ["value", "gradient", "cellular"] {
        for axis in ["u", "v"] {
            cases.push((
                format!("stretch {kind} {axis}"),
                format!(
                    "  n:\n    op: noise\n    type: {kind}\n    scale: 3\n    octaves: 2\n    stretch: 6\n    stretch_axis: {axis}\n"
                ),
            ));
        }
    }
    for spread in ["both", "forward"] {
        cases.push((
            format!("blur {spread}"),
            format!(
                "  c:\n    op: noise\n    type: cellular\n    scale: 5\n  n:\n    op: blur\n    input: c\n    radius: 0.06\n    angle: 37\n    spread: {spread}\n"
            ),
        ));
    }
    for (label, steps) in cases {
        let source = recipe(&steps, "  base_color: n\n  roughness: n\n  height: n\n");
        assert_seamless(&label, &height(&source, 128));
    }
}

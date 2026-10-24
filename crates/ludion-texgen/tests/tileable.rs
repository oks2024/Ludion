//! A tile is seamless: the step across the wrap seam is no bigger than the steps inside it.

mod common;

use common::{MemResolver, recipe};
use ludion_texgen::{Image, evaluate, load};

fn height(source: &str, size: u32) -> Image {
    let loaded = load(source, &MemResolver::default()).expect("recipe loads");
    evaluate(&loaded, size, &mut |_, _| {}).height
}

fn assert_seamless(label: &str, image: &Image) {
    let n = image.size as i64;
    let at = |x: i64, y: i64| image.texel(x, y)[0];
    let (mut interior, mut seam) = (0.0f32, 0.0f32);
    for y in 0..n {
        for x in 0..n - 1 {
            interior = interior.max((at(x + 1, y) - at(x, y)).abs());
        }
        seam = seam.max((at(0, y) - at(n - 1, y)).abs());
    }
    for x in 0..n {
        for y in 0..n - 1 {
            interior = interior.max((at(x, y + 1) - at(x, y)).abs());
        }
        seam = seam.max((at(x, 0) - at(x, n - 1)).abs());
    }
    assert!(
        seam <= interior * 1.5,
        "{label}: seam step {seam} exceeds 1.5x the interior step {interior}"
    );
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

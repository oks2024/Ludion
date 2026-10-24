//! The embedded `std:` macros build, tile and are not flat; the stdlib resolves inside `load`.

mod common;

use common::{MemResolver, assert_seamless, recipe};
use ludion_core::par;
use ludion_texgen::{Image, build, evaluate, load, stdlib};

/// `(macro, its input steps, its output steps)`.
const MACROS: &[(&str, &[&str], &[&str])] = &[
    ("edge_wear", &["height"], &["edges", "mask"]),
    (
        "cavity_grime",
        &["height"],
        &["cavities", "occlusion", "mask"],
    ),
    ("cobbles", &[], &["height", "mask", "stone_id"]),
    ("planks", &[], &["boards", "board_id", "grain", "height"]),
    ("rust_spots", &[], &["mask", "pits"]),
];

const SOURCE: &str = "  h:\n    op: noise\n    type: gradient\n    scale: 4\n    octaves: 3\n";

/// A recipe whose steps are a gradient noise `h` and `use`, the macro bound to `h`.
fn consumer(name: &str, inputs: &[&str], output: &str) -> String {
    let mut bind = String::new();
    if !inputs.is_empty() {
        bind.push_str("    inputs:\n");
        for input in inputs {
            bind.push_str(&format!("      {input}: h\n"));
        }
    }
    recipe(
        &format!(
            "{SOURCE}  use:\n    op: recipe\n    path: std:{name}\n    output: {output}\n{bind}"
        ),
        "  base_color: use\n  roughness: use\n  height: use\n",
    )
}

fn height(source: &str, size: u32) -> Image {
    let loaded = load(source, &MemResolver::default()).expect("recipe loads");
    evaluate(&loaded, size, &mut |_, _| {}).height
}

#[test]
fn the_list_matches_the_embedded_names() {
    let mut listed: Vec<&str> = MACROS.iter().map(|(n, _, _)| *n).collect();
    let mut embedded: Vec<&str> = stdlib::names().collect();
    listed.sort_unstable();
    embedded.sort_unstable();
    assert_eq!(listed, embedded);
}

#[test]
fn every_macro_builds_standalone() {
    for name in stdlib::names() {
        let own = stdlib::get(name).unwrap();
        build(own, &MemResolver::default(), 64).unwrap_or_else(|d| panic!("{name}: {d:?}"));
        let wrapped = recipe(
            &format!("  use:\n    op: recipe\n    path: std:{name}\n    output: mask\n"),
            "  base_color: use\n  roughness: use\n  height: use\n",
        );
        // Only macros with a `mask` step are wrapped by that output; the rest name their own.
        if own.contains("\n  mask:\n") {
            build(&wrapped, &MemResolver::default(), 64)
                .unwrap_or_else(|d| panic!("{name} wrapped: {d:?}"));
        }
    }
}

#[test]
fn every_output_tiles_and_varies() {
    for (name, inputs, outputs) in MACROS {
        for output in *outputs {
            let image = height(&consumer(name, inputs, output), 128);
            assert_seamless(&format!("std:{name}.{output}"), &image);
            let (lo, hi) = image
                .data
                .iter()
                .fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
            assert!(
                hi - lo >= 0.05,
                "std:{name}.{output} is flat: range {lo}..{hi}"
            );
        }
    }
}

#[test]
fn bytes_are_identical_across_thread_counts() {
    let source = consumer("planks", &[], "height");
    let one = par::with_threads(1, || build(&source, &MemResolver::default(), 64).unwrap());
    let eight = par::with_threads(8, || build(&source, &MemResolver::default(), 64).unwrap());
    assert_eq!(one.height, eight.height);
    assert_eq!(one.base_color, eight.base_color);
}

/// A recipe exposing `output` of `std:name` built with `params` (`key: value` lines).
fn with_params(name: &str, output: &str, params: &str) -> String {
    let indented: String = params.lines().map(|l| format!("      {l}\n")).collect();
    recipe(
        &format!(
            "  use:\n    op: recipe\n    path: std:{name}\n    output: {output}\n    params:\n{indented}"
        ),
        "  base_color: use\n  roughness: use\n  height: use\n",
    )
}

/// Lengths of the maximal runs of texels equal to zero along a line, wrapping not considered.
fn zero_runs(image: &Image, at: impl Fn(i64) -> (i64, i64)) -> Vec<usize> {
    let (mut runs, mut run) = (Vec::new(), 0);
    for i in 0..i64::from(image.size) {
        let (x, y) = at(i);
        if image.texel(x, y)[0] == 0.0 {
            run += 1;
        } else if run > 0 {
            runs.push(std::mem::take(&mut run));
        }
    }
    if run > 0 {
        runs.push(run);
    }
    runs
}

#[test]
fn planks_without_a_gap_is_one_board() {
    let params = "gap: 0\ncolumns: 1\nrows: 1\nstagger: 0";
    let id = height(&with_params("planks", "board_id", params), 64);
    assert!(id.data[0] > 0.0 && id.data.iter().all(|v| *v == id.data[0]));
    let boards = height(&with_params("planks", "boards", params), 64);
    assert!(boards.data.iter().all(|v| *v == 1.0));
}

#[test]
fn planks_joints_are_uniform() {
    let boards = height(&with_params("planks", "boards", "gap: 0.03125"), 256);
    // Six rows of 42-43 texels: row 1 spans y 43..85, so y = 64 is inside it; x = 64 is inside a
    // board of the first row's two columns.
    let across = zero_runs(&boards, |x| (x, 64));
    // x = 32 crosses only horizontal joints; the one over the wrap seam splits in two.
    let down: Vec<usize> = zero_runs(&boards, |y| (32, y))
        .into_iter()
        .filter(|r| *r > 6)
        .collect();
    assert!(!across.is_empty() && !down.is_empty());
    for (a, d) in across.iter().zip(&down) {
        assert!(a.abs_diff(*d) <= 1, "{across:?} vs {down:?}");
    }
}

#[test]
fn a_planks_stagger_jitter_variant_tiles() {
    let boards = height(&with_params("planks", "height", "stagger_jitter: 1"), 128);
    assert_seamless("std:planks stagger_jitter", &boards);
}

#[test]
fn cobbles_round_zero_is_the_default() {
    let plain = height(&consumer("cobbles", &[], "height"), 64);
    let zero = height(&with_params("cobbles", "height", "round: 0"), 64);
    assert_eq!(plain, zero);
}

#[test]
fn cobbles_round_removes_facets() {
    let facet = |round: &str| {
        let params = format!("round: {round}");
        let h = height(&with_params("cobbles", "height", &params), 128);
        let mask = height(&with_params("cobbles", "mask", &params), 128);
        let at = |i: &Image, x: i64, y: i64| i.texel(x, y)[0];
        let (mut sum, mut count) = (0.0f32, 0u32);
        for y in 0..128 {
            for x in 0..128 {
                if at(&mask, x, y) == 1.0 && at(&h, x, y) > 0.5 {
                    let lap =
                        at(&h, x - 1, y) + at(&h, x + 1, y) + at(&h, x, y - 1) + at(&h, x, y + 1)
                            - 4.0 * at(&h, x, y);
                    sum += lap.abs();
                    count += 1;
                }
            }
        }
        assert!(count > 0);
        (sum / count as f32, h)
    };
    let (faceted, _) = facet("0");
    let (rounded, h) = facet("1");
    assert!(rounded <= 0.7 * faceted, "{rounded} vs {faceted}");
    assert_seamless("std:cobbles round", &h);
}

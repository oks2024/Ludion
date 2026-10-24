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

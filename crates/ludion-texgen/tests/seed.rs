//! The root seed can be replaced at load: the recipe's own seed reproduces `load`, another moves the maps.

mod common;

use common::{MemResolver, recipe};
use ludion_texgen::{evaluate, load, load_with_seed, parse};

fn source() -> String {
    let body = recipe(
        "  n:\n    op: noise\n    type: gradient\n    scale: 4\n  c:\n    op: colorize\n    input: n\n    stops:\n      - at: 0.0\n        color: \"#000000\"\n      - at: 1.0\n        color: \"#ffffff\"\n",
        "  base_color: c\n  roughness: n\n  height: n\n",
    );
    body.replacen("steps:", "seed: 5\nsteps:", 1)
}

#[test]
fn the_recipe_seed_reproduces_load_and_another_seed_changes_the_maps() {
    let (text, resolver) = (source(), MemResolver::with(&[]));
    let own = parse(&text).unwrap().seed;
    let plain = evaluate(&load(&text, &resolver).unwrap(), 32, &mut |_, _| {});
    let same = evaluate(
        &load_with_seed(&text, &resolver, own).unwrap(),
        32,
        &mut |_, _| {},
    );
    let other = evaluate(
        &load_with_seed(&text, &resolver, own + 1).unwrap(),
        32,
        &mut |_, _| {},
    );
    assert_eq!(plain.base_color.data, same.base_color.data);
    assert_ne!(plain.base_color.data, other.base_color.data);
}

//! The `layer` step: bit-identical to the `blend` chain it replaces, and its error cases.

mod common;

use common::{MemResolver, assert_seamless, first_error, recipe};
use ludion_core::diagnostics::Diagnostic;
use ludion_texgen::eval::Maps;
use ludion_texgen::{evaluate, load};

const OUT_PLAIN: &str = "  base_color: c\n  roughness: r\n  height: h\n";

const SOURCES: &str = "  mask1:
    op: noise
    type: value
    scale: 5
    seed: 1
  mask2:
    op: noise
    type: cellular
    scale: 4
    seed: 2
  mask3:
    op: noise
    type: gradient
    scale: 6
    seed: 3
  base_c:
    op: colorize
    input: mask3
    stops:
      - at: 0.0
        color: \"#203040\"
      - at: 1.0
        color: \"#a0b0c0\"
  metal_c:
    op: colorize
    input: mask1
    stops:
      - at: 0.0
        color: \"#707070\"
      - at: 1.0
        color: \"#d0d0d0\"
  rough0:
    op: noise
    type: value
    scale: 3
    seed: 4
  h0:
    op: noise
    type: value
    scale: 2
    seed: 5
  h1:
    op: noise
    type: gradient
    scale: 7
    seed: 6
";

fn with_params(steps: &str, outputs: &str) -> String {
    recipe(&format!("{SOURCES}{steps}"), outputs)
        .replace("steps:", "params:\n  tint: \"#a03020\"\nsteps:")
}

fn layered() -> String {
    with_params(
        "  s:
    op: layer
    base:
      base_color: base_c
      roughness: rough0
      metallic: 0
      height: h0
    layers:
      - mask: mask1
        opacity: 0.6
        base_color: metal_c
        roughness: 0.3
        metallic: 1
      - mask: mask2
        base_color: \"#14110d\"
        height: h1
      - mask: mask3
        base_color: $tint
",
        "  base_color: s.base_color\n  roughness: s.roughness\n  metallic: s.metallic\n  height: s.height\n",
    )
}

fn chained() -> String {
    with_params(
        "  lit:
    op: constant
    value: \"#14110d\"
  tint_c:
    op: constant
    value: $tint
  r_lit:
    op: constant
    value: 0.3
  m_zero:
    op: constant
    value: 0
  m_one:
    op: constant
    value: 1
  c1:
    op: blend
    a: base_c
    b: metal_c
    mask: mask1
    opacity: 0.6
  c2:
    op: blend
    a: c1
    b: lit
    mask: mask2
  c:
    op: blend
    a: c2
    b: tint_c
    mask: mask3
  r:
    op: blend
    a: rough0
    b: r_lit
    mask: mask1
    opacity: 0.6
  m:
    op: blend
    a: m_zero
    b: m_one
    mask: mask1
    opacity: 0.6
  h:
    op: blend
    a: h0
    b: h1
    mask: mask2
",
        "  base_color: c\n  roughness: r\n  metallic: m\n  height: h\n",
    )
}

fn maps(source: &str, size: u32) -> Maps {
    let loaded = load(source, &MemResolver::default()).expect("recipe loads");
    evaluate(&loaded, size, &mut |_, _| {})
}

fn err(steps: &str, outputs: &str) -> Diagnostic {
    first_error(
        &recipe(&format!("{SOURCES}{steps}"), outputs),
        &MemResolver::default(),
    )
}

fn at(d: &Diagnostic) -> (&str, &str) {
    (d.code.as_str(), d.path.as_deref().unwrap_or(""))
}

#[test]
fn layer_equals_the_blend_chain() {
    assert_eq!(maps(&layered(), 64), maps(&chained(), 64));
}

#[test]
fn layer_maps_tile() {
    let m = maps(&layered(), 128);
    assert_seamless("layered height", &m.height);
    assert_seamless("layered roughness", &m.roughness);
}

#[test]
fn unconsumed_maps_are_not_evaluated() {
    let source = layered().replace("  metallic: s.metallic\n", "");
    let loaded = load(&source, &MemResolver::default()).unwrap();
    let mut seen = Vec::new();
    evaluate(&loaded, 32, &mut |name, _| seen.push(name.to_string()));
    assert!(seen.iter().any(|n| n == "s.base_color"), "{seen:?}");
    assert!(!seen.iter().any(|n| n == "s.metallic"), "{seen:?}");
}

#[test]
fn sub_recipe_output_names_a_layer_map() {
    let lib = with_params(
        "  s:
    op: layer
    base:
      base_color: base_c
      roughness: rough0
    layers:
      - mask: mask1
        roughness: 0.2
",
        "  base_color: s.base_color\n  roughness: s.roughness\n  height: h0\n",
    );
    let parent = recipe(
        "  use:
    op: recipe
    path: lib.texture.yaml
    output: s.roughness
  hh:
    op: noise
    type: value
    scale: 2
",
        "  base_color: hh\n  roughness: use\n  height: hh\n",
    );
    let r = MemResolver::with(&[("lib.texture.yaml", &lib)]);
    let loaded = load(&parent, &r).expect("parent loads");
    let m = evaluate(&loaded, 32, &mut |_, _| {});
    assert_eq!(m.roughness.channels, 1);
}

#[test]
fn map_missing_from_base_is_l1283() {
    let steps = "  s:
    op: layer
    base:
      base_color: base_c
    layers:
      - mask: mask1
        roughness: 0.3
";
    let d = err(
        steps,
        "  base_color: s.base_color\n  roughness: rough0\n  height: h0\n",
    );
    assert_eq!(at(&d), ("L1283", "/steps/s/layers/0/roughness"));
    assert!(
        d.help
            .unwrap()
            .contains("add `roughness: <value>` under `base:`")
    );
}

#[test]
fn layer_with_no_map_is_l1283() {
    let steps = "  s:
    op: layer
    base:
      base_color: base_c
    layers:
      - mask: mask1
";
    let d = err(
        steps,
        "  base_color: s.base_color\n  roughness: rough0\n  height: h0\n",
    );
    assert_eq!(at(&d), ("L1283", "/steps/s/layers/0"));
}

#[test]
fn normal_in_base_is_l1283() {
    let steps = "  s:
    op: layer
    base:
      normal: base_c
    layers:
      - mask: mask1
        normal: base_c
";
    let d = err(steps, OUT_PLAIN);
    assert_eq!(at(&d), ("L1283", "/steps/s/base/normal"));
    assert!(d.help.unwrap().contains("normal_from_height"));
}

#[test]
fn color_into_roughness_is_l1284() {
    let steps = "  s:
    op: layer
    base:
      roughness: rough0
    layers:
      - mask: mask1
        roughness: \"#ff0000\"
";
    let d = err(
        steps,
        "  base_color: base_c\n  roughness: s.roughness\n  height: h0\n",
    );
    assert_eq!(at(&d), ("L1284", "/steps/s/layers/0/roughness"));
    assert!(d.help.unwrap().contains("gray step or a number"));
}

#[test]
fn color_mask_is_l1284() {
    let steps = "  s:
    op: layer
    base:
      base_color: base_c
    layers:
      - mask: base_c
        base_color: metal_c
";
    let d = err(
        steps,
        "  base_color: s.base_color\n  roughness: rough0\n  height: h0\n",
    );
    assert_eq!(at(&d), ("L1284", "/steps/s/layers/0/mask"));
}

#[test]
fn bare_layer_reference_lists_the_maps() {
    let steps = "  s:
    op: layer
    base:
      base_color: base_c
      roughness: rough0
    layers:
      - mask: mask1
        roughness: 0.3
";
    let d = err(
        steps,
        "  base_color: s\n  roughness: s.roughness\n  height: h0\n",
    );
    assert_eq!(at(&d), ("L1285", "/outputs/base_color"));
    let help = d.help.unwrap();
    assert!(
        help.contains("s.base_color") && help.contains("s.roughness"),
        "{help}"
    );
}

#[test]
fn dotted_step_name_is_rejected() {
    let d = err("  a.b:\n    op: constant\n    value: 0.5\n", OUT_PLAIN);
    assert_eq!(at(&d), ("L1283", "/steps/a.b"));
    assert!(d.help.unwrap().contains("rename it"));
}

#[test]
fn unknown_layer_key_suggests_the_nearest() {
    let steps = "  s:
    op: layer
    base:
      base_color: base_c
    layers:
      - mask: mask1
        opacty: 0.5
        base_color: metal_c
";
    let d = err(
        steps,
        "  base_color: s.base_color\n  roughness: rough0\n  height: h0\n",
    );
    assert_eq!(at(&d), ("L1283", "/steps/s/layers/0/opacty"));
    assert!(d.help.unwrap().contains("opacity"));
}

#[test]
fn missing_mask_is_l1283() {
    let steps = "  s:
    op: layer
    base:
      base_color: base_c
    layers:
      - base_color: metal_c
";
    let d = err(
        steps,
        "  base_color: s.base_color\n  roughness: rough0\n  height: h0\n",
    );
    assert_eq!(at(&d), ("L1283", "/steps/s/layers/0/mask"));
}

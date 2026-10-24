//! One case per diagnostic class: the code and the JSON Pointer.

mod common;

use common::{MemResolver, first_error, recipe};

const OUT: &str = "  base_color: n\n  roughness: n\n  height: n\n";

fn noise(extra: &str) -> String {
    format!("  n:\n    op: noise\n    type: value\n    scale: 4\n{extra}")
}

fn err(steps: &str, outputs: &str) -> ludion_core::diagnostics::Diagnostic {
    first_error(&recipe(steps, outputs), &MemResolver::default())
}

#[test]
fn envelope_typo_is_l1119() {
    let d = first_error(
        "fromat: 1\nkind: texture\nsteps: {}\n",
        &MemResolver::default(),
    );
    assert_eq!(d.code.as_str(), "L1119", "{d:?}");
}

#[test]
fn flow_list_is_l1005() {
    let steps = "  n:\n    op: curve\n    input: m\n    points: [0, 1]\n";
    let d = err(steps, OUT);
    assert_eq!(d.code.as_str(), "L1005");
}

#[test]
fn unknown_op_suggests_nearest() {
    let d = err("  n:\n    op: nise\n", OUT);
    assert_eq!(d.code.as_str(), "L1283");
    assert_eq!(d.path.as_deref(), Some("/steps/n/op"));
    assert!(d.help.unwrap().contains("noise"));
}

#[test]
fn unknown_field_suggests_nearest() {
    let d = err(&noise("    scal: 3\n"), OUT);
    assert_eq!(d.code.as_str(), "L1283");
    assert_eq!(d.path.as_deref(), Some("/steps/n/scal"));
    assert!(d.help.unwrap().contains("scale"));
}

#[test]
fn missing_and_wrong_typed_fields_are_structure_errors() {
    let d = err("  n:\n    op: noise\n    type: value\n", OUT);
    assert_eq!(
        (d.code.as_str(), d.path.as_deref()),
        ("L1283", Some("/steps/n/scale"))
    );
    let d = err(
        "  n:\n    op: noise\n    type: value\n    scale: [a]\n",
        OUT,
    );
    assert_eq!(d.code.as_str(), "L1005");
    let d = err(
        "  n:\n    op: noise\n    type: value\n    scale: 2.5\n",
        OUT,
    );
    assert_eq!(
        (d.code.as_str(), d.path.as_deref()),
        ("L1283", Some("/steps/n/scale"))
    );
}

#[test]
fn unknown_and_missing_outputs() {
    let d = err(
        &noise(""),
        "  base_color: n\n  roughness: n\n  height: n\n  emisive: n\n",
    );
    assert_eq!(
        (d.code.as_str(), d.path.as_deref()),
        ("L1283", Some("/outputs/emisive"))
    );
    assert!(d.help.unwrap().contains("emissive"));
    let d = err(&noise(""), "  base_color: n\n  roughness: n\n");
    assert_eq!(
        (d.code.as_str(), d.path.as_deref()),
        ("L1283", Some("/outputs"))
    );
}

#[test]
fn out_of_range_value_is_data() {
    let d = err("  n:\n    op: noise\n    type: value\n    scale: 0\n", OUT);
    assert_eq!(
        (d.code.as_str(), d.path.as_deref()),
        ("L1284", Some("/steps/n/scale"))
    );
    let d = err(
        "  n:\n    op: noise\n    type: value\n    scale: 4096\n    octaves: 6\n",
        OUT,
    );
    assert_eq!(
        (d.code.as_str(), d.path.as_deref()),
        ("L1284", Some("/steps/n/octaves"))
    );
}

#[test]
fn bad_hex_and_unsorted_stops_are_data() {
    let steps = format!(
        "{}  c:\n    op: colorize\n    input: n\n    stops:\n      - at: 0.0\n        color: \"#12\"\n      - at: 1.0\n        color: \"#ffffff\"\n",
        noise("")
    );
    let d = err(&steps, "  base_color: c\n  roughness: n\n  height: n\n");
    assert_eq!(d.code.as_str(), "L1284");
    assert_eq!(d.path.as_deref(), Some("/steps/c/stops/0/color"));
    let steps = format!(
        "{}  c:\n    op: colorize\n    input: n\n    stops:\n      - at: 0.8\n        color: \"#000000\"\n      - at: 0.2\n        color: \"#ffffff\"\n",
        noise("")
    );
    let d = err(&steps, "  base_color: c\n  roughness: n\n  height: n\n");
    assert_eq!(
        (d.code.as_str(), d.path.as_deref()),
        ("L1284", Some("/steps/c/stops/1/at"))
    );
}

#[test]
fn undefined_param_is_data() {
    let d = err(&noise("    gain: $gian\n"), OUT);
    assert_eq!(
        (d.code.as_str(), d.path.as_deref()),
        ("L1284", Some("/steps/n/gain"))
    );
}

#[test]
fn gray_color_mismatches_are_data() {
    let steps = format!(
        "{}  c:\n    op: colorize\n    input: n\n    stops:\n      - at: 0.0\n        color: \"#000000\"\n      - at: 1.0\n        color: \"#ffffff\"\n  d:\n    op: colorize\n    input: c\n    stops:\n      - at: 0.0\n        color: \"#000000\"\n      - at: 1.0\n        color: \"#ffffff\"\n",
        noise("")
    );
    let d = err(&steps, "  base_color: c\n  roughness: n\n  height: n\n");
    assert_eq!(
        (d.code.as_str(), d.path.as_deref()),
        ("L1284", Some("/steps/d/input"))
    );
    let steps = &steps[..steps.find("  d:").unwrap()];
    let d = err(steps, "  base_color: c\n  roughness: c\n  height: n\n");
    assert_eq!(
        (d.code.as_str(), d.path.as_deref()),
        ("L1284", Some("/outputs/roughness"))
    );
    let d = err(
        steps,
        "  base_color: c\n  roughness: n\n  height: n\n  normal: n\n",
    );
    assert_eq!(
        (d.code.as_str(), d.path.as_deref()),
        ("L1284", Some("/outputs/normal"))
    );
}

#[test]
fn unknown_step_reference_suggests_nearest() {
    let steps = format!("{}  m:\n    op: levels\n    input: nn\n", noise(""));
    let d = err(&steps, "  base_color: m\n  roughness: n\n  height: n\n");
    assert_eq!(
        (d.code.as_str(), d.path.as_deref()),
        ("L1285", Some("/steps/m/input"))
    );
    assert!(d.help.unwrap().contains("`n`"));
}

#[test]
fn cycle_message_names_the_cycle() {
    let steps = "  a:\n    op: levels\n    input: b\n  b:\n    op: levels\n    input: a\n";
    let d = err(steps, "  base_color: a\n  roughness: a\n  height: a\n");
    assert_eq!(d.code.as_str(), "L1285");
    assert!(d.message.contains("a → b → a"), "{}", d.message);
}

fn sub_step(path: &str) -> String {
    format!("  n:\n    op: recipe\n    path: {path}\n    output: x\n")
}

#[test]
fn cross_file_cycle_is_graph() {
    let r = MemResolver::with(&[
        ("a.texture.yaml", &recipe(&sub_step("b.texture.yaml"), OUT)),
        ("b.texture.yaml", &recipe(&sub_step("a.texture.yaml"), OUT)),
    ]);
    let d = first_error(&recipe(&sub_step("a.texture.yaml"), OUT), &r);
    assert_eq!(d.code.as_str(), "L1285");
    assert!(d.message.contains("references itself"), "{}", d.message);
    assert!(
        d.message
            .contains("a.texture.yaml → b.texture.yaml → a.texture.yaml"),
        "{}",
        d.message
    );
}

#[test]
fn missing_file_and_std_are_graph() {
    let d = err(&sub_step("nope.texture.yaml"), OUT);
    assert_eq!(
        (d.code.as_str(), d.path.as_deref()),
        ("L1285", Some("/steps/n/path"))
    );
    assert!(d.message.contains("nope.texture.yaml"));
    let d = err(&sub_step("std:bricks"), OUT);
    assert_eq!(d.code.as_str(), "L1285");
    assert!(d.message.contains("std:"));
}

#[test]
fn sub_recipe_diagnostics_are_prefixed() {
    let child = "format: 1\nkind: texture\nsteps:\n  x:\n    op: noise\n    type: value\n    scale: 0\noutputs:\n  base_color: x\n  roughness: x\n  height: x\n";
    let r = MemResolver::with(&[("sub/c.texture.yaml", child)]);
    let d = first_error(&recipe(&sub_step("sub/c.texture.yaml"), OUT), &r);
    assert_eq!(d.code.as_str(), "L1284");
    assert!(
        d.message.starts_with("in `sub/c.texture.yaml`: "),
        "{}",
        d.message
    );
}

#[test]
fn sub_recipe_bindings_are_checked() {
    let child = "format: 1\nkind: texture\nparams:\n  scale: 4\nsteps:\n  x:\n    op: noise\n    type: value\n    scale: $scale\noutputs:\n  base_color: x\n  roughness: x\n  height: x\n";
    let r = MemResolver::with(&[("c.texture.yaml", child)]);
    let call = |extra: &str| {
        recipe(
            &format!(
                "  n:\n    op: recipe\n    path: c.texture.yaml\n    output: x\n    params:\n{extra}"
            ),
            OUT,
        )
    };
    let d = first_error(&call("      scal: 3\n"), &r);
    assert_eq!(
        (d.code.as_str(), d.path.as_deref()),
        ("L1283", Some("/steps/n/params/scal"))
    );
    assert!(d.help.unwrap().contains("scale"));
    let d = first_error(&call("      scale: \"#ffffff\"\n"), &r);
    assert_eq!(
        (d.code.as_str(), d.path.as_deref()),
        ("L1284", Some("/steps/n/params/scale"))
    );
    let d = first_error(&call("      scale: 0\n"), &r);
    assert!(
        d.message.starts_with("in `c.texture.yaml`: "),
        "{}",
        d.message
    );
    let steps = "  n:\n    op: recipe\n    path: c.texture.yaml\n    output: y\n";
    let d = first_error(&recipe(steps, OUT), &r);
    assert_eq!(
        (d.code.as_str(), d.path.as_deref()),
        ("L1285", Some("/steps/n/output"))
    );
}

#[test]
fn build_size_is_checked() {
    let d = ludion_texgen::build(&recipe(&noise(""), OUT), &MemResolver::default(), 4)
        .unwrap_err()
        .remove(0);
    assert_eq!(
        (d.code.as_str(), d.path.as_deref()),
        ("L1284", Some("/size"))
    );
}

#[test]
fn odd_rows_with_an_offset_is_data() {
    let d = err(
        "  n:\n    op: tile_sampler\n    columns: 4\n    rows: 3\n    offset: 0.5\n",
        OUT,
    );
    assert_eq!(
        (d.code.as_str(), d.path.as_deref()),
        ("L1284", Some("/steps/n/offset"))
    );
}

#[test]
fn unknown_shape_suggests_nearest() {
    let d = err(
        "  n:\n    op: tile_sampler\n    columns: 4\n    rows: 4\n    shape: dsic\n",
        OUT,
    );
    assert_eq!(
        (d.code.as_str(), d.path.as_deref()),
        ("L1283", Some("/steps/n/shape"))
    );
    assert!(d.help.unwrap().contains("disc"));
}

#[test]
fn image_files_report_missing_and_undecodable() {
    let step = |path: &str| format!("  n:\n    op: image\n    path: {path}\n");
    let d = err(&step("nope.png"), OUT);
    assert_eq!(
        (d.code.as_str(), d.path.as_deref()),
        ("L1285", Some("/steps/n/path"))
    );
    let resolver = MemResolver::default().with_image("bad.png", b"not a png".to_vec());
    let d = first_error(&recipe(&step("bad.png"), OUT), &resolver);
    assert_eq!(
        (d.code.as_str(), d.path.as_deref()),
        ("L1284", Some("/steps/n/path"))
    );
}

#[test]
fn flood_fill_needs_a_gray_input() {
    let steps =
        "  c:\n    op: constant\n    value: \"#ff0000\"\n  n:\n    op: flood_fill\n    input: c\n";
    let d = err(steps, OUT);
    assert_eq!(
        (d.code.as_str(), d.path.as_deref()),
        ("L1284", Some("/steps/n/input"))
    );
}

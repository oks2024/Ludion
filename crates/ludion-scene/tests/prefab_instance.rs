//! Prefab-instance entries (`prefab:` key, `Plan/transform-hierarchy-and-prefabs.md` §J): canonical
//! byte-idempotence in scenes and prefabs, every `L1231` shape keyed on its JSON Pointer, the
//! override checks `validate` owns, and the `instantiate_into` refusal of an unexpanded entry.

mod common;

use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::math::Trs;
use ludion_ecs::{Registry, World};
use ludion_scene::{
    InstanceHooks, InstantiateOptions, check, check_prefab, emit, emit_prefab, instantiate_into,
    normalize_prefab_path, parse, parse_prefab,
};

fn registry() -> Registry {
    let mut registry = common::registry();
    registry.register::<ludion_ecs::Parent>("Parent").unwrap();
    registry
}

fn has(diagnostics: &[Diagnostic], code: &str, pointer: &str) -> bool {
    diagnostics
        .iter()
        .any(|d| d.code.as_str() == code && d.path.as_deref() == Some(pointer))
}

const SCENE: &str = "\
format: 1
entities:
  - name: plain
    components:
      Health: 3
  - name: lamp-plaza-1
    prefab: props/lamp.prefab.yaml
  - name: lamp-plaza-3
    prefab: props/lamp.prefab.yaml
    transform:
      translation:
        - 12.0
        - 0.0
        - -4.0
      rotation:
        - 0.0
        - 90.0
        - 0.0
    overrides:
      bulb:
        Transform:
          y: 2.0
      shade/rim:
        Sprite:
          layer: 4
        Health: 9
    removed:
      - base
      - shade
";

const PREFAB: &str = "\
format: 1
kind: prefab
name: street
entities:
  - name: pole
    components:
      Health: 1
  - name: lamp
    prefab: props/lamp.prefab.yaml
    transform:
      scale:
        - 2.0
        - 2.0
        - 2.0
    removed:
      - bulb
";

#[test]
fn a_scene_with_instances_round_trips_byte_identically() {
    let registry = registry();
    let scene = check(SCENE, &registry).expect("the scene is valid");
    let instance = scene.entities[2].instance.as_ref().unwrap();
    assert_eq!(instance.prefab, "props/lamp.prefab.yaml");
    assert_eq!(instance.transform.translation, [12.0, 0.0, -4.0]);
    assert_eq!(instance.removed, ["base", "shade"]);
    assert!(scene.entities[2].components.is_empty());
    assert_eq!(emit(&scene, &registry), SCENE);
}

#[test]
fn a_prefab_with_a_nested_instance_round_trips_byte_identically() {
    let registry = registry();
    let doc = check_prefab(PREFAB, &registry).expect("the prefab is valid");
    assert_eq!(emit_prefab(&doc, &registry), PREFAB);
}

#[test]
fn a_default_transform_vanishes_and_overrides_reorder() {
    let registry = registry();
    let written = "\
format: 1
entities:
  - name: lamp
    prefab: lamp.prefab.yaml
    transform:
      translation:
        - 0.0
        - 0
        - 0.0
      scale:
        - 1.0
        - 1.0
        - 1.0
    overrides:
      shade:
        Health: 2
        Transform:
          x: 1.0
      bulb:
        Health: 1
    removed:
      - zed
      - alpha
";
    let canonical = "\
format: 1
entities:
  - name: lamp
    prefab: lamp.prefab.yaml
    overrides:
      bulb:
        Health: 1
      shade:
        Transform:
          x: 1.0
        Health: 2
    removed:
      - alpha
      - zed
";
    let scene = check(written, &registry).unwrap();
    let emitted = emit(&scene, &registry);
    assert_eq!(emitted, canonical);
    assert_eq!(
        emit(&check(&emitted, &registry).unwrap(), &registry),
        emitted
    );
}

#[test]
fn to_trs_converts_euler_degrees_and_scale() {
    let scene = parse(SCENE).unwrap();
    let trs = scene.entities[2]
        .instance
        .as_ref()
        .unwrap()
        .transform
        .to_trs();
    assert_eq!(trs.translation, [12.0, 0.0, -4.0]);
    assert_eq!(trs.scale, [1.0; 3]);
    let identity = scene.entities[1]
        .instance
        .as_ref()
        .unwrap()
        .transform
        .to_trs();
    assert_eq!(identity, Trs::IDENTITY);
    assert!((trs.rotation[1] - 0.707_106_77).abs() < 1e-6);
}

#[test]
fn prefab_paths_normalize() {
    assert_eq!(
        normalize_prefab_path("./a//b\\c.prefab.yaml"),
        "a/b/c.prefab.yaml"
    );
}

#[test]
fn prefab_paths_collapse_parent_segments() {
    assert_eq!(
        normalize_prefab_path("a/../b.prefab.yaml"),
        normalize_prefab_path("b.prefab.yaml")
    );
}

fn parse_errors(body: &str) -> Vec<Diagnostic> {
    parse(&format!("format: 1\nentities:\n{body}")).expect_err("expected diagnostics")
}

#[test]
fn a_missing_name_is_l1231_with_a_hint() {
    let diagnostics = parse_errors("  - prefab: a.prefab.yaml\n");
    assert!(has(&diagnostics, "L1231", "/entities/0"));
    assert!(
        diagnostics[0]
            .help
            .as_deref()
            .unwrap()
            .contains("add `name:")
    );
}

#[test]
fn a_slash_in_the_name_is_l1231() {
    let diagnostics = parse_errors("  - name: a/b\n    prefab: a.prefab.yaml\n");
    assert!(has(&diagnostics, "L1231", "/entities/0/name"));
}

#[test]
fn components_beside_prefab_is_l1231() {
    let diagnostics = parse_errors("  - name: a\n    prefab: a.prefab.yaml\n    components: {}\n");
    assert!(has(&diagnostics, "L1231", "/entities/0/components"));
    assert!(
        diagnostics[0]
            .help
            .as_deref()
            .unwrap()
            .contains("overrides: <part>:")
    );
}

#[test]
fn bad_transform_arity_is_l1231() {
    let diagnostics = parse_errors(
        "  - name: a\n    prefab: a.prefab.yaml\n    transform:\n      translation:\n        - 1.0\n        - 2.0\n",
    );
    assert!(has(
        &diagnostics,
        "L1231",
        "/entities/0/transform/translation"
    ));
}

#[test]
fn a_duplicate_instance_name_is_l1231_at_the_later_one() {
    let diagnostics = parse_errors(
        "  - name: a\n    prefab: a.prefab.yaml\n  - name: a\n    prefab: b.prefab.yaml\n",
    );
    assert!(has(&diagnostics, "L1231", "/entities/1/name"));
    assert_eq!(diagnostics.len(), 1);
}

#[test]
fn a_duplicate_removed_entry_is_l1231() {
    let diagnostics = parse_errors(
        "  - name: a\n    prefab: a.prefab.yaml\n    removed:\n      - x\n      - x\n",
    );
    assert!(has(&diagnostics, "L1231", "/entities/0/removed/1"));
}

#[test]
fn an_unknown_key_suggests_the_nearest() {
    let diagnostics = parse_errors("  - name: a\n    prefab: a.prefab.yaml\n    remvoed: []\n");
    assert!(has(&diagnostics, "L1231", "/entities/0/remvoed"));
    assert_eq!(
        diagnostics[0].help.as_deref(),
        Some("nearest match is `removed`")
    );
}

#[test]
fn a_prefab_typo_on_a_plain_entity_suggests_prefab() {
    let diagnostics = parse_errors("  - name: a\n    prefb: a.prefab.yaml\n");
    assert_eq!(
        diagnostics[0].help.as_deref(),
        Some("nearest match is `prefab`")
    );
}

#[test]
fn a_prefab_instance_name_clash_is_the_part_rule() {
    let diagnostics = parse_prefab(
        "format: 1\nkind: prefab\nentities:\n  - name: a\n    prefab: a.prefab.yaml\n  - name: a\n    prefab: b.prefab.yaml\n",
    )
    .unwrap_err();
    assert!(has(&diagnostics, "L1228", "/entities/1/name"));
}

fn validate_errors(body: &str) -> Vec<Diagnostic> {
    check(&format!("format: 1\nentities:\n{body}"), &registry()).expect_err("expected diagnostics")
}

#[test]
fn an_unknown_override_component_is_l1013() {
    let diagnostics = validate_errors(
        "  - name: a\n    prefab: a.prefab.yaml\n    overrides:\n      bulb:\n        Nope: 1\n",
    );
    assert!(has(
        &diagnostics,
        "L1013",
        "/entities/0/overrides/bulb/Nope"
    ));
}

#[test]
fn an_out_of_range_override_ref_is_l1017() {
    let diagnostics = validate_errors(
        "  - name: a\n    prefab: a.prefab.yaml\n    overrides:\n      bulb:\n        Parent:\n          index: 99\n          generation: 0\n",
    );
    assert!(has(
        &diagnostics,
        "L1017",
        "/entities/0/overrides/bulb/Parent/index"
    ));
}

#[test]
fn a_partial_override_is_not_an_error_here() {
    let source = "format: 1\nentities:\n  - name: a\n    prefab: a.prefab.yaml\n    overrides:\n      bulb:\n        Sprite:\n          layer: 2\n";
    assert!(check(source, &registry()).is_ok());
}

struct NoHooks;

impl InstanceHooks for NoHooks {
    fn name(&self, _: &mut World, _: EntityId, _: &str) {}
    fn place(&self, _: &mut World, _: EntityId, _: &Trs) {}
}

#[test]
fn instantiate_into_refuses_an_unexpanded_instance() {
    let registry = registry();
    let scene = check(SCENE, &registry).unwrap();
    let mut world = World::new();
    let diagnostics = instantiate_into(
        &mut world,
        &scene.entities,
        &registry,
        &InstantiateOptions {
            placement: None,
            name_prefix: None,
            hooks: &NoHooks,
        },
    )
    .unwrap_err();
    assert!(has(&diagnostics, "L1231", "/entities/1/prefab"));
    assert!(has(&diagnostics, "L1231", "/entities/2/prefab"));
    assert_eq!(world.entity_count(), 0);
}

#[test]
fn a_negative_zero_translation_is_written_and_read_back() {
    let registry = registry();
    let source = "\
format: 1
entities:
  - name: lamp
    prefab: lamp.prefab.yaml
    transform:
      translation:
        - -0.0
        - 0.0
        - 0.0
";
    let scene = check(source, &registry).expect("the scene is valid");
    let instance = scene.entities[0].instance.as_ref().unwrap();
    assert!(instance.transform.translation[0].is_sign_negative());
    assert_eq!(emit(&scene, &registry), source);
}

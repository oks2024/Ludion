//! Canonical-form and diagnostic properties for prefab documents (`kind: prefab`,
//! `Plan/transform-hierarchy-and-prefabs.md` §H): canonical byte-idempotence and every
//! `L1226`–`L1228` rule keyed on its JSON Pointer, with the scene's own codes inside the body.

mod common;

use ludion_core::diagnostics::Diagnostic;
use ludion_ecs::Registry;
use ludion_scene::{check_prefab, emit_prefab, parse_prefab};

fn registry() -> Registry {
    let mut registry = common::registry();
    registry.register::<ludion_ecs::Parent>("Parent").unwrap();
    registry
}

const CANONICAL: &str = "\
format: 1
kind: prefab
name: goblin
entities:
  - name: pelvis
    components:
      Transform:
        x: 0.0
        y: 1.0
        z: 0.0
  - name: arm
    components:
      Transform:
        x: 0.5
        y: 0.0
        z: 0.0
      Parent:
        index: 0
        generation: 0
";

fn has(diagnostics: &[Diagnostic], code: &str, pointer: &str) -> bool {
    diagnostics
        .iter()
        .any(|d| d.code.as_str() == code && d.path.as_deref() == Some(pointer))
}

fn prefab(body: &str) -> String {
    format!("format: 1\nkind: prefab\n{body}")
}

#[test]
fn canonical_text_round_trips_byte_identically() {
    let registry = registry();
    let doc = check_prefab(CANONICAL, &registry).expect("the prefab is valid");
    assert_eq!(doc.name.as_deref(), Some("goblin"));
    assert_eq!(doc.entities.len(), 2);
    assert_eq!(emit_prefab(&doc, &registry), CANONICAL);
}

#[test]
fn component_order_is_normalized_and_stable() {
    let registry = registry();
    let swapped = CANONICAL.replace(
        "      Transform:\n        x: 0.5\n        y: 0.0\n        z: 0.0\n      Parent:\n        index: 0\n        generation: 0\n",
        "      Parent:\n        index: 0\n        generation: 0\n      Transform:\n        x: 0.5\n        y: 0.0\n        z: 0.0\n",
    );
    assert_ne!(swapped, CANONICAL);
    let doc = check_prefab(&swapped, &registry).unwrap();
    let emitted = emit_prefab(&doc, &registry);
    assert_eq!(emitted, CANONICAL);
    let again = check_prefab(&emitted, &registry).unwrap();
    assert_eq!(emit_prefab(&again, &registry), emitted);
}

#[test]
fn an_empty_prefab_is_refused() {
    let errors = parse_prefab(&prefab("entities: []\n")).unwrap_err();
    assert!(has(&errors, "L1390", "/entities"), "{errors:?}");
}

#[test]
fn scene_only_keys_are_l1226() {
    let errors =
        parse_prefab(&prefab("seed: 1\nphysics:\n  space: free\nentities: []\n")).unwrap_err();
    assert!(has(&errors, "L1226", "/seed"), "{errors:?}");
    assert!(has(&errors, "L1226", "/physics"), "{errors:?}");
}

#[test]
fn generated_from_round_trips_after_name() {
    let registry = registry();
    let text = "format: 1\nkind: prefab\nname: model\ngenerated_from: \"../scene.gltf\"\nentities:\n  - name: root\n    components: {}\n";
    let doc = check_prefab(text, &registry).unwrap();
    assert_eq!(doc.generated_from.as_deref(), Some("../scene.gltf"));
    assert_eq!(emit_prefab(&doc, &registry), text);
}

#[test]
fn a_wrong_or_missing_envelope_is_l1226() {
    let errors = parse_prefab("format: 1\nkind: scene\nentities: []\n").unwrap_err();
    assert!(has(&errors, "L1226", "/kind"), "{errors:?}");
    let errors = parse_prefab("format: 1\nentities: []\n").unwrap_err();
    assert!(has(&errors, "L1226", ""), "{errors:?}");
    let errors = parse_prefab(&prefab("entites: []\n")).unwrap_err();
    assert!(has(&errors, "L1226", "/entites"), "{errors:?}");
    assert!(parse_prefab("").is_err());
}

#[test]
fn a_bad_part_name_is_l1227() {
    let errors = parse_prefab(&prefab("entities:\n  - components: {}\n")).unwrap_err();
    assert!(has(&errors, "L1227", "/entities/0"), "{errors:?}");
    let errors =
        parse_prefab(&prefab("entities:\n  - name: \"\"\n    components: {}\n")).unwrap_err();
    assert!(has(&errors, "L1227", "/entities/0/name"), "{errors:?}");
    let errors =
        parse_prefab(&prefab("entities:\n  - name: a/b\n    components: {}\n")).unwrap_err();
    assert!(has(&errors, "L1227", "/entities/0/name"), "{errors:?}");
}

#[test]
fn a_duplicate_part_name_is_l1228_at_the_later_part() {
    let errors = parse_prefab(&prefab(
        "entities:\n  - name: a\n    components: {}\n  - name: b\n    components: {}\n  - name: a\n    components: {}\n",
    ))
    .unwrap_err();
    assert!(has(&errors, "L1228", "/entities/2/name"), "{errors:?}");
    assert!(errors[0].message.contains("0 and 2"), "{errors:?}");
}

#[test]
fn the_bodys_references_range_over_the_prefab_and_components_are_checked() {
    let registry = registry();
    let source = prefab(
        "entities:\n  - name: a\n    components:\n      Nope: {}\n  - name: b\n    components:\n      Parent:\n        index: 5\n        generation: 0\n",
    );
    let errors = check_prefab(&source, &registry).unwrap_err();
    assert!(
        has(&errors, "L1013", "/entities/0/components/Nope"),
        "{errors:?}"
    );
    assert!(
        errors
            .iter()
            .any(|d| d.path.as_deref() == Some("/entities/1/components/Parent/index")),
        "{errors:?}"
    );
}

#[test]
fn several_errors_come_back_in_one_pass() {
    let errors = parse_prefab(&prefab(
        "seed: 1\nentities:\n  - components: {}\n  - name: a/b\n    components: {}\n",
    ))
    .unwrap_err();
    assert!(has(&errors, "L1226", "/seed"));
    assert!(has(&errors, "L1227", "/entities/0"));
    assert!(has(&errors, "L1227", "/entities/1/name"));
}

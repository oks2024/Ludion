//! `PrefabDocument::apply_instance`: an instance's collapsed overrides and `removed` parts written
//! into the prefab — plain merges, gained components, nested entries, and removal shifting `Parent`.

mod common;

use std::collections::BTreeMap;

use ludion_core::codes::code;
use ludion_ecs::Registry;
use ludion_scene::{PrefabDocument, check_prefab, emit_prefab};
use serde_json::{Value, json};

fn registry() -> Registry {
    let mut registry = common::registry();
    registry.register::<ludion_ecs::Parent>("Parent").unwrap();
    registry
}

fn load(source: &str) -> PrefabDocument {
    check_prefab(source, &registry()).unwrap_or_else(|d| panic!("{d:?}"))
}

fn overrides(items: &[(&str, &[(&str, Value)])]) -> BTreeMap<String, Vec<(String, Value)>> {
    items
        .iter()
        .map(|(path, components)| {
            (
                (*path).to_owned(),
                components
                    .iter()
                    .map(|(name, value)| ((*name).to_owned(), value.clone()))
                    .collect(),
            )
        })
        .collect()
}

const LAMP: &str = "\
format: 1
kind: prefab
entities:
  - name: post
    components:
      Transform:
        x: 0.0
        y: 1.0
        z: 0.0
  - name: bulb
    components:
      Parent:
        index: 0
        generation: 0
      Sprite:
        texture: glass
        layer: 1
        visible: true
  - name: shade
    components:
      Parent:
        index: 1
        generation: 0
";

const TURRET: &str = "\
format: 1
kind: prefab
entities:
  - name: base
    components:
      Health: 1
  - name: barrel
    prefab: barrel.prefab.yaml
    overrides:
      muzzle:
        Health: 4
";

#[test]
fn a_plain_override_merges_into_the_part() {
    let mut doc = load(LAMP);
    let edits = overrides(&[("post", &[("Transform", json!({"y": 2.5}))])]);
    doc.apply_instance(&edits, &[], &registry()).unwrap();
    let transform = &doc.entities[0].components[0];
    assert_eq!(transform.0, "Transform");
    assert_eq!(transform.1, json!({"x": 0.0, "y": 2.5, "z": 0.0}));
}

#[test]
fn a_gained_component_is_added_whole() {
    let mut doc = load(LAMP);
    let edits = overrides(&[("post", &[("Health", json!(9))])]);
    doc.apply_instance(&edits, &[], &registry()).unwrap();
    assert!(
        doc.entities[0]
            .components
            .iter()
            .any(|(name, value)| name == "Health" && *value == json!(9))
    );
    let text = emit_prefab(&doc, &registry());
    assert_eq!(
        emit_prefab(&check_prefab(&text, &registry()).unwrap(), &registry()),
        text
    );
}

#[test]
fn a_nested_override_merges_onto_the_existing_one() {
    let mut doc = load(TURRET);
    let edits = overrides(&[
        ("barrel/muzzle", &[("Health", json!(7))]),
        ("barrel/tube", &[("Health", json!(2))]),
    ]);
    doc.apply_instance(&edits, &[], &registry()).unwrap();
    let entry = doc.entities[1].instance.as_ref().unwrap();
    assert_eq!(
        entry.overrides["muzzle"],
        vec![("Health".to_owned(), json!(7))]
    );
    assert_eq!(
        entry.overrides["tube"],
        vec![("Health".to_owned(), json!(2))]
    );
}

#[test]
fn a_plain_removal_shifts_the_parents_above_it() {
    let mut doc = load(
        "format: 1\nkind: prefab\nentities:\n  - name: a\n    components: {}\n  - name: b\n    components: {}\n  - name: c\n    components:\n      Parent:\n        index: 0\n        generation: 0\n",
    );
    doc.apply_instance(&BTreeMap::new(), &["b".to_owned()], &registry())
        .unwrap();
    assert_eq!(doc.entities.len(), 2);
    assert_eq!(doc.entities[1].name.as_deref(), Some("c"));
    assert_eq!(
        doc.entities[1].components[0].1,
        json!({"index": 0, "generation": 0})
    );

    let mut doc = load(
        "format: 1\nkind: prefab\nentities:\n  - name: a\n    components: {}\n  - name: b\n    components: {}\n  - name: c\n    components:\n      Parent:\n        index: 1\n        generation: 0\n",
    );
    doc.apply_instance(&BTreeMap::new(), &["a".to_owned()], &registry())
        .unwrap();
    assert_eq!(
        doc.entities[1].components[0].1,
        json!({"index": 0, "generation": 0})
    );
}

#[test]
fn removing_a_part_still_referenced_is_refused() {
    let mut doc = load(LAMP);
    let error = doc
        .apply_instance(&BTreeMap::new(), &["post".to_owned()], &registry())
        .unwrap_err();
    assert_eq!(error.code, code::PREFAB_APPLY);
    assert!(error.message.contains("still refers"), "{}", error.message);
}

#[test]
fn a_nested_removal_lands_on_the_entry_and_drops_its_override() {
    let mut doc = load(TURRET);
    doc.apply_instance(&BTreeMap::new(), &["barrel/muzzle".to_owned()], &registry())
        .unwrap();
    let entry = doc.entities[1].instance.as_ref().unwrap();
    assert_eq!(entry.removed, vec!["muzzle".to_owned()]);
    assert!(entry.overrides.is_empty());
}

#[test]
fn a_removed_entry_name_deletes_the_whole_entry() {
    let mut doc = load(TURRET);
    doc.apply_instance(&BTreeMap::new(), &["barrel".to_owned()], &registry())
        .unwrap();
    assert_eq!(doc.entities.len(), 1);
    assert_eq!(doc.entities[0].name.as_deref(), Some("base"));
}

#[test]
fn an_unknown_path_is_l1237() {
    let mut doc = load(LAMP);
    let edits = overrides(&[("socket", &[("Health", json!(1))])]);
    let error = doc.apply_instance(&edits, &[], &registry()).unwrap_err();
    assert_eq!(error.code, code::PREFAB_APPLY);
    assert!(
        error.message.contains("post, bulb, shade"),
        "{}",
        error.message
    );
    let error = doc
        .apply_instance(&BTreeMap::new(), &["nowhere/x".to_owned()], &registry())
        .unwrap_err();
    assert_eq!(error.code, code::PREFAB_APPLY);
}

//! One golden per schema shape the canonical emitter has to order: a scrambled `INPUT` scene, the
//! inline `CANONICAL` text, and three assertions per shape — the file path (`emit(check(INPUT))`),
//! the fixed point (`emit(check(CANONICAL))`), and the serde path (deserialize the component,
//! serialize it back, emit), so a file and `scene.save` agree byte for byte.
//!
//! Each shape also names the schema keyword `schemars` is expected to emit at its field: a
//! `schemars` bump that reshapes a schema fails with that message instead of a bare text diff.

use std::collections::BTreeMap;

use ludion_ecs::Registry;
use ludion_scene::{Scene, SceneEntity, check, emit};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

// --- (a) flat struct, declaration order != alphabetical ---------------------------------------

#[derive(Serialize, Deserialize, JsonSchema)]
struct Flat {
    zeta: f32,
    alpha: String,
    mid: i32,
}

// --- (b) nested struct through `$ref` ---------------------------------------------------------

#[derive(Serialize, Deserialize, JsonSchema, Clone)]
struct Inner {
    y: i32,
    x: i32,
}

#[derive(Serialize, Deserialize, JsonSchema)]
struct Nested {
    second: Inner,
    first: Inner,
}

// --- (c) externally tagged enum with struct variants (Joint-like) -----------------------------

#[derive(Serialize, Deserialize, JsonSchema, Clone)]
#[serde(rename_all = "snake_case")]
enum Kind {
    Hinge {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limits: Option<[f32; 2]>,
        axis: [f32; 3],
    },
    Ball {
        twist: i32,
        cone: i32,
    },
}

#[derive(Serialize, Deserialize, JsonSchema)]
struct Joint {
    kind: Kind,
}

// --- (d) enum mixing unit and struct variants -------------------------------------------------

#[derive(Serialize, Deserialize, JsonSchema)]
enum Mode {
    Idle,
    Run { speed: f32, dir: String },
}

#[derive(Serialize, Deserialize, JsonSchema)]
struct Machine {
    previous: Mode,
    current: Mode,
}

// --- (e) internally tagged enum ---------------------------------------------------------------

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Tagged {
    A { zeta: i32, alpha: i32 },
    B { beta: i32, aleph: i32 },
}

#[derive(Serialize, Deserialize, JsonSchema)]
struct TaggedHolder {
    tagged: Tagged,
}

// --- (f) untagged enum, two variants with distinct keys ---------------------------------------

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
enum Either {
    Point { y: i32, x: i32 },
    Named { label: String, id: i32 },
}

#[derive(Serialize, Deserialize, JsonSchema)]
struct EitherHolder {
    either: Either,
}

// --- (g) `Option<Struct>` and (h) `Option<Enum>` ----------------------------------------------

#[derive(Serialize, Deserialize, JsonSchema)]
struct MaybeStruct {
    maybe: Option<Inner>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
struct MaybeEnum {
    maybe: Option<Kind>,
}

// --- (i) `BTreeMap<String, Struct>` -----------------------------------------------------------

#[derive(Serialize, Deserialize, JsonSchema)]
struct Entry {
    glyph: String,
    cost: i32,
}

#[derive(Serialize, Deserialize, JsonSchema)]
struct Legend {
    legend: BTreeMap<String, Entry>,
}

// --- (j) `Vec<Enum>` --------------------------------------------------------------------------

#[derive(Serialize, Deserialize, JsonSchema)]
struct Chain {
    links: Vec<Kind>,
}

// --- (k) `#[serde(flatten)]` ------------------------------------------------------------------

#[derive(Serialize, Deserialize, JsonSchema)]
struct Flattened {
    id: i32,
    #[serde(flatten)]
    inner: Inner,
}

// --- (l) every f32 position, and an f64 beside them -------------------------------------------

#[derive(Serialize, Deserialize, JsonSchema)]
struct Probe {
    v: f32,
}

#[derive(Serialize, Deserialize, JsonSchema)]
struct Floats {
    single: f32,
    maybe: Option<f32>,
    triple: [f32; 3],
    by_name: BTreeMap<String, f32>,
    mode: Mode,
    nested: Probe,
    wide: f64,
}

// --- Harness ----------------------------------------------------------------------------------

fn registry() -> Registry {
    let mut registry = Registry::new();
    registry.register::<Floats>("Floats").unwrap();
    registry.register::<Flat>("Flat").unwrap();
    registry.register::<Nested>("Nested").unwrap();
    registry.register::<Joint>("Joint").unwrap();
    registry.register::<Machine>("Machine").unwrap();
    registry.register::<TaggedHolder>("TaggedHolder").unwrap();
    registry.register::<EitherHolder>("EitherHolder").unwrap();
    registry.register::<MaybeStruct>("MaybeStruct").unwrap();
    registry.register::<MaybeEnum>("MaybeEnum").unwrap();
    registry.register::<Legend>("Legend").unwrap();
    registry.register::<Chain>("Chain").unwrap();
    registry.register::<Flattened>("Flattened").unwrap();
    registry
}

/// Asserts the schema of `component` holds `keyword` at JSON `pointer` (local `$ref`s chased), so a
/// `schemars` change of shape is named, not diffed.
fn assert_schema_shape(component: &str, pointer: &str, keyword: &str) {
    let registry = registry();
    let root = registry
        .get(component)
        .unwrap_or_else(|| panic!("{component} is registered"))
        .schema();
    let mut node = root
        .pointer(pointer)
        .unwrap_or_else(|| panic!("{component}: no schema at {pointer}: {root}"));
    while let Some(target) = node
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|reference| reference.strip_prefix('#'))
        .and_then(|target| root.pointer(target))
    {
        node = target;
    }
    assert!(
        node.get(keyword).is_some(),
        "schemars no longer emits `{keyword}` at {component}{pointer} — the emitter's schema walk \
         needs revisiting; the node is {node}"
    );
}

fn emit_component(registry: &Registry, name: &str, value: Value) -> String {
    let scene = Scene {
        name: None,
        generated_from: None,
        seed: None,
        physics: None,
        entities: vec![SceneEntity {
            presentation: false,
            name: None,
            components: vec![(name.to_string(), value)],
            instance: None,
        }],
    };
    emit(&scene, registry)
}

fn canonicalize(registry: &Registry, source: &str) -> String {
    let scene = check(source, registry).unwrap_or_else(|errors| {
        panic!("the scene does not validate: {errors:?}\n{source}");
    });
    emit(&scene, registry)
}

/// The three assertions of a shape golden.
fn golden<T: Serialize + DeserializeOwned>(component: &str, input: &str, canonical: &str) {
    let registry = registry();
    assert_eq!(
        canonicalize(&registry, input),
        canonical,
        "{component}: emit(check(INPUT)) is not CANONICAL"
    );
    assert_eq!(
        canonicalize(&registry, canonical),
        canonical,
        "{component}: CANONICAL is not a fixed point"
    );
    let scene = check(input, &registry).unwrap();
    let value = scene.entities[0].components[0].1.clone();
    let typed: T = serde_json::from_value(value).expect("the checked value deserializes");
    let through_serde = serde_json::to_value(&typed).expect("the component serializes");
    assert_eq!(
        emit_component(&registry, component, through_serde),
        canonical,
        "{component}: the serde path (what scene.save writes) is not CANONICAL"
    );
}

#[test]
fn flat_struct_follows_declaration_order() {
    const INPUT: &str = "\
format: 1
entities:
  - components:
      Flat:
        alpha: a
        mid: 3
        zeta: 1.5
";
    const CANONICAL: &str = "\
format: 1
entities:
  - components:
      Flat:
        zeta: 1.5
        alpha: a
        mid: 3
";
    assert_schema_shape("Flat", "", "properties");
    golden::<Flat>("Flat", INPUT, CANONICAL);
}

#[test]
fn nested_struct_orders_through_a_ref() {
    const INPUT: &str = "\
format: 1
entities:
  - components:
      Nested:
        first:
          x: 1
          y: 2
        second:
          x: 3
          y: 4
";
    const CANONICAL: &str = "\
format: 1
entities:
  - components:
      Nested:
        second:
          y: 4
          x: 3
        first:
          y: 2
          x: 1
";
    assert_schema_shape("Nested", "/properties/second", "properties");
    golden::<Nested>("Nested", INPUT, CANONICAL);
}

#[test]
fn externally_tagged_enum_orders_the_variants_fields() {
    const INPUT: &str = "\
format: 1
entities:
  - components:
      Joint:
        kind:
          hinge:
            axis:
              - 0.5
              - 1.5
              - 2.5
            limits:
              - -1.5
              - 2.5
";
    const CANONICAL: &str = "\
format: 1
entities:
  - components:
      Joint:
        kind:
          hinge:
            limits:
              - -1.5
              - 2.5
            axis:
              - 0.5
              - 1.5
              - 2.5
";
    assert_schema_shape("Joint", "/properties/kind", "oneOf");
    golden::<Joint>("Joint", INPUT, CANONICAL);
}

#[test]
fn enum_mixing_unit_and_struct_variants() {
    const INPUT: &str = "\
format: 1
entities:
  - components:
      Machine:
        current:
          Run:
            dir: n
            speed: 1.5
        previous: Idle
";
    const CANONICAL: &str = "\
format: 1
entities:
  - components:
      Machine:
        previous: Idle
        current:
          Run:
            speed: 1.5
            dir: n
";
    assert_schema_shape("Machine", "/properties/current", "oneOf");
    golden::<Machine>("Machine", INPUT, CANONICAL);
}

#[test]
fn internally_tagged_enum_puts_the_tag_first() {
    const INPUT: &str = "\
format: 1
entities:
  - components:
      TaggedHolder:
        tagged:
          alpha: 1
          zeta: 2
          type: a
";
    const CANONICAL: &str = "\
format: 1
entities:
  - components:
      TaggedHolder:
        tagged:
          type: a
          zeta: 2
          alpha: 1
";
    assert_schema_shape("TaggedHolder", "/properties/tagged", "oneOf");
    golden::<TaggedHolder>("TaggedHolder", INPUT, CANONICAL);
}

#[test]
fn untagged_enum_orders_by_the_matching_variant() {
    const INPUT: &str = "\
format: 1
entities:
  - components:
      EitherHolder:
        either:
          x: 1
          y: 2
";
    const CANONICAL: &str = "\
format: 1
entities:
  - components:
      EitherHolder:
        either:
          y: 2
          x: 1
";
    assert_schema_shape("EitherHolder", "/properties/either", "anyOf");
    golden::<EitherHolder>("EitherHolder", INPUT, CANONICAL);
}

#[test]
fn optional_struct_orders_the_inner_fields() {
    const INPUT: &str = "\
format: 1
entities:
  - components:
      MaybeStruct:
        maybe:
          x: 1
          y: 2
";
    const CANONICAL: &str = "\
format: 1
entities:
  - components:
      MaybeStruct:
        maybe:
          y: 2
          x: 1
";
    assert_schema_shape("MaybeStruct", "/properties/maybe", "anyOf");
    golden::<MaybeStruct>("MaybeStruct", INPUT, CANONICAL);
}

#[test]
fn optional_enum_orders_the_variants_fields() {
    const INPUT: &str = "\
format: 1
entities:
  - components:
      MaybeEnum:
        maybe:
          ball:
            cone: 1
            twist: 2
";
    const CANONICAL: &str = "\
format: 1
entities:
  - components:
      MaybeEnum:
        maybe:
          ball:
            twist: 2
            cone: 1
";
    assert_schema_shape("MaybeEnum", "/properties/maybe", "anyOf");
    golden::<MaybeEnum>("MaybeEnum", INPUT, CANONICAL);
}

#[test]
fn map_keys_sort_in_byte_order_and_values_follow_declaration() {
    const INPUT: &str = "\
format: 1
entities:
  - components:
      Legend:
        legend:
          b:
            cost: 2
            glyph: B
          a:
            cost: 1
            glyph: A
          B:
            cost: 3
            glyph: b
";
    const CANONICAL: &str = "\
format: 1
entities:
  - components:
      Legend:
        legend:
          B:
            glyph: b
            cost: 3
          a:
            glyph: A
            cost: 1
          b:
            glyph: B
            cost: 2
";
    assert_schema_shape("Legend", "/properties/legend", "additionalProperties");
    golden::<Legend>("Legend", INPUT, CANONICAL);
}

#[test]
fn a_sequence_of_enums_orders_each_element_by_its_variant() {
    const INPUT: &str = "\
format: 1
entities:
  - components:
      Chain:
        links:
          - ball:
              cone: 1
              twist: 2
          - hinge:
              axis:
                - 0.5
                - 1.5
                - 2.5
              limits:
                - -1.5
                - 2.5
";
    const CANONICAL: &str = "\
format: 1
entities:
  - components:
      Chain:
        links:
          - ball:
              twist: 2
              cone: 1
          - hinge:
              limits:
                - -1.5
                - 2.5
              axis:
                - 0.5
                - 1.5
                - 2.5
";
    assert_schema_shape("Chain", "/properties/links/items", "oneOf");
    golden::<Chain>("Chain", INPUT, CANONICAL);
}

#[test]
fn flattened_struct_keeps_serdes_field_order() {
    const INPUT: &str = "\
format: 1
entities:
  - components:
      Flattened:
        x: 1
        y: 2
        id: 3
";
    const CANONICAL: &str = "\
format: 1
entities:
  - components:
      Flattened:
        id: 3
        y: 2
        x: 1
";
    assert_schema_shape("Flattened", "", "properties");
    golden::<Flattened>("Flattened", INPUT, CANONICAL);
}

/// An f32 field is written as its shortest f32 decimal wherever it sits in the schema (plain,
/// `Option`, array, map value, enum variant, `$ref`'d struct); an `f64` field holding the same
/// widened value keeps all its digits.
#[test]
fn f32_positions_emit_the_shortest_f32_decimal() {
    let widened = f64::from(0.3_f32);
    let value = serde_json::json!({
        "single": widened,
        "maybe": widened,
        "triple": [widened, widened, widened],
        "by_name": { "a": widened },
        "mode": { "Run": { "speed": widened, "dir": "n" } },
        "nested": { "v": widened },
        "wide": widened,
    });
    assert_schema_shape("Floats", "/properties/single", "format");
    assert_schema_shape("Floats", "/properties/maybe", "format");
    let text = emit_component(&registry(), "Floats", value);
    assert_eq!(
        text,
        "\
format: 1
entities:
  - components:
      Floats:
        single: 0.3
        maybe: 0.3
        triple:
          - 0.3
          - 0.3
          - 0.3
        by_name:
          a: 0.3
        mode:
          Run:
            speed: 0.3
            dir: n
        nested:
          v: 0.3
        wide: 0.30000001192092896
"
    );
}

#[test]
fn presentation_entities_emit_the_flag_after_name_and_only_when_true() {
    let registry = registry();
    let source = "format: 1\nentities:\n  - name: popup\n    presentation: true\n    components: {}\n  - presentation: true\n    components: {}\n  - presentation: false\n    components: {}\n";
    let canonical = "format: 1\nentities:\n  - name: popup\n    presentation: true\n    components: {}\n  - presentation: true\n    components: {}\n  - components: {}\n";
    assert_eq!(canonicalize(&registry, source), canonical);
    assert_eq!(canonicalize(&registry, canonical), canonical);
}

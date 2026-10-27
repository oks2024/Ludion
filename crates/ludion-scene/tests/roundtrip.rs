//! Round-trip and canonical-form properties (architecture doc §7):
//!
//! - **value round trip**: scene → emit → parse loses nothing;
//! - **emit idempotence**: `emit(parse(emit(s)))` is *byte-equal* to `emit(s)`;
//! - **floats round-trip exactly** (ryu shortest form, bit-for-bit);
//! - the emitted text is LF-only and validates cleanly against the registry.

mod common;

use common::registry;
use ludion_scene::{
    InstanceEntry, InstanceTransform, Scene, SceneEntity, SceneLayer, ScenePhysics,
    ScenePhysicsSpace, emit, parse, parse_prefab, validate,
};
use proptest::prelude::*;
use serde_json::{Value, json};

// --- Strategies -------------------------------------------------------------------------------

fn finite_f32() -> impl Strategy<Value = f32> {
    any::<f32>().prop_filter("finite", |f| f.is_finite())
}

fn finite_f64() -> impl Strategy<Value = f64> {
    any::<f64>().prop_filter("finite", |f| f.is_finite())
}

fn transform() -> impl Strategy<Value = (String, Value)> {
    (finite_f32(), finite_f32(), finite_f32())
        .prop_map(|(x, y, z)| ("Transform".to_string(), json!({ "x": x, "y": y, "z": z })))
}

fn sprite() -> impl Strategy<Value = (String, Value)> {
    (any::<String>(), any::<i32>(), any::<bool>()).prop_map(|(texture, layer, visible)| {
        (
            "Sprite".to_string(),
            json!({ "texture": texture, "layer": layer, "visible": visible }),
        )
    })
}

fn palette() -> impl Strategy<Value = (String, Value)> {
    (
        proptest::collection::vec(any::<String>(), 0..4),
        proptest::option::of(finite_f64()),
    )
        .prop_map(|(colors, emissive)| {
            (
                "Palette".to_string(),
                json!({ "colors": colors, "emissive": emissive }),
            )
        })
}

fn shape() -> impl Strategy<Value = (String, Value)> {
    prop_oneof![
        finite_f64().prop_map(|radius| json!({ "Circle": { "radius": radius } })),
        (finite_f64(), finite_f64()).prop_map(|(w, h)| json!({ "Rect": { "w": w, "h": h } })),
    ]
    .prop_map(|value| ("Shape".to_string(), value))
}

fn health() -> impl Strategy<Value = (String, Value)> {
    any::<u32>().prop_map(|hp| ("Health".to_string(), json!(hp)))
}

fn entity() -> impl Strategy<Value = SceneEntity> {
    let components = proptest::collection::vec(
        prop_oneof![transform(), sprite(), palette(), shape(), health()],
        0..4,
    )
    .prop_map(|mut components| {
        // Duplicate component names cannot appear in a mapping; keep the first of each.
        let mut seen = ludion_core::collections::StableSet::new();
        components.retain(|(name, _)| seen.insert(name.clone()));
        components
    });
    (
        proptest::option::of(any::<String>()),
        any::<bool>(),
        components,
    )
        .prop_map(|(name, presentation, components)| SceneEntity {
            name,
            presentation,
            components,
            instance: None,
        })
}

/// An instance entry with a placeholder name (`scene()` renames each to a unique `inst-<n>`).
fn instance_entity() -> impl Strategy<Value = SceneEntity> {
    let triple = || (finite_f64(), finite_f64(), finite_f64()).prop_map(|(x, y, z)| [x, y, z]);
    let part = "[a-z]{1,4}(/[a-z]{1,4})?";
    let placement = (triple(), triple(), triple()).prop_map(|(translation, rotation, scale)| {
        InstanceTransform {
            translation,
            rotation,
            scale,
        }
    });
    let overrides = proptest::collection::btree_map(
        part,
        proptest::collection::vec(prop_oneof![transform(), health()], 0..3).prop_map(
            |mut components| {
                let mut seen = ludion_core::collections::StableSet::new();
                components.retain(|(name, _)| seen.insert(name.clone()));
                components
            },
        ),
        0..3,
    );
    let removed = proptest::collection::btree_set(part, 0..3)
        .prop_map(|set| set.into_iter().collect::<Vec<_>>());
    (
        "[a-z]{1,6}/[a-z]{1,6}\\.prefab\\.yaml",
        placement,
        overrides,
        removed,
    )
        .prop_map(|(prefab, transform, overrides, removed)| SceneEntity {
            name: Some("inst".to_string()),
            instance: Some(Box::new(InstanceEntry {
                prefab,
                transform,
                overrides,
                removed,
            })),
            ..Default::default()
        })
}

const LAYER_POOL: [&str; 4] = ["dummy", "sling", "wall", "ghost"];

/// Valid layers only: pool names in pool order, each ignoring a subset of the declared names and
/// `default`, no duplicates.
fn scene_layers() -> impl Strategy<Value = Vec<SceneLayer>> {
    proptest::collection::vec(
        proptest::option::of(proptest::collection::vec(
            any::<bool>(),
            LAYER_POOL.len() + 1,
        )),
        LAYER_POOL.len(),
    )
    .prop_map(|slots| {
        let declared: Vec<bool> = slots.iter().map(Option::is_some).collect();
        slots
            .iter()
            .enumerate()
            .filter_map(|(i, flags)| {
                let flags = flags.as_ref()?;
                let ignores = LAYER_POOL
                    .iter()
                    .enumerate()
                    .filter(|(j, _)| declared[*j] && flags[*j])
                    .map(|(_, name)| (*name).to_string())
                    .chain(flags[LAYER_POOL.len()].then(|| "default".to_string()))
                    .collect();
                Some(SceneLayer {
                    name: LAYER_POOL[i].to_string(),
                    ignores,
                })
            })
            .collect()
    })
}

fn scene_physics() -> impl Strategy<Value = Option<ScenePhysics>> {
    proptest::option::of(
        (
            prop_oneof![
                Just(ScenePhysicsSpace::Free),
                Just(ScenePhysicsSpace::Planar)
            ],
            proptest::option::of(
                (-1e3..1e3f64, -1e3..1e3f64, -1e3..1e3f64).prop_map(|(x, y, z)| [x, y, z]),
            ),
            proptest::option::of(1u32..=64),
            proptest::option::of(1u32..=64),
            proptest::option::of(any::<u32>()),
            scene_layers(),
        )
            .prop_map(
                |(
                    space,
                    gravity,
                    solver_iterations,
                    stabilization_iterations,
                    sleep_after_ticks,
                    layers,
                )| {
                    ScenePhysics {
                        space,
                        gravity,
                        solver_iterations,
                        stabilization_iterations,
                        sleep_after_ticks,
                        layers,
                    }
                },
            ),
    )
}

fn scene() -> impl Strategy<Value = Scene> {
    (
        proptest::option::of(any::<String>()),
        proptest::option::of(any::<String>()),
        proptest::option::of(any::<u64>()),
        scene_physics(),
        proptest::collection::vec(any::<String>(), 0..3),
        proptest::collection::vec(prop_oneof![3 => entity(), 1 => instance_entity()], 0..5),
    )
        .prop_map(
            |(name, generated_from, seed, physics, preload, mut entities)| {
                for (index, entity) in entities.iter_mut().enumerate() {
                    if entity.instance.is_some() {
                        entity.name = Some(format!("inst-{index}"));
                    }
                }
                Scene {
                    name,
                    generated_from,
                    seed,
                    physics,
                    preload,
                    entities,
                }
            },
        )
}

/// A component value at its type's precision: an `f32` field is written as its shortest f32
/// decimal and reads back as that decimal's f64, so a widened f32 and its re-read differ as f64
/// but agree once both pass through the type. Unknown names are untouched.
fn at_type_precision(name: &str, value: &Value) -> Value {
    match name {
        "Transform" => serde_json::to_value(
            serde_json::from_value::<common::Transform>(value.clone()).expect("a Transform value"),
        )
        .unwrap(),
        _ => value.clone(),
    }
}

fn narrow(components: &mut [(String, Value)]) {
    components.sort_by(|(a, _), (b, _)| a.cmp(b));
    for (name, value) in components {
        *value = at_type_precision(name, value);
    }
}

/// Order-insensitive scene comparison at type precision: the emitter reorders components
/// canonically, so compare with each entity's component list sorted by name (values themselves
/// compare order-insensitively — `serde_json::Map` equality is by content). An instance placement
/// is consumed through `f32`, so it compares narrowed.
fn normalized(mut scene: Scene) -> Scene {
    for entity in &mut scene.entities {
        narrow(&mut entity.components);
        if let Some(instance) = &mut entity.instance {
            for components in instance.overrides.values_mut() {
                narrow(components);
            }
            instance.removed.sort();
            let t = &mut instance.transform;
            for slot in t
                .translation
                .iter_mut()
                .chain(&mut t.rotation)
                .chain(&mut t.scale)
            {
                *slot = f64::from(*slot as f32);
            }
        }
    }
    scene
}

fn small_json() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::from),
        (-9i64..9).prop_map(Value::from),
        proptest::collection::vec(0i64..4, 0..3).prop_map(|items| json!(items)),
    ];
    leaf.prop_recursive(2, 12, 3, |inner| {
        proptest::collection::btree_map("[a-c]", inner, 0..3)
            .prop_map(|map| Value::Object(map.into_iter().collect()))
    })
}

/// `value` with every object key whose value is `null` dropped, recursively: `merge_partial`
/// removes such keys, so a merged diff and the live value agree up to them.
fn without_null_keys(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(_, value)| !value.is_null())
                .map(|(key, value)| (key.clone(), without_null_keys(value)))
                .collect(),
        ),
        _ => value.clone(),
    }
}

// --- Properties -------------------------------------------------------------------------------

proptest! {
    #[test]
    fn emit_then_parse_loses_nothing(s in scene()) {
        let registry = registry();
        let text = emit(&s, &registry);
        let parsed = parse(&text).expect("canonical output must parse cleanly");
        prop_assert_eq!(normalized(parsed), normalized(s));
    }

    #[test]
    fn emit_is_idempotent_byte_for_byte(s in scene()) {
        let registry = registry();
        let first = emit(&s, &registry);
        let reparsed = parse(&first).expect("canonical output must parse cleanly");
        let second = emit(&reparsed, &registry);
        prop_assert_eq!(&second, &first);
        // And once stable, it stays stable.
        let third = emit(&parse(&second).unwrap(), &registry);
        prop_assert_eq!(&third, &first);
    }

    #[test]
    fn emitted_text_is_lf_only_and_validates(s in scene()) {
        let registry = registry();
        let text = emit(&s, &registry);
        prop_assert!(!text.contains('\r'), "canonical output must use LF only");
        prop_assert!(text.ends_with('\n'), "canonical output ends with a newline");
        let parsed = parse(&text).expect("canonical output must parse cleanly");
        let diagnostics = validate(&parsed, &registry);
        prop_assert!(
            diagnostics.is_empty(),
            "canonical output of a well-typed scene must validate: {:?}",
            diagnostics.iter().map(ToString::to_string).collect::<Vec<_>>()
        );
    }

    #[test]
    fn merging_a_partial_diff_reproduces_the_live_value(
        base in small_json(),
        live in small_json(),
    ) {
        let mut merged = base.clone();
        match ludion_scene::partial_diff(&base, &live, &[]) {
            Some(diff) => ludion_scene::merge_partial(&mut merged, diff),
            None => prop_assert_eq!(&base, &live),
        }
        prop_assert_eq!(without_null_keys(&merged), without_null_keys(&live));
    }

    #[test]
    fn floats_round_trip_bit_for_bit(bits in any::<u64>()) {
        let float = f64::from_bits(bits);
        prop_assume!(float.is_finite());
        let registry = registry();
        let s = Scene {
            name: None,
            generated_from: None,
            seed: None,
            physics: None,
            preload: Vec::new(),
            entities: vec![SceneEntity {
                presentation: false,
                name: None,
                components: vec![(
                    "Palette".to_string(),
                    json!({ "colors": [], "emissive": float }),
                )],
                instance: None,
            }],
        };
        let parsed = parse(&emit(&s, &registry)).expect("canonical output must parse");
        let value = &parsed.entities[0].components[0].1["emissive"];
        let back = value.as_f64().expect("emissive must come back as a float");
        prop_assert_eq!(back.to_bits(), float.to_bits());
    }

    #[test]
    fn strings_round_trip_exactly(text in any::<String>()) {
        let registry = registry();
        let s = Scene {
            name: Some(text.clone()),
            generated_from: None,
            seed: None,
            physics: None,
            preload: Vec::new(),
            entities: vec![],
        };
        let parsed = parse(&emit(&s, &registry)).expect("canonical output must parse");
        prop_assert_eq!(parsed.name.as_deref(), Some(text.as_str()));
    }
}

// --- Deterministic golden shape ---------------------------------------------------------------

/// One non-property test pinning the exact canonical text: component order is registry order
/// (not document order), field order is struct order (not alphabetical), ambiguous scalars are
/// quoted, floats are ryu-shortest.
#[test]
fn canonical_form_golden() {
    let registry = registry();
    let source = "\
format: 1
entities:
  - components:
      Health: 100
      Sprite:
        visible: false
        layer: -1
        texture: no
      Transform:
        z: 3
        y: 0.30000001192092896
        x: 1.0
";
    let scene = parse(source).expect("scene should parse");
    let canonical = emit(&scene, &registry);
    assert_eq!(
        canonical,
        "\
format: 1
entities:
  - components:
      Transform:
        x: 1.0
        y: 0.3
        z: 3
      Sprite:
        texture: \"no\"
        layer: -1
        visible: false
      Health: 100
"
    );
}

/// The f32 rule is per schema position: an `f64` field holding a widened f32 keeps every digit.
#[test]
fn an_f64_field_is_not_narrowed() {
    let registry = registry();
    let source = "\
format: 1
entities:
  - components:
      Palette:
        colors: []
        emissive: 0.30000001192092896
";
    let canonical = emit(&parse(source).expect("scene should parse"), &registry);
    assert!(
        canonical.contains("emissive: 0.30000001192092896\n"),
        "{canonical}"
    );
}

/// `generated_from` (the map-generator provenance marker) emits between `name` and `seed`, and
/// survives a parse→emit round trip.
#[test]
fn generated_from_round_trips_between_name_and_seed() {
    let registry = registry();
    let scene = Scene {
        name: Some("plaza".into()),
        generated_from: Some("plaza.map.yaml".into()),
        seed: Some(3),
        physics: None,
        preload: Vec::new(),
        entities: vec![],
    };
    let text = emit(&scene, &registry);
    assert_eq!(
        text,
        "format: 1\nname: plaza\ngenerated_from: plaza.map.yaml\nseed: 3\nentities: []\n"
    );
    assert_eq!(parse(&text).unwrap(), scene);
}

/// Components the registry does not know are preserved (emit is not a validator), after the
/// known ones, in document order.
#[test]
fn unknown_components_are_preserved_after_known_ones() {
    let registry = registry();
    let source = "\
format: 1
entities:
  - components:
      Zeta:
        q: 1
      Health: 5
      Alpha: {}
";
    let scene = parse(source).expect("scene should parse");
    let canonical = emit(&scene, &registry);
    assert_eq!(
        canonical,
        "\
format: 1
entities:
  - components:
      Health: 5
      Zeta:
        q: 1
      Alpha: {}
"
    );
}

/// Nested sequences and mappings emit in block style with the documented indentation shape.
#[test]
fn block_style_nesting_shape() {
    let registry = registry();
    let source = "\
format: 1
entities:
  - name: \"walls\"
    components:
      Palette:
        colors:
          - \"red\"
          - \"blue\"
        emissive: null
";
    let scene = parse(source).expect("scene should parse");
    let canonical = emit(&scene, &registry);
    assert_eq!(
        canonical,
        "\
format: 1
entities:
  - name: walls
    components:
      Palette:
        colors:
          - red
          - blue
        emissive: null
"
    );
}

/// Builds the raw (unescaped) spelling of a mapping key: `plain` `k` characters followed by
/// `controls` DEL characters. Each DEL costs 6 characters once the canonical writer escapes it,
/// so this is the lever for landing on an exact canonical-length target.
fn del_key(plain: usize, controls: usize) -> String {
    let del = "\u{7f}";
    format!("{}{}", "k".repeat(plain), del.repeat(controls))
}

/// The 1024-character canonical-key limit is exact: a component-value key that canonicalizes to
/// precisely 1024 characters parses and round-trips; one character over (1025) is rejected as
/// `L1030`.
#[test]
fn canonical_key_length_boundary_is_exact() {
    let registry = ludion_ecs::Registry::new();

    // 2 quotes + 2 plain chars + 170 * 6 escaped chars = 1024.
    let at_limit = del_key(2, 170);
    let source = format!(
        "format: 1\nentities:\n  - components:\n      Transform:\n        \"{at_limit}\": 1\n"
    );
    let scene = parse(&source).expect("a canonical-length-1024 key parses");
    let canonical = emit(&scene, &registry);
    let reparsed = parse(&canonical).expect("canonical output must parse cleanly");
    assert_eq!(
        emit(&reparsed, &registry),
        canonical,
        "emit must be idempotent"
    );

    // 2 quotes + 3 plain chars + 170 * 6 escaped chars = 1025.
    let over_limit = del_key(3, 170);
    let source = format!(
        "format: 1\nentities:\n  - components:\n      Transform:\n        \"{over_limit}\": 1\n"
    );
    let diagnostics = parse(&source).expect_err("a canonical-length-1025 key must be rejected");
    assert_eq!(diagnostics[0].code.as_str(), "L1030");
}

/// A key containing a control character survives parse-emit-parse-emit byte-for-byte; the
/// control character only ever appears in escaped form in the canonical text.
#[test]
fn control_char_key_round_trips_byte_for_byte() {
    let registry = ludion_ecs::Registry::new();
    let raw_key = del_key(0, 1);
    let raw_key = format!("a{raw_key}b");
    let source = format!(
        "format: 1\nentities:\n  - components:\n      Transform:\n        \"{raw_key}\": 1\n"
    );
    let scene = parse(&source).expect("a control-character key parses");
    let value = &scene.entities[0].components[0].1;
    let map = value.as_object().expect("Transform value is a mapping");
    assert_eq!(
        map.keys().next().map(String::as_str),
        Some(raw_key.as_str())
    );

    let canonical = emit(&scene, &registry);
    assert!(
        canonical.contains("\"a\\u007Fb\": 1"),
        "canonical form was:\n{canonical}"
    );
    let reparsed = parse(&canonical).expect("canonical output must parse cleanly");
    assert_eq!(
        emit(&reparsed, &registry),
        canonical,
        "emit must be idempotent"
    );
}

/// `preload` emits between `physics` and `entities` in document order, an empty list emits
/// nothing, and the list survives a parse→emit round trip.
#[test]
fn preload_round_trips_between_physics_and_entities() {
    let registry = registry();
    let scene = Scene {
        seed: Some(3),
        preload: vec!["prefabs/b.yaml".into(), "prefabs/a.yaml".into()],
        ..Scene::default()
    };
    let text = emit(&scene, &registry);
    assert_eq!(
        text,
        "format: 1\nseed: 3\npreload:\n  - prefabs/b.yaml\n  - prefabs/a.yaml\nentities: []\n"
    );
    assert_eq!(parse(&text).unwrap(), scene);
    let empty = emit(&Scene::default(), &registry);
    assert_eq!(empty, "format: 1\nentities: []\n");
}

#[test]
fn preload_type_errors_are_located() {
    let whole = parse("format: 1\npreload: 3\nentities: []\n").unwrap_err();
    assert!(whole.iter().any(|d| d.path.as_deref() == Some("/preload")));
    let item = parse("format: 1\npreload:\n  - a.yaml\n  - 3\nentities: []\n").unwrap_err();
    assert!(
        item.iter().any(|d| d.path.as_deref() == Some("/preload/1")),
        "{item:?}"
    );
}

#[test]
fn a_prefab_document_refuses_preload() {
    let errors = parse_prefab("format: 1\npreload:\n  - a.yaml\nentities: []\n").unwrap_err();
    assert!(
        errors.iter().any(|d| d.path.as_deref() == Some("/preload")),
        "{errors:?}"
    );
}

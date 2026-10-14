//! Validator integration tests over committed fixtures: a valid gallery scene and a
//! deliberately broken one whose every diagnostic is pinned (code + JSON Pointer + suggestion).

mod common;

use common::registry;
use ludion_scene::{
    SceneLayer, ScenePhysics, ScenePhysicsSpace, check, emit, parse, validate, validate_physics,
};

const VALID: &str = include_str!("fixtures/valid.yaml");
const BROKEN: &str = include_str!("fixtures/broken.yaml");

#[test]
fn valid_fixture_checks_clean() {
    let registry = registry();
    let scene = check(VALID, &registry).expect("valid.yaml must validate");
    assert_eq!(scene.name.as_deref(), Some("gallery"));
    assert_eq!(scene.seed, Some(7));
    assert_eq!(scene.entities.len(), 3);
}

#[test]
fn valid_fixture_survives_a_canonical_round_trip() {
    let registry = registry();
    let scene = check(VALID, &registry).expect("valid.yaml must validate");
    let canonical = emit(&scene, &registry);
    let reparsed = parse(&canonical).expect("canonical output must parse");
    assert!(validate(&reparsed, &registry).is_empty());
    assert_eq!(
        emit(&reparsed, &registry),
        canonical,
        "emit must be idempotent"
    );
}

#[test]
fn broken_fixture_reports_every_defect() {
    let registry = registry();
    let diagnostics = check(BROKEN, &registry).expect_err("broken.yaml must fail");
    let summary: Vec<(String, String)> = diagnostics
        .iter()
        .map(|d| {
            (
                d.code.as_str().to_string(),
                d.path.clone().unwrap_or_default(),
            )
        })
        .collect();
    let expected: Vec<(String, String)> = [
        // entity 1: component name typo
        ("L1013", "/entities/1/components/Trasform"),
        // entity 2: missing required field + field name typo
        ("L1014", "/entities/2/components/Transform"),
        ("L1014", "/entities/2/components/Transform/zz"),
        // entity 3: three wrong field types + one range violation
        ("L1014", "/entities/3/components/Sprite/texture"),
        ("L1014", "/entities/3/components/Sprite/layer"),
        ("L1014", "/entities/3/components/Sprite/visible"),
        ("L1014", "/entities/3/components/Health"),
        // entity 4: no enum variant matches
        ("L1014", "/entities/4/components/Shape"),
    ]
    .into_iter()
    .map(|(code, path)| (code.to_string(), path.to_string()))
    .collect();
    assert_eq!(
        summary,
        expected,
        "full rendering:\n{}",
        diagnostics
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n\n")
    );
}

#[test]
fn broken_fixture_suggestions_are_concrete() {
    let registry = registry();
    let diagnostics = check(BROKEN, &registry).expect_err("broken.yaml must fail");
    let help_for = |path: &str| -> String {
        diagnostics
            .iter()
            .find(|d| d.path.as_deref() == Some(path))
            .and_then(|d| d.help.clone())
            .unwrap_or_default()
    };
    assert_eq!(
        help_for("/entities/1/components/Trasform"),
        "nearest match is `Transform`"
    );
    assert_eq!(
        help_for("/entities/2/components/Transform/zz"),
        "nearest match is `z`"
    );
    assert_eq!(
        help_for("/entities/3/components/Sprite/texture"),
        "quote the value to make it a string"
    );
    // The range violation names the actual bound.
    let health = diagnostics
        .iter()
        .find(|d| d.path.as_deref() == Some("/entities/3/components/Health"))
        .unwrap();
    assert!(
        health.message.contains("at least 0"),
        "message was: {}",
        health.message
    );
}

#[test]
fn every_scene_code_in_the_table_is_constructible() {
    // The codes table registers L1001–L1015 for the scene domain; prove each one can actually
    // come out of this crate (L1015 is the CLI's file-IO code and is exercised there).
    let registry = registry();
    let cases: [(&str, &str); 12] = [
        ("L1001", "a: \"unclosed\n"),
        ("L1002", "a: &x 1\n"),
        ("L1003", "a: !!str 42\n"),
        ("L1004", "format: 1\n---\nformat: 1\n"),
        ("L1005", "a: [1, 2]\n"),
        ("L1006", "a: |\n  text\n"),
        ("L1007", "a: 1\na: 2\n"),
        ("L1008", "1: x\n"),
        ("L1009", "format: 1\nentities: {}\n"),
        ("L1010", "format: 1\nentites: []\n"),
        ("L1011", "entities: []\n"),
        ("L1012", "format: 99\n"),
    ];
    for (code, source) in cases {
        let diagnostics = check(source, &registry).expect_err(code);
        assert_eq!(
            diagnostics[0].code.as_str(),
            code,
            "source: {source:?} produced {}",
            diagnostics[0]
        );
        // Every scene diagnostic's code must be registered so `ludion explain` works.
        assert!(
            ludion_core::codes::explain(code).is_some(),
            "{code} missing from the codes table"
        );
    }
    // L1013 / L1014 come from validation.
    let diagnostics = check(
        "format: 1\nentities:\n  - components:\n      Nope: {}\n",
        &registry,
    )
    .unwrap_err();
    assert_eq!(diagnostics[0].code.as_str(), "L1013");
    let diagnostics = check(
        "format: 1\nentities:\n  - components:\n      Health: \"full\"\n",
        &registry,
    )
    .unwrap_err();
    assert_eq!(diagnostics[0].code.as_str(), "L1014");
}

/// A reserved (field-backed) component declared as a `components:` block is rejected with L1016 —
/// the field is the one authoring channel (see the engine's `Name`).
#[test]
fn reserved_component_as_a_block_is_l1016() {
    let registry = registry();
    let diagnostics = check(
        "format: 1\nentities:\n  - components:\n      Label: hero\n",
        &registry,
    )
    .expect_err("a reserved component block must fail validation");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code.as_str(), "L1016");
    assert_eq!(
        diagnostics[0].path.as_deref(),
        Some("/entities/0/components/Label")
    );
}

/// A mapping key whose canonical (quoted + escaped) spelling crosses the 1024-character scanner
/// limit is rejected as `L1030` even though its source spelling is short: a control character
/// escapes to six canonical characters, so 200 of them alone cross the limit.
#[test]
fn oversized_canonical_key_is_l1030() {
    let del = "\u{7f}";
    let key = format!("translat{}ion", del.repeat(200));
    let source =
        format!("format: 1\nentities:\n  - components:\n      Transform:\n        \"{key}\": 1\n");
    let diagnostics = parse(&source).expect_err("an over-long canonical key must be rejected");
    assert_eq!(diagnostics[0].code.as_str(), "L1030");
}

/// A document nested 10 000 levels deep is rejected as `L1034` — one diagnostic, and the process
/// survives (an unbounded walk over such a tree would overflow the stack).
#[test]
fn over_deep_nesting_is_l1034() {
    let source = format!("format: 1\nentities:\n{}x\n", "- ".repeat(10_000));
    let diagnostics = parse(&source).expect_err("an over-deep document must be rejected");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code.as_str(), "L1034");
}

/// A one-entity scene with `Transform` whose `x` is `x`.
fn transform_scene(x: &str) -> String {
    let body = format!(
        "      Transform:
        x: {x}
        y: 0.0
        z: 0.0
"
    );
    format!(
        "format: 1
entities:
  - components:
{body}"
    )
}

/// A one-entity scene whose `Health` is `value`.
fn health_scene(value: &str) -> String {
    format!(
        "format: 1
entities:
  - components:
      Health: {value}
"
    )
}

fn l1014_paths(source: &str) -> Vec<String> {
    match check(source, &registry()) {
        Ok(_) => Vec::new(),
        Err(diagnostics) => diagnostics
            .iter()
            .filter(|d| d.code.as_str() == "L1014")
            .map(|d| d.path.clone().unwrap_or_default())
            .collect(),
    }
}

#[test]
fn an_f32_field_past_the_f32_range_is_l1014() {
    let source = transform_scene("1e39");
    assert_eq!(l1014_paths(&source), ["/entities/0/components/Transform/x"]);
    let negative = transform_scene("-1e39");
    assert_eq!(
        l1014_paths(&negative),
        ["/entities/0/components/Transform/x"]
    );
    let inside = transform_scene("3.0e38");
    assert!(l1014_paths(&inside).is_empty());
}

#[test]
fn a_u32_field_past_the_u32_range_is_l1014() {
    let over = health_scene("4294967296");
    assert_eq!(l1014_paths(&over), ["/entities/0/components/Health"]);
    let max = health_scene("4294967295");
    assert!(l1014_paths(&max).is_empty());
}

// --- the `physics:` block (Plan/physics.md W2) ----------------------------------------------------

#[test]
fn a_full_physics_block_parses_and_emits_canonically() {
    let registry = registry();
    let source = "\
format: 1
physics:
  space: planar
  gravity:
    - 0.0
    - -9.81
    - 0.0
  solver_iterations: 8
  stabilization_iterations: 2
  sleep_after_ticks: 45
entities: []
";
    let scene = parse(source).expect("a full physics block parses");
    assert_eq!(
        scene.physics,
        Some(ScenePhysics {
            space: ScenePhysicsSpace::Planar,
            gravity: Some([0.0, -9.81, 0.0]),
            solver_iterations: Some(8),
            stabilization_iterations: Some(2),
            sleep_after_ticks: Some(45),
            layers: Vec::new(),
        })
    );
    let canonical = emit(&scene, &registry);
    assert_eq!(canonical, source);
    let reparsed = parse(&canonical).expect("canonical output must parse");
    assert_eq!(
        emit(&reparsed, &registry),
        canonical,
        "emit must be idempotent"
    );
}

#[test]
fn an_absent_physics_block_gives_none_and_emits_no_physics_line() {
    let registry = registry();
    let scene = parse("format: 1\nentities: []\n").expect("a block-free scene parses");
    assert_eq!(scene.physics, None);
    let canonical = emit(&scene, &registry);
    assert!(
        !canonical.contains("physics"),
        "canonical form:\n{canonical}"
    );
}

#[test]
fn an_unknown_physics_key_is_l1010_with_the_nearest_match() {
    let diagnostics =
        parse("format: 1\nphysics:\n  spce: free\n").expect_err("an unknown key must fail");
    assert_eq!(diagnostics[0].code.as_str(), "L1010");
    assert_eq!(diagnostics[0].path.as_deref(), Some("/physics/spce"));
    assert_eq!(
        diagnostics[0].help.as_deref(),
        Some("nearest match is `space`")
    );
}

#[test]
fn a_physics_block_missing_space_is_l1011() {
    let diagnostics = parse("format: 1\nphysics:\n  solver_iterations: 4\n")
        .expect_err("a block without `space` must fail");
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code.as_str() == "L1011" && d.path.as_deref() == Some("/physics")),
        "full rendering:\n{}",
        diagnostics
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n\n")
    );
}

#[test]
fn an_unrecognized_space_value_is_l1033_with_the_nearest_match() {
    let diagnostics =
        parse("format: 1\nphysics:\n  space: planer\n").expect_err("a bad space must fail");
    assert_eq!(diagnostics[0].code.as_str(), "L1033");
    assert_eq!(diagnostics[0].path.as_deref(), Some("/physics/space"));
    assert_eq!(
        diagnostics[0].help.as_deref(),
        Some("nearest match is `planar`")
    );
}

#[test]
fn an_out_of_range_iteration_count_is_l1033() {
    for source in [
        "format: 1\nphysics:\n  space: free\n  solver_iterations: 0\n",
        "format: 1\nphysics:\n  space: free\n  solver_iterations: 65\n",
    ] {
        let diagnostics = parse(source).expect_err("an out-of-range count must fail");
        assert_eq!(diagnostics[0].code.as_str(), "L1033", "source: {source}");
        assert_eq!(
            diagnostics[0].path.as_deref(),
            Some("/physics/solver_iterations"),
            "source: {source}"
        );
    }
}

#[test]
fn a_non_integer_iteration_count_is_l1009() {
    let diagnostics = parse("format: 1\nphysics:\n  space: free\n  solver_iterations: 1.5\n")
        .expect_err("a non-integer count must fail");
    assert_eq!(diagnostics[0].code.as_str(), "L1009");
}

#[test]
fn a_physics_block_as_a_sequence_is_l1009() {
    let diagnostics =
        parse("format: 1\nphysics:\n  - free\n").expect_err("a sequence block must fail");
    assert_eq!(diagnostics[0].code.as_str(), "L1009");
    assert_eq!(diagnostics[0].path.as_deref(), Some("/physics"));
}

#[test]
fn integer_gravity_components_emit_as_canonical_floats() {
    let registry = registry();
    let scene =
        parse("format: 1\nphysics:\n  space: free\n  gravity:\n    - 0\n    - 0\n    - 0\n")
            .expect("integer gravity components parse");
    assert_eq!(
        scene.physics.as_ref().unwrap().gravity,
        Some([0.0, 0.0, 0.0])
    );
    let canonical = emit(&scene, &registry);
    assert!(
        canonical.contains("gravity:\n    - 0.0\n    - 0.0\n    - 0.0\n"),
        "canonical form:\n{canonical}"
    );
}

#[test]
fn a_gravity_of_the_wrong_length_is_l1033() {
    let diagnostics =
        parse("format: 1\nphysics:\n  space: free\n  gravity:\n    - 0.0\n    - 0.0\n")
            .expect_err("a two-element gravity must fail");
    assert_eq!(diagnostics[0].code.as_str(), "L1033");
    assert_eq!(diagnostics[0].path.as_deref(), Some("/physics/gravity"));
}

#[test]
fn a_gravity_element_outside_f32_range_is_l1033() {
    let diagnostics =
        parse("format: 1\nphysics:\n  space: free\n  gravity:\n    - 0.0\n    - 1e39\n    - 0.0\n")
            .expect_err("a gravity element outside f32 range must fail");
    assert_eq!(diagnostics[0].code.as_str(), "L1033");
    assert_eq!(diagnostics[0].path.as_deref(), Some("/physics/gravity/1"));
}

#[test]
fn validate_physics_reruns_the_parsers_value_checks_under_the_given_base() {
    let bad_iterations = ScenePhysics {
        space: ScenePhysicsSpace::Free,
        gravity: None,
        solver_iterations: Some(0),
        stabilization_iterations: None,
        sleep_after_ticks: None,
        layers: Vec::new(),
    };
    let diagnostics = validate_physics(&bad_iterations, "/scene_physics");
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0].code.as_str(), "L1033");
    assert_eq!(
        diagnostics[0].path.as_deref(),
        Some("/scene_physics/solver_iterations")
    );

    let bad_gravity = ScenePhysics {
        space: ScenePhysicsSpace::Free,
        gravity: Some([1e39, 0.0, 0.0]),
        solver_iterations: None,
        stabilization_iterations: None,
        sleep_after_ticks: None,
        layers: Vec::new(),
    };
    let diagnostics = validate_physics(&bad_gravity, "/scene_physics");
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0].code.as_str(), "L1033");
    assert_eq!(
        diagnostics[0].path.as_deref(),
        Some("/scene_physics/gravity/0")
    );

    let valid = ScenePhysics {
        space: ScenePhysicsSpace::Free,
        gravity: Some([0.0, -9.81, 0.0]),
        solver_iterations: Some(8),
        stabilization_iterations: Some(2),
        sleep_after_ticks: Some(45),
        layers: Vec::new(),
    };
    assert!(validate_physics(&valid, "/scene_physics").is_empty());
}

#[test]
fn a_non_number_gravity_element_is_l1009() {
    let diagnostics = parse(
        "format: 1\nphysics:\n  space: free\n  gravity:\n    - 0.0\n    - 0.0\n    - \"x\"\n",
    )
    .expect_err("a string gravity element must fail");
    assert_eq!(diagnostics[0].code.as_str(), "L1009");
    assert_eq!(diagnostics[0].path.as_deref(), Some("/physics/gravity/2"));
}

#[test]
fn a_scalar_gravity_is_l1009() {
    let diagnostics = parse("format: 1\nphysics:\n  space: free\n  gravity: 3\n")
        .expect_err("a scalar gravity must fail");
    assert_eq!(diagnostics[0].code.as_str(), "L1009");
    assert_eq!(diagnostics[0].path.as_deref(), Some("/physics/gravity"));
}

const LAYERED: &str = "format: 1
physics:
  space: free
  layers:
    - name: dummy
      ignores:
        - dummy
        - sling
    - name: sling
entities: []
";

#[test]
fn physics_layers_parse_emit_canonically_and_resolve() {
    let registry = registry();
    let scene = parse(LAYERED).expect("a layered block parses");
    let physics = scene.physics.as_ref().expect("physics block");
    assert_eq!(
        physics.layers,
        vec![
            SceneLayer {
                name: "dummy".into(),
                ignores: vec!["dummy".into(), "sling".into()]
            },
            SceneLayer {
                name: "sling".into(),
                ignores: Vec::new()
            },
        ]
    );
    assert_eq!(emit(&scene, &registry), LAYERED);
    let matrix = physics.layer_matrix();
    assert_eq!(matrix.names(), ["default", "dummy", "sling"]);
}

#[test]
fn an_empty_layers_list_canonicalizes_away() {
    let registry = registry();
    let scene = parse(
        "format: 1
physics:
  space: free
  layers: []
",
    )
    .expect("parses");
    assert!(scene.physics.as_ref().unwrap().layers.is_empty());
    assert_eq!(
        emit(&scene, &registry),
        "format: 1
physics:
  space: free
entities: []
"
    );
}

fn layer_error(layers: &str) -> Vec<(String, String)> {
    let source = format!(
        "format: 1
physics:
  space: free
  layers:
{layers}"
    );
    parse(&source)
        .expect_err("the layers must be refused")
        .into_iter()
        .map(|d| (d.code.as_str().to_string(), d.path.unwrap_or_default()))
        .collect()
}

#[test]
fn physics_layer_value_errors_are_l1033_with_pointers() {
    let duplicate = layer_error(
        "    - name: a
    - name: a
",
    );
    assert_eq!(
        duplicate,
        [("L1033".into(), "/physics/layers/1/name".into())]
    );
    let reserved = layer_error(
        "    - name: default
",
    );
    assert_eq!(
        reserved,
        [("L1033".into(), "/physics/layers/0/name".into())]
    );
    let unknown = layer_error(
        "    - name: a
      ignores:
        - b
",
    );
    assert_eq!(
        unknown,
        [("L1033".into(), "/physics/layers/0/ignores/0".into())]
    );
    let repeated = layer_error(
        "    - name: a
      ignores:
        - a
        - a
",
    );
    assert_eq!(
        repeated,
        [("L1033".into(), "/physics/layers/0/ignores/1".into())]
    );
    let names: String = (0..32)
        .map(|i| {
            format!(
                "    - name: l{i}
"
            )
        })
        .collect();
    assert_eq!(
        layer_error(&names),
        [("L1033".into(), "/physics/layers".into())]
    );
}

#[test]
fn physics_layer_structural_errors_use_the_scene_codes() {
    assert_eq!(
        layer_error(
            "    - ignores: []
"
        ),
        [("L1011".into(), "/physics/layers/0".into())]
    );
    assert_eq!(
        layer_error(
            "    - name: a
      ignore: []
"
        ),
        [("L1010".into(), "/physics/layers/0/ignore".into())]
    );
    assert_eq!(
        layer_error(
            "    - name: 3
"
        ),
        [("L1009".into(), "/physics/layers/0/name".into())]
    );
    assert_eq!(
        layer_error(
            "    - a
"
        ),
        [("L1009".into(), "/physics/layers/0".into())]
    );
}

#[test]
fn validate_physics_reruns_the_layer_checks_under_the_given_base() {
    let block = ScenePhysics {
        space: ScenePhysicsSpace::Free,
        gravity: None,
        solver_iterations: None,
        stabilization_iterations: None,
        sleep_after_ticks: None,
        layers: vec![
            SceneLayer {
                name: "a".into(),
                ignores: Vec::new(),
            },
            SceneLayer {
                name: "a".into(),
                ignores: Vec::new(),
            },
        ],
    };
    let diagnostics = validate_physics(&block, "/scene_physics");
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0].code.as_str(), "L1033");
    assert_eq!(
        diagnostics[0].path.as_deref(),
        Some("/scene_physics/layers/1/name")
    );
}

fn parent_registry() -> ludion_ecs::Registry {
    let mut registry = registry();
    registry
        .register::<ludion_ecs::Parent>("Parent")
        .expect("Parent registers");
    registry
}

/// An entity with a `Parent` naming `target`, flagged presentation or not.
fn parented(presentation: bool, name: &str, target: Option<usize>) -> String {
    let flag = if presentation {
        "    presentation: true\n"
    } else {
        ""
    };
    let parent = target
        .map(|t| format!("      Parent:\n        index: {t}\n        generation: 0\n"))
        .unwrap_or_default();
    let components = if parent.is_empty() {
        " {}\n".to_string()
    } else {
        format!("\n{parent}")
    };
    format!("  - name: {name}\n{flag}    components:{components}")
}

fn parent_codes(entities: &[String]) -> Vec<(String, String)> {
    let source = format!("format: 1\nentities:\n{}", entities.concat());
    let registry = parent_registry();
    match check(&source, &registry) {
        Ok(_) => Vec::new(),
        Err(diagnostics) => diagnostics
            .iter()
            .map(|d| {
                (
                    d.code.as_str().to_string(),
                    d.path.clone().unwrap_or_default(),
                )
            })
            .collect(),
    }
}

#[test]
fn a_sim_entity_naming_a_presentation_entity_is_l1036() {
    let codes = parent_codes(&[
        parented(true, "popup", None),
        parented(false, "unit", Some(0)),
    ]);
    assert_eq!(
        codes,
        [(
            "L1036".to_string(),
            "/entities/1/components/Parent/index".to_string()
        )]
    );
    let registry = parent_registry();
    let source = format!(
        "format: 1\nentities:\n{}{}",
        parented(true, "popup", None),
        parented(false, "unit", Some(0))
    );
    let diagnostic = check(&source, &registry).unwrap_err().remove(0);
    let message = &diagnostic.message;
    assert!(
        message.contains("sim entity 1 (`unit`) names presentation entity 0 (`popup`)")
            && message.contains("the sim would hash a presentation id"),
        "{message}"
    );
    let help = diagnostic.help.as_deref().unwrap_or_default();
    assert!(help.contains("`presentation: true` too"), "{help}");
}

#[test]
fn presentation_may_name_sim_and_presentation() {
    assert!(
        parent_codes(&[
            parented(false, "unit", None),
            parented(true, "popup", Some(0)),
            parented(true, "label", Some(1)),
        ])
        .is_empty()
    );
}

#[test]
fn a_non_boolean_presentation_flag_is_l1009() {
    let registry = registry();
    let errors = check(
        "format: 1\nentities:\n  - presentation: 3\n    components: {}\n",
        &registry,
    )
    .unwrap_err();
    assert_eq!(errors[0].code.as_str(), "L1009");
    assert_eq!(errors[0].path.as_deref(), Some("/entities/0/presentation"));
}

//! Scene-structure extraction: the strict-YAML tree becomes a [`Scene`].
//!
//! The scene skeleton is fixed (see the crate docs): a top-level mapping with `format` (required),
//! `name`, `seed`, `physics`, and `entities`; each entity a mapping with `name` and `components`.
//! Everything inside a component value is passed through as a [`serde_json::Value`] — its shape
//! belongs to the component's JSON Schema and is checked by [`crate::validate`], not here.
//!
//! Unlike the YAML layer (which fails fast because a broken token stream is unrecoverable), this
//! pass collects every structure error it can find in one run, so an agent fixes a bad scene in
//! one iteration instead of one-error-per-run whack-a-mole.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use serde_json::Value;

use crate::envelope;
use crate::nav_block;
use crate::physics;
use crate::render_layers;
use crate::yaml::{self, Node, NodeValue};
use crate::{FORMAT_VERSION, Scene, SceneEntity};

const SCENE_KEYS: [&str; 9] = [
    "format",
    "name",
    "generated_from",
    "seed",
    "physics",
    "nav",
    "render_layers",
    "preload",
    "entities",
];
const ENTITY_KEYS: [&str; 4] = ["name", "presentation", "components", "prefab"];

/// Parses strict-subset YAML into a [`Scene`], collecting structure diagnostics.
pub(crate) fn parse(source: &str) -> Result<Scene, Vec<Diagnostic>> {
    let root = yaml::parse_document(source).map_err(|diagnostic| vec![diagnostic])?;
    let Some(root) = root else {
        return Err(vec![
            Diagnostic::new(
                code::SCENE_MISSING_KEY,
                "the scene document is empty; a scene needs at least `format: 1`",
            )
            .with_path("")
            .with_help("a minimal scene is:\n  format: 1\n  entities: []"),
        ]);
    };
    let NodeValue::Mapping(entries) = root.value else {
        return Err(vec![
            Diagnostic::new(
                code::SCENE_TYPE_MISMATCH,
                format!(
                    "the scene document root must be a mapping, found {} (line {})",
                    root.value.type_phrase(),
                    root.line
                ),
            )
            .with_path("")
            .with_help("start the file with the top-level keys `format:` and `entities:`"),
        ]);
    };

    let mut diagnostics = Vec::new();
    let mut scene = Scene::default();
    let mut format_seen = false;

    for entry in &entries {
        match entry.key.as_str() {
            "format" => {
                format_seen = true;
                envelope::check_envelope(
                    entry,
                    code::SCENE_TYPE_MISMATCH,
                    code::SCENE_UNSUPPORTED_FORMAT,
                    FORMAT_VERSION,
                    "the scene",
                    "scene",
                    "scene",
                    &mut diagnostics,
                );
            }
            "name" => {
                scene.name = envelope::expect_string(
                    &entry.node,
                    "/name",
                    "the scene name",
                    code::SCENE_TYPE_MISMATCH,
                    &mut diagnostics,
                );
            }
            "generated_from" => {
                scene.generated_from = envelope::expect_string(
                    &entry.node,
                    "/generated_from",
                    "the generated-from provenance marker",
                    code::SCENE_TYPE_MISMATCH,
                    &mut diagnostics,
                );
            }
            "seed" => {
                scene.seed = expect_u64(&entry.node, "/seed", "the RNG seed", &mut diagnostics);
            }
            "physics" => {
                scene.physics = physics::parse_physics(&entry.node, &mut diagnostics);
            }
            "nav" => {
                scene.nav = nav_block::parse_nav(&entry.node, &mut diagnostics);
            }
            "render_layers" => {
                if let Some(layers) = render_layers::parse_render_layers(&entry.node, &mut diagnostics) {
                    scene.render_layers = layers;
                }
            }
            "preload" => match &entry.node.value {
                NodeValue::Sequence(items) => {
                    for (index, item) in items.iter().enumerate() {
                        if let Some(path) = envelope::expect_string(
                            item,
                            &format!("/preload/{index}"),
                            "a preload entry",
                            code::SCENE_TYPE_MISMATCH,
                            &mut diagnostics,
                        ) {
                            scene.preload.push(path);
                        }
                    }
                }
                other => diagnostics.push(
                    Diagnostic::new(
                        code::SCENE_TYPE_MISMATCH,
                        format!(
                            "`preload` must be a sequence, found {} (line {})",
                            other.type_phrase(),
                            entry.node.line
                        ),
                    )
                    .with_path("/preload")
                    .with_help(
                        "list the prefab paths a system will spawn, one per `- ` item:\n  preload:\n    - \"prefabs/enemy.prefab.yaml\"",
                    ),
                ),
            },
            "entities" => match &entry.node.value {
                NodeValue::Sequence(items) => {
                    let mut positions = Vec::new();
                    for (index, item) in items.iter().enumerate() {
                        if let Some(entity) = parse_entity(item, index, &mut diagnostics) {
                            scene.entities.push(entity);
                            positions.push(index);
                        }
                    }
                    let entries: Vec<_> = positions.into_iter().zip(&scene.entities).collect();
                    crate::instance::check_duplicate_names(&entries, &mut diagnostics);
                }
                other => diagnostics.push(
                    Diagnostic::new(
                        code::SCENE_TYPE_MISMATCH,
                        format!(
                            "`entities` must be a sequence, found {} (line {})",
                            other.type_phrase(),
                            entry.node.line
                        ),
                    )
                    .with_path("/entities")
                    .with_help("write one entity per `- ` item, or `entities: []` for none"),
                ),
            },
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                entry,
                "",
                "a scene document",
                SCENE_KEYS,
                code::SCENE_UNKNOWN_KEY,
            )),
        }
    }

    if !format_seen {
        diagnostics.push(
            Diagnostic::new(
                code::SCENE_MISSING_KEY,
                "the scene document is missing the required top-level key `format`",
            )
            .with_path("")
            .with_help("add `format: 1` as the first line"),
        );
    }

    if diagnostics.is_empty() {
        Ok(scene)
    } else {
        Err(diagnostics)
    }
}

pub(crate) fn parse_entity(
    node: &Node,
    index: usize,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<SceneEntity> {
    let base = format!("/entities/{index}");
    let NodeValue::Mapping(entries) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::SCENE_TYPE_MISMATCH,
                format!(
                    "each entity must be a mapping, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(base)
            .with_help("an entity looks like:\n  - name: \"paddle\"\n    components: {}"),
        );
        return None;
    };
    if entries.iter().any(|entry| entry.key == "prefab") {
        return crate::instance::parse_entry(entries, index, diagnostics);
    }
    let mut entity = SceneEntity::default();
    let before = diagnostics.len();
    for entry in entries {
        match entry.key.as_str() {
            "name" => {
                entity.name = envelope::expect_string(
                    &entry.node,
                    &format!("{base}/name"),
                    "the entity name",
                    code::SCENE_TYPE_MISMATCH,
                    diagnostics,
                );
            }
            "presentation" => match &entry.node.value {
                NodeValue::Scalar(Value::Bool(flag)) => entity.presentation = *flag,
                other => diagnostics.push(
                    Diagnostic::new(
                        code::SCENE_TYPE_MISMATCH,
                        format!(
                            "`presentation` must be a boolean, found {} (line {})",
                            other.type_phrase(),
                            entry.node.line
                        ),
                    )
                    .with_path(format!("{base}/presentation"))
                    .with_help("write `presentation: true`, or omit the key for a sim entity"),
                ),
            },
            "components" => match &entry.node.value {
                NodeValue::Mapping(components) => {
                    for component in components {
                        entity
                            .components
                            .push((component.key.clone(), node_to_value(&component.node)));
                    }
                }
                other => diagnostics.push(
                    Diagnostic::new(
                        code::SCENE_TYPE_MISMATCH,
                        format!(
                            "`components` must be a mapping of component name to value, found \
                             {} (line {})",
                            other.type_phrase(),
                            entry.node.line
                        ),
                    )
                    .with_path(format!("{base}/components"))
                    .with_help("write `components: {}` for an entity with no components"),
                ),
            },
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                entry,
                &base,
                "an entity",
                ENTITY_KEYS,
                code::SCENE_UNKNOWN_KEY,
            )),
        }
    }
    (diagnostics.len() == before).then_some(entity)
}

pub(crate) fn expect_u64(
    node: &Node,
    pointer: &str,
    what: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<u64> {
    match &node.value {
        NodeValue::Scalar(Value::Number(n)) if n.as_u64().is_some() => n.as_u64(),
        other => {
            diagnostics.push(
                Diagnostic::new(
                    code::SCENE_TYPE_MISMATCH,
                    format!(
                        "{what} must be a non-negative integer, found {} (line {})",
                        other.type_phrase(),
                        node.line
                    ),
                )
                .with_path(pointer),
            );
            None
        }
    }
}

/// Converts a parsed subtree into a JSON value, preserving document key order.
pub(crate) fn node_to_value(node: &Node) -> Value {
    match &node.value {
        NodeValue::Scalar(value) => value.clone(),
        NodeValue::Sequence(items) => Value::Array(items.iter().map(node_to_value).collect()),
        NodeValue::Mapping(entries) => Value::Object(
            entries
                .iter()
                .map(|entry| (entry.key.clone(), node_to_value(&entry.node)))
                .collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_errs(source: &str) -> Vec<Diagnostic> {
        parse(source).expect_err("expected diagnostics")
    }

    #[test]
    fn parses_a_full_scene() {
        let scene = parse(
            "format: 1\n\
             name: \"level-1\"\n\
             seed: 42\n\
             entities:\n\
             \x20 - name: \"paddle\"\n\
             \x20   components:\n\
             \x20     Transform:\n\
             \x20       x: 1.5\n\
             \x20       y: -2.0\n\
             \x20 - components: {}\n",
        )
        .expect("scene should parse");
        assert_eq!(scene.name.as_deref(), Some("level-1"));
        assert_eq!(scene.seed, Some(42));
        assert_eq!(scene.entities.len(), 2);
        assert_eq!(scene.entities[0].name.as_deref(), Some("paddle"));
        let (component, value) = &scene.entities[0].components[0];
        assert_eq!(component, "Transform");
        assert_eq!(value["x"], Value::from(1.5));
        assert!(scene.entities[1].components.is_empty());
    }

    #[test]
    fn minimal_scene_is_two_lines() {
        let scene = parse("format: 1\nentities: []\n").expect("scene should parse");
        assert_eq!(scene, Scene::default());
    }

    #[test]
    fn missing_format_is_l1011() {
        let diagnostics = parse_errs("entities: []\n");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, code::SCENE_MISSING_KEY);
    }

    #[test]
    fn empty_document_is_l1011() {
        let diagnostics = parse_errs("");
        assert_eq!(diagnostics[0].code, code::SCENE_MISSING_KEY);
    }

    #[test]
    fn future_format_is_l1012() {
        let diagnostics = parse_errs("format: 2\nentities: []\n");
        assert_eq!(diagnostics[0].code, code::SCENE_UNSUPPORTED_FORMAT);
        assert_eq!(diagnostics[0].path.as_deref(), Some("/format"));
    }

    #[test]
    fn root_scalar_is_l1009() {
        let diagnostics = parse_errs("just a string\n");
        assert_eq!(diagnostics[0].code, code::SCENE_TYPE_MISMATCH);
    }

    #[test]
    fn typo_keys_get_suggestions() {
        let diagnostics = parse_errs("format: 1\nentites: []\n");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, code::SCENE_UNKNOWN_KEY);
        assert_eq!(diagnostics[0].path.as_deref(), Some("/entites"));
        assert_eq!(
            diagnostics[0].help.as_deref(),
            Some("nearest match is `entities`")
        );
    }

    #[test]
    fn multiple_errors_come_back_in_one_pass() {
        let diagnostics = parse_errs(
            "format: 1\n\
             seed: -3\n\
             entities:\n\
             \x20 - 17\n\
             \x20 - naem: \"x\"\n\
             \x20   components: {}\n",
        );
        let codes: Vec<&str> = diagnostics.iter().map(|d| d.code.as_str()).collect();
        assert_eq!(codes, ["L1009", "L1009", "L1010"]);
        assert_eq!(diagnostics[1].path.as_deref(), Some("/entities/0"));
        assert_eq!(diagnostics[2].path.as_deref(), Some("/entities/1/naem"));
        assert_eq!(
            diagnostics[2].help.as_deref(),
            Some("nearest match is `name`")
        );
    }

    #[test]
    fn entity_component_order_is_document_order() {
        let scene = parse(
            "format: 1\n\
             entities:\n\
             \x20 - components:\n\
             \x20     Zeta: {}\n\
             \x20     Alpha: {}\n",
        )
        .expect("scene should parse");
        let names: Vec<&str> = scene.entities[0]
            .components
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        assert_eq!(names, ["Zeta", "Alpha"]);
    }
}

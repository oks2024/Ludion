//! [`LayerSet`] and [`QueryLayers`]: layer names that only queries see.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_ecs::SemanticValidate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// An ordered list of layer names, in the author's order (never sorted, so a scene's text does not
/// churn). Serializes as a plain list of strings.
///
/// ```
/// use ludion_physics::LayerSet;
///
/// let set = LayerSet::from(vec!["light_occluder".to_string()]);
/// assert_eq!(set.as_slice(), ["light_occluder"]);
/// assert!(!set.is_empty());
/// assert_eq!(serde_json::to_string(&set).unwrap(), r#"["light_occluder"]"#);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Eq, Default)]
#[serde(transparent)]
pub struct LayerSet(Vec<String>);

impl LayerSet {
    /// The names in author order.
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(String::as_str)
    }

    /// The names in author order.
    pub fn as_slice(&self) -> &[String] {
        &self.0
    }

    /// Whether the set names no layer.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<String>> for LayerSet {
    fn from(names: Vec<String>) -> Self {
        Self(names)
    }
}

/// Extra layers an entity is on **for queries only**. A query's `include_layer` and `ignore_layer`
/// see the entity's body layer plus these; the solver sees only the body layer, so contacts,
/// traces and hashes do not change. Put it beside a `Collider` or a `CharacterMotor`; an entity with
/// neither is `L1501` in `check-scene`. Every name must be `default` or declared under
/// `physics.layers` (`L1186`), and an entry is not empty, repeated, or the entity's own body layer
/// (`L1500`).
///
/// ```yaml
/// components:
///   QueryLayers:
///     layers: [light_occluder]
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct QueryLayers {
    /// The extra layer names, in author order.
    pub layers: LayerSet,
}

impl SemanticValidate for QueryLayers {
    /// Rejects an empty or repeated entry (`L1500`, pointer `/layers/{j}`). A wrong *shape* is the
    /// schema's job and adds nothing here.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let Some(entries) = value.get("layers").and_then(Value::as_array) else {
            return Vec::new();
        };
        let mut seen: Vec<&str> = Vec::new();
        let mut found = Vec::new();
        for (index, entry) in entries.iter().enumerate() {
            let Some(name) = entry.as_str() else {
                continue;
            };
            let problem = if name.is_empty() {
                Some("is empty".to_string())
            } else if seen.contains(&name) {
                Some(format!("repeats `{name}`"))
            } else {
                None
            };
            seen.push(name);
            if let Some(problem) = problem {
                found.push(
                    Diagnostic::new(
                        code::QUERY_LAYERS_INVALID,
                        format!("`QueryLayers` `layers[{index}]` {problem}"),
                    )
                    .with_path(format!("/layers/{index}"))
                    .with_help(
                        "name each extra layer once, using a layer `physics.layers` declares",
                    ),
                );
            }
        }
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn paths(value: &Value) -> Vec<String> {
        QueryLayers::validate_semantics(value)
            .iter()
            .map(|d| d.path.clone().unwrap_or_default())
            .collect()
    }

    #[test]
    fn distinct_names_validate_clean() {
        assert!(paths(&json!({})).is_empty());
        assert!(paths(&json!({ "layers": ["a", "b"] })).is_empty());
    }

    #[test]
    fn an_empty_or_repeated_entry_is_l1500_at_its_index() {
        assert_eq!(
            paths(&json!({ "layers": ["a", "", "a", "b", "b"] })),
            ["/layers/1", "/layers/2", "/layers/4"]
        );
        let found = QueryLayers::validate_semantics(&json!({ "layers": [""] }));
        assert_eq!(found[0].code, code::QUERY_LAYERS_INVALID);
    }

    #[test]
    fn the_set_keeps_author_order_through_a_round_trip() {
        let parsed: QueryLayers = serde_json::from_value(json!({ "layers": ["z", "a"] })).unwrap();
        assert_eq!(parsed.layers.as_slice(), ["z", "a"]);
        assert_eq!(
            serde_json::to_value(&parsed).unwrap(),
            json!({ "layers": ["z", "a"] })
        );
    }
}

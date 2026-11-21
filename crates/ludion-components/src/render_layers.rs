//! The [`RenderLayers`] component and the layer-name check `Camera.layers` and `RenderView.layers` share.
//! Render-only; `ludion docs render-views` is the guide.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_ecs::SemanticValidate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The layer an entity is in when it names none, and the layer a `RenderView` draws when it names none.
pub const DEFAULT_RENDER_LAYER: &str = "default";

/// Puts this entity, and every `Parent` descendant without its own `RenderLayers`, in the named
/// layers. An entity without one is in `default`. A `Camera.layers` or `RenderView.layers` list picks
/// which layers an image draws: meshes, sprites, ground markers, drop shadows, decals, particles and
/// world text, and their shadows.
///
/// ```
/// use ludion_components::RenderLayers;
///
/// let layers: RenderLayers =
///     serde_json::from_value(serde_json::json!({ "layers": ["radar"] })).unwrap();
/// assert_eq!(layers.layers, ["radar"]);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq, Eq)]
pub struct RenderLayers {
    /// The layer names, at least one, each non-empty and listed once.
    pub layers: Vec<String>,
}

/// `L2375` for a layer-name list that is empty, or holds an empty or repeated name. `pointer` is the
/// list's JSON Pointer relative to the component (`/layers`); each bad name gets `<pointer>/<i>`.
pub fn layer_names_diagnostics(names: &[String], pointer: &str) -> Vec<Diagnostic> {
    let reject = |path: String, what: String| {
        Diagnostic::new(code::RENDER_LAYERS_INVALID, what)
            .with_path(path)
            .with_help("see `ludion docs render-views`; `ludion explain L2375`")
    };
    if names.is_empty() {
        return vec![reject(
            pointer.to_string(),
            "a layer list is empty; name at least one layer".into(),
        )];
    }
    let mut out = Vec::new();
    for (i, name) in names.iter().enumerate() {
        if name.is_empty() {
            out.push(reject(
                format!("{pointer}/{i}"),
                "a layer name is empty".into(),
            ));
        } else if names[..i].contains(name) {
            out.push(reject(
                format!("{pointer}/{i}"),
                format!("the layer `{name}` is listed twice"),
            ));
        }
    }
    out
}

/// [`layer_names_diagnostics`] for the list at `field` of a component document, if it is authored.
pub(crate) fn field_layer_diagnostics(value: &Value, field: &str) -> Vec<Diagnostic> {
    let Some(items) = value.get(field).and_then(Value::as_array) else {
        return Vec::new();
    };
    let names: Vec<String> = items
        .iter()
        .map(|n| n.as_str().unwrap_or("x").to_string())
        .collect();
    layer_names_diagnostics(&names, &format!("/{field}"))
}

impl SemanticValidate for RenderLayers {
    /// Rejects an empty `layers` list and an empty or repeated name (`L2375`, pointer `/layers` or
    /// `/layers/<i>`). A wrong *shape* is the schema's job.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        field_layer_diagnostics(value, "layers")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn paths(value: Value) -> Vec<String> {
        RenderLayers::validate_semantics(&value)
            .into_iter()
            .map(|d| d.path.unwrap())
            .collect()
    }

    #[test]
    fn a_layer_list_round_trips() {
        let value = json!({ "layers": ["radar", "world"] });
        let l: RenderLayers = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&l).unwrap(), value);
        assert!(serde_json::from_value::<RenderLayers>(json!({})).is_err());
        assert!(paths(value).is_empty());
    }

    #[test]
    fn each_bad_list_names_its_pointer() {
        assert_eq!(paths(json!({ "layers": [] })), ["/layers"]);
        assert_eq!(paths(json!({ "layers": ["a", ""] })), ["/layers/1"]);
        assert_eq!(paths(json!({ "layers": ["a", "b", "a"] })), ["/layers/2"]);
    }

    #[test]
    fn the_diagnostic_is_l2375() {
        let d = RenderLayers::validate_semantics(&json!({ "layers": [] }));
        assert_eq!(d[0].code, code::RENDER_LAYERS_INVALID);
    }
}

//! The scene's top-level `nav:` block (`Plan/debug-draw.md` decision 8): a `NavConfig` as YAML.
//! The scene crate keeps it as an erased JSON value — `ludion-engine` owns the type, merges the
//! mapping over its defaults and validates it — and knows only the key order the emitter writes.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use serde_json::Value;

use crate::parse::node_to_value;
use crate::yaml::{Node, NodeValue};

/// `NavConfig`'s field names in declaration order (pinned by a `ludion-engine` test).
pub const NAV_KEY_ORDER: [&str; 14] = [
    "origin",
    "cell_size",
    "nx",
    "nz",
    "y_min",
    "y_max",
    "agent_radius",
    "agent_height",
    "max_step",
    "max_slope_deg",
    "flow_radius",
    "ignore_layers",
    "max_path_requests_per_tick",
    "classes",
];

/// `NavClass`'s field names in declaration order (pinned by a `ludion-engine` test).
pub const NAV_CLASS_KEY_ORDER: [&str; 5] =
    ["name", "radius", "height", "max_step", "max_slope_deg"];

/// Parses the `nav:` mapping into a JSON value, document key order kept. `None` when it is not a
/// mapping.
pub(crate) fn parse_nav(node: &Node, diagnostics: &mut Vec<Diagnostic>) -> Option<Value> {
    if !matches!(node.value, NodeValue::Mapping(_)) {
        diagnostics.push(
            Diagnostic::new(
                code::SCENE_TYPE_MISMATCH,
                format!(
                    "`nav` must be a mapping of nav settings, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path("/nav")
            .with_help("write the settings you change as keys:\n  nav:\n    cell_size: 0.25"),
        );
        return None;
    }
    Some(node_to_value(node))
}

/// The synthesized schema the emitter orders `nav:` keys by: known keys in declaration order.
pub(crate) fn nav_schema() -> &'static Value {
    static SCHEMA: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    SCHEMA.get_or_init(|| {
        let props = |keys: &[&str]| -> serde_json::Map<String, Value> {
            keys.iter()
                .map(|key| ((*key).to_string(), serde_json::json!({})))
                .collect()
        };
        let mut properties = props(&NAV_KEY_ORDER);
        properties.insert(
            "classes".to_string(),
            serde_json::json!({ "items": { "properties": props(&NAV_CLASS_KEY_ORDER) } }),
        );
        serde_json::json!({ "properties": properties })
    })
}

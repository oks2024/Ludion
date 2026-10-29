//! Asset-path fields: the schema marker a component field carries, the walk that finds every
//! marked field in a component's schema, and the visitor that hands each present path to a callback.
//!
//! A field holding a content-root-relative asset path borrows a proxy through
//! `#[schemars(with = …)]`. The proxy stamps the field's schema with `"x-asset-path": true`, and the
//! [`Registry`] derives [`ComponentInfo::asset_paths`] from it once, at registration. A game
//! component opts in the same way.
//!
//! [`Registry`]: crate::Registry
//! [`ComponentInfo::asset_paths`]: crate::ComponentInfo::asset_paths

use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde_json::Value;

/// The schema keyword an asset-path field carries: `true`.
pub const ASSET_PATH_KEYWORD: &str = "x-asset-path";

/// Schema proxy for a `String` field holding a content-root asset path. Never constructed: use it as
/// `#[schemars(with = "ludion_ecs::AssetPathSchema")]`, or `with = "Option<ludion_ecs::AssetPathSchema>"`
/// for an optional field.
///
/// # Example
/// ```
/// use ludion_ecs::Registry;
/// use schemars::JsonSchema;
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Serialize, Deserialize, JsonSchema)]
/// struct Banner {
///     #[schemars(with = "ludion_ecs::AssetPathSchema")]
///     texture: String,
/// }
///
/// let mut registry = Registry::new();
/// registry.register::<Banner>("Banner").unwrap();
/// let info = registry.get("Banner").unwrap();
/// assert_eq!(info.asset_paths(), ["/texture"]);
///
/// let mut value = serde_json::json!({"texture": "art/a.png"});
/// let mut seen = Vec::new();
/// info.for_each_asset_path(&mut value, &mut |pointer, path| {
///     seen.push(pointer.to_string());
///     path.insert_str(0, "../");
/// });
/// assert_eq!(seen, ["/texture"]);
/// assert_eq!(value["texture"], "../art/a.png");
/// ```
pub struct AssetPathSchema;

impl JsonSchema for AssetPathSchema {
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("AssetPath")
    }

    fn inline_schema() -> bool {
        true
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "description": "An asset file path.",
            "x-asset-path": true
        })
    }
}

/// Finds every marked asset-path field in a component's JSON Schema, as sorted, deduplicated
/// suffix patterns relative to the component's value (`""` is the value itself).
///
/// Walks `properties`, `$ref` (resolved against `schema`), `anyOf`/`oneOf`/`allOf`,
/// `additionalProperties` (a map's values, segment `*`) and `items` (a list's elements, segment
/// `*`). Stops at a marked node.
pub fn asset_path_fields(schema: &Value) -> Vec<String> {
    let mut found = Vec::new();
    walk(schema, schema, "", 0, &mut found);
    found.sort();
    found.dedup();
    found
}

fn walk(root: &Value, node: &Value, suffix: &str, depth: usize, found: &mut Vec<String>) {
    if depth > 12 {
        return;
    }
    if node.get(ASSET_PATH_KEYWORD).is_some() {
        found.push(suffix.to_string());
        return;
    }
    if let Some(target) = node.get("$ref").and_then(Value::as_str)
        && let Some(resolved) = root.pointer(target.trim_start_matches('#'))
    {
        walk(root, resolved, suffix, depth + 1, found);
    }
    if let Some(properties) = node.get("properties").and_then(Value::as_object) {
        for (key, property) in properties {
            walk(root, property, &format!("{suffix}/{key}"), depth + 1, found);
        }
    }
    for combinator in ["anyOf", "oneOf", "allOf"] {
        for branch in node
            .get(combinator)
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice)
        {
            walk(root, branch, suffix, depth + 1, found);
        }
    }
    for each in ["additionalProperties", "items"] {
        if let Some(inner) = node.get(each).filter(|inner| inner.is_object()) {
            walk(root, inner, &format!("{suffix}/*"), depth + 1, found);
        }
    }
}

/// Hands each present asset-path string of `value` to `f`, with its concrete JSON Pointer (keys
/// escaped with `~0`/`~1`), for in-place rewriting.
///
/// `patterns` come from [`asset_path_fields`]. A `*` segment visits every key of an object (in map
/// order) or every element of an array. A missing key or a non-string leaf is skipped silently: a
/// partial override value holds only some fields.
pub fn for_each_asset_path(
    patterns: &[String],
    value: &mut Value,
    f: &mut dyn FnMut(&str, &mut String),
) {
    for pattern in patterns {
        let segments: Vec<&str> = if pattern.is_empty() {
            Vec::new()
        } else {
            pattern.split('/').skip(1).collect()
        };
        visit(&segments, value, &mut String::new(), f);
    }
}

fn visit(
    segments: &[&str],
    value: &mut Value,
    pointer: &mut String,
    f: &mut dyn FnMut(&str, &mut String),
) {
    let Some((segment, rest)) = segments.split_first() else {
        if let Value::String(path) = value {
            f(pointer, path);
        }
        return;
    };
    let keep = pointer.len();
    if *segment == "*" {
        match value {
            Value::Object(map) => {
                for (key, child) in map.iter_mut() {
                    push_escaped(pointer, key);
                    visit(rest, child, pointer, f);
                    pointer.truncate(keep);
                }
            }
            Value::Array(items) => {
                for (index, child) in items.iter_mut().enumerate() {
                    pointer.push('/');
                    pointer.push_str(&index.to_string());
                    visit(rest, child, pointer, f);
                    pointer.truncate(keep);
                }
            }
            _ => {}
        }
    } else if let Some(child) = value.get_mut(*segment) {
        push_escaped(pointer, segment);
        visit(rest, child, pointer, f);
        pointer.truncate(keep);
    }
}

fn push_escaped(pointer: &mut String, key: &str) {
    pointer.push('/');
    pointer.push_str(&key.replace('~', "~0").replace('/', "~1"));
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn marked() -> Value {
        json!({"type": "string", "x-asset-path": true})
    }

    fn fields(schema: &Value) -> Vec<String> {
        asset_path_fields(schema)
    }

    #[test]
    fn a_marked_root_is_the_empty_pattern() {
        assert_eq!(fields(&marked()), [""]);
    }

    #[test]
    fn properties_are_found_sorted() {
        let schema = json!({
            "type": "object",
            "properties": {"b": marked(), "a": marked(), "n": {"type": "integer"}}
        });
        assert_eq!(fields(&schema), ["/a", "/b"]);
    }

    #[test]
    fn refs_resolve_against_the_root() {
        let schema = json!({
            "type": "object",
            "properties": {"tex": {"$ref": "#/$defs/Tex"}},
            "$defs": {"Tex": marked()}
        });
        assert_eq!(fields(&schema), ["/tex"]);
    }

    #[test]
    fn enum_variants_each_contribute_their_suffix() {
        let schema = json!({
            "oneOf": [
                {"type": "object", "properties": {"asset": marked()}},
                {"type": "object", "properties": {"voxel": marked()}},
                {"type": "object", "properties": {"nested": {"type": "object", "properties": {"asset": marked()}}}},
                {"type": "string", "enum": ["cube"]}
            ]
        });
        assert_eq!(fields(&schema), ["/asset", "/nested/asset", "/voxel"]);
    }

    #[test]
    fn map_values_and_array_elements_use_a_star_segment() {
        let schema = json!({
            "type": "object",
            "properties": {
                "legend": {"type": "object", "additionalProperties": {"type": "object", "properties": {"texture": marked()}}},
                "list": {"type": "array", "items": marked()}
            }
        });
        assert_eq!(fields(&schema), ["/legend/*/texture", "/list/*"]);
    }

    #[test]
    fn an_unmarked_schema_has_no_fields() {
        let schema = json!({"type": "object", "properties": {"hp": {"type": "integer"}}});
        assert!(fields(&schema).is_empty());
    }

    fn collect(patterns: &[&str], value: &mut Value) -> Vec<(String, String)> {
        let patterns: Vec<String> = patterns.iter().map(|p| p.to_string()).collect();
        let mut seen = Vec::new();
        for_each_asset_path(&patterns, value, &mut |pointer, path| {
            seen.push((pointer.to_string(), path.clone()));
            path.push('!');
        });
        seen
    }

    #[test]
    fn the_visitor_hands_over_concrete_pointers_and_rewrites_in_place() {
        let mut value = json!({
            "legend": {"a": {"shape": {"asset": "x.glb"}}, "b": {"shape": {"asset": "y.glb"}}},
            "list": ["p", "q"]
        });
        let seen = collect(&["/legend/*/shape/asset", "/list/*"], &mut value);
        assert_eq!(
            seen,
            [
                ("/legend/a/shape/asset".to_string(), "x.glb".to_string()),
                ("/legend/b/shape/asset".to_string(), "y.glb".to_string()),
                ("/list/0".to_string(), "p".to_string()),
                ("/list/1".to_string(), "q".to_string()),
            ]
        );
        assert_eq!(value["legend"]["b"]["shape"]["asset"], "y.glb!");
        assert_eq!(value["list"][1], "q!");
    }

    #[test]
    fn a_missing_key_or_non_string_leaf_is_skipped() {
        let mut value = json!({"texture": 3, "other": null});
        assert!(collect(&["/texture", "/atlas", "/other", "/legend/*/x"], &mut value).is_empty());
        assert_eq!(value, json!({"texture": 3, "other": null}));
    }

    #[test]
    fn keys_are_escaped_in_the_pointer() {
        let mut value = json!({"m": {"a/b": "p", "c~d": "q"}});
        let seen = collect(&["/m/*"], &mut value);
        assert_eq!(seen[0].0, "/m/a~1b");
        assert_eq!(seen[1].0, "/m/c~0d");
    }

    #[test]
    fn the_empty_pattern_is_the_value_itself() {
        let mut value = json!("a.png");
        assert_eq!(collect(&[""], &mut value).len(), 1);
        assert_eq!(value, "a.png!");
    }
}

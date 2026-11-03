//! A small cursor over a component's JSON Schema (as produced by `schemars` via the registry).
//!
//! Both the validator (to check values) and the canonical emitter (to order fields in *struct
//! declaration order* rather than alphabetically) walk schemas the same way; this cursor is that
//! shared walk. It resolves local `$ref`s (`#/$defs/...`) against the component's schema root and
//! navigates `properties` (through `allOf`), `additionalProperties`, `items`, and `prefixItems`;
//! [`SchemaCursor::select`] picks the `oneOf`/`anyOf` alternative a value takes. Everything is
//! best-effort: an unresolvable reference or an alternative nothing matches simply ends navigation
//! (the emitter falls back to document order; the validator skips what it cannot see).

use serde_json::Value;

/// How a JSON object's keys are ordered at a schema position.
pub(crate) enum KeyOrder<'a> {
    /// The schema declares properties: their declaration order.
    Declared(Vec<&'a str>),
    /// A map (`additionalProperties`, no declared properties): byte order, the order a
    /// `BTreeMap<String, _>` serializes in.
    Sorted,
    /// The schema says nothing: document order.
    Document,
}

/// Bound on `$ref`/alternative/`allOf` chains, so a reference cycle cannot hang a walk.
const MAX_DEPTH: usize = 16;

/// A position inside one component's JSON Schema.
#[derive(Clone, Copy)]
pub(crate) struct SchemaCursor<'a> {
    /// The component's schema root — the document `$ref`s resolve against.
    root: &'a Value,
    node: &'a Value,
}

impl<'a> SchemaCursor<'a> {
    /// A cursor at the root of a component schema.
    pub fn new(root: &'a Value) -> Self {
        SchemaCursor { root, node: root }
    }

    /// A sibling cursor at `node` (same root, e.g. for a `oneOf` alternative).
    pub fn with(self, node: &'a Value) -> Self {
        SchemaCursor {
            root: self.root,
            node,
        }
    }

    /// The current subschema with local `$ref`s chased (bounded, so reference cycles cannot
    /// hang the walk).
    pub fn resolved(self) -> &'a Value {
        let mut node = self.node;
        for _ in 0..16 {
            let Some(reference) = node.get("$ref").and_then(Value::as_str) else {
                break;
            };
            let Some(target) = reference
                .strip_prefix('#')
                .and_then(|pointer| self.root.pointer(pointer))
            else {
                break;
            };
            node = target;
        }
        node
    }

    /// The nodes on the `$ref` chain of [`resolved`](Self::resolved) that hold a `$ref`: where
    /// keywords written beside a reference (a `schemars` range on a newtype field) live.
    pub fn ref_siblings(self) -> Vec<&'a Value> {
        let mut siblings = Vec::new();
        let mut node = self.node;
        for _ in 0..MAX_DEPTH {
            let Some(reference) = node.get("$ref").and_then(Value::as_str) else {
                break;
            };
            siblings.push(node);
            let Some(target) = reference
                .strip_prefix('#')
                .and_then(|pointer| self.root.pointer(pointer))
            else {
                break;
            };
            node = target;
        }
        siblings
    }

    /// The declared property names in schema (= struct declaration) order, if this subschema
    /// describes an object with `properties` (its own first, then each `allOf` part's).
    pub fn property_order(self) -> Option<Vec<&'a str>> {
        self.property_order_at(0)
    }

    fn property_order_at(self, depth: usize) -> Option<Vec<&'a str>> {
        if depth > MAX_DEPTH {
            return None;
        }
        let node = self.resolved();
        let mut names: Vec<&'a str> = Vec::new();
        let mut found = false;
        if let Some(properties) = node.get("properties").and_then(Value::as_object) {
            found = true;
            names.extend(properties.keys().map(String::as_str));
        }
        if let Some(parts) = node.get("allOf").and_then(Value::as_array) {
            for part in parts {
                if let Some(part_names) = self.with(part).property_order_at(depth + 1) {
                    found = true;
                    for name in part_names {
                        if !names.contains(&name) {
                            names.push(name);
                        }
                    }
                }
            }
        }
        found.then_some(names)
    }

    /// How an object at this position orders its keys. Const-valued properties — an internally
    /// tagged enum's tag, which `schemars` lists last and serde writes first — lead, in declared
    /// order.
    pub fn key_order(self) -> KeyOrder<'a> {
        match self.property_order() {
            Some(mut names) if !names.is_empty() => {
                let is_tag = |name: &&str| {
                    self.child(name)
                        .is_some_and(|child| child.resolved().get("const").is_some())
                };
                // Stable: tags and the rest each keep their declared order.
                names.sort_by_key(|name| !is_tag(name));
                KeyOrder::Declared(names)
            }
            _ if self.open_map() => KeyOrder::Sorted,
            _ => KeyOrder::Document,
        }
    }

    /// `true` when `additionalProperties` is a schema or `true`: undeclared keys are data (a map's
    /// entries), not typos.
    pub fn open_map(self) -> bool {
        matches!(
            self.resolved().get("additionalProperties"),
            Some(Value::Object(_) | Value::Bool(true))
        )
    }

    /// Descends into the schema of field `key`: a declared property (through `allOf`), else the
    /// `additionalProperties` schema.
    pub fn child(self, key: &str) -> Option<Self> {
        self.child_at(key, 0)
    }

    fn child_at(self, key: &str, depth: usize) -> Option<Self> {
        if depth > MAX_DEPTH {
            return None;
        }
        let node = self.resolved();
        if let Some(declared) = node.get("properties").and_then(|p| p.get(key)) {
            return Some(self.with(declared));
        }
        if let Some(parts) = node.get("allOf").and_then(Value::as_array) {
            for part in parts {
                if let Some(child) = self.with(part).child_at(key, depth + 1) {
                    return Some(child);
                }
            }
        }
        match node.get("additionalProperties") {
            Some(schema @ Value::Object(_)) => Some(self.with(schema)),
            _ => None,
        }
    }

    /// Descends `$ref`/`oneOf`/`anyOf` to the alternative `value` takes (first structural match, in
    /// schema order); unchanged when nothing matches or there is nothing to choose.
    pub fn select(self, value: &Value) -> Self {
        let mut cursor = self;
        for _ in 0..MAX_DEPTH {
            let Some(alternatives) = alternatives(cursor.resolved()) else {
                break;
            };
            let Some(chosen) = alternatives
                .iter()
                .find(|alternative| cursor.with(alternative).matches(value, 0))
            else {
                break;
            };
            cursor = cursor.with(chosen);
        }
        cursor
    }

    /// Whether `value` structurally fits the current subschema: JSON type, `const`/`enum`, and for
    /// an object the declared properties (required present, none undeclared, tag constants equal).
    /// Shallow on purpose — enough to tell alternatives apart, not a validator.
    fn matches(self, value: &Value, depth: usize) -> bool {
        if depth > MAX_DEPTH {
            return true;
        }
        let node = self.resolved();
        if !type_admits(node, value) || !literal_admits(node, value) {
            return false;
        }
        if let (Some(properties), Some(object)) = (
            node.get("properties").and_then(Value::as_object),
            value.as_object(),
        ) {
            let required = node.get("required").and_then(Value::as_array);
            if required
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .any(|key| !object.contains_key(key))
            {
                return false;
            }
            for (key, field) in object {
                let Some(declared) = properties.get(key) else {
                    return false;
                };
                if !literal_admits(self.with(declared).resolved(), field) {
                    return false;
                }
            }
        }
        match alternatives(node) {
            Some(alternatives) => alternatives
                .iter()
                .any(|alternative| self.with(alternative).matches(value, depth + 1)),
            None => true,
        }
    }

    /// Descends into the schema of array element `index` (`prefixItems` first, then `items`).
    pub fn item(self, index: usize) -> Option<Self> {
        let resolved = self.resolved();
        if let Some(prefix) = resolved.get("prefixItems").and_then(Value::as_array)
            && let Some(node) = prefix.get(index)
        {
            return Some(self.with(node));
        }
        resolved.get("items").map(|node| self.with(node))
    }
}

/// The `oneOf` (else `anyOf`) alternatives of a resolved node.
fn alternatives(node: &Value) -> Option<&Vec<Value>> {
    node.get("oneOf")
        .or_else(|| node.get("anyOf"))
        .and_then(Value::as_array)
}

/// `false` when the node's `type` (a name or a list of names) excludes `value`'s JSON type.
fn type_admits(node: &Value, value: &Value) -> bool {
    let admits = |name: &str| match (name, value) {
        ("null", Value::Null)
        | ("boolean", Value::Bool(_))
        | ("object", Value::Object(_))
        | ("array", Value::Array(_))
        | ("string", Value::String(_))
        | ("number", Value::Number(_)) => true,
        ("integer", Value::Number(number)) => number.is_i64() || number.is_u64(),
        _ => false,
    };
    match node.get("type") {
        Some(Value::String(name)) => admits(name),
        Some(Value::Array(names)) => names.iter().filter_map(Value::as_str).any(admits),
        _ => true,
    }
}

/// `false` when the node's `const` or `enum` excludes `value`.
fn literal_admits(node: &Value, value: &Value) -> bool {
    if let Some(expected) = node.get("const")
        && expected != value
    {
        return false;
    }
    if let Some(allowed) = node.get("enum").and_then(Value::as_array)
        && !allowed.contains(value)
    {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn resolves_refs_and_orders_properties() {
        let root = json!({
            "type": "object",
            "properties": {
                "zulu": { "$ref": "#/$defs/Inner" },
                "alpha": { "type": "number" }
            },
            "$defs": {
                "Inner": {
                    "type": "object",
                    "properties": { "b": { "type": "string" }, "a": { "type": "string" } }
                }
            }
        });
        let cursor = SchemaCursor::new(&root);
        // preserve_order keeps declaration order — this is what "struct order, not
        // alphabetical" rests on.
        assert_eq!(cursor.property_order().unwrap(), vec!["zulu", "alpha"]);
        let inner = cursor.child("zulu").unwrap();
        assert_eq!(inner.property_order().unwrap(), vec!["b", "a"]);
    }

    #[test]
    fn item_prefers_prefix_items() {
        let root = json!({
            "type": "array",
            "prefixItems": [ { "type": "number" }, { "type": "string" } ],
            "items": { "type": "boolean" }
        });
        let cursor = SchemaCursor::new(&root);
        assert_eq!(cursor.item(1).unwrap().resolved()["type"], "string");
        assert_eq!(cursor.item(2).unwrap().resolved()["type"], "boolean");
    }

    #[test]
    fn unresolvable_ref_stops_gracefully() {
        let root = json!({ "$ref": "#/$defs/Missing" });
        let cursor = SchemaCursor::new(&root);
        assert!(cursor.property_order().is_none());
    }

    fn order(cursor: SchemaCursor<'_>) -> Vec<&str> {
        cursor.property_order().expect("an object schema")
    }

    #[test]
    fn one_of_picks_the_variant_by_key() {
        let root = json!({
            "oneOf": [
                {
                    "type": "object",
                    "properties": { "hinge": { "type": "object", "properties": {
                        "limits": {"type": "array"}, "axis": {"type": "array"} } } },
                    "required": ["hinge"],
                    "additionalProperties": false
                },
                {
                    "type": "object",
                    "properties": { "ball": { "type": "object", "properties": {
                        "cone": {"type": "number"}, "twist": {"type": "number"} } } },
                    "required": ["ball"],
                    "additionalProperties": false
                }
            ]
        });
        let cursor = SchemaCursor::new(&root);
        let hinge = cursor.select(&json!({ "hinge": { "axis": [], "limits": [] } }));
        assert_eq!(order(hinge), vec!["hinge"]);
        assert_eq!(order(hinge.child("hinge").unwrap()), vec!["limits", "axis"]);
        let ball = cursor.select(&json!({ "ball": {} }));
        assert_eq!(order(ball), vec!["ball"]);
        assert_eq!(order(ball.child("ball").unwrap()), vec!["cone", "twist"]);
    }

    #[test]
    fn any_of_with_null_picks_the_ref() {
        let root = json!({
            "anyOf": [ { "$ref": "#/$defs/Inner" }, { "type": "null" } ],
            "$defs": { "Inner": { "type": "object", "properties": { "b": {}, "a": {} } } }
        });
        let cursor = SchemaCursor::new(&root);
        assert_eq!(
            order(cursor.select(&json!({ "a": 1, "b": 2 }))),
            vec!["b", "a"]
        );
        assert_eq!(cursor.select(&Value::Null).resolved()["type"], "null");
    }

    #[test]
    fn internally_tagged_variants_pick_by_const() {
        let root = json!({
            "oneOf": [
                { "type": "object",
                  "properties": { "type": { "type": "string", "const": "a" }, "z": {}, "y": {} },
                  "required": ["type"] },
                { "type": "object",
                  "properties": { "type": { "type": "string", "const": "b" }, "y": {}, "z": {} },
                  "required": ["type"] }
            ]
        });
        let cursor = SchemaCursor::new(&root);
        assert_eq!(
            order(cursor.select(&json!({ "type": "a", "y": 1 }))),
            vec!["type", "z", "y"]
        );
        assert_eq!(
            order(cursor.select(&json!({ "type": "b", "y": 1 }))),
            vec!["type", "y", "z"]
        );
        // schemars lists the tag last; the emitted order leads with it, as serde writes it.
        let last = json!({ "properties": { "z": {}, "y": {}, "type": { "const": "a" } } });
        assert!(matches!(
            SchemaCursor::new(&last).key_order(),
            KeyOrder::Declared(names) if names == ["type", "z", "y"]
        ));
    }

    #[test]
    fn mixed_unit_and_struct_variants() {
        let root = json!({
            "oneOf": [
                { "type": "string", "const": "Idle" },
                { "type": "object", "properties": { "Run": {} }, "required": ["Run"] }
            ]
        });
        let cursor = SchemaCursor::new(&root);
        assert_eq!(cursor.select(&json!("Idle")).resolved()["const"], "Idle");
        assert_eq!(order(cursor.select(&json!({ "Run": 1 }))), vec!["Run"]);
    }

    #[test]
    fn nested_option_of_enum_descends_through_both() {
        let root = json!({
            "anyOf": [ { "$ref": "#/$defs/Kind" }, { "type": "null" } ],
            "$defs": { "Kind": { "oneOf": [
                { "type": "object", "properties": { "a": {} }, "required": ["a"] },
                { "type": "object", "properties": { "b": {} }, "required": ["b"] }
            ] } }
        });
        let cursor = SchemaCursor::new(&root);
        assert_eq!(order(cursor.select(&json!({ "b": 1 }))), vec!["b"]);
    }

    #[test]
    fn additional_properties_sort_and_resolve_children() {
        let root = json!({
            "type": "object",
            "additionalProperties": { "$ref": "#/$defs/Entry" },
            "$defs": { "Entry": { "type": "object", "properties": { "b": {}, "a": {} } } }
        });
        let cursor = SchemaCursor::new(&root);
        assert!(matches!(cursor.key_order(), KeyOrder::Sorted));
        assert_eq!(order(cursor.child("any-key").unwrap()), vec!["b", "a"]);
        let closed = json!({ "type": "object", "properties": { "x": {} } });
        assert!(matches!(
            SchemaCursor::new(&closed).key_order(),
            KeyOrder::Declared(names) if names == ["x"]
        ));
        let bare = json!({ "type": "object" });
        assert!(matches!(
            SchemaCursor::new(&bare).key_order(),
            KeyOrder::Document
        ));
    }

    #[test]
    fn all_of_concatenates_properties_without_duplicates() {
        let root = json!({
            "properties": { "own": {} },
            "allOf": [
                { "properties": { "b": { "type": "string" }, "a": {} } },
                { "properties": { "a": {}, "c": {} } }
            ]
        });
        let cursor = SchemaCursor::new(&root);
        assert_eq!(order(cursor), vec!["own", "b", "a", "c"]);
        assert_eq!(cursor.child("b").unwrap().resolved()["type"], "string");
        assert!(cursor.child("missing").is_none());
    }

    #[test]
    fn no_match_leaves_the_cursor_unchanged() {
        let root = json!({
            "oneOf": [ { "type": "string", "const": "x" }, { "type": "number" } ]
        });
        let cursor = SchemaCursor::new(&root);
        assert!(std::ptr::eq(cursor.select(&json!(true)).resolved(), &root));
    }
}

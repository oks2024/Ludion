//! Entity-reference fields: the schema marker a component field carries, and the walk that finds
//! every marked field in a component's schema.
//!
//! `EntityId` lives in `ludion-core`, which has no `schemars` dependency, so a component field
//! holding one borrows a proxy through `#[schemars(with = …)]`. The proxy is also the marker: it
//! stamps the field's schema with `"x-entity-ref": {"allow_self": …}`, and the [`Registry`] derives
//! [`ComponentInfo::entity_refs`] from it once, at registration. `scene.check`'s range check and
//! `scene.save`'s remap both read that list, so a game component's reference is covered the moment
//! its field is marked.
//!
//! [`Registry`]: crate::Registry
//! [`ComponentInfo::entity_refs`]: crate::ComponentInfo::entity_refs

use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde_json::{Value, json};

/// The schema keyword an entity-reference field carries: `{"allow_self": bool}`, plus
/// `"hierarchy": true` on a tree link.
pub const ENTITY_REF_KEYWORD: &str = "x-entity-ref";

/// Schema proxy for an [`EntityId`](ludion_core::ids::EntityId) field that must **not** name the
/// entity carrying the component. Never constructed: use it as
/// `#[schemars(with = "ludion_ecs::EntityRefSchema")]`, or `with = "Option<ludion_ecs::EntityRefSchema>"`
/// for an optional field.
///
/// # Example
/// ```
/// use ludion_core::ids::EntityId;
/// use ludion_ecs::Registry;
/// use schemars::JsonSchema;
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Serialize, Deserialize, JsonSchema)]
/// struct Leash {
///     #[schemars(with = "ludion_ecs::EntityRefSchema")]
///     holder: EntityId,
/// }
///
/// let mut registry = Registry::new();
/// registry.register::<Leash>("Leash").unwrap();
/// let fields = registry.get("Leash").unwrap().entity_refs();
/// assert_eq!(fields[0].suffix, "/holder");
/// assert!(!fields[0].allow_self);
/// ```
pub struct EntityRefSchema;

/// Schema proxy for an [`EntityId`](ludion_core::ids::EntityId) field that **may** name the entity
/// carrying the component; see [`EntityRefSchema`].
pub struct EntityRefSelfSchema;

pub(crate) fn entity_ref_schema(description: &'static str, allow_self: bool) -> Schema {
    ref_schema(description, json!({ "allow_self": allow_self }))
}

/// The marker of a tree link (`Parent`): never self, and a scene rejects cycles through it.
pub(crate) fn hierarchy_ref_schema(description: &'static str) -> Schema {
    ref_schema(
        description,
        json!({ "allow_self": false, "hierarchy": true }),
    )
}

fn ref_schema(description: &'static str, marker: Value) -> Schema {
    json_schema!({
        "type": "object",
        "description": description,
        "properties": {
            "index": { "type": "integer", "minimum": 0 },
            "generation": { "type": "integer", "minimum": 0 }
        },
        "required": ["index", "generation"],
        "x-entity-ref": marker
    })
}

impl JsonSchema for EntityRefSchema {
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("EntityRef")
    }

    fn inline_schema() -> bool {
        true
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        entity_ref_schema(
            "A reference to another entity by its generational EntityId.",
            false,
        )
    }
}

impl JsonSchema for EntityRefSelfSchema {
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("EntityRefSelf")
    }

    fn inline_schema() -> bool {
        true
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        entity_ref_schema(
            "A reference to an entity, possibly the carrier itself, by its generational EntityId.",
            true,
        )
    }
}

/// One entity-reference field of a registered component; see
/// [`ComponentInfo::entity_refs`](crate::ComponentInfo::entity_refs).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntityRefField {
    /// JSON Pointer suffix into the component's *own* value (relative to its document position;
    /// `""` means the component's value itself, the `Parent` shape).
    pub suffix: String,
    /// Whether the reference may name the entity that carries the component.
    pub allow_self: bool,
    /// An `Option<EntityId>` field: absent or `null` is no reference, and `scene.save` writes a
    /// dangling one as `null` instead of refusing.
    pub optional: bool,
    /// The field is a tree link (`Parent`): a scene rejects a cycle through it (`L1035`).
    pub hierarchy: bool,
}

/// Finds every marked entity-reference field in a component's JSON Schema, sorted by suffix.
///
/// Walks `properties`, `$ref` (resolved against `schema`) and `anyOf`/`oneOf`/`allOf`. A field is
/// optional when it is absent from its object's `required`, when a `null` branch or `"null"` type
/// admits `null`, when an enclosing object is optional (optionality is inherited), or when it sits
/// in one variant of several (`anyOf`/`oneOf` with more than one non-null branch): it is then
/// present only with its variant. A suffix found more than once (the same field in several
/// variants) is merged into one entry, refusing self-reference unless every occurrence allows it.
/// Stops at a marked node. Arrays are not walked: a marked field inside one is not found, so a
/// component holding a list of references is a documented gap, not a silent one (the engine's
/// guard test compares this walk against the `{index, generation}` shape).
pub fn entity_ref_fields(schema: &Value) -> Vec<EntityRefField> {
    let mut found = Vec::new();
    walk(schema, schema, "", false, 0, &mut found);
    found.sort_by(|a, b| a.suffix.cmp(&b.suffix));
    found.dedup_by(|later, kept| {
        let same = later.suffix == kept.suffix;
        if same {
            kept.allow_self &= later.allow_self;
            kept.optional |= later.optional;
            kept.hierarchy |= later.hierarchy;
        }
        same
    });
    found
}

fn walk(
    root: &Value,
    node: &Value,
    suffix: &str,
    optional: bool,
    depth: usize,
    found: &mut Vec<EntityRefField>,
) {
    if depth > 12 {
        return;
    }
    let optional = optional || is_nullable(node);
    if let Some(marker) = node.get(ENTITY_REF_KEYWORD) {
        found.push(EntityRefField {
            suffix: suffix.to_string(),
            allow_self: marker
                .get("allow_self")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            optional,
            hierarchy: marker
                .get("hierarchy")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        });
        return;
    }
    if let Some(target) = node.get("$ref").and_then(Value::as_str)
        && let Some(resolved) = root.pointer(target.trim_start_matches('#'))
    {
        walk(root, resolved, suffix, optional, depth + 1, found);
    }
    if let Some(properties) = node.get("properties").and_then(Value::as_object) {
        let required = node.get("required").and_then(Value::as_array);
        for (key, property) in properties {
            let is_required = required.is_some_and(|names| names.iter().any(|n| n == key));
            let at = format!("{suffix}/{key}");
            walk(
                root,
                property,
                &at,
                optional || !is_required,
                depth + 1,
                found,
            );
        }
    }
    for combinator in ["anyOf", "oneOf", "allOf"] {
        let branches = node
            .get(combinator)
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        let non_null = branches.iter().filter(|b| !is_null_branch(b)).count();
        let variants = combinator != "allOf" && non_null > 1;
        for branch in branches {
            walk(root, branch, suffix, optional || variants, depth + 1, found);
        }
    }
}

fn is_null_type(ty: &Value) -> bool {
    match ty {
        Value::String(name) => name == "null",
        Value::Array(names) => names.iter().any(|n| n == "null"),
        _ => false,
    }
}

fn is_null_branch(branch: &Value) -> bool {
    branch.get("type").is_some_and(is_null_type)
}

/// Whether `node` admits `null`: a `"null"` type (alone or in a type array) or a `null` branch of
/// an `anyOf`/`oneOf`.
fn is_nullable(node: &Value) -> bool {
    node.get("type").is_some_and(is_null_type)
        || ["anyOf", "oneOf"].iter().any(|combinator| {
            node.get(combinator)
                .and_then(Value::as_array)
                .is_some_and(|branches| branches.iter().any(is_null_branch))
        })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn marker(allow_self: bool) -> Value {
        json!({
            "type": "object",
            "properties": {"index": {"type": "integer"}, "generation": {"type": "integer"}},
            "required": ["index", "generation"],
            "x-entity-ref": {"allow_self": allow_self}
        })
    }

    fn field(suffix: &str, allow_self: bool, optional: bool) -> EntityRefField {
        EntityRefField {
            suffix: suffix.to_string(),
            allow_self,
            optional,
            hierarchy: false,
        }
    }

    #[test]
    fn only_the_hierarchy_marker_sets_the_hierarchy_flag() {
        let tree = serde_json::to_value(hierarchy_ref_schema("tree link")).unwrap();
        let plain = serde_json::to_value(entity_ref_schema("plain link", false)).unwrap();
        let mut expected = field("", false, false);
        expected.hierarchy = true;
        assert_eq!(entity_ref_fields(&tree), [expected]);
        assert_eq!(entity_ref_fields(&plain), [field("", false, false)]);
        assert_eq!(
            plain["x-entity-ref"],
            json!({"allow_self": false}),
            "the plain marker keeps its exact output"
        );
    }

    #[test]
    fn a_marked_root_is_the_empty_suffix() {
        assert_eq!(entity_ref_fields(&marker(false)), [field("", false, false)]);
    }

    #[test]
    fn nested_fields_are_found_sorted_and_required_ones_are_not_optional() {
        let schema = json!({
            "type": "object",
            "properties": {"b": marker(true), "a": marker(false), "n": {"type": "integer"}},
            "required": ["a", "b", "n"]
        });
        assert_eq!(
            entity_ref_fields(&schema),
            [field("/a", false, false), field("/b", true, false)]
        );
    }

    #[test]
    fn a_field_missing_from_required_is_optional() {
        let schema = json!({
            "type": "object",
            "properties": {"source": marker(true)},
        });
        assert_eq!(entity_ref_fields(&schema), [field("/source", true, true)]);
    }

    #[test]
    fn a_null_branch_or_null_type_is_optional() {
        let schema = json!({
            "type": "object",
            "properties": {
                "any": {"anyOf": [marker(false), {"type": "null"}]},
                "ty": {"type": ["object", "null"], "x-entity-ref": {"allow_self": true}}
            },
            "required": ["any", "ty"]
        });
        assert_eq!(
            entity_ref_fields(&schema),
            [field("/any", false, true), field("/ty", true, true)]
        );
    }

    #[test]
    fn refs_resolve_against_the_root() {
        let schema = json!({
            "type": "object",
            "properties": {"holder": {"$ref": "#/$defs/Holder"}},
            "required": ["holder"],
            "$defs": {"Holder": marker(true)}
        });
        assert_eq!(entity_ref_fields(&schema), [field("/holder", true, false)]);
    }

    #[test]
    fn a_marked_field_inside_an_optional_struct_is_optional() {
        let schema = json!({
            "type": "object",
            "properties": {
                "inner": {"anyOf": [{"$ref": "#/$defs/Inner"}, {"type": "null"}]},
                "outer": {"type": "object", "properties": {"holder": marker(false)}, "required": ["holder"]}
            },
            "required": ["inner"],
            "$defs": {
                "Inner": {"type": "object", "properties": {"holder": marker(false)}, "required": ["holder"]}
            }
        });
        assert_eq!(
            entity_ref_fields(&schema),
            [
                field("/inner/holder", false, true),
                field("/outer/holder", false, true)
            ]
        );
    }

    #[test]
    fn variant_fields_are_optional_and_shared_suffixes_merge() {
        let schema = json!({
            "oneOf": [
                {
                    "type": "object",
                    "properties": {"target": marker(true), "left": marker(false)},
                    "required": ["target", "left"]
                },
                {
                    "type": "object",
                    "properties": {"target": marker(false), "right": marker(true)},
                    "required": ["target", "right"]
                }
            ]
        });
        assert_eq!(
            entity_ref_fields(&schema),
            [
                field("/left", false, true),
                field("/right", true, true),
                field("/target", false, true)
            ]
        );
    }

    #[test]
    fn an_unmarked_schema_has_no_fields() {
        let schema = json!({"type": "object", "properties": {"hp": {"type": "integer"}}});
        assert!(entity_ref_fields(&schema).is_empty());
    }
}

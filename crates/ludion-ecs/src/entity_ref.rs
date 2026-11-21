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
use std::marker::PhantomData;

use ludion_core::ids::EntityId;
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde_json::{Value, json};

use crate::asset_path::pointer_segment;

/// The schema keyword an entity-reference field carries: `{"allow_self": bool}`, plus
/// `"hierarchy": true` on a tree link.
pub const ENTITY_REF_KEYWORD: &str = "x-entity-ref";

/// The schema keyword [`EntityRefCollection`] stamps on a list or map: its elements may hold marked
/// entity references.
pub const ENTITY_REF_COLLECTION_KEYWORD: &str = "x-entity-ref-collection";

/// Schema proxy for an [`EntityId`](ludion_core::ids::EntityId) field that must **not** name the
/// entity carrying the component. Never constructed: a game writes
/// `#[schemars(with = "EntityRefSchema")]` (the name is in `ludion::prelude`), or
/// `with = "Option<EntityRefSchema>"` for an optional field. A list or map of references is refused
/// at registration (`L1392`) unless wrapped in [`EntityRefCollection`]; otherwise hold each
/// reference in its own field.
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

/// Schema proxy that sanctions a list (`Vec<T>`) or a string-keyed map (`BTreeMap<String, V>`) whose
/// elements hold marked entity references at any depth. Never constructed: a game writes
/// `#[schemars(with = "EntityRefCollection<Vec<T>>")]`. The walk reports each such reference as a
/// pattern field whose suffix holds a `/*` step; [`ComponentInfo::entity_ref_sites`] expands it to the
/// concrete references of a value. Use `Option<EntityRefSchema>` for the elements' references so a
/// despawned target nulls on save or carry instead of refusing the save. A tuple is never sanctioned.
///
/// # Example
/// ```
/// use ludion_core::ids::EntityId;
/// use ludion_ecs::{EntityRefCollection, EntityRefSchema, Registry};
/// use schemars::JsonSchema;
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Serialize, Deserialize, JsonSchema)]
/// struct Slot {
///     #[schemars(with = "Option<EntityRefSchema>")]
///     who: Option<EntityId>,
/// }
///
/// #[derive(Serialize, Deserialize, JsonSchema)]
/// struct Roster {
///     #[schemars(with = "EntityRefCollection<Vec<Slot>>")]
///     slots: Vec<Slot>,
/// }
///
/// let mut registry = Registry::new();
/// registry.register::<Roster>("Roster").unwrap();
/// let info = registry.get("Roster").unwrap();
/// assert_eq!(info.entity_refs()[0].suffix, "/slots/*/who");
/// ```
///
/// [`ComponentInfo::entity_ref_sites`]: crate::ComponentInfo::entity_ref_sites
pub struct EntityRefCollection<C>(PhantomData<C>);

impl<C: JsonSchema> JsonSchema for EntityRefCollection<C> {
    fn schema_name() -> Cow<'static, str> {
        Cow::Owned(format!("EntityRefCollection<{}>", C::schema_name()))
    }

    fn inline_schema() -> bool {
        true
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let mut schema = C::json_schema(generator);
        schema.insert(ENTITY_REF_COLLECTION_KEYWORD.to_string(), Value::Bool(true));
        schema
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
    /// The field may be absent (a missing key, an `Option`, an optional enclosing struct, or one
    /// variant of several): absent is no reference.
    pub optional: bool,
    /// `null` is a valid value at the field's own position (`Option<EntityId>`): `scene.save` and a
    /// scene carry write a dangling one as `null` instead of refusing or dropping it. A field that
    /// is only optional through an enclosing `Option<struct>` is not nullable.
    pub nullable: bool,
    /// The field is a tree link (`Parent`): a scene rejects a cycle through it (`L1035`).
    pub hierarchy: bool,
    /// The suffix holds a `/*` wildcard step (the field sits under an [`EntityRefCollection`]): it
    /// is a pattern, and [`EntityRefField::sites`] expands it against a value.
    pub collection: bool,
}

impl EntityRefField {
    /// The concrete fields of this one in `value`: itself when it holds no wildcard, otherwise one
    /// clone per element (array index ascending, object keys in byte order) with `*` replaced.
    /// A branch whose container is absent, `null` or not a list/map yields none; the segments after
    /// the last wildcard are not checked, so an absent optional field still has its site.
    ///
    /// # Example
    /// ```
    /// use ludion_ecs::EntityRefField;
    /// use serde_json::json;
    ///
    /// let field = EntityRefField {
    ///     suffix: "/slots/*/who".to_string(),
    ///     allow_self: false,
    ///     optional: true,
    ///     nullable: true,
    ///     hierarchy: false,
    ///     collection: true,
    /// };
    /// let sites = field.sites(&json!({"slots": [{"who": null}, {}]}));
    /// assert_eq!(sites[1].suffix, "/slots/1/who");
    /// ```
    pub fn sites(&self, value: &Value) -> Vec<EntityRefField> {
        if !self.collection {
            return vec![self.clone()];
        }
        let segments: Vec<&str> = self.suffix.split('/').skip(1).collect();
        let mut pointers = Vec::new();
        expand_sites(value, &segments, String::new(), &mut pointers);
        pointers
            .into_iter()
            .map(|suffix| EntityRefField {
                suffix,
                collection: false,
                ..self.clone()
            })
            .collect()
    }

    /// Whether the concrete `pointer` is a site of this field: segment by segment, a `*` matches
    /// exactly one segment.
    pub fn matches(&self, pointer: &str) -> bool {
        let mut pattern = self.suffix.split('/');
        let mut concrete = pointer.split('/');
        loop {
            match (pattern.next(), concrete.next()) {
                (None, None) => return true,
                (Some(want), Some(have)) if want == "*" || want == have => {}
                _ => return false,
            }
        }
    }

    /// The entity this field names in `value` (the component's own JSON value), or `None` when the
    /// field is absent, `null` or not an `{index, generation}` id.
    ///
    /// # Example
    /// ```
    /// use ludion_core::ids::EntityId;
    /// use ludion_ecs::EntityRefField;
    /// use serde_json::json;
    ///
    /// let field = EntityRefField {
    ///     suffix: "/holder".to_string(),
    ///     allow_self: false,
    ///     optional: false,
    ///     nullable: false,
    ///     hierarchy: false,
    ///     collection: false,
    /// };
    /// let value = json!({"holder": {"index": 3, "generation": 0}});
    /// assert_eq!(field.target(&value), Some(EntityId::new(3, 0)));
    /// assert_eq!(field.target(&json!({"holder": null})), None);
    /// ```
    pub fn target(&self, value: &Value) -> Option<EntityId> {
        let at = if self.suffix.is_empty() {
            value
        } else {
            value.pointer(&self.suffix)?
        };
        serde_json::from_value(at.clone()).ok()
    }
}

/// Appends to `out` the concrete pointers `segments` (escaped, possibly holding `*`) denotes in
/// `value`, below `path`.
fn expand_sites(value: &Value, segments: &[&str], path: String, out: &mut Vec<String>) {
    let Some(star) = segments.iter().position(|s| *s == "*") else {
        out.push(segments.iter().fold(path, |at, s| format!("{at}/{s}")));
        return;
    };
    let mut here = value;
    let mut at = path;
    for segment in &segments[..star] {
        let key = segment.replace("~1", "/").replace("~0", "~");
        let next = match here {
            Value::Object(map) => map.get(&key),
            Value::Array(items) => key.parse::<usize>().ok().and_then(|i| items.get(i)),
            _ => None,
        };
        let Some(next) = next else { return };
        here = next;
        at = format!("{at}/{segment}");
    }
    let rest = &segments[star + 1..];
    match here {
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                expand_sites(item, rest, format!("{at}/{index}"), out);
            }
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_unstable();
            for key in keys {
                expand_sites(
                    &map[key],
                    rest,
                    format!("{at}/{}", pointer_segment(key)),
                    out,
                );
            }
        }
        _ => {}
    }
}

/// Finds every marked entity-reference field in a component's JSON Schema, sorted by suffix.
///
/// Walks `properties`, `$ref` (resolved against `schema`) and `anyOf`/`oneOf`/`allOf`. A field is
/// optional when it is absent from its object's `required`, when a `null` branch or `"null"` type
/// admits `null`, when an enclosing object is optional (optionality is inherited), or when it sits
/// in one variant of several (`anyOf`/`oneOf` with more than one non-null branch): it is then
/// present only with its variant. A suffix found more than once (the same field in several
/// variants) is merged into one entry, refusing self-reference unless every occurrence allows it.
/// Stops at a marked node. `items`, `additionalProperties` and `prefixItems` are walked only to
/// find a marked field inside a list, map or tuple, which [`entity_refs_in_collections`] reports
/// and this function leaves out (the registry refuses such a component).
///
/// `nullable` is set only when `null` is valid at the field's own position; it resets on every
/// step into `properties`, so a required field of an `Option<struct>` is optional but not nullable.
///
/// # Example
/// ```
/// use ludion_ecs::{EntityRefSchema, entity_ref_fields};
/// use schemars::{JsonSchema, schema_for};
///
/// #[derive(JsonSchema)]
/// struct Leash {
///     #[schemars(with = "Option<EntityRefSchema>")]
///     holder: Option<u32>,
/// }
///
/// let schema = serde_json::to_value(schema_for!(Leash)).unwrap();
/// let fields = entity_ref_fields(&schema);
/// assert_eq!(fields[0].suffix, "/holder");
/// assert!(fields[0].optional && fields[0].nullable);
/// ```
pub fn entity_ref_fields(schema: &Value) -> Vec<EntityRefField> {
    let mut found = Found::default();
    walk(schema, schema, At::ROOT, 0, &mut found);
    let mut fields = found.fields;
    fields.sort_by(|a, b| a.suffix.cmp(&b.suffix));
    fields.dedup_by(|later, kept| {
        let same = later.suffix == kept.suffix;
        if same {
            kept.allow_self &= later.allow_self;
            kept.optional |= later.optional;
            kept.nullable &= later.nullable;
            kept.hierarchy |= later.hierarchy;
            kept.collection |= later.collection;
        }
        same
    });
    fields
}

/// The sorted suffixes of marked entity references that sit inside a list, a map or a tuple
/// (`/*` for a list's elements or a map's values, `/<i>` for a tuple's). The registry refuses a
/// component with any (`L1392`).
///
/// # Example
/// ```
/// use std::collections::BTreeMap;
/// use ludion_ecs::{EntityRefSchema, entity_refs_in_collections};
/// use schemars::{JsonSchema, schema_for};
///
/// #[derive(JsonSchema)]
/// struct Pack {
///     #[schemars(with = "BTreeMap<String, EntityRefSchema>")]
///     members: u32,
/// }
///
/// let schema = serde_json::to_value(schema_for!(Pack)).unwrap();
/// assert_eq!(entity_refs_in_collections(&schema), ["/members/*"]);
/// ```
pub fn entity_refs_in_collections(schema: &Value) -> Vec<String> {
    let mut found = Found::default();
    walk(schema, schema, At::ROOT, 0, &mut found);
    found.in_collections.sort();
    found.in_collections.dedup();
    found.in_collections
}

/// The sorted suffixes at which `schema` holds something shaped like an entity reference — an
/// object whose properties are exactly `index` and `generation` — marked or not. A list or map of
/// them is recorded with a `/*` step and a tuple with `/<i>`, so a proxy hand-written under a
/// collection shows up. `assert_entity_refs_marked` in `ludion-testkit` compares this against
/// [`ComponentInfo::entity_refs`](crate::ComponentInfo::entity_refs).
///
/// # Example
/// ```
/// use ludion_ecs::entity_ref_shapes;
/// use serde_json::json;
///
/// let schema = json!({
///     "type": "object",
///     "properties": {
///         "holder": {"type": "object", "properties": {
///             "index": {"type": "integer"}, "generation": {"type": "integer"}}},
///         "hp": {"type": "integer"}
///     }
/// });
/// assert_eq!(entity_ref_shapes(&schema), ["/holder"]);
/// ```
pub fn entity_ref_shapes(schema: &Value) -> Vec<String> {
    let mut found = std::collections::BTreeSet::new();
    shapes(schema, schema, "", 0, &mut found);
    found.into_iter().collect()
}

fn shapes(
    root: &Value,
    node: &Value,
    suffix: &str,
    depth: usize,
    found: &mut std::collections::BTreeSet<String>,
) {
    if depth > 12 {
        return;
    }
    if let Some(resolved) = resolve(root, node) {
        shapes(root, resolved, suffix, depth + 1, found);
    }
    if let Some(properties) = node.get("properties").and_then(Value::as_object) {
        let mut keys: Vec<&str> = properties.keys().map(String::as_str).collect();
        keys.sort_unstable();
        if keys == ["generation", "index"] {
            found.insert(suffix.to_string());
        } else {
            for (key, property) in properties {
                let at = format!("{suffix}/{}", pointer_segment(key));
                shapes(root, property, &at, depth + 1, found);
            }
        }
    }
    for combinator in ["anyOf", "oneOf", "allOf"] {
        for branch in node
            .get(combinator)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            shapes(root, branch, suffix, depth + 1, found);
        }
    }
    for each in ["items", "additionalProperties"] {
        if let Some(inner) = node.get(each).filter(|inner| inner.is_object()) {
            shapes(root, inner, &format!("{suffix}/*"), depth + 1, found);
        }
    }
    for (index, entry) in prefix_items(node).iter().enumerate() {
        shapes(root, entry, &format!("{suffix}/{index}"), depth + 1, found);
    }
}

fn resolve<'a>(root: &'a Value, node: &Value) -> Option<&'a Value> {
    let target = node.get("$ref").and_then(Value::as_str)?;
    root.pointer(target.trim_start_matches('#'))
}

fn prefix_items(node: &Value) -> &[Value] {
    node.get("prefixItems")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
}

#[derive(Default)]
struct Found {
    fields: Vec<EntityRefField>,
    in_collections: Vec<String>,
}

/// The walk's position: where it stands and what it has passed on the way down.
#[derive(Clone, Copy)]
struct At<'a> {
    suffix: &'a str,
    /// The field may be absent.
    optional: bool,
    /// `null` is valid at the field's own position.
    nullable: bool,
    /// Below a list, map or tuple.
    in_collection: bool,
}

impl At<'static> {
    const ROOT: Self = Self {
        suffix: "",
        optional: false,
        nullable: false,
        in_collection: false,
    };
}

fn walk(root: &Value, node: &Value, at: At<'_>, depth: usize, found: &mut Found) {
    if depth > 12 {
        return;
    }
    let optional = at.optional || is_nullable(node);
    let nullable = at.nullable || is_nullable(node);
    if let Some(marker) = node.get(ENTITY_REF_KEYWORD) {
        if at.in_collection {
            found.in_collections.push(at.suffix.to_string());
        } else {
            found.fields.push(EntityRefField {
                suffix: at.suffix.to_string(),
                allow_self: marker
                    .get("allow_self")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                optional,
                nullable,
                hierarchy: marker
                    .get("hierarchy")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                collection: at.suffix.split('/').any(|segment| segment == "*"),
            });
        }
        return;
    }
    let here = At {
        optional,
        nullable,
        ..at
    };
    if let Some(resolved) = resolve(root, node) {
        walk(root, resolved, here, depth + 1, found);
    }
    if let Some(properties) = node.get("properties").and_then(Value::as_object) {
        let required = node.get("required").and_then(Value::as_array);
        for (key, property) in properties {
            let is_required = required.is_some_and(|names| names.iter().any(|n| n == key));
            let child = format!("{}/{}", at.suffix, pointer_segment(key));
            let step = At {
                suffix: &child,
                optional: optional || !is_required,
                nullable: false,
                in_collection: at.in_collection,
            };
            walk(root, property, step, depth + 1, found);
        }
    }
    for combinator in ["anyOf", "oneOf", "allOf"] {
        let branches = node
            .get(combinator)
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        let non_null = branches.iter().filter(|b| !is_null_branch(b)).count();
        let variants = combinator != "allOf" && non_null > 1;
        let step = At {
            optional: optional || variants,
            ..here
        };
        for branch in branches {
            walk(root, branch, step, depth + 1, found);
        }
    }
    let sanctioned = node.get(ENTITY_REF_COLLECTION_KEYWORD) == Some(&Value::Bool(true));
    for each in ["items", "additionalProperties"] {
        if let Some(inner) = node.get(each).filter(|inner| inner.is_object()) {
            let child = format!("{}/*", at.suffix);
            let step = At {
                suffix: &child,
                optional: if sanctioned { optional } else { true },
                nullable: false,
                in_collection: at.in_collection || !sanctioned,
            };
            walk(root, inner, step, depth + 1, found);
        }
    }
    for (index, entry) in prefix_items(node).iter().enumerate() {
        let child = format!("{}/{index}", at.suffix);
        let step = At {
            suffix: &child,
            optional: true,
            nullable: false,
            in_collection: true,
        };
        walk(root, entry, step, depth + 1, found);
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
            nullable: false,
            hierarchy: false,
            collection: false,
        }
    }

    fn nullable(suffix: &str, allow_self: bool) -> EntityRefField {
        EntityRefField {
            nullable: true,
            ..field(suffix, allow_self, true)
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
            [nullable("/any", false), nullable("/ty", true)]
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
    fn a_required_ref_inside_an_option_struct_is_optional_but_not_nullable() {
        let schema = json!({
            "type": "object",
            "properties": {
                "inner": {"anyOf": [{"$ref": "#/$defs/Inner"}, {"type": "null"}]},
                "both": {"type": "object", "properties": {
                    "holder": {"anyOf": [marker(false), {"type": "null"}]}
                }, "required": ["holder"]}
            },
            "required": ["inner", "both"],
            "$defs": {
                "Inner": {"type": "object", "properties": {"holder": marker(false)}, "required": ["holder"]}
            }
        });
        assert_eq!(
            entity_ref_fields(&schema),
            [
                nullable("/both/holder", false),
                field("/inner/holder", false, true)
            ]
        );
    }

    #[test]
    fn variant_fields_are_optional_but_not_nullable() {
        let schema = json!({
            "oneOf": [
                {"type": "object", "properties": {"a": marker(false)}, "required": ["a"]},
                {"type": "object", "properties": {"b": marker(false)}, "required": ["b"]}
            ]
        });
        assert_eq!(
            entity_ref_fields(&schema),
            [field("/a", false, true), field("/b", false, true)]
        );
    }

    #[test]
    fn marked_refs_in_collections_are_reported_not_found() {
        let schema = json!({
            "type": "object",
            "properties": {
                "list": {"type": "array", "items": marker(false)},
                "map": {"type": "object", "additionalProperties": marker(false)},
                "pair": {"type": "array", "prefixItems": [marker(false), {"type": "integer"}]},
                "one": marker(false)
            },
            "required": ["list", "map", "pair", "one"]
        });
        assert_eq!(entity_ref_fields(&schema), [field("/one", false, false)]);
        assert_eq!(
            entity_refs_in_collections(&schema),
            ["/list/*", "/map/*", "/pair/0"]
        );
    }

    fn sanctioned(mut schema: Value) -> Value {
        schema[ENTITY_REF_COLLECTION_KEYWORD] = json!(true);
        schema
    }

    fn pattern(suffix: &str, nullable: bool) -> EntityRefField {
        EntityRefField {
            nullable,
            collection: true,
            ..field(suffix, false, true)
        }
    }

    #[test]
    fn a_sanctioned_list_and_map_are_pattern_fields_not_collection_refs() {
        let schema = json!({
            "type": "object",
            "properties": {
                "list": sanctioned(json!({"type": "array", "items": {
                    "type": "object",
                    "properties": {"source": marker(false)},
                    "required": ["source"]
                }})),
                "map": sanctioned(json!({"type": "object", "additionalProperties": marker(false)}))
            },
            "required": ["list", "map"]
        });
        let required = |suffix| EntityRefField {
            optional: false,
            ..pattern(suffix, false)
        };
        assert_eq!(
            entity_ref_fields(&schema),
            [required("/list/*/source"), required("/map/*")]
        );
        assert!(entity_refs_in_collections(&schema).is_empty());
    }

    #[test]
    fn a_plain_collection_inside_a_sanctioned_element_is_still_reported() {
        let schema = json!({
            "type": "object",
            "properties": {"list": sanctioned(json!({"type": "array", "items": {
                "type": "object",
                "properties": {"inner": {"type": "array", "items": marker(false)}},
                "required": ["inner"]
            }}))},
            "required": ["list"]
        });
        assert!(entity_ref_fields(&schema).is_empty());
        assert_eq!(entity_refs_in_collections(&schema), ["/list/*/inner/*"]);
    }

    #[test]
    fn a_sanctioned_tuple_is_still_reported() {
        let schema = json!({
            "type": "object",
            "properties": {"pair": sanctioned(json!({
                "type": "array", "prefixItems": [marker(false), {"type": "integer"}]
            }))},
            "required": ["pair"]
        });
        assert!(entity_ref_fields(&schema).is_empty());
        assert_eq!(entity_refs_in_collections(&schema), ["/pair/0"]);
    }

    #[test]
    fn sites_expand_arrays_and_maps_in_order() {
        let list = pattern("/slots/*/who", true);
        let value = json!({"slots": [{"who": null}, {}, {"who": {"index": 1, "generation": 0}}]});
        let suffixes: Vec<String> = list.sites(&value).into_iter().map(|s| s.suffix).collect();
        assert_eq!(suffixes, ["/slots/0/who", "/slots/1/who", "/slots/2/who"]);
        assert!(
            list.sites(&value)
                .iter()
                .all(|s| !s.collection && s.nullable)
        );

        let map = pattern("/by/*", false);
        let value = json!({"by": {"b": null, "a/x": null, "a": null}});
        let suffixes: Vec<String> = map.sites(&value).into_iter().map(|s| s.suffix).collect();
        assert_eq!(suffixes, ["/by/a", "/by/a~1x", "/by/b"]);
    }

    #[test]
    fn sites_of_an_empty_missing_or_null_container_are_empty() {
        let list = pattern("/slots/*/who", true);
        assert!(list.sites(&json!({"slots": []})).is_empty());
        assert!(list.sites(&json!({})).is_empty());
        assert!(list.sites(&json!({"slots": null})).is_empty());
        assert!(list.sites(&json!({"slots": 3})).is_empty());
    }

    #[test]
    fn a_field_without_a_wildcard_is_its_own_site() {
        let plain = field("/holder", false, true);
        assert_eq!(plain.sites(&json!({})), std::slice::from_ref(&plain));
    }

    #[test]
    fn matches_is_segment_wise() {
        let list = pattern("/slots/*/who", true);
        assert!(list.matches("/slots/3/who"));
        assert!(list.matches("/slots/*/who"));
        assert!(!list.matches("/slots/3"));
        assert!(!list.matches("/slots/3/who/x"));
        assert!(!list.matches("/slots/3/whom"));
        assert!(!list.matches("/other/3/who"));
        assert!(field("/holder", false, false).matches("/holder"));
    }

    #[test]
    fn shapes_find_unmarked_proxies_everywhere() {
        let bare = json!({
            "type": "object",
            "properties": {"index": {"type": "integer"}, "generation": {"type": "integer"}}
        });
        let schema = json!({
            "type": "object",
            "properties": {
                "a/b": bare,
                "list": {"type": "array", "items": bare},
                "map": {"type": "object", "additionalProperties": bare},
                "pair": {"type": "array", "prefixItems": [{"type": "integer"}, bare]},
                "opt": {"anyOf": [bare, {"type": "null"}]}
            }
        });
        assert_eq!(
            entity_ref_shapes(&schema),
            ["/a~1b", "/list/*", "/map/*", "/opt", "/pair/1"]
        );
    }

    #[test]
    fn a_key_with_a_slash_is_one_pointer_segment() {
        let schema = json!({
            "type": "object",
            "properties": {"a/b": marker(false)},
            "required": ["a/b"]
        });
        let fields = entity_ref_fields(&schema);
        assert_eq!(fields, [field("/a~1b", false, false)]);
        let value = json!({"a/b": {"index": 2, "generation": 0}});
        assert_eq!(fields[0].target(&value), Some(EntityId::new(2, 0)));
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
    fn target_reads_the_suffix_and_ignores_absent_null_or_malformed() {
        let id = json!({"index": 4, "generation": 1});
        let root = field("", false, false);
        let nested = field("/holder", false, true);
        assert_eq!(root.target(&id), Some(EntityId::new(4, 1)));
        assert_eq!(
            nested.target(&json!({"holder": id})),
            Some(EntityId::new(4, 1))
        );
        assert_eq!(nested.target(&json!({})), None);
        assert_eq!(nested.target(&json!({"holder": null})), None);
        assert_eq!(
            nested.target(&json!({"holder": {"index": 4_294_967_296u64, "generation": 0}})),
            None
        );
    }

    #[test]
    fn an_unmarked_schema_has_no_fields() {
        let schema = json!({"type": "object", "properties": {"hp": {"type": "integer"}}});
        assert!(entity_ref_fields(&schema).is_empty());
    }
}

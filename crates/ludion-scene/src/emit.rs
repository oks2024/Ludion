//! The canonical scene writer.
//!
//! One scene has exactly one canonical text (per registry), so any client's edit produces a
//! minimal git diff. The canonical form (`engine-architecture.md` §7):
//!
//! - **LF line endings**, 2-space indent, block style everywhere; the only flow forms are the
//!   empty `[]` and `{}`.
//! - **Stable key order.** Top-level: `format`, `name`, `generated_from`, `seed`, `physics`,
//!   `nav` (omitted when absent; keys in `NavConfig` field order), `render_layers` (omitted when empty), `preload` (omitted when empty), `entities` (a present `physics:` block itself orders `space`, `gravity`, `solver_iterations`,
//!   `stabilization_iterations`, `sleep_after_ticks`, `layers`, skipping absent fields). Entity:
//!   `name`, `presentation` (only when true), `components`. Components: registry registration order (unregistered names follow, in
//!   document order). Component fields: schema property order — i.e. struct declaration order —
//!   with undeclared fields after, in document order (an internally tagged enum's tag — a
//!   const-valued property — leads, as serde writes it). An enum variant, an `Option` and an untagged
//!   value order by the alternative they match; map (`additionalProperties`) keys sort in byte
//!   order — the order a `BTreeMap<String, _>` serializes in, so a file and `scene.save` agree.
//! - **Entity order is data**: the `entities` sequence is emitted exactly as given.
//! - **Floats are ryu shortest-round-trip** ([`crate::scalar::format_float`]) — never fixed
//!   precision, which would be a data-loss bug. At a schema position marked `"format": "float"`
//!   (an `f32` field) an f32-exact value is written as the shortest *f32* decimal
//!   ([`crate::scalar::format_f32_float`]): `0.3`, not the widened `0.30000001192092896`. Integers
//!   stay integers (`1` and `1.0` are different scalars).
//! - **Ambiguous strings are always double-quoted** ([`crate::scalar::plain_safe`]), which kills
//!   the Norway problem in emitted files by construction.
//!
//! Emission is idempotent: `emit(parse(emit(s)))` is byte-equal to `emit(s)` (property-tested).

use ludion_ecs::Registry;
use serde_json::Value;

use crate::physics::ScenePhysics;
use crate::scalar::{format_f32_float, format_float, scalar_string};
use crate::schema::{KeyOrder, SchemaCursor};
use crate::{Scene, SceneEntity};

/// Writes `scene` in canonical form.
pub(crate) fn emit(scene: &Scene, registry: &Registry) -> String {
    let mut out = String::new();
    out.push_str("format: 1\n");
    if let Some(name) = &scene.name {
        out.push_str("name: ");
        out.push_str(&scalar_string(name));
        out.push('\n');
    }
    if let Some(generated_from) = &scene.generated_from {
        out.push_str("generated_from: ");
        out.push_str(&scalar_string(generated_from));
        out.push('\n');
    }
    if let Some(seed) = scene.seed {
        out.push_str("seed: ");
        out.push_str(&seed.to_string());
        out.push('\n');
    }
    if let Some(physics) = &scene.physics {
        emit_physics(&mut out, physics);
    }
    if let Some(nav) = &scene.nav {
        out.push_str("nav:");
        let schema = crate::nav_block::nav_schema();
        write_after_key(&mut out, nav, Some(SchemaCursor::new(schema)), 0);
    }
    if !scene.render_layers.is_empty() {
        out.push_str("render_layers:\n");
        for layer in &scene.render_layers {
            out.push_str("  - name: ");
            out.push_str(&scalar_string(&layer.name));
            out.push('\n');
        }
    }
    if !scene.preload.is_empty() {
        out.push_str("preload:\n");
        for path in &scene.preload {
            out.push_str("  - ");
            out.push_str(&scalar_string(path));
            out.push('\n');
        }
    }
    if scene.entities.is_empty() {
        out.push_str("entities: []\n");
    } else {
        out.push_str("entities:\n");
        for entity in &scene.entities {
            emit_entity(&mut out, entity, registry);
        }
    }
    out
}

/// Writes `physics` at indent 2, field order `space`, `gravity`, `solver_iterations`,
/// `stabilization_iterations`, `sleep_after_ticks`, `layers` (declaration order, `ignores` omitted
/// when empty), `surfaces`, `default_surface`, skipping absent fields — a hand-authored block round-trips minimally.
fn emit_physics(out: &mut String, physics: &ScenePhysics) {
    out.push_str("physics:\n");
    out.push_str("  space: ");
    out.push_str(physics.space.as_str());
    out.push('\n');
    if let Some(gravity) = physics.gravity {
        out.push_str("  gravity:\n");
        for component in gravity {
            out.push_str("    - ");
            out.push_str(&format_float(component));
            out.push('\n');
        }
    }
    if let Some(solver_iterations) = physics.solver_iterations {
        out.push_str("  solver_iterations: ");
        out.push_str(&solver_iterations.to_string());
        out.push('\n');
    }
    if let Some(stabilization_iterations) = physics.stabilization_iterations {
        out.push_str("  stabilization_iterations: ");
        out.push_str(&stabilization_iterations.to_string());
        out.push('\n');
    }
    if let Some(sleep_after_ticks) = physics.sleep_after_ticks {
        out.push_str("  sleep_after_ticks: ");
        out.push_str(&sleep_after_ticks.to_string());
        out.push('\n');
    }
    if !physics.layers.is_empty() {
        out.push_str("  layers:\n");
        for layer in &physics.layers {
            out.push_str("    - name: ");
            out.push_str(&scalar_string(&layer.name));
            out.push('\n');
            if !layer.ignores.is_empty() {
                out.push_str("      ignores:\n");
                for ignored in &layer.ignores {
                    out.push_str("        - ");
                    out.push_str(&scalar_string(ignored));
                    out.push('\n');
                }
            }
        }
    }
    if !physics.surfaces.is_empty() {
        out.push_str("  surfaces:\n");
        for surface in &physics.surfaces {
            out.push_str("    - ");
            out.push_str(&scalar_string(surface));
            out.push('\n');
        }
    }
    if let Some(default_surface) = &physics.default_surface {
        out.push_str("  default_surface: ");
        out.push_str(&scalar_string(default_surface));
        out.push('\n');
    }
}

pub(crate) fn emit_entity(out: &mut String, entity: &SceneEntity, registry: &Registry) {
    // The entity item sits at indent level 1 (`  - `); its fields continue at level 2.
    let mut lead = "  - ";
    if let Some(name) = &entity.name {
        out.push_str(lead);
        lead = "    ";
        out.push_str("name: ");
        out.push_str(&scalar_string(name));
        out.push('\n');
    }
    if entity.presentation {
        out.push_str(lead);
        lead = "    ";
        out.push_str("presentation: true\n");
    }
    if let Some(instance) = &entity.instance {
        crate::instance::emit_fields(out, lead, instance, registry);
        if entity.components.is_empty() {
            return;
        }
        // Components on an entry are written so the parser refuses the text (`L1231`) rather
        // than the data vanishing.
        lead = "    ";
    }
    out.push_str(lead);
    if entity.components.is_empty() {
        out.push_str("components: {}\n");
        return;
    }
    out.push_str("components:\n");
    for (name, value) in ordered_components(&entity.components, registry) {
        write_indent(out, 3);
        out.push_str(&scalar_string(name));
        out.push(':');
        let schema = registry
            .get(name)
            .map(|info| SchemaCursor::new(info.schema()));
        write_after_key(out, value, schema, 3);
    }
}

/// Components in canonical order: registry registration order for registered names, then
/// unregistered names in document order.
pub(crate) fn ordered_components<'a>(
    components: &'a [(String, Value)],
    registry: &Registry,
) -> Vec<(&'a String, &'a Value)> {
    let registry_index = |name: &str| registry.iter().position(|info| info.name() == name);
    let mut ordered: Vec<(usize, usize, &'a String, &'a Value)> = components
        .iter()
        .enumerate()
        .map(|(document_index, (name, value))| {
            (
                registry_index(name).unwrap_or(usize::MAX),
                document_index,
                name,
                value,
            )
        })
        .collect();
    ordered.sort_by_key(|&(registry_index, document_index, _, _)| (registry_index, document_index));
    ordered
        .into_iter()
        .map(|(_, _, name, value)| (name, value))
        .collect()
}

/// Writes a game data-doc in canonical form: the `format`/`kind` envelope, then `value`'s fields
/// (an object) ordered by `schema`'s declaration order — the same generic value emitter the scene
/// writer uses, rooted at the top level. Shared with [`crate::doc`]; behavior-neutral for the scene
/// path, since it *reuses* [`write_map_block`] rather than changing it. A non-object `value` emits
/// the envelope alone (the doc machinery only ever passes a struct's object form).
pub(crate) fn emit_doc(kind: &str, value: &Value, schema: &Value) -> String {
    debug_assert!(
        value
            .as_object()
            .is_none_or(|map| !map.contains_key("format") && !map.contains_key("kind")),
        "a `{kind}` doc value has a top-level `format`/`kind` key that would duplicate the \
         envelope; registered types are refused earlier by `check_doc_type`"
    );
    let mut out = String::new();
    out.push_str("format: 1\n");
    out.push_str("kind: ");
    out.push_str(&scalar_string(kind));
    out.push('\n');
    if let Some(map) = value.as_object() {
        let cursor = SchemaCursor::new(schema).select(value);
        write_map_block(&mut out, map, Some(cursor), 0);
    }
    out
}

/// Writes the value of a `key:` whose colon is already written. Scalars and empty collections go
/// on the same line; non-empty collections open a block one level deeper than the key's
/// `indent`.
pub(crate) fn write_after_key(
    out: &mut String,
    value: &Value,
    schema: Option<SchemaCursor<'_>>,
    indent: usize,
) {
    let schema = schema.map(|cursor| cursor.select(value));
    match value {
        Value::Object(map) if !map.is_empty() => {
            out.push('\n');
            write_map_block(out, map, schema, indent + 1);
        }
        Value::Array(items) if !items.is_empty() => {
            out.push('\n');
            write_seq_block(out, items, schema, indent + 1);
        }
        leaf => {
            out.push(' ');
            out.push_str(&leaf_text(leaf, schema));
            out.push('\n');
        }
    }
}

fn write_map_block(
    out: &mut String,
    map: &serde_json::Map<String, Value>,
    schema: Option<SchemaCursor<'_>>,
    indent: usize,
) {
    for key in ordered_keys(map, schema) {
        write_indent(out, indent);
        out.push_str(&scalar_string(key));
        out.push(':');
        let child = schema.and_then(|cursor| cursor.child(key));
        write_after_key(out, &map[key], child, indent);
    }
}

fn write_seq_block(
    out: &mut String,
    items: &[Value],
    schema: Option<SchemaCursor<'_>>,
    indent: usize,
) {
    for (index, item) in items.iter().enumerate() {
        write_indent(out, indent);
        out.push_str("- ");
        let item_schema = schema.and_then(|cursor| cursor.item(index));
        write_inline(out, item, item_schema, indent + 1);
    }
}

/// Writes a value whose first character lands right after `- ` (or another inline lead-in).
/// Continuation lines are indented at `indent`, which lines up under the first character because
/// `- ` is exactly one indent unit wide.
fn write_inline(out: &mut String, value: &Value, schema: Option<SchemaCursor<'_>>, indent: usize) {
    let schema = schema.map(|cursor| cursor.select(value));
    match value {
        Value::Object(map) if !map.is_empty() => {
            for (position, key) in ordered_keys(map, schema).into_iter().enumerate() {
                if position > 0 {
                    write_indent(out, indent);
                }
                out.push_str(&scalar_string(key));
                out.push(':');
                let child = schema.and_then(|cursor| cursor.child(key));
                write_after_key(out, &map[key], child, indent);
            }
        }
        Value::Array(items) if !items.is_empty() => {
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    write_indent(out, indent);
                }
                out.push_str("- ");
                let item_schema = schema.and_then(|cursor| cursor.item(index));
                write_inline(out, item, item_schema, indent + 1);
            }
        }
        leaf => {
            out.push_str(&leaf_text(leaf, schema));
            out.push('\n');
        }
    }
}

/// Map keys in canonical order: declared fields in schema (struct declaration) order, then
/// undeclared ones — byte order when the schema makes them map entries (`additionalProperties`),
/// document order otherwise; a map with no declared fields is all byte order.
fn ordered_keys<'a>(
    map: &'a serde_json::Map<String, Value>,
    schema: Option<SchemaCursor<'_>>,
) -> Vec<&'a str> {
    let order = schema.map_or(KeyOrder::Document, SchemaCursor::key_order);
    let mut keys: Vec<&'a str> = Vec::with_capacity(map.len());
    match order {
        KeyOrder::Declared(declared) => {
            for name in &declared {
                // Look the declared name up in the document map (borrow from the map, not the schema).
                if let Some((key, _)) = map.get_key_value(*name) {
                    keys.push(key.as_str());
                }
            }
            let declared_count = keys.len();
            keys.extend(
                map.keys()
                    .map(String::as_str)
                    .filter(|key| !declared.contains(key)),
            );
            if schema.is_some_and(SchemaCursor::open_map) {
                keys[declared_count..].sort_unstable();
            }
        }
        KeyOrder::Sorted => {
            keys.extend(map.keys().map(String::as_str));
            keys.sort_unstable();
        }
        KeyOrder::Document => keys.extend(map.keys().map(String::as_str)),
    }
    keys
}

/// The canonical text of a scalar leaf (`null`, booleans, numbers, strings, and the two empty
/// collections).
fn leaf_text(value: &Value, schema: Option<SchemaCursor<'_>>) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(true) => "true".to_string(),
        Value::Bool(false) => "false".to_string(),
        Value::Number(number) => match number.as_f64() {
            Some(float) if number.is_f64() => {
                let is_f32 = schema.is_some_and(|cursor| {
                    cursor.resolved().get("format").and_then(Value::as_str) == Some("float")
                });
                if is_f32 {
                    format_f32_float(float)
                } else {
                    format_float(float)
                }
            }
            _ => number.to_string(),
        },
        Value::String(text) => scalar_string(text),
        Value::Array(items) => {
            debug_assert!(items.is_empty(), "non-empty arrays are handled by callers");
            "[]".to_string()
        }
        Value::Object(map) => {
            debug_assert!(map.is_empty(), "non-empty objects are handled by callers");
            "{}".to_string()
        }
    }
}

pub(crate) fn write_indent(out: &mut String, indent: usize) {
    for _ in 0..indent {
        out.push_str("  ");
    }
}

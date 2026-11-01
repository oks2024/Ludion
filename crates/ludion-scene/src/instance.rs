//! Prefab-instance entries: an entity entry with a `prefab:` key.
//!
//! An instance names a `kind: prefab` file, places it with an optional `transform`, and edits it
//! with `overrides` (part path to partial component values) and `removed` (part paths). This module
//! owns the entry's shape — parse, canonical emit, path normalization — and never names a concrete
//! component type or reads a file; expansion into plain entities lives with the callers that hold
//! the prefab text.

use std::collections::BTreeMap;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::math::{Trs, euler_xyz_degrees_to_quat};
use ludion_ecs::Registry;
use serde_json::Value;

use crate::SceneEntity;
use crate::emit::{ordered_components, write_after_key, write_indent};
use crate::envelope;
use crate::parse::node_to_value;
use crate::scalar::{format_f32_float, scalar_string};
use crate::schema::SchemaCursor;
use crate::yaml::{MapEntry, Node, NodeValue, pointer_token};

const INSTANCE_KEYS: [&str; 6] = [
    "name",
    "prefab",
    "presentation",
    "transform",
    "overrides",
    "removed",
];
const TRANSFORM_KEYS: [&str; 3] = ["translation", "rotation", "scale"];

/// A prefab instance: one entry of a scene's or prefab's `entities:` list with a `prefab:` key.
///
/// Floats are kept as parsed `f64` so a document round-trips losslessly; they narrow to `f32`
/// only in [`InstanceTransform::to_trs`].
#[derive(Clone, Debug, PartialEq, Default)]
pub struct InstanceEntry {
    /// The prefab path as written, relative to the content root (see [`normalize_prefab_path`]).
    pub prefab: String,
    /// Every part of the instance, nested instances included, spawns presentation.
    pub presentation: bool,
    /// Where the instance's root sits.
    pub transform: InstanceTransform,
    /// Part path to `(component name, partial value)` pairs, merged over the prefab's part.
    pub overrides: BTreeMap<String, Vec<(String, Value)>>,
    /// Part paths dropped from the instance, unique, in authored order (the writer sorts them).
    pub removed: Vec<String>,
}

/// An instance's placement: translation, Euler XYZ rotation in degrees, and scale.
#[derive(Clone, Debug, PartialEq)]
pub struct InstanceTransform {
    /// Translation, `[x, y, z]`.
    pub translation: [f64; 3],
    /// Euler XYZ rotation in degrees, like a `Transform` component.
    pub rotation: [f64; 3],
    /// Per-axis scale.
    pub scale: [f64; 3],
}

impl Default for InstanceTransform {
    fn default() -> Self {
        Self {
            translation: [0.0; 3],
            rotation: [0.0; 3],
            scale: [1.0; 3],
        }
    }
}

impl InstanceTransform {
    /// The placement as a [`Trs`] (rotation converted through the pinned Euler convention).
    pub fn to_trs(&self) -> Trs {
        let narrow = |v: [f64; 3]| [v[0] as f32, v[1] as f32, v[2] as f32];
        Trs {
            translation: narrow(self.translation),
            rotation: euler_xyz_degrees_to_quat(narrow(self.rotation)),
            scale: narrow(self.scale),
        }
    }
}

/// Normalizes a prefab path the way the parse-time rebase does: forward slashes, empty and `.`
/// segments dropped, `..` collapsed against the segment before it, so one file has one key.
///
/// # Example
/// ```
/// assert_eq!(
///     ludion_scene::normalize_prefab_path(".\\props//lamp.prefab.yaml"),
///     "props/lamp.prefab.yaml"
/// );
/// assert_eq!(
///     ludion_scene::normalize_prefab_path("props/../lamp.prefab.yaml"),
///     "lamp.prefab.yaml"
/// );
/// ```
pub fn normalize_prefab_path(path: &str) -> String {
    match ludion_core::paths::normalize(path.trim_start_matches(['/', '\\'])).as_str() {
        "." => String::new(),
        normalized => normalized.to_owned(),
    }
}

fn bad(
    diagnostics: &mut Vec<Diagnostic>,
    pointer: String,
    message: String,
    help: impl Into<String>,
) {
    diagnostics.push(
        Diagnostic::new(code::PREFAB_INSTANCE_STRUCTURE, message)
            .with_path(pointer)
            .with_help(help),
    );
}

/// Parses the entity mapping `entries` (known to hold a `prefab` key) as an instance entry.
pub(crate) fn parse_entry(
    entries: &[MapEntry],
    index: usize,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<SceneEntity> {
    let base = format!("/entities/{index}");
    let before = diagnostics.len();
    let mut name: Option<String> = None;
    let mut instance = InstanceEntry::default();
    for entry in entries {
        let pointer = format!("{base}/{}", pointer_token(&entry.key));
        match entry.key.as_str() {
            "name" => match &entry.node.value {
                NodeValue::Scalar(Value::String(text))
                    if !text.is_empty() && !text.contains('/') =>
                {
                    name = Some(text.clone());
                }
                _ => bad(
                    diagnostics,
                    pointer,
                    format!(
                        "an instance name must be a non-empty string without `/` (line {})",
                        entry.node.line
                    ),
                    "its parts become `<instance>/<part>`, so the name cannot contain `/`",
                ),
            },
            "prefab" => match &entry.node.value {
                NodeValue::Scalar(Value::String(text))
                    if !normalize_prefab_path(text).is_empty() =>
                {
                    instance.prefab = text.clone();
                }
                _ => bad(
                    diagnostics,
                    pointer,
                    format!(
                        "`prefab` must be a non-empty path string (line {})",
                        entry.node.line
                    ),
                    "write `prefab: props/lamp.prefab.yaml`, relative to the content root",
                ),
            },
            "presentation" => match &entry.node.value {
                NodeValue::Scalar(Value::Bool(flag)) => instance.presentation = *flag,
                _ => bad(
                    diagnostics,
                    pointer,
                    format!(
                        "`presentation` must be a boolean (line {})",
                        entry.node.line
                    ),
                    "write `presentation: true`",
                ),
            },
            "transform" => parse_transform(&entry.node, &pointer, &mut instance, diagnostics),
            "overrides" => parse_overrides(&entry.node, &pointer, &mut instance, diagnostics),
            "removed" => parse_removed(&entry.node, &pointer, &mut instance, diagnostics),
            "components" => bad(
                diagnostics,
                pointer,
                format!(
                    "an instance does not take `components` (line {})",
                    entry.key_line
                ),
                "an instance takes component changes under `overrides: <part>:`",
            ),
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                entry,
                &base,
                "an instance",
                INSTANCE_KEYS,
                code::PREFAB_INSTANCE_STRUCTURE,
            )),
        }
    }
    if name.is_none() && !entries.iter().any(|entry| entry.key == "name") {
        bad(
            diagnostics,
            base,
            "an instance has no `name`".to_string(),
            "an instance is named: add `name: <instance>`; its parts become `<instance>/<part>`",
        );
    }
    (diagnostics.len() == before).then(|| SceneEntity {
        name,
        instance: Some(Box::new(instance)),
        ..Default::default()
    })
}

fn parse_transform(
    node: &Node,
    pointer: &str,
    instance: &mut InstanceEntry,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let NodeValue::Mapping(entries) = &node.value else {
        bad(
            diagnostics,
            pointer.to_string(),
            format!(
                "`transform` must be a mapping, found {} (line {})",
                node.value.type_phrase(),
                node.line
            ),
            "write `transform:` with any of `translation`, `rotation`, `scale`",
        );
        return;
    };
    for entry in entries {
        let field_pointer = format!("{pointer}/{}", pointer_token(&entry.key));
        let slot = match entry.key.as_str() {
            "translation" => &mut instance.transform.translation,
            "rotation" => &mut instance.transform.rotation,
            "scale" => &mut instance.transform.scale,
            unknown => {
                diagnostics.push(envelope::unknown_key(
                    unknown,
                    entry,
                    pointer,
                    "an instance transform",
                    TRANSFORM_KEYS,
                    code::PREFAB_INSTANCE_STRUCTURE,
                ));
                continue;
            }
        };
        match triple(&entry.node) {
            Some(value) => *slot = value,
            None => bad(
                diagnostics,
                field_pointer,
                format!(
                    "`{}` must be a sequence of three numbers (line {})",
                    entry.key, entry.node.line
                ),
                "write one number per `- ` item: x, y, z",
            ),
        }
    }
}

fn triple(node: &Node) -> Option<[f64; 3]> {
    let NodeValue::Sequence(items) = &node.value else {
        return None;
    };
    let numbers: Vec<f64> = items
        .iter()
        .map(|item| match &item.value {
            NodeValue::Scalar(Value::Number(n)) => n.as_f64(),
            _ => None,
        })
        .collect::<Option<_>>()?;
    numbers.try_into().ok()
}

fn parse_overrides(
    node: &Node,
    pointer: &str,
    instance: &mut InstanceEntry,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let NodeValue::Mapping(parts) = &node.value else {
        bad(
            diagnostics,
            pointer.to_string(),
            format!(
                "`overrides` must be a mapping of part path to components, found {} (line {})",
                node.value.type_phrase(),
                node.line
            ),
            "write `overrides:` then `<part>:` then `<Component>:` with the fields to change",
        );
        return;
    };
    for part in parts {
        let part_pointer = format!("{pointer}/{}", pointer_token(&part.key));
        let NodeValue::Mapping(components) = &part.node.value else {
            bad(
                diagnostics,
                part_pointer,
                format!(
                    "the overrides for part `{}` must be a mapping of component name to value, \
                     found {} (line {})",
                    part.key,
                    part.node.value.type_phrase(),
                    part.node.line
                ),
                "write `<Component>:` with the fields to change under the part",
            );
            continue;
        };
        instance.overrides.insert(
            part.key.clone(),
            components
                .iter()
                .map(|component| (component.key.clone(), node_to_value(&component.node)))
                .collect(),
        );
    }
}

fn parse_removed(
    node: &Node,
    pointer: &str,
    instance: &mut InstanceEntry,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let NodeValue::Sequence(items) = &node.value else {
        bad(
            diagnostics,
            pointer.to_string(),
            format!(
                "`removed` must be a sequence of part paths, found {} (line {})",
                node.value.type_phrase(),
                node.line
            ),
            "write one part path per `- ` item",
        );
        return;
    };
    for (position, item) in items.iter().enumerate() {
        let item_pointer = format!("{pointer}/{position}");
        let NodeValue::Scalar(Value::String(path)) = &item.value else {
            bad(
                diagnostics,
                item_pointer,
                format!("a `removed` entry must be a string (line {})", item.line),
                "write the part path, e.g. `- shade`",
            );
            continue;
        };
        if instance.removed.contains(path) {
            bad(
                diagnostics,
                item_pointer,
                format!("`removed` lists `{path}` twice (line {})", item.line),
                "list each part once",
            );
        } else {
            instance.removed.push(path.clone());
        }
    }
}

/// `L1231` at the later of two instance entries sharing a name. `entries` pairs each parsed entity
/// with its document position; plain entities are skipped.
pub(crate) fn check_duplicate_names(
    entries: &[(usize, &SceneEntity)],
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut first_seen: Vec<(&str, usize)> = Vec::new();
    for (index, entity) in entries {
        let (Some(name), Some(_)) = (entity.name.as_deref(), &entity.instance) else {
            continue;
        };
        match first_seen.iter().find(|(seen, _)| *seen == name) {
            Some((_, first)) => bad(
                diagnostics,
                format!("/entities/{index}/name"),
                format!("instances {first} and {index} are both named `{name}`"),
                "rename one of the instances; instance names are unique within a document",
            ),
            None => first_seen.push((name, *index)),
        }
    }
}

/// Appends the instance's fields after the `name:` line of an entity item; `lead` is the line
/// prefix the entry's next line takes (`    ` once `name:` is written).
pub(crate) fn emit_fields(
    out: &mut String,
    mut lead: &str,
    instance: &InstanceEntry,
    registry: &Registry,
) {
    out.push_str(lead);
    lead = "    ";
    out.push_str("prefab: ");
    out.push_str(&scalar_string(&instance.prefab));
    out.push('\n');
    if instance.presentation {
        out.push_str(lead);
        out.push_str("presentation: true\n");
    }
    let default = InstanceTransform::default();
    let transform = &instance.transform;
    let bits = |value: [f64; 3]| value.map(f64::to_bits);
    let fields = [
        ("translation", transform.translation, default.translation),
        ("rotation", transform.rotation, default.rotation),
        ("scale", transform.scale, default.scale),
    ];
    if fields
        .iter()
        .any(|(_, value, zero)| bits(*value) != bits(*zero))
    {
        out.push_str(lead);
        out.push_str("transform:\n");
        for (key, value, zero) in fields {
            if bits(value) == bits(zero) {
                continue;
            }
            write_indent(out, 3);
            out.push_str(key);
            out.push_str(":\n");
            for component in value {
                write_indent(out, 4);
                out.push_str("- ");
                out.push_str(&format_f32_float(component));
                out.push('\n');
            }
        }
    }
    if !instance.overrides.is_empty() {
        out.push_str(lead);
        out.push_str("overrides:\n");
        for (part, components) in &instance.overrides {
            write_indent(out, 3);
            out.push_str(&scalar_string(part));
            if components.is_empty() {
                out.push_str(": {}\n");
                continue;
            }
            out.push_str(":\n");
            for (name, value) in ordered_components(components, registry) {
                write_indent(out, 4);
                out.push_str(&scalar_string(name));
                out.push(':');
                let schema = registry
                    .get(name)
                    .map(|info| SchemaCursor::new(info.schema()));
                write_after_key(out, value, schema, 4);
            }
        }
    }
    if !instance.removed.is_empty() {
        out.push_str(lead);
        out.push_str("removed:\n");
        let mut removed: Vec<&String> = instance.removed.iter().collect();
        removed.sort();
        for path in removed {
            write_indent(out, 3);
            out.push_str("- ");
            out.push_str(&scalar_string(path));
            out.push('\n');
        }
    }
}

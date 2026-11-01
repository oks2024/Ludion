//! Scene validation against the component registry's JSON Schemas.
//!
//! Two checks per component instance: the component *name* must exist in the registry (`L1013`,
//! with a nearest-match suggestion), and the component *value* must match that component's JSON
//! Schema (`L1014`, with a JSON Pointer to the exact offending field).
//!
//! The schema checker is a deliberate subset of JSON Schema — exactly the keywords `schemars`
//! emits for the component types the engine supports: `type`, `enum`, `const`, `properties` /
//! `required` / `additionalProperties`, `items` / `prefixItems` / `minItems` / `maxItems`,
//! numeric bounds, numeric `format`s (`float` and the fixed-width integers), `oneOf` / `anyOf` /
//! `allOf`, and local `$ref`. Unknown keywords are ignored (the schema stays the source of truth;
//! this checker only reports what it understands). Two deliberate strictnesses beyond vanilla JSON
//! Schema, both tuned for agent iteration loops:
//!
//! - a field not declared in `properties` is an error (typos are flagged, never ignored), and
//! - `integer` rejects float-typed numbers like `1.0`, because serde will reject them at load
//!   time anyway.

use ludion_core::codes::code;
use ludion_core::diagnostics::{Diagnostic, DiagnosticCode, value_phrase};
use ludion_core::suggest;
use ludion_ecs::{ComponentInfo, Registry};
use serde_json::Value;

use crate::schema::SchemaCursor;
use crate::yaml::pointer_token;
use crate::{InstanceEntry, Scene};

/// Validates every component instance in `scene` against `registry`.
///
/// Returns all diagnostics found (empty = valid).
pub(crate) fn validate(scene: &Scene, registry: &Registry) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for (index, entity) in scene.entities.iter().enumerate() {
        if let Some(instance) = &entity.instance {
            check_overrides(instance, (index, scene), registry, &mut diagnostics);
        }
        for (name, value) in &entity.components {
            let pointer = format!("/entities/{index}/components/{}", pointer_token(name));
            match registry.get(name) {
                None => diagnostics.push(unknown_component(name, registry, pointer)),
                Some(info) if info.reserved() => {
                    diagnostics.push(reserved_component(name, pointer))
                }
                Some(info) => {
                    let checker = Checker {
                        subject: Subject::Component(name),
                    };
                    let before = diagnostics.len();
                    checker.check(
                        value,
                        SchemaCursor::new(info.schema()),
                        &pointer,
                        &mut diagnostics,
                    );
                    // Component-level semantic checks — cross-field invariants no JSON Schema can
                    // express (e.g. a tilemap's equal-length rows), registered *alongside* the schema
                    // in the registry and invoked generically here. This crate never names a concrete
                    // component type: it asks the registry entry to validate itself and rebases the
                    // returned relative JSON Pointers onto the component's document position. Only run
                    // on a schema-clean value — a shape mismatch is already reported, and the hook is
                    // defensive but adds nothing useful atop it.
                    if diagnostics.len() == before {
                        for diagnostic in info.validate_semantics(value) {
                            let rebased = match &diagnostic.path {
                                Some(relative) => format!("{pointer}{relative}"),
                                None => pointer.clone(),
                            };
                            diagnostics.push(diagnostic.with_path(rebased));
                        }
                    }
                    // Entity references carry a structural invariant no JSON Schema can express
                    // (`index` must be a real document position). The check lives here — not in
                    // `ludion-ecs` — because "document position" is a scene concept; which fields
                    // are references is the registry's, read from their schema marker.
                    for field in info.entity_refs() {
                        check_entity_ref(
                            value,
                            name,
                            &field.suffix,
                            field.allow_self,
                            (index, scene),
                            &pointer,
                            &mut diagnostics,
                        );
                    }
                }
            }
        }
    }
    for cycle in hierarchy_cycles(scene, registry) {
        diagnostics.push(cycle_diagnostic(scene, &cycle));
    }
    diagnostics
}

/// Schema-checks one component value against its registered schema, with pointers relative to the
/// component. An unregistered name yields nothing — the caller's insert reports it.
pub(crate) fn check_component(name: &str, value: &Value, registry: &Registry) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    if let Some(info) = registry.get(name) {
        Checker {
            subject: Subject::Component(name),
        }
        .check(
            value,
            SchemaCursor::new(info.schema()),
            "",
            &mut diagnostics,
        );
        if diagnostics.is_empty() {
            check_asset_paths(info, name, value, "", &mut diagnostics);
        }
    }
    diagnostics
}

/// Refuses every asset path in the scene's own components that would leave the content root
/// (`L1321`). A separate pass from [`validate`]: a prefab body is checked by its own sibling-relative
/// pass, not this one. Components that fail their schema are skipped — that is already reported.
pub(crate) fn check_scene_asset_paths(scene: &Scene, registry: &Registry) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for (index, entity) in scene.entities.iter().enumerate() {
        for (name, value) in &entity.components {
            let Some(info) = registry.get(name).filter(|info| !info.reserved()) else {
                continue;
            };
            let pointer = format!("/entities/{index}/components/{}", pointer_token(name));
            let mut schema = Vec::new();
            Checker {
                subject: Subject::Component(name),
            }
            .check(
                value,
                SchemaCursor::new(info.schema()),
                &pointer,
                &mut schema,
            );
            if schema.is_empty() {
                check_asset_paths(info, name, value, &pointer, &mut diagnostics);
            }
        }
    }
    diagnostics
}

/// Refuses every asset path in `value` that would leave the content root (`L1321`), pointing at
/// the field. Run only on a schema-clean value.
fn check_asset_paths(
    info: &ComponentInfo,
    name: &str,
    value: &Value,
    pointer: &str,
    out: &mut Vec<Diagnostic>,
) {
    let mut value = value.clone();
    info.for_each_asset_path(&mut value, &mut |field, path| {
        if path.is_empty() {
            return;
        }
        if let Some(refusal) = ludion_core::paths::content_refusal(path) {
            out.push(
                Diagnostic::new(
                    code::CONTENT_PATH_OUTSIDE_ROOT,
                    format!("`{name}` path `{path}` {}", refusal.describe()),
                )
                .with_path(format!("{pointer}{field}"))
                .with_help(
                    "an asset path is relative to the content root (the scene's own directory) and must stay inside it",
                ),
            );
        }
    });
}

/// Name and entity-reference checks on an instance's override components. The partials are not
/// schema-checked: they are incomplete until merged over the prefab's part.
fn check_overrides(
    instance: &InstanceEntry,
    (index, scene): (usize, &Scene),
    registry: &Registry,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for (part, components) in &instance.overrides {
        for (name, value) in components {
            let pointer = format!(
                "/entities/{index}/overrides/{}/{}",
                pointer_token(part),
                pointer_token(name)
            );
            match registry.get(name) {
                None => diagnostics.push(unknown_component(name, registry, pointer)),
                Some(info) if info.reserved() => {
                    diagnostics.push(reserved_component(name, pointer))
                }
                Some(info) => {
                    for field in info.entity_refs() {
                        check_entity_ref(
                            value,
                            name,
                            &field.suffix,
                            // A ref to the containing entry is its first surviving part, never
                            // a self-reference here; expansion catches the true one.
                            true,
                            (index, scene),
                            &pointer,
                            diagnostics,
                        );
                    }
                }
            }
        }
    }
}

/// A loop of tree links (a component field the registry marks `hierarchy`, i.e. `Parent`): each
/// member's parent is the next, and the last member's parent is the first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HierarchyCycle {
    /// The component carrying the link, e.g. `Parent`.
    pub component: String,
    /// The link's JSON Pointer suffix within the component (`""` when the component is the link).
    pub suffix: String,
    /// Document positions of the entities on the loop, starting at the lowest.
    pub members: Vec<usize>,
}

/// Finds every cycle in the scene's tree links, each reported once.
///
/// A link is read only when its component carries an in-range integer `index` other than its own
/// entity's, with `generation` 0; every other shape is already `L1014` or `L1017` and counts as no
/// link. Self-parenting is therefore never a cycle here.
pub(crate) fn hierarchy_cycles(scene: &Scene, registry: &Registry) -> Vec<HierarchyCycle> {
    let mut cycles = Vec::new();
    for info in registry.iter() {
        for field in info.entity_refs().iter().filter(|field| field.hierarchy) {
            let parents: Vec<Option<usize>> = scene
                .entities
                .iter()
                .enumerate()
                .map(|(index, entity)| {
                    let (_, value) = entity.components.iter().find(|(n, _)| n == info.name())?;
                    let reference = if field.suffix.is_empty() {
                        value
                    } else {
                        value.pointer(&field.suffix)?
                    };
                    let object = reference.as_object()?;
                    let parent = usize::try_from(object.get("index")?.as_u64()?).ok()?;
                    let generation = object.get("generation")?.as_u64()?;
                    (generation == 0 && parent < scene.entities.len() && parent != index)
                        .then_some(parent)
                })
                .collect();
            for start in 0..parents.len() {
                let Some(mut members) = cycle_from(start, &parents) else {
                    continue;
                };
                // Reporting only from the cycle's lowest member names each cycle exactly once.
                if members.iter().min() == Some(&start) {
                    let at = members.iter().position(|&m| m == start).unwrap_or(0);
                    members.rotate_left(at);
                    cycles.push(HierarchyCycle {
                        component: info.name().to_string(),
                        suffix: field.suffix.clone(),
                        members,
                    });
                }
            }
        }
    }
    cycles
}

/// The cycle reachable by following `parents` from `start`, in walk order beginning at the first
/// repeated node, or `None` when the chain ends at a root.
fn cycle_from(start: usize, parents: &[Option<usize>]) -> Option<Vec<usize>> {
    let mut path: Vec<usize> = Vec::new();
    let mut current = start;
    loop {
        if let Some(position) = path.iter().position(|&node| node == current) {
            return Some(path[position..].to_vec());
        }
        path.push(current);
        current = parents[current]?;
    }
}

pub(crate) fn cycle_diagnostic(scene: &Scene, cycle: &HierarchyCycle) -> Diagnostic {
    let label = |position: usize| match &scene.entities[position].name {
        Some(name) => format!("entity {position} (`{name}`)"),
        None => format!("entity {position}"),
    };
    let mut chain: Vec<String> = cycle.members.iter().map(|&m| label(m)).collect();
    chain.push(label(cycle.members[0]));
    let lowest = cycle.members[0];
    let pointer = format!(
        "/entities/{lowest}/components/{}{}/index",
        pointer_token(&cycle.component),
        cycle.suffix
    );
    Diagnostic::new(
        code::SCENE_PARENT_CYCLE,
        format!(
            "the `{}` chain loops: {}",
            cycle.component,
            chain.join(" -> ")
        ),
    )
    .with_path(pointer)
    .with_help(format!(
        "a hierarchy is a tree: re-point one link in the loop, or remove that entity's `{}` \
         component to make it a root",
        cycle.component
    ))
}

/// Structural entity-reference checks beyond JSON Schema: `index` names a real entity (in range,
/// and not itself unless `allow_self`), and `generation` is 0. Each field is checked only when it
/// is integer-shaped — a wrong-typed field is already reported against the schema as `L1014`, so
/// this adds no duplicate.
fn check_entity_ref(
    component_value: &Value,
    component: &str,
    suffix: &str,
    allow_self: bool,
    (entity_index, scene): (usize, &Scene),
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let reference = if suffix.is_empty() {
        Some(component_value)
    } else {
        component_value.pointer(suffix)
    };
    let Some(object) = reference.and_then(Value::as_object) else {
        return;
    };
    let entity_count = scene.entities.len();
    let field_pointer = format!("{pointer}{suffix}");
    let field_name = suffix.trim_start_matches('/');
    // `Parent`'s wording (`field_name` empty) stays byte-identical to before this generalized; a
    // named field (`Joint`'s `a`/`b`) gets analogous wording naming it.
    let noun = if field_name.is_empty() {
        format!("`{component}`")
    } else {
        format!("`{component}`'s `{field_name}`")
    };
    if let Some(index) = object.get("index").and_then(Value::as_u64) {
        let index = index as usize;
        if index >= entity_count {
            diagnostics.push(
                Diagnostic::new(
                    code::SCENE_INVALID_ENTITY_REF,
                    format!(
                        "{noun} index {index} is out of range: the scene has {entity_count} \
                         entities (valid document positions 0..{entity_count})"
                    ),
                )
                .with_path(format!("{field_pointer}/index"))
                .with_help(if field_name.is_empty() {
                    "set `index` to the 0-based document position of the parent entity".to_string()
                } else {
                    format!(
                        "set `index` to the 0-based document position of the entity `{field_name}` \
                         should refer to"
                    )
                }),
            );
        } else if !allow_self && index == entity_index {
            diagnostics.push(
                Diagnostic::new(
                    code::SCENE_INVALID_ENTITY_REF,
                    format!("{noun} index refers to the entity itself"),
                )
                .with_path(format!("{field_pointer}/index"))
                .with_help(if field_name.is_empty() {
                    "an entity cannot be its own parent; point `index` at another entity".to_string()
                } else {
                    format!("`{field_name}` cannot name the entity itself here; point it at another entity")
                }),
            );
        } else if !scene.entities[entity_index].presentation && scene.entities[index].presentation {
            diagnostics.push(presentation_ref_diagnostic(
                scene,
                (entity_index, index),
                &noun,
                &field_pointer,
            ));
        }
    }
    if let Some(generation) = object.get("generation").and_then(Value::as_u64)
        && generation != 0
    {
        diagnostics.push(
            Diagnostic::new(
                code::SCENE_INVALID_ENTITY_REF,
                format!("{noun} generation must be 0 in a scene, found {generation}"),
            )
            .with_path(format!("{field_pointer}/generation"))
            .with_help(if field_name.is_empty() {
                "a scene names a parent by document position, always with generation 0".to_string()
            } else {
                "a scene names an entity by document position, always with generation 0".to_string()
            }),
        );
    }
}

/// `L1036`: sim entity `carrier` holds a reference to presentation entity `target`.
fn presentation_ref_diagnostic(
    scene: &Scene,
    (carrier, target): (usize, usize),
    noun: &str,
    field_pointer: &str,
) -> Diagnostic {
    let label = |position: usize| match &scene.entities[position].name {
        Some(name) => format!("{position} (`{name}`)"),
        None => position.to_string(),
    };
    Diagnostic::new(
        code::SCENE_PRESENTATION_REF,
        format!(
            "{noun} on sim entity {} names presentation entity {}; the sim would hash a presentation id",
            label(carrier),
            label(target)
        ),
    )
    .with_path(format!("{field_pointer}/index"))
    .with_help("point the reference at a sim entity, or make the carrier `presentation: true` too")
}

/// `L1036` for every in-range reference a sim entity of the (expanded) `scene` holds to a
/// presentation entity. Instance expansion can turn an entry's parts presentation after the
/// document-level check ran.
pub(crate) fn presentation_refs(scene: &Scene, registry: &Registry) -> Vec<Diagnostic> {
    let mut found = Vec::new();
    for (carrier, entity) in scene
        .entities
        .iter()
        .enumerate()
        .filter(|(_, entity)| !entity.presentation)
    {
        for (name, value) in &entity.components {
            let Some(info) = registry.get(name) else {
                continue;
            };
            for field in info.entity_refs() {
                let reference = if field.suffix.is_empty() {
                    Some(value)
                } else {
                    value.pointer(&field.suffix)
                };
                let Some(target) = reference
                    .and_then(|object| object.get("index"))
                    .and_then(Value::as_u64)
                    .and_then(|index| usize::try_from(index).ok())
                    .filter(|&index| index < scene.entities.len() && index != carrier)
                else {
                    continue;
                };
                if !scene.entities[target].presentation {
                    continue;
                }
                let field_name = field.suffix.trim_start_matches('/');
                let noun = if field_name.is_empty() {
                    format!("`{name}`")
                } else {
                    format!("`{name}`'s `{field_name}`")
                };
                let pointer = format!(
                    "/entities/{carrier}/components/{}{}",
                    pointer_token(name),
                    field.suffix
                );
                found.push(presentation_ref_diagnostic(
                    scene,
                    (carrier, target),
                    &noun,
                    &pointer,
                ));
            }
        }
    }
    found
}

fn reserved_component(name: &str, pointer: String) -> Diagnostic {
    Diagnostic::new(
        code::SCENE_RESERVED_COMPONENT,
        format!("`{name}` is a reserved component and cannot be declared as a `components:` block"),
    )
    .with_path(pointer)
    .with_help(format!(
        "set it through the entity's `{}:` field instead",
        name.to_ascii_lowercase()
    ))
}

fn unknown_component(name: &str, registry: &Registry, pointer: String) -> Diagnostic {
    // The help line is the registry's (`Registry::name_suggestion`), shared with `L0004`: the scene
    // code and the ECS code differ, but "you named a component that is not registered" is one
    // reader-facing moment and gets one suggestion.
    Diagnostic::new(
        code::SCENE_UNKNOWN_COMPONENT,
        format!("no component named `{name}` is registered"),
    )
    .with_path(pointer)
    .with_help(registry.name_suggestion(name))
}

/// What a [`Checker`] is validating — the subject decides both the diagnostic code and the noun
/// every message carries, so the one generic schema walk serves both scene components (`L1014`)
/// and game data-docs (`L1120`) without either wording bleeding into the other.
pub(crate) enum Subject<'a> {
    /// A scene component instance, named by its registry name.
    Component(&'a str),
    /// A game data-doc, named by its `kind` (`crate::doc`).
    Doc(&'a str),
}

impl Subject<'_> {
    /// The diagnostic code a shape mismatch reports for this subject.
    fn code(&self) -> DiagnosticCode {
        match self {
            Subject::Component(_) => code::SCENE_COMPONENT_MISMATCH,
            Subject::Doc(_) => code::DOC_SCHEMA_MISMATCH,
        }
    }

    /// A value-mismatch message with the subject's noun in front of `detail`.
    fn mismatch_message(&self, detail: &str) -> String {
        match self {
            Subject::Component(name) => format!("component `{name}`: {detail}"),
            Subject::Doc(kind) => format!("data-doc `{kind}`: {detail}"),
        }
    }

    /// The "no such field" message for an undeclared key.
    fn unknown_field_message(&self, key: &str) -> String {
        match self {
            Subject::Component(name) => format!("`{name}` has no field named `{key}`"),
            Subject::Doc(kind) => format!("data-doc `{kind}` has no field named `{key}`"),
        }
    }
}

/// One subject's schema check. Carries the subject so every diagnostic names it and uses the right
/// code (`L1014` for a component, `L1120` for a data-doc).
pub(crate) struct Checker<'a> {
    pub(crate) subject: Subject<'a>,
}

impl Checker<'_> {
    /// Validates `value` against the subschema at `cursor`, appending diagnostics.
    pub(crate) fn check(
        &self,
        value: &Value,
        cursor: SchemaCursor<'_>,
        pointer: &str,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        self.check_inner(value, cursor, pointer, true, diagnostics);
    }

    /// `report_unknown` is `false` inside an `allOf` part: a key the part does not declare may belong
    /// to a sibling, so only the node holding the whole `allOf` decides what is unknown.
    fn check_inner(
        &self,
        value: &Value,
        cursor: SchemaCursor<'_>,
        pointer: &str,
        report_unknown: bool,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        let schema = cursor.resolved();
        // JSON Schema boolean form: `true` allows anything, `false` allows nothing.
        match schema {
            Value::Bool(true) => return,
            Value::Bool(false) => {
                diagnostics.push(self.mismatch(pointer, "no value is allowed here".to_string()));
                return;
            }
            _ => {}
        }

        if let Some(expected) = schema.get("type")
            && !type_matches(expected, value)
        {
            let mut diagnostic = self.mismatch(
                pointer,
                format!(
                    "expected {}, found {}",
                    expected_phrase(expected),
                    value_phrase(value)
                ),
            );
            diagnostic = add_type_help(diagnostic, expected, value);
            diagnostics.push(diagnostic);
            return; // deeper checks against the wrong type would only add noise
        }

        if let Some(allowed) = schema.get("enum").and_then(Value::as_array)
            && !allowed.contains(value)
        {
            diagnostics.push(
                self.mismatch(
                    pointer,
                    format!("{} is not one of the allowed values", value_phrase(value)),
                )
                .with_help(format!("allowed values are: {}", value_list(allowed))),
            );
            return;
        }
        if let Some(expected) = schema.get("const")
            && expected != value
        {
            diagnostics.push(self.mismatch(
                pointer,
                format!("the only allowed value here is {}", compact(expected)),
            ));
            return;
        }

        if let Some(all_of) = schema.get("allOf").and_then(Value::as_array) {
            for sub in all_of {
                self.check_inner(value, cursor.with(sub), pointer, false, diagnostics);
            }
        }
        for keyword in ["oneOf", "anyOf"] {
            if let Some(alternatives) = schema.get(keyword).and_then(Value::as_array) {
                self.check_alternatives(value, cursor, alternatives, pointer, diagnostics);
            }
        }

        self.check_number(value, schema, pointer, diagnostics);
        if let Some(object) = value.as_object() {
            self.check_object(object, cursor, schema, pointer, report_unknown, diagnostics);
        }
        if let Some(array) = value.as_array() {
            self.check_array(array, cursor, schema, pointer, diagnostics);
        }
    }

    /// `oneOf` / `anyOf`: accept if any alternative fully matches; otherwise report against the
    /// nearest alternative (the one with the fewest problems).
    fn check_alternatives(
        &self,
        value: &Value,
        cursor: SchemaCursor<'_>,
        alternatives: &[Value],
        pointer: &str,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        let mut best: Option<Vec<Diagnostic>> = None;
        for alternative in alternatives {
            let mut scratch = Vec::new();
            self.check(value, cursor.with(alternative), pointer, &mut scratch);
            if scratch.is_empty() {
                return;
            }
            if best.as_ref().is_none_or(|b| scratch.len() < b.len()) {
                best = Some(scratch);
            }
        }
        let mut diagnostic = self.mismatch(
            pointer,
            format!(
                "the value does not match any of the {} allowed shapes",
                alternatives.len()
            ),
        );
        if let Some(best) = best {
            for related in best.into_iter().take(2) {
                diagnostic = diagnostic.with_related(related);
            }
        }
        diagnostics.push(diagnostic);
    }

    fn check_number(
        &self,
        value: &Value,
        schema: &Value,
        pointer: &str,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        /// One numeric-bound keyword: its name, the message phrase, and the passing predicate.
        type Bound = (&'static str, &'static str, fn(f64, f64) -> bool);
        let Some(number) = value.as_f64() else { return };
        let bounds: [Bound; 4] = [
            ("minimum", "at least", |v, b| v >= b),
            ("maximum", "at most", |v, b| v <= b),
            ("exclusiveMinimum", "greater than", |v, b| v > b),
            ("exclusiveMaximum", "less than", |v, b| v < b),
        ];
        // An `f32` field stores the narrowed value, so the bounds judge that: `1e-46` is `0` there,
        // not strictly positive. A value that overflows stays raw; the format check refuses it.
        let single = schema.get("format").and_then(Value::as_str) == Some("float")
            && ludion_core::number::fits_f32(number);
        let narrow = |v: f64| if single { f64::from(v as f32) } else { v };
        let stored = narrow(number);
        let before = diagnostics.len();
        for (keyword, phrase, ok) in bounds {
            if let Some(bound) = schema.get(keyword).and_then(Value::as_f64)
                && !ok(stored, narrow(bound))
            {
                let note = if stored == number {
                    String::new()
                } else {
                    format!(" (as an f32 it is {})", stored as f32)
                };
                diagnostics.push(self.mismatch(
                    pointer,
                    format!("{} must be {phrase} {bound}{note}", compact(value)),
                ));
            }
        }
        // A bound the schema already states (`minimum: 0` on an unsigned type) has said it once.
        if diagnostics.len() == before
            && let Some(format) = schema.get("format").and_then(Value::as_str)
            && let Some(message) = format_violation(value, number, format)
        {
            diagnostics.push(self.mismatch(pointer, message));
        }
    }

    fn check_object(
        &self,
        object: &serde_json::Map<String, Value>,
        cursor: SchemaCursor<'_>,
        schema: &Value,
        pointer: &str,
        report_unknown: bool,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            for field in required.iter().filter_map(Value::as_str) {
                if !object.contains_key(field) {
                    diagnostics.push(
                        self.mismatch(pointer, format!("missing required field `{field}`"))
                            .with_help(format!("required fields are: {}", value_list(required))),
                    );
                }
            }
        }
        let properties = schema.get("properties").and_then(Value::as_object);
        let additional = schema.get("additionalProperties");
        // Own `properties` plus every `allOf` part's: what the object as a whole declares.
        let declared = cursor.property_order();
        let parts_open = schema
            .get("allOf")
            .and_then(Value::as_array)
            .is_some_and(|parts| parts.iter().any(|part| cursor.with(part).open_map()));
        for (key, field_value) in object {
            let field_pointer = format!("{pointer}/{}", pointer_token(key));
            if properties.is_some_and(|properties| properties.contains_key(key)) {
                if let Some(child) = cursor.child(key) {
                    self.check(field_value, child, &field_pointer, diagnostics);
                }
                continue;
            }
            // Declared by an `allOf` part, which checked it; or not ours to judge inside a part.
            if !report_unknown
                || declared
                    .as_ref()
                    .is_some_and(|names| names.contains(&key.as_str()))
            {
                continue;
            }
            match additional {
                Some(Value::Bool(true)) => {}
                Some(schema @ Value::Object(_)) => {
                    self.check(
                        field_value,
                        cursor.with(schema),
                        &field_pointer,
                        diagnostics,
                    );
                }
                // `additionalProperties: false`, or no `additionalProperties` at all: JSON
                // Schema would allow anything in the latter case, but a field the component
                // does not declare is a typo until proven otherwise.
                _ => {
                    if let Some(names) = &declared
                        && !parts_open
                    {
                        let mut diagnostic =
                            self.mismatch(&field_pointer, self.subject.unknown_field_message(key));
                        diagnostic = match suggest::nearest(key, names.iter().copied()) {
                            Some(candidate) => {
                                diagnostic.with_help(format!("nearest match is `{candidate}`"))
                            }
                            None => diagnostic
                                .with_help(format!("declared fields are: {}", names.join(", "))),
                        };
                        diagnostics.push(diagnostic);
                    }
                }
            }
        }
    }

    fn check_array(
        &self,
        array: &[Value],
        cursor: SchemaCursor<'_>,
        schema: &Value,
        pointer: &str,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        if let Some(min) = schema.get("minItems").and_then(Value::as_u64)
            && (array.len() as u64) < min
        {
            diagnostics.push(self.mismatch(
                pointer,
                format!("expected at least {min} elements, found {}", array.len()),
            ));
        }
        if let Some(max) = schema.get("maxItems").and_then(Value::as_u64)
            && (array.len() as u64) > max
        {
            diagnostics.push(self.mismatch(
                pointer,
                format!("expected at most {max} elements, found {}", array.len()),
            ));
        }
        for (index, element) in array.iter().enumerate() {
            if let Some(item) = cursor.item(index) {
                self.check(element, item, &format!("{pointer}/{index}"), diagnostics);
            }
        }
    }

    fn mismatch(&self, pointer: &str, detail: String) -> Diagnostic {
        Diagnostic::new(self.subject.code(), self.subject.mismatch_message(&detail))
            .with_path(pointer)
    }
}

/// Whether `value` matches a JSON Schema `type` keyword (a string or an array of strings).
fn type_matches(expected: &Value, value: &Value) -> bool {
    match expected {
        Value::String(name) => single_type_matches(name, value),
        Value::Array(names) => names
            .iter()
            .filter_map(Value::as_str)
            .any(|name| single_type_matches(name, value)),
        _ => true, // malformed keyword: nothing we can check
    }
}

fn single_type_matches(name: &str, value: &Value) -> bool {
    match name {
        "null" => value.is_null(),
        "boolean" => value.is_boolean(),
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "number" => value.is_number(),
        // Strict: serde rejects `1.0` for an integer field, so the validator does too.
        "integer" => value.is_i64() || value.is_u64(),
        _ => true,
    }
}

/// "a string", "a mapping or null", … for the `type` keyword.
fn expected_phrase(expected: &Value) -> String {
    let names: Vec<&str> = match expected {
        Value::String(name) => vec![name.as_str()],
        Value::Array(names) => names.iter().filter_map(Value::as_str).collect(),
        _ => return "a value".to_string(),
    };
    let phrases: Vec<&str> = names.iter().map(|name| type_phrase(name)).collect();
    phrases.join(" or ")
}

fn type_phrase(name: &str) -> &'static str {
    match name {
        "null" => "null",
        "boolean" => "a boolean",
        "object" => "a mapping",
        "array" => "a sequence",
        "string" => "a string",
        "number" => "a number",
        "integer" => "an integer",
        _ => "a value",
    }
}

fn add_type_help(diagnostic: Diagnostic, expected: &Value, value: &Value) -> Diagnostic {
    let expects = |name: &str| match expected {
        Value::String(s) => s == name,
        Value::Array(names) => names.iter().any(|n| n.as_str() == Some(name)),
        _ => false,
    };
    if expects("string") && !value.is_string() && !value.is_object() && !value.is_array() {
        return diagnostic.with_help("quote the value to make it a string");
    }
    if expects("integer") && value.is_f64() {
        return diagnostic.with_help("write the value without a fractional part");
    }
    diagnostic
}

/// The message for a number that does not fit the type its schema `format` names, or `None` when it
/// fits or the format is not a numeric one this checker enforces (`double`, unknown formats).
/// `float` is the engine's `f32`; integers compare through `i64`/`u64`, never `f64`, so a `u64` above
/// `i64::MAX` is judged exactly.
fn format_violation(value: &Value, number: f64, format: &str) -> Option<String> {
    if format == "float" {
        return (!ludion_core::number::fits_f32(number))
            .then(|| format!("{} is outside the f32 range (±3.4e38)", compact(value)));
    }
    let (min, max): (i128, i128) = match format {
        "int8" => (i8::MIN.into(), i8::MAX.into()),
        "int16" => (i16::MIN.into(), i16::MAX.into()),
        "int32" => (i32::MIN.into(), i32::MAX.into()),
        "int64" => (i64::MIN.into(), i64::MAX.into()),
        "uint8" => (0, u8::MAX.into()),
        "uint16" => (0, u16::MAX.into()),
        "uint32" => (0, u32::MAX.into()),
        "uint64" | "uint" => (0, u64::MAX.into()),
        _ => return None,
    };
    let number = value
        .as_i64()
        .map(i128::from)
        .or_else(|| value.as_u64().map(i128::from))?;
    (number < min || number > max).then(|| {
        format!(
            "{} does not fit in {format} (range {min}..={max})",
            compact(value)
        )
    })
}

/// A short `a, b, c` rendering of a list of JSON values.
fn value_list(values: &[Value]) -> String {
    values.iter().map(compact).collect::<Vec<_>>().join(", ")
}

/// A compact single-token rendering of a JSON value for messages.
fn compact(value: &Value) -> String {
    match value {
        Value::String(s) => format!("`{s}`"),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};

    use ludion_core::ids::EntityId;

    use super::*;

    /// Stand-ins for the real `ludion-spatial`/`ludion-ecs` `Joint`/`Parent` shapes — this crate
    /// never names either concrete type (the boundary rule); the reference fields are whatever the
    /// registry derived from the `ludion_ecs` schema proxies.
    #[derive(Serialize, Deserialize, JsonSchema)]
    struct TestParent(#[schemars(with = "ludion_ecs::EntityRefSchema")] EntityId);

    #[derive(Serialize, Deserialize, JsonSchema)]
    struct TestJoint {
        #[schemars(with = "ludion_ecs::EntityRefSelfSchema")]
        a: EntityId,
        #[schemars(with = "ludion_ecs::EntityRefSelfSchema")]
        b: EntityId,
    }

    /// A game component under an arbitrary name: its marked field is checked with no table entry.
    #[derive(Serialize, Deserialize, JsonSchema)]
    struct TestTurret {
        #[schemars(with = "Option<ludion_ecs::EntityRefSchema>")]
        target: Option<EntityId>,
    }

    fn registry() -> Registry {
        let mut registry = Registry::new();
        registry.register::<TestParent>("Parent").unwrap();
        registry.register::<TestJoint>("Joint").unwrap();
        registry.register::<TestTurret>("Turret").unwrap();
        registry
    }

    /// The point of marking the field in its schema: a component no table has heard of is still
    /// range-checked, and forbidden from naming itself.
    #[test]
    fn a_marked_field_on_any_component_is_range_checked() {
        let registry = registry();
        let scene = crate::parse(
            "format: 1\n\
             entities:\n  \
               - components:\n      \
                   Turret:\n        \
                     target:\n          \
                       index: 7\n          \
                       generation: 0\n",
        )
        .unwrap();
        let diagnostics = validate(&scene, &registry);
        let d = diagnostics
            .iter()
            .find(|d| d.code.as_str() == "L1017")
            .expect("expected L1017");
        assert_eq!(
            d.path.as_deref(),
            Some("/entities/0/components/Turret/target/index")
        );
        assert!(d.message.contains("`Turret`'s `target`"), "{}", d.message);
    }

    #[test]
    fn a_joint_a_out_of_range_is_l1017_at_the_field_pointer() {
        let registry = registry();
        let scene = crate::parse(
            "format: 1\n\
             entities:\n  \
               - components:\n      \
                   Joint:\n        \
                     a:\n          \
                       index: 5\n          \
                       generation: 0\n        \
                     b:\n          \
                       index: 0\n          \
                       generation: 0\n",
        )
        .unwrap();
        let diagnostics = validate(&scene, &registry);
        let d = diagnostics
            .iter()
            .find(|d| d.code.as_str() == "L1017")
            .expect("expected L1017");
        assert_eq!(
            d.path.as_deref(),
            Some("/entities/0/components/Joint/a/index")
        );
    }

    /// `Joint`'s `a`/`b` each individually allow naming the joint's own carrier — a runtime grab or
    /// a self-anchored joint is legitimate (`Plan/physics.md` design decision 2).
    #[test]
    fn a_joint_a_naming_its_own_carrier_is_allowed() {
        let registry = registry();
        let scene = crate::parse(
            "format: 1\n\
             entities:\n  \
               - components:\n      \
                   Joint:\n        \
                     a:\n          \
                       index: 0\n          \
                       generation: 0\n        \
                     b:\n          \
                       index: 1\n          \
                       generation: 0\n  \
               - components: {}\n",
        )
        .unwrap();
        let diagnostics = validate(&scene, &registry);
        assert!(
            diagnostics.iter().all(|d| d.code.as_str() != "L1017"),
            "a joint naming its own carrier must pass: {diagnostics:?}"
        );
    }

    /// The real `Parent` (the only component whose schema carries the hierarchy marker).
    fn hierarchy_registry() -> Registry {
        let mut registry = Registry::new();
        registry.register::<ludion_ecs::Parent>("Parent").unwrap();
        registry.register::<TestJoint>("Joint").unwrap();
        registry
    }

    /// A scene whose entity `i` has `Parent` pointing at `parents[i]` (`None`: no `Parent`).
    fn parent_scene(parents: &[Option<usize>]) -> Scene {
        let mut source = String::from("format: 1\nentities:\n");
        for (i, parent) in parents.iter().enumerate() {
            match parent {
                Some(p) => source.push_str(&format!(
                    "  - name: e{i}\n    components:\n      Parent:\n        index: {p}\n        generation: 0\n"
                )),
                None => source.push_str(&format!("  - name: e{i}\n    components: {{}}\n")),
            }
        }
        crate::parse(&source).unwrap()
    }

    fn cycle_errors(diagnostics: &[Diagnostic]) -> Vec<&Diagnostic> {
        diagnostics
            .iter()
            .filter(|d| d.code.as_str() == "L1035")
            .collect()
    }

    #[test]
    fn a_two_cycle_and_a_three_cycle_each_report_once_at_the_lowest_member() {
        let registry = hierarchy_registry();
        // 1 <-> 3 is a 2-cycle; 0 hangs off it. 4 -> 6 -> 5 -> 4 is a 3-cycle.
        let scene = parent_scene(&[Some(1), Some(3), None, Some(1), Some(6), Some(4), Some(5)]);
        let diagnostics = validate(&scene, &registry);
        let cycles = cycle_errors(&diagnostics);
        assert_eq!(cycles.len(), 2, "{diagnostics:?}");
        assert_eq!(
            cycles[0].path.as_deref(),
            Some("/entities/1/components/Parent/index")
        );
        assert_eq!(
            cycles[0].message,
            "the `Parent` chain loops: entity 1 (`e1`) -> entity 3 (`e3`) -> entity 1 (`e1`)"
        );
        assert_eq!(
            cycles[1].path.as_deref(),
            Some("/entities/4/components/Parent/index")
        );
        assert_eq!(
            cycles[1].message,
            "the `Parent` chain loops: entity 4 (`e4`) -> entity 6 (`e6`) -> entity 5 (`e5`) -> entity 4 (`e4`)"
        );
    }

    #[test]
    fn a_chain_and_a_forest_are_not_cycles() {
        let registry = hierarchy_registry();
        let scene = parent_scene(&[None, Some(0), Some(1), None, Some(3), Some(0)]);
        assert!(cycle_errors(&validate(&scene, &registry)).is_empty());
    }

    #[test]
    fn self_parenting_is_only_l1017() {
        let registry = hierarchy_registry();
        let scene = parent_scene(&[Some(0), Some(0)]);
        let diagnostics = validate(&scene, &registry);
        assert!(cycle_errors(&diagnostics).is_empty(), "{diagnostics:?}");
        assert_eq!(
            diagnostics
                .iter()
                .filter(|d| d.code.as_str() == "L1017")
                .count(),
            1
        );
    }

    #[test]
    fn joint_pairs_that_loop_are_not_hierarchy_cycles() {
        let registry = hierarchy_registry();
        let scene = crate::parse(
            "format: 1\n\
             entities:\n  \
               - components:\n      \
                   Joint:\n        \
                     a:\n          index: 1\n          generation: 0\n        \
                     b:\n          index: 1\n          generation: 0\n  \
               - components:\n      \
                   Joint:\n        \
                     a:\n          index: 0\n          generation: 0\n        \
                     b:\n          index: 0\n          generation: 0\n",
        )
        .unwrap();
        assert!(cycle_errors(&validate(&scene, &registry)).is_empty());
    }

    /// `Parent` keeps its stricter rule: an entity may never be its own parent, unlike `Joint`'s
    /// `a`/`b`.
    #[test]
    fn a_parent_self_reference_is_still_rejected() {
        let registry = registry();
        let scene = crate::parse(
            "format: 1\n\
             entities:\n  \
               - components:\n      \
                   Parent:\n        \
                     index: 0\n        \
                     generation: 0\n",
        )
        .unwrap();
        let diagnostics = validate(&scene, &registry);
        let d = diagnostics
            .iter()
            .find(|d| d.code.as_str() == "L1017")
            .expect("expected L1017");
        assert_eq!(
            d.path.as_deref(),
            Some("/entities/0/components/Parent/index")
        );
        assert!(d.message.contains("itself"), "{}", d.message);
    }

    #[derive(Serialize, Deserialize, JsonSchema)]
    struct FlatInner {
        b: f32,
    }

    #[derive(Serialize, Deserialize, JsonSchema)]
    struct FlatOuter {
        a: f32,
        #[serde(flatten)]
        inner: FlatInner,
    }

    /// schemars 1.x merges a flattened struct into the parent's `properties` (no `allOf`).
    #[test]
    fn a_flattened_struct_checks_clean() {
        let mut registry = Registry::new();
        registry.register::<FlatOuter>("FlatOuter").unwrap();
        let value = serde_json::json!({ "a": 1, "b": 2 });
        assert!(check_component("FlatOuter", &value, &registry).is_empty());
        let typo = serde_json::json!({ "a": 1, "c": 2 });
        assert_eq!(check_component("FlatOuter", &typo, &registry).len(), 2); // missing `b`, unknown `c`
    }

    fn check_schema(schema: &Value, value: Value) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        Checker {
            subject: Subject::Component("T"),
        }
        .check(&value, SchemaCursor::new(schema), "", &mut diagnostics);
        diagnostics
    }

    /// A field declared by an `allOf` sibling is neither unknown nor unchecked.
    #[test]
    fn an_all_of_part_declares_fields_of_the_whole_object() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "a": { "type": "number" } },
            "allOf": [{ "properties": { "b": { "type": "number" } } }],
        });
        assert!(check_schema(&schema, serde_json::json!({ "a": 1, "b": 2 })).is_empty());

        let unknown = check_schema(&schema, serde_json::json!({ "a": 1, "c": 2 }));
        assert_eq!(unknown.len(), 1, "{unknown:?}");
        assert_eq!(unknown[0].path.as_deref(), Some("/c"));

        let mistyped = check_schema(&schema, serde_json::json!({ "a": 1, "b": "x" }));
        assert_eq!(mistyped.len(), 1, "{mistyped:?}");
        assert_eq!(mistyped[0].path.as_deref(), Some("/b"));
    }

    /// An outer node with no `properties` of its own and several parts: fields of any part are fine.
    #[test]
    fn all_of_parts_alone_declare_the_object() {
        let schema = serde_json::json!({
            "allOf": [
                { "properties": { "a": { "type": "number" } } },
                { "properties": { "b": { "type": "number" } } },
            ],
        });
        assert!(check_schema(&schema, serde_json::json!({ "a": 1, "b": 2 })).is_empty());
        let unknown = check_schema(&schema, serde_json::json!({ "a": 1, "c": 2 }));
        assert_eq!(unknown.len(), 1, "{unknown:?}");
        assert_eq!(unknown[0].path.as_deref(), Some("/c"));
    }

    /// A `float` field stores the narrowed value, so the bounds judge that: `1e-46` is `0.0` as an
    /// `f32`, and `0.99999999999` is `1.0`.
    #[test]
    fn float_bounds_judge_the_narrowed_value() {
        let min =
            serde_json::json!({ "type": "number", "format": "float", "exclusiveMinimum": 0.0 });
        let refused = check_schema(&min, serde_json::json!(1e-46));
        assert_eq!(refused.len(), 1, "{refused:?}");
        assert!(
            refused[0].message.contains("as an f32 it is 0"),
            "{}",
            refused[0].message
        );
        assert!(check_schema(&min, serde_json::json!(1e-30)).is_empty());
        // A `double` keeps the wide comparison.
        let double =
            serde_json::json!({ "type": "number", "format": "double", "exclusiveMinimum": 0.0 });
        assert!(check_schema(&double, serde_json::json!(1e-46)).is_empty());

        let max =
            serde_json::json!({ "type": "number", "format": "float", "exclusiveMaximum": 1.0 });
        assert_eq!(
            check_schema(&max, serde_json::json!(0.99999999999)).len(),
            1
        );
        assert!(check_schema(&max, serde_json::json!(0.5)).is_empty());
    }

    #[derive(Serialize, Deserialize, JsonSchema)]
    struct TestSkin {
        #[schemars(with = "ludion_ecs::AssetPathSchema")]
        texture: String,
    }

    fn skin_scene(texture: &str) -> Scene {
        crate::parse(&format!(
            "format: 1\nentities:\n  - name: e\n    components:\n      Skin:\n        texture: \"{texture}\"\n"
        ))
        .unwrap()
    }

    #[test]
    fn an_asset_path_leaving_the_content_root_is_refused_at_its_field() {
        let mut registry = Registry::new();
        registry.register::<TestSkin>("Skin").unwrap();
        for path in ["../x.png", "/etc/x.png", "C:x.png"] {
            let diagnostics = check_scene_asset_paths(&skin_scene(path), &registry);
            assert_eq!(diagnostics.len(), 1, "{path}: {diagnostics:?}");
            assert_eq!(diagnostics[0].code, code::CONTENT_PATH_OUTSIDE_ROOT);
            assert_eq!(
                diagnostics[0].path.as_deref(),
                Some("/entities/0/components/Skin/texture")
            );
            let wire = check_component("Skin", &serde_json::json!({ "texture": path }), &registry);
            assert_eq!(wire.len(), 1, "{path}: {wire:?}");
            assert_eq!(wire[0].path.as_deref(), Some("/texture"));
        }
        assert!(check_scene_asset_paths(&skin_scene("a/../b.png"), &registry).is_empty());
    }
}

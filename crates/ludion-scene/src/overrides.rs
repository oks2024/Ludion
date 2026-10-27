//! Render-time scene overrides: an RFC 6901 JSON Pointer plus the value to write there, applied to
//! a parsed [`Scene`] in memory.
//!
//! An override never creates a component or an entity, and never conjures an intermediate node: the
//! document's shape stays what the file says. The **one** thing it may add is a missing *final*
//! field inside a component map the file already carries — otherwise tuning a field a scene simply
//! left at its default would mean rewriting the whole component map, and the sweep instrument would
//! be unusable on exactly the knobs most worth sweeping.
//!
//! That addition is safe for the same reason the rest of the module has no second rejection path:
//! overrides are applied to a *parsed but not yet validated* scene, so a misspelled field name is
//! the ordinary schema validation's `L1014` at the same JSON Pointer, naming the nearest declared
//! field — the same answer the same typo would get written into the file. An ill-typed or
//! out-of-range value likewise comes back as `L1014`, never as an override diagnostic.
//!
//! A **sweep** ([`parse_sweep`]) is the same thing with a list of values at one pointer: one
//! candidate per rendered image. It shares [`SceneOverride::parse`]'s value-parsing rule exactly, so
//! the two spellings cannot mean different things by the same text.

use ludion_core::codes::code;
use ludion_core::diagnostics::{Diagnostic, value_phrase};
use ludion_core::suggest;
use serde_json::Value;

use crate::yaml::pointer_token;
use crate::{Scene, SceneEntity};

/// The scene document's top-level keys, for nearest-match suggestions.
const SCENE_KEYS: [&str; 6] = [
    "name",
    "generated_from",
    "seed",
    "physics",
    "preload",
    "entities",
];

/// An entity's keys, for nearest-match suggestions.
const ENTITY_KEYS: [&str; 7] = [
    "name",
    "components",
    "presentation",
    "prefab",
    "transform",
    "overrides",
    "removed",
];

/// One render-time override: a JSON Pointer into the scene document and the value to write there.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneOverride {
    /// An RFC 6901 JSON Pointer into the scene document, e.g.
    /// `/entities/0/components/Transform/translation/1`.
    pub pointer: String,
    /// The value to write at `pointer`.
    pub value: Value,
}

/// What one override actually changed, for echoing back to the operator.
#[derive(Clone, Debug, PartialEq)]
pub struct AppliedOverride {
    /// The pointer the override addressed.
    pub pointer: String,
    /// The value the file held there, or `None` if the field did not exist and the override created
    /// it. Not the same as `Some(Value::Null)` — a field authored as `null` did exist — and worth
    /// telling apart in the echo, because "this scene never mentioned that field" is exactly the
    /// case where an operator wants to double-check the spelling.
    pub from: Option<Value>,
    /// The value that replaced it.
    pub to: Value,
}

impl SceneOverride {
    /// Parses the `<json-pointer>=<value>` spelling.
    ///
    /// The pointer is everything before the first `=`; the value is everything after it. The value
    /// is parsed as JSON, and taken as a plain JSON string when it is not valid JSON — so
    /// `/entities/0/components/PostProcess/bloom_strength=0.8` is the number `0.8` and `/name=night`
    /// is the string `"night"`.
    ///
    /// # Errors
    /// `L4008` when the spec has no `=`, an empty pointer, or a pointer that does not start with
    /// `/`.
    ///
    /// # Example
    /// ```
    /// use ludion_scene::SceneOverride;
    /// use serde_json::json;
    ///
    /// let numeric = SceneOverride::parse("/seed=7").unwrap();
    /// assert_eq!(numeric.pointer, "/seed");
    /// assert_eq!(numeric.value, json!(7));
    ///
    /// // Not valid JSON, so it is a plain string.
    /// let textual = SceneOverride::parse("/name=night").unwrap();
    /// assert_eq!(textual.value, json!("night"));
    ///
    /// assert_eq!(SceneOverride::parse("nonsense").unwrap_err().code.as_str(), "L4008");
    /// ```
    pub fn parse(spec: &str) -> Result<SceneOverride, Diagnostic> {
        let Some((pointer, raw)) = spec.split_once('=') else {
            return Err(malformed(
                spec,
                "there is no `=` separating the pointer from the value",
            ));
        };
        if pointer.is_empty() {
            return Err(malformed(spec, "the pointer before `=` is empty"));
        }
        if !pointer.starts_with('/') {
            return Err(malformed(
                spec,
                format!("the pointer `{pointer}` does not start with `/`"),
            ));
        }
        Ok(SceneOverride {
            pointer: pointer.to_string(),
            value: parse_value(raw),
        })
    }
}

/// The one value-parsing rule both override spellings share: the text as JSON when it parses,
/// otherwise the text itself as a plain JSON string.
fn parse_value(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_string()))
}

/// Parses the `<json-pointer>=<v,v,…>` sweep spelling into the pointer and the values to try there,
/// one per rendered candidate.
///
/// The pointer is everything before the **first** `=`; the values are everything after it, split on
/// `,`. Each value is parsed by [`SceneOverride::parse`]'s rule (JSON first, plain string on
/// failure), so `…/bloom_strength=0,0.4,0.8` is three numbers and `/name=night,day` is two strings.
///
/// **The split is literal and there is no escape: a value cannot itself contain a comma**, so a
/// sequence or an object cannot be swept. Nor is whitespace trimmed — `0.4, 0.8` sweeps `0.4` and
/// the string `" 0.8"` if the pointer expects text, and JSON's own leading-whitespace tolerance if
/// it expects a number. Sweep one scalar leaf at a time; that is what the instrument is for.
///
/// # Errors
/// `L4008` when the spec has no `=`, an empty or non-`/` pointer, no values at all, or an empty
/// value between two separators.
///
/// # Example
/// ```
/// use ludion_scene::parse_sweep;
/// use serde_json::json;
///
/// let (pointer, values) = parse_sweep("/entities/0/components/PostProcess/bloom_strength=0,0.4,0.8").unwrap();
/// assert_eq!(pointer, "/entities/0/components/PostProcess/bloom_strength");
/// assert_eq!(values, vec![json!(0), json!(0.4), json!(0.8)]);
///
/// assert_eq!(parse_sweep("/seed=").unwrap_err().code.as_str(), "L4008");
/// ```
pub fn parse_sweep(spec: &str) -> Result<(String, Vec<Value>), Diagnostic> {
    let Some((pointer, raw)) = spec.split_once('=') else {
        return Err(malformed_sweep(
            spec,
            "there is no `=` separating the pointer from the values",
        ));
    };
    if pointer.is_empty() {
        return Err(malformed_sweep(spec, "the pointer before `=` is empty"));
    }
    if !pointer.starts_with('/') {
        return Err(malformed_sweep(
            spec,
            format!("the pointer `{pointer}` does not start with `/`"),
        ));
    }
    if raw.is_empty() {
        return Err(malformed_sweep(
            spec,
            "there are no values after `=`; a sweep needs at least one",
        ));
    }
    if raw.split(',').any(str::is_empty) {
        return Err(malformed_sweep(
            spec,
            "one of the comma-separated values is empty",
        ));
    }
    Ok((
        pointer.to_string(),
        raw.split(',').map(parse_value).collect(),
    ))
}

/// Applies `overrides` to `scene` in order, returning what each one changed.
///
/// Diagnostics raised here say "override" rather than naming a flag: the same code path serves both
/// `--set` and `--sweep`, and a sweep told to fix its `--set` would be a wrong instruction.
///
/// Every pointer must resolve to a node the document already has, except for its last segment: a
/// missing *field* inside a component map that does resolve is created (see the module docs).
/// Failures are collected, so a batch of overrides reports all of its bad pointers in one pass; on
/// failure `scene` is left partially overridden and the caller must discard it.
///
/// # Errors
/// `L4008` per override whose pointer does not resolve, is empty, addresses a whole entity or the
/// component map, addresses a field overrides cannot write (`/physics`, and an entity's
/// `presentation`, `prefab`, `transform`, `overrides` and `removed`), or writes a scene-level
/// field (`/name`, `/generated_from`, `/seed`) a value it cannot hold.
///
/// # Example
/// ```
/// use ludion_scene::{SceneOverride, apply_overrides};
/// use serde_json::json;
///
/// let source = "format: 1\nentities:\n  - components:\n      Transform:\n        x: 1.5\n";
/// let mut scene = ludion_scene::parse(source).unwrap();
/// let overrides = [
///     SceneOverride::parse("/entities/0/components/Transform/x=4.0").unwrap(),
///     // A field the file left at its default is created, not refused.
///     SceneOverride::parse("/entities/0/components/Transform/y=2.0").unwrap(),
/// ];
///
/// let applied = apply_overrides(&mut scene, &overrides).unwrap();
/// assert_eq!(applied[0].from, Some(json!(1.5)));
/// assert_eq!(applied[0].to, json!(4.0));
/// assert_eq!(applied[1].from, None, "`y` was not in the file");
/// assert_eq!(scene.entities[0].components[0].1["x"], json!(4.0));
/// assert_eq!(scene.entities[0].components[0].1["y"], json!(2.0));
/// ```
pub fn apply_overrides(
    scene: &mut Scene,
    overrides: &[SceneOverride],
) -> Result<Vec<AppliedOverride>, Vec<Diagnostic>> {
    let mut applied = Vec::with_capacity(overrides.len());
    let mut diagnostics = Vec::new();
    for over in overrides {
        match apply_one(scene, over) {
            Ok(change) => applied.push(change),
            Err(diagnostic) => diagnostics.push(diagnostic),
        }
    }
    if diagnostics.is_empty() {
        Ok(applied)
    } else {
        Err(diagnostics)
    }
}

fn apply_one(scene: &mut Scene, over: &SceneOverride) -> Result<AppliedOverride, Diagnostic> {
    let segments: Vec<String> = over.pointer.split('/').skip(1).map(unescape).collect();
    let from = write(scene, &segments, &over.pointer, &over.value)?;
    Ok(AppliedOverride {
        pointer: over.pointer.clone(),
        from,
        to: over.value.clone(),
    })
}

/// Reverses RFC 6901's escaping (`~1` → `/`, `~0` → `~`, in that order).
///
/// Shared with [`crate::voxel`]'s palette applier: a voxel glyph may legitimately *be* `/` or `~`,
/// so a model pointer needs exactly this unescaping.
pub(crate) fn unescape(segment: &str) -> String {
    segment.replace("~1", "/").replace("~0", "~")
}

/// The value a pointer held before the override, or `None` if the override created the field.
type Replaced = Result<Option<Value>, Diagnostic>;

fn write(scene: &mut Scene, segments: &[String], pointer: &str, value: &Value) -> Replaced {
    let Some(head) = segments.first() else {
        return Err(not_a_leaf(pointer, "the whole scene document"));
    };
    match head.as_str() {
        "physics" => Err(not_overridable(
            pointer,
            "/physics",
            "the physics block is typed scene configuration; edit the scene file",
        )),
        "preload" => Err(not_overridable(
            pointer,
            "/preload",
            "the preload list is scene configuration; edit the scene file",
        )),
        "name" => write_optional_string(&mut scene.name, segments, pointer, value, "/name"),
        "generated_from" => write_optional_string(
            &mut scene.generated_from,
            segments,
            pointer,
            value,
            "/generated_from",
        ),
        "seed" => {
            if segments.len() > 1 {
                return Err(no_children(pointer, "/seed", "an integer"));
            }
            let from = scene.seed.map_or(Value::Null, Value::from);
            match value {
                Value::Null => scene.seed = None,
                Value::Number(number) => match number.as_u64() {
                    Some(seed) => scene.seed = Some(seed),
                    None => {
                        return Err(wrong_scalar(
                            pointer,
                            "a non-negative integer, or null to clear it",
                            value,
                        ));
                    }
                },
                _ => {
                    return Err(wrong_scalar(
                        pointer,
                        "a non-negative integer, or null to clear it",
                        value,
                    ));
                }
            }
            Ok(Some(from))
        }
        "entities" => write_in_entities(scene, segments, pointer, value),
        other => Err(unresolved(
            pointer,
            format!("the scene document has no `{other}`"),
            suggest::nearest(other, SCENE_KEYS),
        )),
    }
}

fn write_optional_string(
    slot: &mut Option<String>,
    segments: &[String],
    pointer: &str,
    value: &Value,
    resolved: &str,
) -> Replaced {
    if segments.len() > 1 {
        return Err(no_children(pointer, resolved, "a string"));
    }
    let from = slot.clone().map_or(Value::Null, Value::String);
    match value {
        Value::String(text) => *slot = Some(text.clone()),
        Value::Null => *slot = None,
        _ => {
            return Err(wrong_scalar(
                pointer,
                "a string, or null to clear it",
                value,
            ));
        }
    }
    Ok(Some(from))
}

fn write_in_entities(
    scene: &mut Scene,
    segments: &[String],
    pointer: &str,
    value: &Value,
) -> Replaced {
    let Some(index_segment) = segments.get(1) else {
        return Err(not_a_leaf(pointer, "the entity list"));
    };
    let count = scene.entities.len();
    let index = index_segment.parse::<usize>().ok().filter(|i| *i < count);
    let Some(index) = index else {
        return Err(unresolved(
            pointer,
            format!(
                "`/entities` resolves to {count} entit{}, so `{index_segment}` is not one of them",
                if count == 1 { "y" } else { "ies" }
            ),
            None,
        ));
    };
    let entity = &mut scene.entities[index];
    match segments.get(2).map(String::as_str) {
        None => Err(not_a_leaf(pointer, "a whole entity")),
        Some("name") => write_optional_string(
            &mut entity.name,
            &segments[2..],
            pointer,
            value,
            &format!("/entities/{index}/name"),
        ),
        Some("components") => write_in_components(entity, &segments[3..], pointer, index, value),
        Some(field @ ("presentation" | "prefab" | "transform" | "overrides" | "removed")) => {
            Err(not_overridable(
                pointer,
                &format!("/entities/{index}/{field}"),
                "an entity's allocator and prefab-instance fields are structure; overrides write \
                 component fields",
            ))
        }
        Some(other) => Err(unresolved(
            pointer,
            format!("`/entities/{index}` resolves, but the entity has no `{other}`"),
            suggest::nearest(other, ENTITY_KEYS),
        )),
    }
}

fn write_in_components(
    entity: &mut SceneEntity,
    segments: &[String],
    pointer: &str,
    index: usize,
    value: &Value,
) -> Replaced {
    let Some(name) = segments.first() else {
        return Err(not_a_leaf(pointer, "a whole component map"));
    };
    let Some(position) = entity.components.iter().position(|(n, _)| n == name) else {
        let names: Vec<&str> = entity.components.iter().map(|(n, _)| n.as_str()).collect();
        let suggestion = suggest::nearest(name, names.iter().copied());
        return Err(unresolved(
            pointer,
            format!(
                "`/entities/{index}/components` resolves, but the entity has no `{name}` component"
            ),
            suggestion,
        ));
    };
    let base = format!(
        "/entities/{index}/components/{}",
        pointer_token(&entity.components[position].0)
    );
    let slot = &mut entity.components[position].1;
    let inside = &segments[1..];
    if inside.is_empty() {
        return Ok(Some(std::mem::replace(slot, value.clone())));
    }
    write_in_value(slot, inside, pointer, base, value)
}

fn write_in_value(
    root: &mut Value,
    segments: &[String],
    pointer: &str,
    base: String,
    value: &Value,
) -> Replaced {
    let mut resolved = base;
    let mut node = root;
    for (position, segment) in segments.iter().enumerate() {
        let last = position + 1 == segments.len();
        node = match node {
            Value::Object(map) => {
                if !map.contains_key(segment.as_str()) {
                    // The one thing an override adds rather than replaces: a final field a
                    // component simply left at its default. An *intermediate* miss still fails —
                    // that would be conjuring structure, not tuning a knob — and a misspelled final
                    // field is caught downstream by the schema's own `L1014`, at this same pointer
                    // and with the same nearest-match suggestion.
                    if last {
                        map.insert(segment.clone(), value.clone());
                        return Ok(None);
                    }
                    let keys: Vec<&str> = map.keys().map(String::as_str).collect();
                    let suggestion = suggest::nearest(segment, keys.iter().copied());
                    return Err(unresolved(
                        pointer,
                        format!("`{resolved}` resolves, but it has no `{segment}`"),
                        suggestion,
                    ));
                }
                resolved.push('/');
                resolved.push_str(&pointer_token(segment));
                map.get_mut(segment.as_str())
                    .expect("presence checked just above")
            }
            Value::Array(items) => {
                let len = items.len();
                let index = segment.parse::<usize>().ok().filter(|i| *i < len);
                let Some(index) = index else {
                    return Err(unresolved(
                        pointer,
                        format!(
                            "`{resolved}` resolves to a sequence of {len}, so `{segment}` is not one of its entries"
                        ),
                        None,
                    ));
                };
                resolved.push('/');
                resolved.push_str(&pointer_token(segment));
                &mut items[index]
            }
            leaf => return Err(no_children(pointer, &resolved, value_phrase(leaf))),
        };
    }
    Ok(Some(std::mem::replace(node, value.clone())))
}

fn malformed(spec: &str, detail: impl Into<String>) -> Diagnostic {
    Diagnostic::new(
        code::SCENE_OVERRIDE_INVALID,
        format!("`--set {spec}` is not a valid override: {}", detail.into()),
    )
    .with_help(
        "spell an override `<json-pointer>=<value>`, e.g. \
         `--set /entities/0/components/Transform/translation/1=2.5`",
    )
}

fn malformed_sweep(spec: &str, detail: impl Into<String>) -> Diagnostic {
    Diagnostic::new(
        code::SCENE_OVERRIDE_INVALID,
        format!("`--sweep {spec}` is not a valid sweep: {}", detail.into()),
    )
    .with_help(
        "spell a sweep `<json-pointer>=<v,v,…>`, e.g. \
         `--sweep /entities/0/components/PostProcess/bloom_strength=0,0.4,0.8`; the values are \
         split on `,` with no escape, so a value cannot itself contain a comma",
    )
}

fn unresolved(pointer: &str, detail: String, suggestion: Option<&str>) -> Diagnostic {
    let help = match suggestion {
        Some(candidate) => format!("{detail} — nearest match is `{candidate}`"),
        None => detail,
    };
    Diagnostic::new(
        code::SCENE_OVERRIDE_INVALID,
        format!("override pointer `{pointer}` does not resolve in this scene"),
    )
    .with_path(pointer.to_string())
    .with_help(help)
}

fn no_children(pointer: &str, resolved: &str, phrase: &str) -> Diagnostic {
    unresolved(
        pointer,
        format!("`{resolved}` resolves to {phrase}, which has no children"),
        None,
    )
}

fn not_a_leaf(pointer: &str, what: &str) -> Diagnostic {
    Diagnostic::new(
        code::SCENE_OVERRIDE_INVALID,
        format!("override pointer `{pointer}` addresses {what}, not a value"),
    )
    .with_path(pointer.to_string())
    .with_help(
        "an override writes one value; point at a component \
         (`/entities/0/components/Transform`) or a field inside one",
    )
}

fn not_overridable(pointer: &str, resolved: &str, why: &str) -> Diagnostic {
    Diagnostic::new(
        code::SCENE_OVERRIDE_INVALID,
        format!(
            "override pointer `{pointer}` addresses `{resolved}`, which overrides cannot write"
        ),
    )
    .with_path(pointer.to_string())
    .with_help(why)
}

fn wrong_scalar(pointer: &str, expected: &str, value: &Value) -> Diagnostic {
    Diagnostic::new(
        code::SCENE_OVERRIDE_INVALID,
        format!(
            "the override at `{pointer}` expects {expected}, but the value is {}",
            value_phrase(value)
        ),
    )
    .with_path(pointer.to_string())
    .with_help("scene-level fields are part of the document skeleton, not schema-validated data")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scene() -> Scene {
        crate::parse(
            "format: 1\nname: demo\nseed: 3\nentities:\n  - name: hero\n    components:\n      \
             Transform:\n        x: 1.5\n        list:\n          - 1\n          - 2\n",
        )
        .unwrap()
    }

    fn apply(spec: &str) -> Result<(Scene, Vec<AppliedOverride>), Vec<Diagnostic>> {
        let mut scene = scene();
        let over = SceneOverride::parse(spec).map_err(|d| vec![d])?;
        let applied = apply_overrides(&mut scene, &[over])?;
        Ok((scene, applied))
    }

    #[test]
    fn scalar_inside_a_component_is_replaced() {
        let (scene, applied) = apply("/entities/0/components/Transform/x=4.25").unwrap();
        assert_eq!(applied[0].from, Some(json!(1.5)));
        assert_eq!(applied[0].to, json!(4.25));
        assert_eq!(scene.entities[0].components[0].1["x"], json!(4.25));
    }

    #[test]
    fn sequence_entries_are_addressable() {
        let (scene, applied) = apply("/entities/0/components/Transform/list/1=9").unwrap();
        assert_eq!(applied[0].from, Some(json!(2)));
        assert_eq!(scene.entities[0].components[0].1["list"], json!([1, 9]));
    }

    /// A field a component simply left at its default is created, not refused — otherwise tuning
    /// one knob would mean rewriting the whole component map, and every sweep over a newly-added
    /// field would be unusable. `from: None` is how the caller tells "created" from "replaced".
    #[test]
    fn a_missing_final_field_is_created() {
        let (scene, applied) = apply("/entities/0/components/Transform/y=6.0").unwrap();
        assert_eq!(applied[0].from, None);
        assert_eq!(applied[0].to, json!(6.0));
        assert_eq!(scene.entities[0].components[0].1["y"], json!(6.0));
        assert_eq!(
            scene.entities[0].components[0].1["x"],
            json!(1.5),
            "creating one field leaves the rest of the map alone",
        );
    }

    /// Creation stops at the last segment. A miss any earlier would be conjuring structure — a
    /// pointer that could grow nodes would let a typo render a scene nobody authored.
    #[test]
    fn a_missing_intermediate_node_is_still_unresolved() {
        let diagnostics = apply("/entities/0/components/Transform/nested/y=1").unwrap_err();
        assert_eq!(diagnostics[0].code.as_str(), "L4008");
        assert_eq!(
            scene().entities[0].components[0].1.get("nested"),
            None,
            "a rejected override leaves the map untouched",
        );
    }

    /// Nor may creation reach a component, an entity, or a sequence entry: absence is meaningful for
    /// an opt-in singleton (`Environment`, `PostProcess`), so conjuring one would change the
    /// pipeline the scene renders through.
    #[test]
    fn creation_never_reaches_a_component_an_entity_or_a_sequence() {
        for spec in [
            "/entities/0/components/Environment/ambient=1",
            "/entities/1/components/Transform/x=1",
            "/entities/0/components/Transform/list/2=3",
        ] {
            let diagnostics = apply(spec).unwrap_err();
            assert_eq!(diagnostics[0].code.as_str(), "L4008", "{spec}");
        }
    }

    #[test]
    fn an_empty_pointer_is_refused_without_panicking() {
        let mut scene = scene();
        let over = SceneOverride {
            pointer: String::new(),
            value: json!(1),
        };
        let diagnostics = apply_overrides(&mut scene, &[over]).unwrap_err();
        assert_eq!(diagnostics[0].code.as_str(), "L4008");
    }

    #[test]
    fn typed_blocks_and_entity_structure_are_not_overridable() {
        let diagnostics = apply("/physics/gravity/0=1").unwrap_err();
        assert_eq!(diagnostics[0].code.as_str(), "L4008");
        assert!(diagnostics[0].help.as_deref().unwrap().contains("physics"));

        let diagnostics = apply("/entities/0/presentation=true").unwrap_err();
        assert_eq!(diagnostics[0].code.as_str(), "L4008");
        assert!(diagnostics[0].message.contains("cannot write"));
    }

    #[test]
    fn a_whole_component_value_can_be_replaced() {
        let (scene, _) = apply(r#"/entities/0/components/Transform={"x": 0.0}"#).unwrap();
        assert_eq!(scene.entities[0].components[0].1, json!({ "x": 0.0 }));
    }

    #[test]
    fn scene_level_fields_are_addressable() {
        let (scene, applied) = apply("/name=night").unwrap();
        assert_eq!(applied[0].from, Some(json!("demo")));
        assert_eq!(scene.name.as_deref(), Some("night"));

        let (scene, applied) = apply("/seed=11").unwrap();
        assert_eq!(applied[0].from, Some(json!(3)));
        assert_eq!(scene.seed, Some(11));

        let (scene, _) = apply("/entities/0/name=villain").unwrap();
        assert_eq!(scene.entities[0].name.as_deref(), Some("villain"));
    }

    /// An unresolvable *intermediate* key names the deepest prefix that did resolve and the nearest
    /// match among its own keys. (A misspelled *final* field is created instead, and the schema's
    /// `L1014` catches it downstream — see `a_missing_final_field_is_created`.)
    #[test]
    fn an_unknown_intermediate_key_suggests_the_nearest_one() {
        let diagnostics = apply("/entities/0/components/Transform/lsit/0=1").unwrap_err();
        assert_eq!(diagnostics[0].code.as_str(), "L4008");
        assert_eq!(
            diagnostics[0].path.as_deref(),
            Some("/entities/0/components/Transform/lsit/0")
        );
        let help = diagnostics[0].help.as_deref().unwrap();
        assert!(help.contains("/entities/0/components/Transform"), "{help}");
        assert!(help.contains("nearest match is `list`"), "{help}");
    }

    #[test]
    fn an_unknown_component_suggests_the_nearest_one() {
        let diagnostics = apply("/entities/0/components/Trasnform/x=1").unwrap_err();
        let help = diagnostics[0].help.as_deref().unwrap();
        assert!(help.contains("nearest match is `Transform`"), "{help}");
    }

    #[test]
    fn out_of_range_indices_do_not_resolve() {
        for spec in [
            "/entities/7/components/Transform/x=1",
            "/entities/0/components/Transform/list/9=1",
        ] {
            let diagnostics = apply(spec).unwrap_err();
            assert_eq!(diagnostics[0].code.as_str(), "L4008", "{spec}");
        }
    }

    #[test]
    fn a_pointer_cannot_descend_through_a_leaf() {
        let diagnostics = apply("/entities/0/components/Transform/x/deeper=1").unwrap_err();
        let help = diagnostics[0].help.as_deref().unwrap();
        assert!(help.contains("has no children"), "{help}");
    }

    #[test]
    fn whole_entities_and_the_component_map_are_rejected() {
        for spec in [
            "/entities/0={}",
            "/entities/0/components={}",
            "/entities=[]",
        ] {
            let diagnostics = apply(spec).unwrap_err();
            assert_eq!(diagnostics[0].code.as_str(), "L4008", "{spec}");
        }
    }

    #[test]
    fn malformed_specs_are_rejected() {
        for spec in ["nonsense", "=4", "entities/0/name=x"] {
            let diagnostic = SceneOverride::parse(spec).unwrap_err();
            assert_eq!(diagnostic.code.as_str(), "L4008", "{spec}");
        }
    }

    /// A sweep is one pointer and the values to try there, typed by the same rule as `--set`.
    #[test]
    fn sweeps_parse_into_a_pointer_and_values() {
        let (pointer, values) =
            parse_sweep("/entities/0/components/Transform/x=0,0.5,1e2,null").unwrap();
        assert_eq!(pointer, "/entities/0/components/Transform/x");
        assert_eq!(
            values,
            vec![json!(0), json!(0.5), json!(1e2), Value::Null],
            "each value is JSON when it parses as JSON",
        );

        let (_, textual) = parse_sweep("/name=night,day").unwrap();
        assert_eq!(
            textual,
            vec![json!("night"), json!("day")],
            "a value that is not valid JSON is a plain string",
        );

        let (_, single) = parse_sweep("/seed=7").unwrap();
        assert_eq!(single, vec![json!(7)], "a one-value sweep is well-formed");
    }

    /// Only the *first* `=` separates the pointer, so a value may contain one.
    #[test]
    fn a_swept_value_may_contain_an_equals_sign() {
        let (pointer, values) = parse_sweep("/name=a=b,c").unwrap();
        assert_eq!(pointer, "/name");
        assert_eq!(values, vec![json!("a=b"), json!("c")]);
    }

    #[test]
    fn malformed_sweeps_are_rejected() {
        for spec in [
            "nonsense",
            "=0,1",
            "entities/0/name=x",
            "/seed=",
            "/seed=1,,2",
            "/seed=,1",
        ] {
            let diagnostic = parse_sweep(spec).unwrap_err();
            assert_eq!(diagnostic.code.as_str(), "L4008", "{spec}");
        }
    }

    /// The two spellings share one value-parsing rule — the same comma-free text means the same
    /// value under `--set` and under a one-value `--sweep`.
    #[test]
    fn set_and_sweep_type_a_value_identically() {
        for raw in ["0.8", "7", "null", "true", "night", r#""7""#, r#"{"a": 1}"#] {
            let spec = format!("/name={raw}");
            let set = SceneOverride::parse(&spec).unwrap().value;
            let (_, swept) = parse_sweep(&spec).unwrap();
            assert_eq!(vec![set], swept, "{raw}");
        }
    }

    /// The comma split is literal and has no escape: a JSON sequence is not a sweepable value, it
    /// simply splits into fragments.
    #[test]
    fn a_comma_bearing_value_splits_rather_than_escaping() {
        let (_, values) = parse_sweep("/name=[1,2]").unwrap();
        assert_eq!(values, vec![json!("[1"), json!("2]")]);
    }

    #[test]
    fn escaped_segments_round_trip() {
        let mut scene = Scene {
            entities: vec![SceneEntity {
                presentation: false,
                name: None,
                components: vec![("Odd".into(), json!({ "a/b": 1, "c~d": 2 }))],
                instance: None,
            }],
            ..Default::default()
        };
        let overrides = [
            SceneOverride::parse("/entities/0/components/Odd/a~1b=7").unwrap(),
            SceneOverride::parse("/entities/0/components/Odd/c~0d=8").unwrap(),
        ];
        apply_overrides(&mut scene, &overrides).unwrap();
        assert_eq!(
            scene.entities[0].components[0].1,
            json!({ "a/b": 7, "c~d": 8 })
        );
    }

    #[test]
    fn every_failure_is_reported_in_one_pass() {
        let mut scene = scene();
        let overrides = [
            SceneOverride::parse("/entities/0/components/Trasnform/x=1").unwrap(),
            SceneOverride::parse("/nmae=x").unwrap(),
        ];
        let diagnostics = apply_overrides(&mut scene, &overrides).unwrap_err();
        assert_eq!(diagnostics.len(), 2);
    }

    #[test]
    fn later_overrides_see_earlier_ones() {
        let mut scene = scene();
        let overrides = [
            SceneOverride::parse("/entities/0/components/Transform/x=2.0").unwrap(),
            SceneOverride::parse("/entities/0/components/Transform/x=3.0").unwrap(),
        ];
        let applied = apply_overrides(&mut scene, &overrides).unwrap();
        assert_eq!(applied[1].from, Some(json!(2.0)));
        assert_eq!(scene.entities[0].components[0].1["x"], json!(3.0));
    }
}

//! Prefab-instance expansion: a document holding instance entries becomes a flat scene.
//!
//! [`expand`] resolves each instance's prefab through a caller-supplied resolver (this crate reads no
//! file), recurses into nested instances depth-first, applies `removed` and the per-part partial
//! `overrides`, and composes entity names `<instance>/<part path>`. Entity references name document
//! positions throughout; a reference to an instance entry lands on its first surviving part.
//!
//! Every path resolves against the document it is written in: a prefab's asset paths, its nested
//! `prefab:` entries and its own overrides are rebased to content-root form when the prefab is read
//! ([`PrefabDocument::rebase`](crate::PrefabDocument::rebase)); the scene's own paths are unchanged.
//!
//! A diagnostic about an expanded prefab file keeps its `/entities/{i}/…` pointer into that file, has
//! its message prefixed `` in `<prefab path>`: `` once (by the level that read the file), and carries
//! one `related` diagnostic per enclosing instance, outermost last.

use std::collections::BTreeMap;
use std::ops::Range;
use std::rc::Rc;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::math::Trs;
use ludion_core::suggest;
use ludion_ecs::{ComponentInfo, Registry, World};
use serde_json::Value;

use crate::instantiate::{InstantiateOptions, instantiate_into, links_into};
use crate::prefab::PrefabDocument;
use crate::schema::SchemaCursor;
use crate::validate::{Checker, Subject, cycle_diagnostic, hierarchy_cycles, presentation_refs};
use crate::yaml::pointer_token;
use crate::{InstanceEntry, Scene, SceneEntity, normalize_prefab_path, prefab};

/// A document expanded to plain entities, with the instance metadata the caller needs.
#[derive(Clone, Debug)]
pub struct ExpandedScene {
    /// The flat scene: no instance entry is left. Names, seed, physics and provenance are the
    /// document's own.
    pub scene: Scene,
    /// Every instance, **post-order** (an instance's nested instances come before it).
    pub instances: Vec<ExpandedInstance>,
    /// Per flat entity: the position of the document entry it came from (itself, or its instance).
    pub entry_of: Vec<usize>,
}

/// One expanded instance.
#[derive(Clone, Debug)]
pub struct ExpandedInstance {
    /// The entry's name in the document that holds it.
    pub name: String,
    /// The entry's position in the document that holds it.
    pub entry: usize,
    /// `0` for an entry of the expanded document itself, `1` for one inside its prefabs, and so on.
    pub depth: usize,
    /// The prefab path, normalized.
    pub source: String,
    /// The instance's surviving parts, as positions in [`ExpandedScene::scene`]`.entities`.
    pub parts: Range<usize>,
    /// Per part: its path inside this instance (`bulb`, `barrel/muzzle`).
    pub paths: Vec<String>,
    /// The entry's `transform`.
    pub placement: Trs,
}

/// A diagnostic and whether its pointer is already attributed to a file by an inner level.
type Located = (Diagnostic, bool);

struct Level {
    entities: Vec<SceneEntity>,
    instances: Vec<ExpandedInstance>,
    entry_of: Vec<usize>,
}

struct Context<'a> {
    registry: &'a Registry,
    resolve: &'a mut dyn FnMut(&str) -> Result<String, Diagnostic>,
    /// Each prefab path's parsed, validated document: a path is resolved once per expansion.
    parsed: BTreeMap<String, Rc<PrefabDocument>>,
}

/// One resolved instance entry: the prefab's flat parts and what survives `removed`.
struct Resolved {
    source: String,
    sub: Level,
    names: Vec<String>,
    keep: Vec<bool>,
    /// Per prefab part: its position among the survivors.
    compact: Vec<usize>,
    /// Per prefab-part boundary `0..=len`: how many survivors precede it.
    before: Vec<usize>,
    span: usize,
}

pub(crate) fn expand(
    scene: &Scene,
    registry: &Registry,
    resolve: &mut dyn FnMut(&str) -> Result<String, Diagnostic>,
) -> Result<ExpandedScene, Vec<Diagnostic>> {
    let mut context = Context {
        registry,
        resolve,
        parsed: BTreeMap::new(),
    };
    let level = expand_level(&mut context, &scene.entities, &mut Vec::new(), "")
        .map_err(|diagnostics| diagnostics.into_iter().map(|(d, _)| d).collect::<Vec<_>>())?;
    let expanded = ExpandedScene {
        scene: Scene {
            name: scene.name.clone(),
            generated_from: scene.generated_from.clone(),
            seed: scene.seed,
            physics: scene.physics.clone(),
            preload: scene.preload.clone(),
            entities: level.entities,
        },
        instances: level.instances,
        entry_of: level.entry_of,
    };
    // A loop closed across levels (an override parenting a part under an entity that parents back)
    // is invisible to either document alone.
    let mut cycles: Vec<Diagnostic> = hierarchy_cycles(&expanded.scene, registry)
        .iter()
        .map(|cycle| expanded.locate(cycle_diagnostic(&expanded.scene, cycle)))
        .collect();
    cycles.extend(
        presentation_refs(&expanded.scene, registry)
            .into_iter()
            .map(|refusal| expanded.locate(refusal)),
    );
    if cycles.is_empty() {
        Ok(expanded)
    } else {
        Err(cycles)
    }
}

impl ExpandedScene {
    /// The positions of `instance`'s parts that no `Parent` link ties into the instance itself:
    /// the parts [`instantiate_expanded`](crate::instantiate_expanded) places with its `transform`.
    pub fn roots(&self, instance: &ExpandedInstance, registry: &Registry) -> Vec<usize> {
        instance
            .parts
            .clone()
            .filter(|&position| {
                !links_into(&self.scene.entities[position], registry, &instance.parts)
            })
            .collect()
    }

    /// Whether `position` is a root of any instance that holds it, so a placement is baked onto its
    /// stored `Transform`.
    pub fn is_placed(&self, position: usize, registry: &Registry) -> bool {
        self.instances.iter().any(|instance| {
            instance.parts.contains(&position)
                && !links_into(&self.scene.entities[position], registry, &instance.parts)
        })
    }

    /// Maps a diagnostic raised on the flat scene back to the document: a pointer
    /// `/entities/{flat}{rest}` on a plain entity becomes `/entities/{entry}{rest}`; on a part of an
    /// instance it becomes `/entities/{entry}` with the message saying which part and where. Other
    /// pointers pass through unchanged.
    pub fn locate(&self, mut diagnostic: Diagnostic) -> Diagnostic {
        let Some(pointer) = diagnostic.path.as_deref() else {
            return diagnostic;
        };
        let Some(tail) = pointer.strip_prefix("/entities/") else {
            return diagnostic;
        };
        let digits = tail
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(tail.len());
        let (rest, Ok(flat)) = (&tail[digits..], tail[..digits].parse::<usize>()) else {
            return diagnostic;
        };
        let Some(&entry) = self.entry_of.get(flat) else {
            return diagnostic;
        };
        let rest = rest.to_owned();
        let part = self
            .instances
            .iter()
            .find(|instance| instance.depth == 0 && instance.parts.contains(&flat));
        match part {
            None => diagnostic.path = Some(format!("/entities/{entry}{rest}")),
            Some(instance) => {
                let at = if rest.is_empty() {
                    String::new()
                } else {
                    format!(", at {rest}")
                };
                diagnostic.message = format!(
                    "part `{}` of instance `{}` (`{}`){at}: {}",
                    instance.paths[flat - instance.parts.start],
                    instance.name,
                    instance.source,
                    diagnostic.message
                );
                diagnostic.path = Some(format!("/entities/{entry}"));
            }
        }
        diagnostic
    }
}

/// `L1036` when `parts` (an instance's or prefab's parts, in order) start with a presentation part
/// yet hold a sim one: every part's `PrefabInstance.root` names the first part, so the sim hash would
/// carry a presentation id. The caller sets the pointer.
pub(crate) fn mixed_root(parts: &[&SceneEntity]) -> Option<Diagnostic> {
    let first = parts.first().filter(|part| part.presentation)?;
    let sim = parts.iter().find(|part| !part.presentation)?;
    let label = |part: &SceneEntity| part.name.as_deref().unwrap_or("(unnamed)").to_owned();
    Some(
        Diagnostic::new(
            code::SCENE_PRESENTATION_REF,
            format!(
                "first part `{}` is presentation but part `{}` is sim; the sim part's PrefabInstance.root \
                 would name a presentation id",
                label(first),
                label(sim)
            ),
        )
        .with_help(
            "put a sim part first (the one with the Collider) — it is the instance's root, which every \
             part's PrefabInstance names — or make every part `presentation: true`",
        ),
    )
}

/// `mixed_root` over a flat prefab body, pointed at its first entity.
pub fn mixed_root_diagnostic(entities: &[SceneEntity]) -> Option<Diagnostic> {
    let parts: Vec<&SceneEntity> = entities.iter().collect();
    mixed_root(&parts).map(|found| found.with_path("/entities/0/presentation"))
}

/// Wraps the diagnostics of a read prefab file: names the file on the ones not yet attributed and
/// adds the `related` link to the instance entry that pulled the file in.
fn attribute(source: &str, entry: usize, composed: &str, inner: Vec<Located>) -> Vec<Located> {
    inner
        .into_iter()
        .map(|(mut diagnostic, attributed)| {
            if !attributed {
                diagnostic.message = format!("in `{source}`: {}", diagnostic.message);
            }
            let related = Diagnostic::new(
                diagnostic.code,
                format!("expanded from instance `{composed}`"),
            )
            .with_path(format!("/entities/{entry}"));
            diagnostic.related.push(related);
            (diagnostic, true)
        })
        .collect()
}

fn unattributed(diagnostics: Vec<Diagnostic>) -> Vec<Located> {
    diagnostics.into_iter().map(|d| (d, false)).collect()
}

fn part_diagnostic(pointer: String, message: String, help: String) -> Located {
    (
        Diagnostic::new(code::PREFAB_INSTANCE_PART, message)
            .with_path(pointer)
            .with_help(help),
        false,
    )
}

fn nearest_help(path: &str, names: &[String]) -> String {
    match suggest::nearest(path, names.iter().map(String::as_str)) {
        Some(nearest) => format!("nearest match is `{nearest}`"),
        None => {
            let shown: Vec<String> = names.iter().take(8).map(|n| format!("`{n}`")).collect();
            format!("the prefab's parts are {}", shown.join(", "))
        }
    }
}

/// Every in-range reference position `value` (component `name`) holds.
pub(crate) fn ref_targets(registry: &Registry, name: &str, value: &Value) -> Vec<usize> {
    let Some(info) = registry.get(name) else {
        return Vec::new();
    };
    info.entity_refs()
        .iter()
        .filter_map(|field| {
            let reference = if field.suffix.is_empty() {
                Some(value)
            } else {
                value.pointer(&field.suffix)
            };
            reference?.get("index")?.as_u64().map(|p| p as usize)
        })
        .collect()
}

/// Rewrites each entity-reference `index` of `value` through `map`; `None` leaves it alone.
/// `generation` stays as written (a scene's is 0).
pub(crate) fn remap_positions(
    registry: &Registry,
    name: &str,
    value: &mut Value,
    map: &dyn Fn(usize) -> Option<usize>,
) {
    let Some(info) = registry.get(name) else {
        return;
    };
    for field in info.entity_refs() {
        let reference = if field.suffix.is_empty() {
            Some(&mut *value)
        } else {
            value.pointer_mut(&field.suffix)
        };
        let Some(object) = reference.and_then(Value::as_object_mut) else {
            continue;
        };
        let Some(position) = object
            .get("index")
            .and_then(Value::as_u64)
            .and_then(|position| map(position as usize))
        else {
            continue;
        };
        object.insert("index".to_owned(), Value::from(position as u64));
    }
}

/// Merges the partial value `over` into `base`: objects merge key by key, anything else (arrays
/// included) replaces — the rule an instance's `overrides` follow.
///
/// # Example
/// ```
/// use serde_json::json;
///
/// let mut base = json!({"a": {"x": 1, "y": 2}, "b": [1, 2]});
/// ludion_scene::merge_partial(&mut base, json!({"a": {"y": 3}, "b": [9]}));
/// assert_eq!(base, json!({"a": {"x": 1, "y": 3}, "b": [9]}));
/// ludion_scene::merge_partial(&mut base, json!({"a": {"x": null}}));
/// assert_eq!(base, json!({"a": {"y": 3}, "b": [9]}));
/// ```
pub fn merge_partial(base: &mut Value, over: Value) {
    match (base, over) {
        (Value::Object(base), Value::Object(over)) => {
            for (key, value) in over {
                if value.is_null() {
                    base.shift_remove(&key);
                    continue;
                }
                match base.get_mut(&key) {
                    Some(slot) => merge_partial(slot, value),
                    None => {
                        let mut fresh = Value::Object(Default::default());
                        merge_partial(&mut fresh, value);
                        base.insert(key, fresh);
                    }
                }
            }
        }
        (slot, over) => *slot = over,
    }
}

/// Structural equality of two JSON values where two `f64` numbers compare by bits, so `-0.0` and
/// `0.0` differ; any other pair compares with `==`. Object key order is ignored, as with `==`.
///
/// # Example
/// ```
/// use serde_json::json;
///
/// assert!(ludion_scene::identical(&json!({"a": [1.5, 0.0]}), &json!({"a": [1.5, 0.0]})));
/// assert!(!ludion_scene::identical(&json!({"a": [-0.0]}), &json!({"a": [0.0]})));
/// assert_eq!(json!(-0.0), json!(0.0), "plain equality cannot tell them apart");
/// ```
pub fn identical(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) if x.is_f64() && y.is_f64() => {
            x.as_f64().map(f64::to_bits) == y.as_f64().map(f64::to_bits)
        }
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(x, y)| identical(x, y))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(key, x)| y.get(key).is_some_and(|y| identical(x, y)))
        }
        _ => a == b,
    }
}

/// The partial value that [`merge_partial`] turns `base` into `live` with; `None` when they are
/// equal. Objects diff key by key (a key missing from `base` is taken whole, a key missing from
/// `live` becomes `null`); anything else — arrays, scalars, and a value at a JSON-pointer path listed
/// in `atomic` (`""` for the whole value) — is taken whole when it differs.
///
/// # Example
/// ```
/// use serde_json::json;
///
/// let base = json!({"a": {"x": 1, "y": 2}, "target": {"index": 0, "generation": 0}});
/// let live = json!({"a": {"x": 1, "y": 3}, "target": {"index": 4, "generation": 0}});
/// let diff = ludion_scene::partial_diff(&base, &live, &["/target"]).unwrap();
/// assert_eq!(diff, json!({"a": {"y": 3}, "target": {"index": 4, "generation": 0}}));
/// let mut merged = base.clone();
/// ludion_scene::merge_partial(&mut merged, diff);
/// assert_eq!(merged, live);
/// assert!(ludion_scene::partial_diff(&base, &base, &[]).is_none());
/// ```
pub fn partial_diff(base: &Value, live: &Value, atomic: &[&str]) -> Option<Value> {
    diff_at(base, live, atomic, "")
}

fn diff_at(base: &Value, live: &Value, atomic: &[&str], path: &str) -> Option<Value> {
    if identical(base, live) {
        return None;
    }
    let (Value::Object(base_map), Value::Object(live_map)) = (base, live) else {
        return Some(live.clone());
    };
    if atomic.contains(&path) {
        return Some(live.clone());
    }
    let mut out = serde_json::Map::new();
    for (key, value) in live_map {
        match base_map.get(key) {
            Some(held) => {
                let child = format!("{path}/{key}");
                if let Some(diff) = diff_at(held, value, atomic, &child) {
                    out.insert(key.clone(), diff);
                }
            }
            None => {
                out.insert(key.clone(), value.clone());
            }
        }
    }
    for key in base_map.keys().filter(|key| !live_map.contains_key(*key)) {
        out.insert(key.clone(), Value::Null);
    }
    Some(Value::Object(out))
}

fn expand_level(
    context: &mut Context<'_>,
    entities: &[SceneEntity],
    stack: &mut Vec<String>,
    enclosing: &str,
) -> Result<Level, Vec<Located>> {
    let mut diagnostics: Vec<Located> = Vec::new();
    let compose = |name: &str| {
        if enclosing.is_empty() {
            name.to_owned()
        } else {
            format!("{enclosing}/{name}")
        }
    };

    // Pass 1: resolve every instance and size it.
    let mut resolved: Vec<Option<Resolved>> = Vec::with_capacity(entities.len());
    let mut start = Vec::with_capacity(entities.len());
    let mut offset = 0;
    for (index, entity) in entities.iter().enumerate() {
        start.push(offset);
        let Some(instance) = &entity.instance else {
            offset += 1;
            resolved.push(None);
            continue;
        };
        let entry_name = entity.name.clone().unwrap_or_default();
        let outcome = resolve_entry(
            context,
            instance,
            index,
            &compose(&entry_name),
            stack,
            &mut diagnostics,
        );
        offset += outcome.as_ref().map_or(0, |r| r.span);
        resolved.push(outcome);
    }

    // Pass 2: build the flat list.
    let mut flat: Vec<SceneEntity> = Vec::with_capacity(offset);
    let mut instances = Vec::new();
    let mut entry_of = Vec::with_capacity(offset);
    let start_of = |position: usize| start.get(position).copied();
    for (index, entity) in entities.iter().enumerate() {
        let Some(instance) = &entity.instance else {
            let mut plain = entity.clone();
            for (name, value) in &mut plain.components {
                remap_positions(context.registry, name, value, &start_of);
            }
            flat.push(plain);
            entry_of.push(index);
            continue;
        };
        let Some(r) = resolved[index].take() else {
            continue;
        };
        let offset = start[index];
        let entry_name = entity.name.clone().unwrap_or_default();
        let intrinsic = |position: usize| {
            r.keep
                .get(position)
                .copied()
                .unwrap_or(false)
                .then(|| offset + r.compact[position])
        };
        let mut paths = Vec::with_capacity(r.span);
        // Flat positions this entry's `presentation: true` turned from sim to presentation.
        let mut forced: Vec<usize> = Vec::new();
        for (k, part) in r
            .sub
            .entities
            .iter()
            .enumerate()
            .filter(|(k, _)| r.keep[*k])
        {
            let mut part = part.clone();
            for (name, value) in &mut part.components {
                remap_positions(context.registry, name, value, &intrinsic);
            }
            part.name = Some(format!("{entry_name}/{}", r.names[k]));
            if instance.presentation && !part.presentation {
                part.presentation = true;
                forced.push(flat.len());
            }
            paths.push(r.names[k].clone());
            flat.push(part);
            entry_of.push(index);
        }
        apply_overrides(
            context.registry,
            instance,
            index,
            (&r, offset),
            &start_of,
            &mut flat,
            &mut diagnostics,
        );
        for &position in &forced {
            check_sim_only(
                context.registry,
                &flat[position],
                &paths[position - offset],
                &entry_name,
                index,
                &mut diagnostics,
            );
        }
        for nested in &r.sub.instances {
            let (from, to) = (r.before[nested.parts.start], r.before[nested.parts.end]);
            if from == to {
                continue;
            }
            let mut nested = nested.clone();
            nested.depth += 1;
            nested.paths = nested
                .parts
                .clone()
                .zip(std::mem::take(&mut nested.paths))
                .filter(|(position, _)| r.keep[*position])
                .map(|(_, path)| path)
                .collect();
            nested.parts = offset + from..offset + to;
            check_mixed_root(&flat, &nested.parts, index, &mut diagnostics);
            instances.push(nested);
        }
        check_mixed_root(&flat, &(offset..offset + r.span), index, &mut diagnostics);
        instances.push(ExpandedInstance {
            name: entry_name,
            entry: index,
            depth: 0,
            source: r.source.clone(),
            parts: offset..offset + r.span,
            paths,
            placement: instance.transform.to_trs(),
        });
    }

    if diagnostics.is_empty() {
        Ok(Level {
            entities: flat,
            instances,
            entry_of,
        })
    } else {
        Err(diagnostics)
    }
}

const SIM_ONLY_HELP: &str = "drop the `presentation` key and keep the instance sim, or keep that part sim \
     in a separate instance and parent the presentation body to it through `overrides`";

/// The names of the sim-only components `part` carries.
fn sim_only_names<'a>(
    registry: &'a Registry,
    part: &'a SceneEntity,
) -> impl Iterator<Item = &'a str> {
    part.components
        .iter()
        .map(|(name, _)| name.as_str())
        .filter(|name| registry.get(name).is_some_and(ComponentInfo::sim_only))
}

/// `L1320` for each sim-only component `part` of a presentation instance carries.
fn check_sim_only(
    registry: &Registry,
    part: &SceneEntity,
    path: &str,
    entry_name: &str,
    entry: usize,
    diagnostics: &mut Vec<Located>,
) {
    for name in sim_only_names(registry, part) {
        diagnostics.push((
            Diagnostic::new(
                code::PRESENTATION_INSTANCE_SIM_COMPONENT,
                format!(
                    "presentation instance `{entry_name}`: part `{path}` carries `{name}`, \
                     which only the sim reads"
                ),
            )
            .with_path(format!("/entities/{entry}/presentation"))
            .with_help(SIM_ONLY_HELP),
            false,
        ));
    }
}

/// `expanded` with every sim part turned presentation, for a runtime `presentation` spawn of the
/// prefab `prefab`.
pub(crate) fn presentation_copy(
    expanded: &ExpandedScene,
    registry: &Registry,
    prefab: &str,
) -> Result<ExpandedScene, Vec<Diagnostic>> {
    let mut copy = expanded.clone();
    let mut diagnostics = Vec::new();
    for (k, part) in copy.scene.entities.iter_mut().enumerate() {
        if part.presentation {
            continue;
        }
        part.presentation = true;
        for name in sim_only_names(registry, part) {
            diagnostics.push(
                Diagnostic::new(
                    code::PRESENTATION_INSTANCE_SIM_COMPONENT,
                    format!(
                        "presentation spawn of `{prefab}`: part `{}` carries `{name}`, which only \
                         the sim reads",
                        part.name.as_deref().unwrap_or("(unnamed)")
                    ),
                )
                .with_path(format!("/entities/{k}/components/{name}"))
                .with_help(SIM_ONLY_HELP),
            );
        }
    }
    if diagnostics.is_empty() {
        Ok(copy)
    } else {
        Err(diagnostics)
    }
}

fn check_mixed_root(
    flat: &[SceneEntity],
    parts: &Range<usize>,
    entry: usize,
    diagnostics: &mut Vec<Located>,
) {
    let Some(parts) = flat.get(parts.clone()) else {
        return;
    };
    let parts: Vec<&SceneEntity> = parts.iter().collect();
    if let Some(found) = mixed_root(&parts) {
        diagnostics.push((found.with_path(format!("/entities/{entry}")), false));
    }
}

/// Resolves one instance entry and applies `removed`; `None` means a diagnostic was pushed.
fn resolve_entry(
    context: &mut Context<'_>,
    instance: &InstanceEntry,
    index: usize,
    composed: &str,
    stack: &mut Vec<String>,
    diagnostics: &mut Vec<Located>,
) -> Option<Resolved> {
    let source = normalize_prefab_path(&instance.prefab);
    if let Some(at) = stack.iter().position(|path| *path == source) {
        let mut chain: Vec<&str> = stack[at..].iter().map(String::as_str).collect();
        chain.push(&source);
        diagnostics.push((
            Diagnostic::new(
                code::PREFAB_INSTANCE_CYCLE,
                format!("prefab instances form a loop: {}", chain.join(" -> ")),
            )
            .with_path(format!("/entities/{index}/prefab"))
            .with_help(
                "a prefab cannot contain itself, directly or through another prefab; \
                 remove or re-point one instance in the loop",
            ),
            false,
        ));
        return None;
    }
    let doc = match context.parsed.get(&instance.prefab) {
        Some(doc) => Rc::clone(doc),
        None => {
            let text = match (context.resolve)(&instance.prefab) {
                Ok(text) => text,
                Err(mut diagnostic) => {
                    diagnostic
                        .path
                        .get_or_insert_with(|| format!("/entities/{index}/prefab"));
                    diagnostics.push((diagnostic, false));
                    return None;
                }
            };
            let doc = match prefab::parse(&text) {
                Ok(doc) => doc,
                Err(errors) => {
                    diagnostics.extend(attribute(&source, index, composed, unattributed(errors)));
                    return None;
                }
            };
            let errors = prefab::validate(&doc, context.registry);
            if !errors.is_empty() {
                diagnostics.extend(attribute(&source, index, composed, unattributed(errors)));
                return None;
            }
            let mut doc = doc;
            doc.rebase(&source, context.registry);
            let doc = Rc::new(doc);
            context
                .parsed
                .insert(instance.prefab.clone(), Rc::clone(&doc));
            doc
        }
    };
    stack.push(source.clone());
    let sub = expand_level(context, &doc.entities, stack, composed);
    stack.pop();
    let sub = match sub {
        Ok(sub) => sub,
        Err(errors) => {
            diagnostics.extend(attribute(&source, index, composed, errors));
            return None;
        }
    };

    let names: Vec<String> = sub
        .entities
        .iter()
        .map(|part| part.name.clone().unwrap_or_default())
        .collect();
    let mut keep = vec![true; names.len()];
    let before_removed = diagnostics.len();
    for (k, path) in instance.removed.iter().enumerate() {
        match names.iter().position(|name| name == path) {
            Some(position) => keep[position] = false,
            None => diagnostics.push(part_diagnostic(
                format!("/entities/{index}/removed/{k}"),
                format!("`{path}` is not a part of `{source}`"),
                nearest_help(path, &names),
            )),
        }
    }
    for (referrer, part) in sub.entities.iter().enumerate().filter(|(k, _)| keep[*k]) {
        for (component, value) in &part.components {
            for target in ref_targets(context.registry, component, value) {
                if keep.get(target) != Some(&false) {
                    continue;
                }
                let removed = instance.removed.iter().position(|p| *p == names[target]);
                diagnostics.push(part_diagnostic(
                    format!("/entities/{index}/removed/{}", removed.unwrap_or(0)),
                    format!(
                        "cannot remove `{}`: part `{}` refers to it through `{component}`",
                        names[target], names[referrer]
                    ),
                    format!(
                        "remove `{}` too, or keep `{}`",
                        names[referrer], names[target]
                    ),
                ));
            }
        }
    }
    if !keep.contains(&true) {
        diagnostics.push(part_diagnostic(
            format!("/entities/{index}/removed"),
            format!("`removed` leaves no part of `{source}`"),
            "an instance needs at least one part: drop the entry instead".to_owned(),
        ));
    }
    if diagnostics.len() != before_removed {
        return None;
    }
    let mut compact = vec![0; names.len()];
    let mut before = Vec::with_capacity(names.len() + 1);
    let mut survivors = 0;
    for (k, &kept) in keep.iter().enumerate() {
        before.push(survivors);
        compact[k] = survivors;
        survivors += usize::from(kept);
    }
    before.push(survivors);
    Some(Resolved {
        source,
        sub,
        names,
        keep,
        compact,
        before,
        span: survivors,
    })
}

/// Merges the entry's overrides into the flat parts just pushed at `offset..`, remapping the
/// override values' references (document positions) first and schema-checking each merged component.
fn apply_overrides(
    registry: &Registry,
    instance: &InstanceEntry,
    index: usize,
    (r, offset): (&Resolved, usize),
    start_of: &dyn Fn(usize) -> Option<usize>,
    flat: &mut [SceneEntity],
    diagnostics: &mut Vec<Located>,
) {
    for (path, components) in &instance.overrides {
        let base = format!("/entities/{index}/overrides/{}", pointer_token(path));
        let Some(position) = r.names.iter().position(|name| name == path) else {
            diagnostics.push(part_diagnostic(
                base,
                format!("`{path}` is not a part of `{}`", r.source),
                nearest_help(path, &r.names),
            ));
            continue;
        };
        if !r.keep[position] {
            diagnostics.push(part_diagnostic(
                base,
                format!("part `{path}` is both overridden and in `removed`"),
                "drop the override or the `removed` entry".to_owned(),
            ));
            continue;
        }
        let part = &mut flat[offset + r.compact[position]];
        for (name, value) in components {
            let mut over = value.clone();
            remap_positions(registry, name, &mut over, start_of);
            if let Some(info) = registry.get(name) {
                let own = offset + r.compact[position];
                for field in info.entity_refs().iter().filter(|f| !f.allow_self) {
                    let reference = if field.suffix.is_empty() {
                        Some(&over)
                    } else {
                        over.pointer(&field.suffix)
                    };
                    let target = reference
                        .and_then(|reference| reference.get("index"))
                        .and_then(Value::as_u64);
                    if target == Some(own as u64) {
                        diagnostics.push((
                            Diagnostic::new(
                                code::SCENE_INVALID_ENTITY_REF,
                                format!(
                                    "`{name}` on part `{path}` refers to the part itself (a reference to the entry is its first surviving part)"
                                ),
                            )
                            .with_path(format!(
                                "{base}/{}{}/index",
                                pointer_token(name),
                                field.suffix
                            ))
                            .with_help("point it at another part".to_owned()),
                            false,
                        ));
                    }
                }
            }
            let merged = match part.components.iter_mut().find(|(n, _)| n == name) {
                Some((_, slot)) => {
                    merge_partial(slot, over);
                    slot.clone()
                }
                None => {
                    let mut fresh = Value::Object(Default::default());
                    merge_partial(&mut fresh, over);
                    part.components.push((name.clone(), fresh.clone()));
                    fresh
                }
            };
            let Some(info) = registry.get(name) else {
                continue;
            };
            let pointer = format!("{base}/{}", pointer_token(name));
            let mut found = Vec::new();
            Checker {
                subject: Subject::Component(name),
            }
            .check(
                &merged,
                SchemaCursor::new(info.schema()),
                &pointer,
                &mut found,
            );
            if found.is_empty() {
                for diagnostic in info.validate_semantics(&merged) {
                    let rebased = match &diagnostic.path {
                        Some(relative) => format!("{pointer}{relative}"),
                        None => pointer.clone(),
                    };
                    found.push(diagnostic.with_path(rebased));
                }
            }
            diagnostics.extend(unattributed(found));
        }
    }
}

/// Spawns an [`ExpandedScene`]; see [`crate::instantiate_expanded`] for the API contract.
pub(crate) fn instantiate_expanded(
    world: &mut World,
    expanded: &ExpandedScene,
    registry: &Registry,
    options: &InstantiateOptions<'_>,
) -> Result<Vec<EntityId>, Vec<Diagnostic>> {
    let entities = &expanded.scene.entities;
    let inner = InstantiateOptions {
        placement: None,
        name_prefix: options.name_prefix,
        hooks: options.hooks,
    };
    let ids = instantiate_into(world, entities, registry, &inner)?;
    for instance in &expanded.instances {
        for position in instance.parts.clone() {
            if !links_into(&entities[position], registry, &instance.parts) {
                options
                    .hooks
                    .place(world, ids[position], &instance.placement);
            }
        }
    }
    if let Some(placement) = &options.placement {
        for (entity, &id) in entities.iter().zip(&ids) {
            if !links_into(entity, registry, &(0..entities.len())) {
                options.hooks.place(world, id, placement);
            }
        }
    }
    Ok(ids)
}
